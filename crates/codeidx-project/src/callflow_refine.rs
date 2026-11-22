use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use codeidx_lsp::{Location, LspBridge};
use tokio::time::{timeout_at, Instant};

use super::index::{Graph, NodeId, SiteId};

const TOTAL_WAIT: Duration = Duration::from_secs(15);
const MAX_REQUESTS: usize = 8;
// Keep part of the lookup allowance available to validate existing edges.
const MAX_ZERO_CANDIDATE_REQUESTS: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Resolution {
    NotProbed,
    Project(NodeId),
    External,
    NoResult,
    Inconclusive,
    Timeout,
    Limited,
}

pub(super) struct Refiner<'a> {
    bridge: Option<&'a LspBridge>,
    root: PathBuf,
    enabled: bool,
    deadline: Option<Instant>,
    answers: HashMap<SiteId, Resolution>,
    zero_candidate_requests: usize,
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
            zero_candidate_requests: 0,
            attempted: 0,
            refined: 0,
            degraded: false,
        }
    }

    pub fn cached(&self, id: SiteId) -> Resolution {
        self.answers
            .get(&id)
            .copied()
            .unwrap_or(Resolution::NotProbed)
    }

    pub async fn resolve(&mut self, graph: &Graph, id: SiteId) -> Resolution {
        if !self.enabled || graph.probe_priority(id).is_none() {
            return Resolution::NotProbed;
        }
        if let Some(answer) = self.answers.get(&id) {
            return *answer;
        }
        let answer = self.query(graph, id).await;
        self.answers.insert(id, answer);
        match answer {
            Resolution::Project(_) | Resolution::External => self.refined += 1,
            Resolution::NotProbed => (),
            _ => self.degraded = true,
        }
        answer
    }

    async fn query(&mut self, graph: &Graph, id: SiteId) -> Resolution {
        let site = &graph.sites[id];
        let (Some(bridge), Some(lang), Some(character)) = (self.bridge, site.lang, site.character)
        else {
            return Resolution::Inconclusive;
        };
        let zero_candidates = site.targets.is_empty();
        if self.attempted >= MAX_REQUESTS
            || (zero_candidates && self.zero_candidate_requests >= MAX_ZERO_CANDIDATE_REQUESTS)
        {
            return Resolution::Limited;
        }
        let deadline = *self
            .deadline
            .get_or_insert_with(|| Instant::now() + TOTAL_WAIT);
        if Instant::now() >= deadline {
            return Resolution::Timeout;
        }
        let file = &site.location.file;
        let Some(hash) = graph.hashes.get(file) else {
            return Resolution::Inconclusive;
        };
        let text = match timeout_at(deadline, read_snapshot(&self.root, file, hash)).await {
            Ok(Some(text)) => text,
            Ok(None) => return Resolution::Inconclusive,
            Err(_) => return Resolution::Timeout,
        };
        self.attempted += 1;
        self.zero_candidate_requests += usize::from(zero_candidates);
        let locations = match timeout_at(
            deadline,
            bridge.definition_for_trace(
                &self.root,
                lang,
                file,
                site.location.line,
                character,
                &text,
            ),
        )
        .await
        {
            Ok(Ok(Some(locations))) => locations,
            Ok(_) => return Resolution::Inconclusive,
            Err(_) => return Resolution::Timeout,
        };
        let answer = match timeout_at(deadline, classify(graph, &self.root, &locations)).await {
            Ok(answer) => answer,
            Err(_) => return Resolution::Timeout,
        };
        let target_file = match answer {
            Resolution::Project(target) => Some(graph.nodes[target].location.file.as_str()),
            Resolution::External => None,
            _ => return answer,
        };
        // A watcher or editor may have invalidated either side while awaiting
        // the server. Recheck both before pruning or replacing any static edge.
        for name in
            std::iter::once(file.as_str()).chain(target_file.filter(|&target| target != file))
        {
            let Some(hash) = graph.hashes.get(name) else {
                return Resolution::Inconclusive;
            };
            match timeout_at(deadline, read_snapshot(&self.root, name, hash)).await {
                Ok(Some(_)) => (),
                Ok(None) => return Resolution::Inconclusive,
                Err(_) => return Resolution::Timeout,
            }
        }
        answer
    }
}

async fn classify(graph: &Graph, root: &Path, locations: &[Location]) -> Resolution {
    let [target] = locations else {
        return if locations.is_empty() {
            Resolution::NoResult
        } else {
            Resolution::Inconclusive
        };
    };
    let target_path = Path::new(&target.file);
    if target_path.is_absolute() {
        // A path in the same root may be unindexed, or have a different
        // spelling (drive-letter case, dot components, etc.). Never remove a
        // static edge based on an absolute path we cannot canonicalize.
        let path = target_path.to_owned();
        let root = root.to_owned();
        return if tokio::task::spawn_blocking(move || {
            path.canonicalize()
                .is_ok_and(|path| !path.starts_with(root))
        })
        .await
        .unwrap_or(false)
        {
            Resolution::External
        } else {
            Resolution::Inconclusive
        };
    }
    let mut matches = graph
        .definitions_by_file
        .get(&target.file)
        .into_iter()
        .flatten()
        .copied()
        // An arbitrary definition inside a body can be a local binding, not
        // the containing function. Prefer a conservative exact-line match.
        .filter(|&id| graph.nodes[id].location.line == target.line);
    let Some(hit) = matches.next() else {
        return Resolution::Inconclusive;
    };
    // Do not guess where multiple definitions occupy the target line. The
    // current indexed definition spans have no columns.
    if matches.next().is_some() {
        return Resolution::Inconclusive;
    }
    Resolution::Project(hit)
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
