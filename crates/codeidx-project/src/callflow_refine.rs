use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use codeidx_lsp::{Location, LspBridge};
use tokio::time::{timeout_at, Instant};

use super::index::{Graph, NodeId, SiteId};
use super::TraceConfidence;

const TOTAL_WAIT: Duration = Duration::from_secs(15);
const MAX_REQUESTS: usize = 8;

pub(super) struct Refiner<'a> {
    bridge: Option<&'a LspBridge>,
    root: PathBuf,
    enabled: bool,
    deadline: Option<Instant>,
    answers: HashMap<SiteId, Option<NodeId>>,
    pub attempted: usize,
    pub refined: usize,
    pub degraded: bool,
}

impl<'a> Refiner<'a> {
    pub fn new(bridge: Option<&'a LspBridge>, root: PathBuf, enabled: bool) -> Self {
        Self {
            bridge,
            root,
            enabled,
            deadline: None,
            answers: HashMap::new(),
            attempted: 0,
            refined: 0,
            degraded: false,
        }
    }

    pub async fn resolve(&mut self, graph: &Graph, id: SiteId) -> Option<NodeId> {
        let site = &graph.sites[id];
        if !self.enabled
            || !matches!(site.confidence, Some(TraceConfidence::Ambiguous))
            || site.inferred
            || site.callback
        {
            return None;
        }
        if let Some(answer) = self.answers.get(&id) {
            return *answer;
        }
        let answer = self.query(graph, id).await;
        self.answers.insert(id, answer);
        if answer.is_some() {
            self.refined += 1;
        } else {
            self.degraded = true;
        }
        answer
    }

    async fn query(&mut self, graph: &Graph, id: SiteId) -> Option<NodeId> {
        let site = &graph.sites[id];
        let bridge = self.bridge?;
        let (lang, character) = (site.lang?, site.character?);
        let deadline = *self
            .deadline
            .get_or_insert_with(|| Instant::now() + TOTAL_WAIT);
        if Instant::now() >= deadline || self.attempted >= MAX_REQUESTS {
            return None;
        }
        let file = &site.location.file;
        let text = read_snapshot(&self.root, file, graph.hashes.get(file)?).await?;
        if Instant::now() >= deadline {
            return None;
        }
        self.attempted += 1;
        let locations = timeout_at(
            deadline,
            bridge.definition(&self.root, lang, file, site.location.line, character, &text),
        )
        .await
        .ok()?
        .ok()?;
        let target = unique_match(graph, &site.targets, &locations)?;
        let target_file = &graph.nodes[target].location.file;
        if target_file != file {
            read_snapshot(&self.root, target_file, graph.hashes.get(target_file)?).await?;
        }
        Some(target)
    }
}

fn unique_match(graph: &Graph, candidates: &[NodeId], locations: &[Location]) -> Option<NodeId> {
    let first = locations.first()?;
    if !locations.iter().all(|other| other == first) {
        return None;
    }
    let mut matches = candidates.iter().copied().filter(|&id| {
        let node = &graph.nodes[id];
        node.location.file == first.file
            && (node.location.line..=node.end_line).contains(&first.line)
    });
    let hit = matches.next()?;
    matches.next().is_none().then_some(hit)
}

async fn read_snapshot(root: &Path, file: &str, hash: &str) -> Option<String> {
    let root = root.to_path_buf();
    let file = file.to_owned();
    let hash = hash.to_owned();
    tokio::task::spawn_blocking(move || {
        use std::io::Read;
        let path = root.join(file).canonicalize().ok()?;
        if !path.starts_with(&root) {
            return None;
        }
        let mut bytes = Vec::new();
        std::fs::File::open(path)
            .ok()?
            .take(512 * 1024 + 1)
            .read_to_end(&mut bytes)
            .ok()?;
        if bytes.len() > 512 * 1024 || blake3::hash(&bytes).to_hex().as_str() != hash {
            return None;
        }
        String::from_utf8(bytes).ok()
    })
    .await
    .ok()?
}
