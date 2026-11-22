//! Shared call trace model for MCP and the native terminal interface.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceDirection {
    #[default]
    Down,
    Up,
    Both,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceBranch {
    Down,
    Up,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceConfidence {
    Unique,
    SameFile,
    Ambiguous,
    Lsp,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TraceLocation {
    pub file: String,
    pub line: u32,
    pub symbol: String,
}

#[derive(Debug, Clone)]
pub struct TraceOptions {
    pub symbol: Option<String>,
    pub file_path: Option<String>,
    pub from: Option<TraceLocation>,
    pub direction: TraceDirection,
    pub max_depth: usize,
    pub max_roots: usize,
    pub max_nodes: usize,
    pub budget: usize,
    pub refine_lsp: bool,
}

impl Default for TraceOptions {
    fn default() -> Self {
        Self {
            symbol: None,
            file_path: None,
            from: None,
            direction: TraceDirection::Down,
            max_depth: 5,
            max_roots: 10,
            max_nodes: 128,
            budget: 1200,
            refine_lsp: true,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct TraceTruncation {
    /// Number of automatic roots omitted by max_roots.
    pub roots: usize,
    /// Number of known direct children omitted at the depth frontier.
    pub depth: usize,
    /// Number of known roots or direct children omitted by max_nodes.
    pub nodes: usize,
    /// Number of known roots or direct children omitted by the compact body budget.
    pub budget: usize,
    /// Number of candidate definitions omitted at encountered sites by the index cap.
    pub candidates: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct TraceNode {
    pub location: TraceLocation,
    pub confidence: Option<TraceConfidence>,
    pub branch: Option<TraceBranch>,
    pub candidates: usize,
    pub cycle: bool,
    pub shared: bool,
    pub dynamic: bool,
    pub callback: bool,
    pub inferred: bool,
    pub file_root: bool,
    /// Number of known direct children suppressed. Descendants were not counted.
    pub truncated: usize,
    pub children: Vec<TraceNode>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TraceReport {
    pub direction: TraceDirection,
    pub roots: Vec<TraceNode>,
    pub max_depth: usize,
    pub nodes: usize,
    pub edges: usize,
    pub truncated: TraceTruncation,
    pub lsp_attempted: usize,
    pub lsp_refined: usize,
    pub lsp_degraded: bool,
    pub skipped: usize,
}
