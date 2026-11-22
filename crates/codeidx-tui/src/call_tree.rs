//! Terminal navigation over a bounded result from the shared call-trace engine.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use codeidx_project::callflow::{TraceDirection, TraceLocation, TraceNode, TraceReport};
use crossterm::event::KeyCode;

use crate::app::Cmd;

const HISTORY_LIMIT: usize = 8;

#[derive(Debug, Clone, Default)]
struct View {
    report: Option<TraceReport>,
    from: Option<TraceLocation>,
    collapsed: HashSet<Vec<usize>>,
    selected: usize,
}

#[derive(Debug, Clone, Default)]
pub struct CallTree {
    view: View,
    history: Vec<View>,
    next_id: u64,
    pending: Option<u64>,
    pub direction: TraceDirection,
    pub refine: bool,
    pub error: String,
}

pub struct Row<'a> {
    key: Vec<usize>,
    pub node: &'a TraceNode,
    pub collapsed: bool,
}

impl Row<'_> {
    pub fn indent(&self) -> usize {
        self.key.len().saturating_sub(1) * 2
    }
}

impl CallTree {
    pub fn new() -> Self {
        Self {
            refine: true,
            ..Self::default()
        }
    }

    pub fn reset(&mut self) {
        let next_id = self.next_id;
        *self = Self {
            next_id,
            ..Self::new()
        };
    }

    pub fn busy(&self) -> bool {
        self.pending.is_some()
    }

    pub fn report(&self) -> Option<&TraceReport> {
        self.view.report.as_ref()
    }

    pub fn selected(&self) -> usize {
        self.view.selected
    }

    pub fn rows(&self) -> Vec<Row<'_>> {
        fn visit<'a>(
            nodes: &'a [TraceNode],
            path: &mut Vec<usize>,
            collapsed: &HashSet<Vec<usize>>,
            out: &mut Vec<Row<'a>>,
        ) {
            for (index, node) in nodes.iter().enumerate() {
                path.push(index);
                let hidden = collapsed.contains(path);
                out.push(Row {
                    key: path.clone(),
                    node,
                    collapsed: hidden,
                });
                if !hidden {
                    visit(&node.children, path, collapsed, out);
                }
                path.pop();
            }
        }
        let mut rows = Vec::new();
        if let Some(report) = self.report() {
            visit(
                &report.roots,
                &mut Vec::new(),
                &self.view.collapsed,
                &mut rows,
            );
        }
        rows
    }

    pub fn enter(&mut self, path: PathBuf) -> Vec<Cmd> {
        if self.report().is_none() && !self.busy() {
            return self.request(path, None);
        }
        Vec::new()
    }

    fn request(&mut self, path: PathBuf, from: Option<TraceLocation>) -> Vec<Cmd> {
        self.next_id = self.next_id.wrapping_add(1);
        self.pending = Some(self.next_id);
        self.error.clear();
        vec![Cmd::TraceCalls {
            path,
            id: self.next_id,
            from,
            direction: self.direction,
            refine: self.refine,
        }]
    }

    pub fn apply(
        &mut self,
        id: u64,
        from: Option<TraceLocation>,
        result: Result<TraceReport, String>,
    ) {
        if self.pending != Some(id) {
            return;
        }
        self.pending = None;
        match result {
            Ok(report) => {
                let last = std::mem::replace(
                    &mut self.view,
                    View {
                        report: Some(report),
                        from: from.clone(),
                        ..View::default()
                    },
                );
                if from.is_none() {
                    self.history.clear();
                } else if last.report.is_some() && last.from != from {
                    if self.history.len() == HISTORY_LIMIT {
                        self.history.remove(0);
                    }
                    self.history.push(last);
                }
            }
            Err(error) => self.error = error,
        }
    }

    pub fn handle(&mut self, path: &Path, key: KeyCode) -> Vec<Cmd> {
        let rows = self.rows();
        let row = rows.get(self.view.selected);
        match key {
            KeyCode::Up => self.view.selected = self.view.selected.saturating_sub(1),
            KeyCode::Down => {
                self.view.selected = (self.view.selected + 1).min(rows.len().saturating_sub(1))
            }
            KeyCode::Home => self.view.selected = 0,
            KeyCode::End => self.view.selected = rows.len().saturating_sub(1),
            KeyCode::Left => {
                if let Some(row) = row {
                    if !row.node.children.is_empty() && !row.collapsed {
                        let key = row.key.clone();
                        self.view.collapsed.insert(key);
                    } else if row.key.len() > 1 {
                        let parent = &row.key[..row.key.len() - 1];
                        if let Some(index) = rows.iter().position(|r| r.key == parent) {
                            self.view.selected = index;
                        }
                    }
                }
            }
            KeyCode::Right => {
                if let Some(row) = row.filter(|r| !r.node.children.is_empty()) {
                    if row.collapsed {
                        let key = row.key.clone();
                        self.view.collapsed.remove(&key);
                    } else {
                        self.view.selected += 1;
                    }
                }
            }
            KeyCode::Enter if !self.busy() => {
                if let Some(row) = row.filter(|r| !r.node.dynamic && !r.node.file_root) {
                    return vec![Cmd::ReadTraceSymbol {
                        path: path.to_path_buf(),
                        location: row.node.location.clone(),
                    }];
                }
            }
            KeyCode::Char('t') if !self.busy() => {
                if let Some(row) = row.filter(|r| !r.node.dynamic) {
                    return self.request(path.to_path_buf(), Some(row.node.location.clone()));
                }
            }
            KeyCode::Esc if !self.busy() => {
                if let Some(last) = self.history.pop() {
                    if let Some(report) = &last.report {
                        self.direction = report.direction;
                    }
                    self.view = last;
                    self.error.clear();
                }
            }
            KeyCode::Char('r') if !self.busy() => return self.request(path.to_path_buf(), None),
            KeyCode::Char('d') if !self.busy() => {
                self.direction = match self.direction {
                    TraceDirection::Down => TraceDirection::Up,
                    TraceDirection::Up => TraceDirection::Both,
                    TraceDirection::Both => TraceDirection::Down,
                };
                return self.request(path.to_path_buf(), self.view.from.clone());
            }
            KeyCode::Char('l') if !self.busy() => {
                self.refine = !self.refine;
                return self.request(path.to_path_buf(), self.view.from.clone());
            }
            _ => {}
        }
        Vec::new()
    }
}
