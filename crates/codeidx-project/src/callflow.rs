//! Bounded shared call-chain engine.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use codeidx_lsp::LspBridge;
use codeidx_protocol::ToolError;

pub use crate::callflow_types::*;
use crate::ProjectManager;

#[path = "callflow_index.rs"]
mod index;
#[path = "callflow_refine.rs"]
mod refine;
#[cfg(test)]
#[path = "callflow_tests.rs"]
mod tests;

use index::{Graph, NodeId, SiteId};
use refine::Refiner;

/// Trace one bounded tree from the current index. The LSP bridge is shared with
/// the host's semantic tools and is only queried for visited ambiguous call sites.
pub async fn trace_calls(
    pm: &ProjectManager,
    bridge: Option<&LspBridge>,
    path: &str,
    mut options: TraceOptions,
) -> Result<TraceReport, ToolError> {
    options.max_depth = options.max_depth.min(16);
    options.max_roots = options.max_roots.clamp(1, 50);
    options.max_nodes = options.max_nodes.clamp(1, 1000);
    options.budget = options.budget.clamp(200, 8000);
    let context = pm.callflow_context(Path::new(path))?;
    let snapshot_context = context.clone();
    let rank = options.symbol.is_none() && options.from.is_none();
    let (root, graph) = tokio::task::spawn_blocking(move || {
        let ctx = snapshot_context
            .read()
            .map_err(|_| ToolError::new("project index lock poisoned"))?;
        Ok::<_, ToolError>((ctx.root.clone(), index::snapshot(&ctx, rank)?))
    })
    .await
    .map_err(|err| ToolError::new(format!("call graph snapshot failed: {err}")))??;
    let mut walk = Walk::new(
        &graph,
        &options,
        Refiner::new(bridge, root, options.refine_lsp),
    );
    let mut roots = graph.select_roots(&options)?;
    walk.truncated.roots = roots.len().saturating_sub(options.max_roots);
    roots.truncate(options.max_roots);
    let mut trees = Vec::new();
    for (i, &id) in roots.iter().enumerate() {
        let line = Entry::root(id);
        if !walk.reserve(&line, 0, roots.len() - i) {
            break;
        }
        trees.push(walk.visit(line, 0).await);
    }
    // A remount can replace the graph while an LSP request is in flight. Never
    // present refinements against the previous project as the current result.
    if !std::sync::Arc::ptr_eq(&context, &pm.callflow_context(Path::new(path))?) {
        return Err(ToolError::new("project changed during call trace; retry"));
    }
    Ok(TraceReport {
        direction: options.direction,
        roots: trees,
        max_depth: options.max_depth,
        nodes: walk.nodes,
        edges: walk.edges,
        truncated: walk.truncated,
        lsp_attempted: walk.refiner.attempted,
        lsp_refined: walk.refiner.refined,
        lsp_degraded: walk.refiner.degraded,
        skipped: graph.skipped,
    })
}

#[derive(Clone, Copy)]
struct Entry {
    node: Option<NodeId>,
    site: Option<SiteId>,
    confidence: Option<TraceConfidence>,
    branch: Option<TraceBranch>,
    candidates: usize,
}

impl Entry {
    fn root(id: NodeId) -> Self {
        Self {
            node: Some(id),
            site: None,
            confidence: None,
            branch: None,
            candidates: 0,
        }
    }
}

struct Walk<'a> {
    graph: &'a Graph,
    options: &'a TraceOptions,
    refiner: Refiner<'a>,
    active: HashSet<NodeId>,
    expanded: HashMap<(NodeId, Option<TraceBranch>), usize>,
    counted_sites: HashSet<SiteId>,
    nodes: usize,
    edges: usize,
    remaining_bytes: usize,
    truncated: TraceTruncation,
}

impl<'a> Walk<'a> {
    fn new(graph: &'a Graph, options: &'a TraceOptions, refiner: Refiner<'a>) -> Self {
        Self {
            graph,
            options,
            refiner,
            active: HashSet::new(),
            expanded: HashMap::new(),
            counted_sites: HashSet::new(),
            nodes: 0,
            edges: 0,
            remaining_bytes: options.budget * 4,
            truncated: TraceTruncation::default(),
        }
    }

    fn location(&self, entry: &Entry) -> &TraceLocation {
        if let Some(id) = entry.node {
            &self.graph.nodes[id].location
        } else {
            &self.graph.sites[entry.site.expect("unresolved entry has a site")].location
        }
    }

    fn reserve(&mut self, entry: &Entry, depth: usize, remaining: usize) -> bool {
        if self.nodes == self.options.max_nodes {
            self.truncated.nodes += remaining;
            return false;
        }
        let location = self.location(entry);
        // Upper bound for the compact row, including control-escaped labels,
        // indentation, line number, confidence markers and omission suffix.
        let escaped = |text: &str| {
            text.chars()
                .map(|c| match c {
                    '"' | '\\' => 2,
                    c if c.is_control() => 10.max(c.len_utf8()),
                    c => c.len_utf8(),
                })
                .sum::<usize>()
        };
        let cost = depth * 2 + escaped(&location.symbol) + escaped(&location.file) + 96;
        if cost > self.remaining_bytes {
            self.truncated.budget += remaining;
            return false;
        }
        self.remaining_bytes -= cost;
        self.nodes += 1;
        self.edges += usize::from(entry.site.is_some());
        true
    }

    async fn visit(&mut self, entry: Entry, depth: usize) -> TraceNode {
        let site = entry.site.map(|id| &self.graph.sites[id]);
        let mut node = TraceNode {
            location: self.location(&entry).clone(),
            confidence: entry.confidence,
            branch: entry.branch,
            candidates: entry.candidates,
            cycle: false,
            shared: false,
            dynamic: entry.node.is_none() && site.is_some_and(|s| s.dynamic),
            callback: site.is_some_and(|s| s.callback),
            inferred: site.is_some_and(|s| s.inferred),
            file_root: entry.node.is_some_and(|id| self.graph.nodes[id].file_root),
            truncated: 0,
            children: Vec::new(),
        };
        let Some(id) = entry.node else { return node };
        if self.active.contains(&id) {
            node.cycle = true;
            return node;
        }
        let remaining_depth = self.options.max_depth - depth;
        let key = (id, entry.branch);
        if self
            .expanded
            .get(&key)
            .is_some_and(|&done| done >= remaining_depth)
        {
            node.shared = true;
            return node;
        }
        let truncation_before = self.truncated.depth + self.truncated.nodes + self.truncated.budget;
        self.active.insert(id);
        let raw = self
            .graph
            .frontier(id, entry.branch, self.options.direction);
        if remaining_depth == 0 {
            node.truncated = raw
                .iter()
                .map(|&(site, branch)| self.known_count(site, branch))
                .sum();
            self.truncated.depth += node.truncated;
        } else {
            for (i, &(site, branch)) in raw.iter().enumerate() {
                if self.nodes == self.options.max_nodes || self.remaining_bytes < 96 {
                    let missing: usize =
                        raw[i..].iter().map(|&(s, b)| self.known_count(s, b)).sum();
                    if self.nodes == self.options.max_nodes {
                        self.truncated.nodes += missing;
                    } else {
                        self.truncated.budget += missing;
                    }
                    node.truncated += missing;
                    break;
                }
                let edges = self.edges_at(id, site, branch).await;
                for (j, edge) in edges.iter().enumerate() {
                    if !self.reserve(edge, depth + 1, edges.len() - j) {
                        let more: usize = raw[i + 1..]
                            .iter()
                            .map(|&(s, b)| self.known_count(s, b))
                            .sum::<usize>();
                        if self.nodes == self.options.max_nodes {
                            self.truncated.nodes += more;
                        } else {
                            self.truncated.budget += more;
                        }
                        node.truncated += edges.len() - j + more;
                        break;
                    }
                    node.children
                        .push(Box::pin(self.visit(*edge, depth + 1)).await);
                }
                if node.truncated > 0 {
                    break;
                }
            }
        }
        self.active.remove(&id);
        if truncation_before == self.truncated.depth + self.truncated.nodes + self.truncated.budget
        {
            self.expanded
                .entry(key)
                .and_modify(|done| *done = (*done).max(remaining_depth))
                .or_insert(remaining_depth);
        }
        node
    }

    fn known_count(&self, site: SiteId, branch: TraceBranch) -> usize {
        match branch {
            TraceBranch::Down => self.graph.sites[site].targets.len().max(1),
            TraceBranch::Up => 1,
        }
    }

    async fn edges_at(&mut self, id: NodeId, site_id: SiteId, branch: TraceBranch) -> Vec<Entry> {
        let site = &self.graph.sites[site_id];
        let refined = self.refiner.resolve(self.graph, site_id).await;
        let (confidence, targets, candidates) = if let Some(refined) = refined {
            (Some(TraceConfidence::Lsp), vec![refined], 1)
        } else {
            if self.counted_sites.insert(site_id) {
                self.truncated.candidates += site.omitted;
            }
            (
                site.confidence,
                site.targets.clone(),
                site.targets.len() + site.omitted,
            )
        };
        let entry = |node| Entry {
            node,
            site: Some(site_id),
            branch: Some(branch),
            confidence,
            candidates,
        };
        match branch {
            TraceBranch::Down if targets.is_empty() => vec![entry(None)],
            TraceBranch::Down => targets.into_iter().map(|n| entry(Some(n))).collect(),
            TraceBranch::Up if targets.contains(&id) => vec![entry(Some(site.caller))],
            TraceBranch::Up => Vec::new(),
        }
    }
}
