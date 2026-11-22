use std::collections::{HashMap, HashSet};

use codeidx_ast::{detect_lang, pagerank, Lang};
use codeidx_protocol::ToolError;
use codeidx_store::CallConfidence;

use super::{TraceBranch, TraceConfidence, TraceDirection, TraceLocation, TraceOptions};
use crate::ProjectContext;

pub(super) type NodeId = usize;
pub(super) type SiteId = usize;

pub(super) struct Node {
    pub location: TraceLocation,
    pub file_root: bool,
    pub method: bool,
    owner: Option<String>,
    score: f64,
}

pub(super) struct Site {
    pub location: TraceLocation,
    pub caller: NodeId,
    pub targets: Vec<NodeId>,
    pub confidence: Option<TraceConfidence>,
    pub omitted: usize,
    pub dynamic: bool,
    pub callback: bool,
    pub inferred: bool,
    pub character: Option<u32>,
    pub lang: Option<Lang>,
    owner_hint: Option<String>,
}

pub(super) struct Graph {
    pub nodes: Vec<Node>,
    pub sites: Vec<Site>,
    down: Vec<Vec<SiteId>>,
    up: Vec<Vec<SiteId>>,
    reverse_probes: HashMap<(Lang, String), Vec<SiteId>>,
    definition_names: HashSet<(Lang, String)>,
    pub definitions_by_file: HashMap<String, Vec<NodeId>>,
    pub hashes: HashMap<String, String>,
    pub skipped: usize,
}

pub(super) fn snapshot(ctx: &ProjectContext, rank_roots: bool) -> Result<Graph, ToolError> {
    let graph = ctx.store.call_graph()?;
    let mut scores = HashMap::new();
    if rank_roots {
        let mut paths = ctx.parsed.keys().collect::<Vec<_>>();
        paths.sort_unstable();
        let symbols = paths
            .iter()
            .flat_map(|p| ctx.parsed[*p].symbols.iter().cloned())
            .collect::<Vec<_>>();
        let refs = paths
            .iter()
            .flat_map(|p| ctx.parsed[*p].refs.iter().cloned())
            .collect::<Vec<_>>();
        scores.extend(
            pagerank(&symbols, &refs, &[])
                .into_iter()
                .map(|(n, score)| ((n.rel_path, n.name), score)),
        );
    }
    let mut nodes = Vec::new();
    let mut ids = HashMap::new();
    for sym in graph.symbols {
        ids.insert(sym.id, nodes.len());
        let score = scores
            .get(&(sym.rel_path.clone(), sym.name.clone()))
            .copied()
            .unwrap_or(0.0);
        nodes.push(Node {
            location: TraceLocation {
                file: sym.rel_path,
                line: sym.start_line,
                symbol: sym.name,
            },
            file_root: false,
            method: sym.parent.is_some(),
            owner: sym.parent,
            score,
        });
    }
    let mut sites = Vec::new();
    let mut file_roots = HashMap::new();
    for site in graph.sites {
        use codeidx_ast::CallSiteKind;
        let caller = match site.caller_id {
            Some(id) => *ids
                .get(&id)
                .ok_or_else(|| ToolError::new("missing caller in graph snapshot"))?,
            None => *file_roots.entry(site.rel_path.clone()).or_insert_with(|| {
                let id = nodes.len();
                nodes.push(Node {
                    location: TraceLocation {
                        file: site.rel_path.clone(),
                        line: 0,
                        symbol: "<file>".into(),
                    },
                    file_root: true,
                    method: false,
                    owner: None,
                    score: 0.0,
                });
                id
            }),
        };
        let targets: Vec<_> = site
            .candidates
            .iter()
            .filter_map(|id| ids.get(id).copied())
            .collect();
        let confidence = match site.confidence {
            CallConfidence::Unique => Some(TraceConfidence::Unique),
            CallConfidence::SameFile => Some(TraceConfidence::SameFile),
            CallConfidence::Ambiguous => Some(TraceConfidence::Ambiguous),
            CallConfidence::Unresolved => None,
        };
        sites.push(Site {
            location: TraceLocation {
                symbol: if site.name.is_empty() {
                    "<dynamic>".into()
                } else {
                    site.name
                },
                line: site.line,
                file: site.rel_path.clone(),
            },
            caller,
            confidence,
            omitted: site.candidate_count.saturating_sub(targets.len()),
            dynamic: targets.is_empty() && site.kind == CallSiteKind::Call,
            callback: site.kind == CallSiteKind::Callback,
            inferred: site.kind == CallSiteKind::Reference,
            character: site.character,
            lang: detect_lang(&site.rel_path),
            owner_hint: site.owner_hint,
            targets,
        });
    }
    let mut down = vec![Vec::new(); nodes.len()];
    let mut up = vec![Vec::new(); nodes.len()];
    let mut definitions_by_file = HashMap::<String, Vec<_>>::new();
    let mut definition_names = HashSet::new();
    for (id, node) in nodes.iter().enumerate().filter(|(_, n)| !n.file_root) {
        definitions_by_file
            .entry(node.location.file.clone())
            .or_default()
            .push(id);
        if let Some(lang) = detect_lang(&node.location.file) {
            definition_names.insert((lang, node.location.symbol.clone()));
        }
    }
    let mut reverse_probes = HashMap::<(Lang, String), Vec<_>>::new();
    for (id, site) in sites.iter().enumerate() {
        down[site.caller].push(id);
        for &target in &site.targets {
            up[target].push(id);
        }
        if let (Some(lang), Some(_)) = (site.lang, probe_priority(&nodes, site)) {
            let key = (lang, site.location.symbol.clone());
            if definition_names.contains(&key) {
                reverse_probes.entry(key).or_default().push(id);
            }
        }
    }
    let hashes = ctx
        .files
        .iter()
        .map(|(file, record)| (file.clone(), record.hash.clone()))
        .collect();
    Ok(Graph {
        nodes,
        sites,
        down,
        up,
        reverse_probes,
        definition_names,
        definitions_by_file,
        hashes,
        skipped: ctx.callflow_skipped,
    })
}

/// Only precise, named calls can consume LSP budget. Probe an ambiguous or
/// method-shaped candidate before a named zero-candidate site. The latter must
/// additionally have at least one same-language definition in the snapshot.
fn probe_priority(nodes: &[Node], site: &Site) -> Option<u8> {
    if site.inferred
        || site.callback
        || site.character.is_none()
        || site.location.symbol == "<dynamic>"
    {
        return None;
    }
    if site.confidence == Some(TraceConfidence::Ambiguous) {
        Some(0)
    } else if site.targets.iter().any(|&id| nodes[id].method) {
        Some(1)
    } else if site.targets.is_empty() {
        Some(2)
    } else {
        None
    }
}

impl Graph {
    pub fn probe_priority(&self, id: SiteId) -> Option<u8> {
        let site = &self.sites[id];
        let priority = probe_priority(&self.nodes, site)?;
        (priority < 2
            || self
                .definition_names
                .contains(&(site.lang?, site.location.symbol.clone())))
        .then_some(priority)
    }

    pub fn frontier(
        &self,
        id: NodeId,
        branch: Option<TraceBranch>,
        direction: TraceDirection,
        refine_lsp: bool,
    ) -> Vec<(SiteId, TraceBranch)> {
        let mut out = Vec::new();
        if branch != Some(TraceBranch::Up) && (branch.is_some() || direction != TraceDirection::Up)
        {
            out.extend(self.down[id].iter().map(|&site| (site, TraceBranch::Down)));
        }
        if branch != Some(TraceBranch::Down)
            && (branch.is_some() || direction != TraceDirection::Down)
        {
            let hits = &self.up[id];
            out.extend(hits.iter().map(|&site| (site, TraceBranch::Up)));
            if refine_lsp {
                let loc = &self.nodes[id].location;
                if let Some(probes) = detect_lang(&loc.file)
                    .and_then(|lang| self.reverse_probes.get(&(lang, loc.symbol.clone())))
                {
                    out.extend(
                        probes
                            .iter()
                            .copied()
                            .filter(|&site| {
                                hits.binary_search(&site).is_err()
                                    && self.sites[site].owner_hint == self.nodes[id].owner
                            })
                            .chain(probes.iter().copied().filter(|&site| {
                                hits.binary_search(&site).is_err()
                                    && self.sites[site].owner_hint != self.nodes[id].owner
                            }))
                            .map(|site| (site, TraceBranch::Up)),
                    );
                }
            }
        }
        out
    }

    pub fn select_roots(&self, options: &TraceOptions) -> Result<Vec<NodeId>, ToolError> {
        if let Some(from) = &options.from {
            if options.symbol.is_some() || options.file_path.is_some() {
                return Err(ToolError::new(
                    "from cannot be combined with symbol or file_path",
                ));
            }
            let ids = self
                .nodes
                .iter()
                .enumerate()
                .filter(|(_, node)| &node.location == from)
                .map(|(id, _)| id)
                .collect();
            return self.require_one(ids, "from location");
        }
        let file_matches = |node: &Node| {
            options
                .file_path
                .as_ref()
                .is_none_or(|file| file == &node.location.file)
        };
        if let Some(symbol) = &options.symbol {
            let ids = self
                .nodes
                .iter()
                .enumerate()
                .filter(|(_, n)| !n.file_root && file_matches(n) && &n.location.symbol == symbol)
                .map(|(id, _)| id)
                .collect();
            return self.require_one(ids, "symbol");
        }
        let mut roots: Vec<_> = self
            .nodes
            .iter()
            .enumerate()
            .filter(|(id, n)| {
                file_matches(n)
                    && (n.file_root
                        || n.location.symbol == "main"
                        || self.up[*id].iter().all(|s| self.sites[*s].caller == *id))
            })
            .map(|(id, _)| id)
            .collect();
        roots.sort_by(|&a, &b| {
            let a = &self.nodes[a];
            let b = &self.nodes[b];
            a.file_root
                .cmp(&b.file_root)
                .then_with(|| b.score.total_cmp(&a.score))
                .then_with(|| a.location.file.cmp(&b.location.file))
                .then_with(|| a.location.line.cmp(&b.location.line))
                .then_with(|| a.location.symbol.cmp(&b.location.symbol))
        });
        Ok(roots)
    }

    fn require_one(&self, ids: Vec<NodeId>, label: &str) -> Result<Vec<NodeId>, ToolError> {
        match ids.len() {
            1 => Ok(ids),
            0 => Err(ToolError::new(format!(
                "call trace {label} not found in current index"
            ))),
            n => {
                let locations: Vec<_> = ids
                    .iter()
                    .take(12)
                    .map(|&id| {
                        let loc = &self.nodes[id].location;
                        format!("{:?}:{}", loc.file, loc.line)
                    })
                    .collect();
                Err(ToolError::new(format!(
                    "call trace {label} is ambiguous ({n} matches): {}",
                    locations.join(", ")
                )))
            }
        }
    }
}
