use codeidx_project::callflow::{TraceBranch, TraceConfidence, TraceDirection};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, List, ListItem, Paragraph};
use ratatui::Frame;

use crate::call_tree::CallTree;

pub(super) fn draw(f: &mut Frame, area: Rect, tree: &CallTree) {
    let [status, list_area, help] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(0),
        Constraint::Length(2),
    ])
    .areas(area);
    let status_text = if tree.busy() {
        "正在查询调用链…".to_owned()
    } else if !tree.error.is_empty() {
        tree.error.clone()
    } else if let Some(report) = tree.report() {
        let direction = match report.direction {
            TraceDirection::Down => "被调用方",
            TraceDirection::Up => "调用方",
            TraceDirection::Both => "双向",
        };
        let tr = &report.truncated;
        let total = tr.roots + tr.depth + tr.nodes + tr.budget + tr.candidates;
        let lsp = if !tree.refine {
            "关闭"
        } else if report.lsp_degraded {
            "部分回退到静态"
        } else {
            "按需"
        };
        format!("{direction} · {} 根 · {} 节点 · {} 边 · 省略 {total} · 跳过 {}\nLSP {lsp} · 已精化 {}/{}", report.roots.len(), report.nodes, report.edges, report.skipped, report.lsp_refined, report.lsp_attempted)
    } else {
        "按 r 查询调用链".to_owned()
    };
    let status_style = if !tree.error.is_empty() {
        Style::new().fg(Color::Red)
    } else {
        Style::new().fg(Color::Cyan)
    };
    f.render_widget(Paragraph::new(status_text).style(status_style), status);

    let block = Block::bordered().title("调用链");
    let rows = tree.rows();
    let mut state = super::list_viewport(
        rows.len(),
        tree.selected(),
        block.inner(list_area).height as usize,
    );
    if rows.is_empty() {
        let message = if tree.report().is_some() {
            "没有可显示的入口"
        } else {
            ""
        };
        f.render_widget(Paragraph::new(message).block(block), list_area);
    } else {
        let items = rows.iter().map(|row| {
            let node = row.node;
            let indent = row.indent();
            let fold = if node.children.is_empty() {
                '·'
            } else if row.collapsed {
                '▸'
            } else {
                '▾'
            };
            let direction = match node.branch {
                Some(TraceBranch::Up) => "↑ ",
                Some(TraceBranch::Down) => "↓ ",
                None => "",
            };
            let mut marks = String::new();
            match node.confidence {
                Some(TraceConfidence::Lsp) => marks.push_str(" ✓"),
                Some(TraceConfidence::Ambiguous) => {
                    marks.push_str(&format!(" ?{}", node.candidates))
                }
                Some(TraceConfidence::Unique | TraceConfidence::SameFile) => {
                    marks.push_str(" 静态")
                }
                None => {}
            }
            if node.cycle {
                marks.push_str(" ↻环");
            }
            if node.shared {
                marks.push_str(" ↪共享");
            }
            if node.dynamic {
                marks.push_str(" ⇢未解析");
            }
            if node.callback {
                marks.push_str(" 回调");
            }
            if node.inferred {
                marks.push_str(" 推断");
            }
            if node.truncated > 0 {
                marks.push_str(&format!(" …+{}", node.truncated));
            }
            let at = &node.location;
            ListItem::new(format!(
                "{:<indent$}{fold} {direction}{}{}  {}:{}",
                "", at.symbol, marks, at.file, at.line
            ))
        });
        f.render_stateful_widget(
            List::new(items)
                .block(block)
                .highlight_style(Style::new().fg(Color::Black).bg(Color::Cyan)),
            list_area,
            &mut state,
        );
    }
    f.render_widget(
        Paragraph::new("↑↓ 选择   ←→ 折叠/展开   Enter 源码   t 从选中项追踪\nEsc 返回   d 方向   l LSP   r 刷新根列表").style(Style::new().fg(Color::DarkGray)),
        help,
    );
}
