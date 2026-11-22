//! 渲染层：只读 App，全部 ratatui 画布。TestBackend 单测消费同一入口。

use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Clear, List, ListItem, ListState, Paragraph, Row, Table, Tabs, Wrap,
};
use ratatui::Frame;

use codeidx_protocol::tools::{SearchMode, SymbolCode};

use crate::app::{App, Dialog, Ev, Focus, LspKind, Tab};

/// spinner 帧（mounting / busy）。
const SPINNER: [&str; 8] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧"];

/// 全屏渲染入口。
pub fn draw(f: &mut Frame, app: &App) {
    let full = f.area();
    // 主区 + toast(可选) + 状态栏。
    let constraints = if app.toast.is_some() {
        vec![
            Constraint::Min(0),
            Constraint::Length(1),
            Constraint::Length(1),
        ]
    } else {
        vec![Constraint::Min(0), Constraint::Length(1)]
    };
    let mut chunks = Layout::vertical(constraints).split(full).to_vec();
    let status = chunks.pop().expect("status area");
    let toast_area = if app.toast.is_some() {
        Some(chunks.pop().expect("toast area"))
    } else {
        None
    };
    let main = chunks.pop().expect("main area");

    if app.on_mount_page() || app.mount_page.busy.is_some() {
        draw_mount_page(f, main, app);
    } else {
        draw_tabs(f, main, app);
    }
    if let (Some(area), Some(toast)) = (toast_area, &app.toast) {
        let line = Line::from(vec![
            Span::styled(
                " toast ",
                Style::new().bg(Color::Yellow).fg(Color::Black).bold(),
            ),
            Span::raw(" "),
            Span::raw(toast.text.clone()),
        ]);
        f.render_widget(Paragraph::new(line), area);
    }
    draw_status(f, status, app);
    draw_dialog(f, full, app);
}

/// 未挂载页：路径输入 + HOME 目录列表 + vector 开关。
fn draw_mount_page(f: &mut Frame, area: Rect, app: &App) {
    let page = &app.mount_page;
    let [hint, input_area, list_area, err_area] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(3),
        Constraint::Min(0),
        Constraint::Length(2),
    ])
    .areas(area);

    let head = Line::from(vec![
        Span::styled("codeidx-tui", Style::new().fg(Color::Cyan).bold()),
        Span::raw(" — 本地代码索引终端前端  "),
        Span::styled(
            if page.enable_vector {
                "[Ctrl+V] vector: ON"
            } else {
                "[Ctrl+V] vector: OFF"
            },
            if page.enable_vector {
                Style::new().fg(Color::Green).bold()
            } else {
                Style::new().fg(Color::DarkGray)
            },
        ),
    ]);
    f.render_widget(Paragraph::new(head), hint);

    if let Some(busy) = &page.busy {
        let spin = SPINNER[(app.tick as usize) % SPINNER.len()];
        f.render_widget(
            Paragraph::new(Line::from(format!("{spin} mounting {} …", busy.display())))
                .style(Style::new().fg(Color::Yellow)),
            input_area,
        );
    } else {
        let input = if page.input.is_empty() {
            "输入项目根路径…".to_string()
        } else {
            page.input.clone()
        };
        f.render_widget(
            Paragraph::new(input).block(Block::bordered().title(Span::styled(
                "path (Enter: mount)",
                Style::new().fg(Color::Cyan).bold(),
            ))),
            input_area,
        );
    }

    let block = Block::bordered().title("HOME 目录 (↑↓: 填充路径)");
    let visible = block.inner(list_area).height as usize;
    let mut state = list_viewport(page.home_dirs.len(), page.sel, visible);
    let items = page.home_dirs.iter().map(|dir| ListItem::new(dir.as_str()));
    f.render_stateful_widget(
        List::new(items)
            .block(block)
            .style(Style::new().fg(Color::DarkGray))
            .highlight_style(Style::new().fg(Color::Black).bg(Color::Cyan)),
        list_area,
        &mut state,
    );

    let tip = if page.err.is_empty() {
        "Enter: mount   Ctrl+V: enable_vector 开关   ↑↓: 选目录   Ctrl+C: 强制退出".to_string()
    } else {
        page.err.clone()
    };
    let style = if page.err.is_empty() {
        Style::new().fg(Color::DarkGray)
    } else {
        Style::new().fg(Color::Red)
    };
    f.render_widget(Paragraph::new(tip).style(style), err_area);
}

/// 从选中项和当前视口高度推导滚动位置，终端缩放时也能保持可见。
pub(crate) fn list_viewport(len: usize, selected: usize, visible: usize) -> ListState {
    let Some(last) = len.checked_sub(1) else {
        return ListState::default();
    };
    let selected = selected.min(last);
    let offset = selected
        .saturating_sub(visible / 2)
        .min(len.saturating_sub(visible));
    ListState::default()
        .with_offset(offset)
        .with_selected(Some(selected))
}

/// 挂载后主区：tab 标题栏 + tab 内容。
fn draw_tabs(f: &mut Frame, area: Rect, app: &App) {
    let [bar, body] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);
    let titles = [
        Tab::Skeleton,
        Tab::Search,
        Tab::Outline,
        Tab::Lsp,
        Tab::Projects,
    ];
    let selected = titles.iter().position(|t| *t == app.tab).unwrap_or(0);
    let tabs = Tabs::new(titles.iter().map(|t| Line::from(t.title())))
        .highlight_style(Style::new().fg(Color::Black).bg(Color::Cyan).bold())
        .select(selected);
    f.render_widget(tabs, bar);
    match app.tab {
        Tab::Skeleton => draw_skeleton(f, body, app),
        Tab::Search => draw_search(f, body, app),
        Tab::Outline => draw_outline(f, body, app),
        Tab::Lsp => draw_lsp(f, body, app),
        Tab::Projects => draw_projects(f, body, app),
    }
}

fn draw_skeleton(f: &mut Frame, area: Rect, app: &App) {
    let s = &app.skeleton;
    let title = format!(
        "repo_map  budget={} (PgUp/PgDn: ±500, 500-8000)  ↑↓: scroll",
        s.budget
    );
    let body = if s.busy {
        let spin = SPINNER[(app.tick as usize) % SPINNER.len()];
        format!("{spin} building repo_map…")
    } else if !s.err.is_empty() {
        format!("error: {}", s.err)
    } else if s.text.is_empty() {
        "(empty)".to_string()
    } else {
        s.text.clone()
    };
    let style = if !s.err.is_empty() {
        Style::new().fg(Color::Red)
    } else {
        Style::new()
    };
    f.render_widget(
        Paragraph::new(body)
            .style(style)
            .block(Block::bordered().title(title))
            .scroll((s.scroll, 0)),
        area,
    );
}

fn draw_search(f: &mut Frame, area: Rect, app: &App) {
    let s = &app.search;
    let [mode_line, input_area, table_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(3),
        Constraint::Min(0),
    ])
    .areas(area);

    let mode_span = |m: SearchMode, label: &'static str| {
        let on = s.mode == m;
        Span::styled(
            label,
            if on {
                Style::new().fg(Color::Cyan).bold()
            } else {
                Style::new().fg(Color::DarkGray)
            },
        )
    };
    let mut mode_line_spans = vec![
        Span::raw("Tab: mode ["),
        mode_span(SearchMode::Keyword, "keyword"),
        Span::raw("|"),
        mode_span(SearchMode::Hybrid, "hybrid"),
        Span::raw("|"),
        mode_span(SearchMode::Vector, "vector"),
        Span::raw("]  "),
        Span::styled(format!("top_k={} ", s.top_k), Style::new().bold()),
        Span::raw("(+/-)  Enter: search"),
    ];
    if let Some(outcome) = &s.outcome {
        mode_line_spans.push(Span::raw(format!(
            "  → {} mode={} {}ms",
            outcome.hits.len(),
            mode_name(outcome.effective_mode),
            outcome.elapsed_ms
        )));
        if outcome.degraded {
            mode_line_spans.push(Span::styled(
                format!(" (degraded: {})", outcome.degraded_reason),
                Style::new().fg(Color::Yellow),
            ));
        }
    }
    f.render_widget(Paragraph::new(Line::from(mode_line_spans)), mode_line);

    let focused = s.focus == Focus::Input;
    let border_style = if focused {
        Style::new().fg(Color::Cyan)
    } else {
        Style::new().fg(Color::DarkGray)
    };
    let input = if s.query.is_empty() {
        "输入检索词…"
    } else {
        s.query.as_str()
    };
    let busy_mark = if s.busy {
        format!(" {}", SPINNER[(app.tick as usize) % SPINNER.len()])
    } else {
        String::new()
    };
    f.render_widget(
        Paragraph::new(format!("{input}{busy_mark}")).block(
            Block::bordered()
                .title(Span::styled("query", Style::new().bold()))
                .border_style(border_style),
        ),
        input_area,
    );

    if !s.err.is_empty() {
        f.render_widget(
            Paragraph::new(format!("error: {}", s.err)).style(Style::new().fg(Color::Red)),
            table_area,
        );
        return;
    }
    let rows: Vec<Row> = s
        .results
        .iter()
        .enumerate()
        .map(|(i, hit)| {
            let style = if i == s.selected && s.focus == Focus::Results {
                Style::new().bg(Color::Cyan).fg(Color::Black)
            } else {
                Style::new()
            };
            Row::new(vec![
                hit.file.clone(),
                hit.symbol.clone().unwrap_or_else(|| "-".into()),
                format!("{}-{}", hit.start_line, hit.end_line),
                format!("{:.3}", hit.score),
                snippet_one_line(&hit.snippet),
            ])
            .style(style)
        })
        .collect();
    let header = Row::new(vec!["file", "symbol", "lines", "score", "snippet"])
        .style(Style::new().fg(Color::Yellow).bold());
    let table = Table::new(
        rows,
        [
            Constraint::Percentage(24),
            Constraint::Percentage(18),
            Constraint::Percentage(9),
            Constraint::Percentage(9),
            Constraint::Percentage(40),
        ],
    )
    .header(header)
    .block(Block::bordered().title("results (↑↓: select  Enter: 下钻  Esc: back)"));
    f.render_widget(table, table_area);
}

fn draw_outline(f: &mut Frame, area: Rect, app: &App) {
    let o = &app.outline;
    let [input_area, table_area] =
        Layout::vertical([Constraint::Length(3), Constraint::Min(0)]).areas(area);
    let focused = o.focus == Focus::Input;
    let border_style = if focused {
        Style::new().fg(Color::Cyan)
    } else {
        Style::new().fg(Color::DarkGray)
    };
    let input = if o.file.is_empty() {
        "输入相对路径（如 src/lib.rs）"
    } else {
        o.file.as_str()
    };
    let busy_mark = if o.busy {
        format!(" {}", SPINNER[(app.tick as usize) % SPINNER.len()])
    } else {
        String::new()
    };
    f.render_widget(
        Paragraph::new(format!("{input}{busy_mark}")).block(
            Block::bordered()
                .title(Span::styled(
                    "file_path (Enter: outline)",
                    Style::new().bold(),
                ))
                .border_style(border_style),
        ),
        input_area,
    );
    if !o.err.is_empty() {
        f.render_widget(
            Paragraph::new(format!("error: {}", o.err)).style(Style::new().fg(Color::Red)),
            table_area,
        );
        return;
    }
    let rows: Vec<Row> = o
        .entries
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let style = if i == o.selected && o.focus == Focus::Results {
                Style::new().bg(Color::Cyan).fg(Color::Black)
            } else {
                Style::new()
            };
            Row::new(vec![
                e.kind.clone(),
                e.name.clone(),
                format!("{}-{}", e.start_line, e.end_line),
                e.sig.clone(),
            ])
            .style(style)
        })
        .collect();
    let header =
        Row::new(vec!["kind", "name", "lines", "sig"]).style(Style::new().fg(Color::Yellow).bold());
    let table = Table::new(
        rows,
        [
            Constraint::Percentage(8),
            Constraint::Percentage(22),
            Constraint::Percentage(10),
            Constraint::Percentage(60),
        ],
    )
    .header(header)
    .block(Block::bordered().title("outline (↑↓: select  Enter: 下钻全文)"));
    f.render_widget(table, table_area);
}

fn draw_lsp(f: &mut Frame, area: Rect, app: &App) {
    let l = &app.lsp;
    let [hint, input_area, results_area] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(3),
        Constraint::Min(0),
    ])
    .areas(area);

    let mut hint_line = vec![Span::raw("Enter: definition  Ctrl+R: references  ")];
    if l.ever_spawned {
        hint_line.push(Span::styled(
            "language server 已就绪",
            Style::new().fg(Color::Green),
        ));
    } else if l.busy {
        hint_line.push(Span::styled(
            format!(
                "{} 语言服务器冷启动中（首次调用需数秒）…",
                SPINNER[(app.tick as usize) % SPINNER.len()]
            ),
            Style::new().fg(Color::Yellow),
        ));
    } else {
        hint_line.push(Span::styled(
            "首次调用将拉起语言服务器（rust-analyzer 等冷启动需数秒）",
            Style::new().fg(Color::DarkGray),
        ));
    }
    f.render_widget(Paragraph::new(Line::from(hint_line)), hint);

    let busy_mark = if l.busy {
        format!(" {}", SPINNER[(app.tick as usize) % SPINNER.len()])
    } else {
        String::new()
    };
    let input = if l.input.is_empty() {
        "rel_path:line:character  如 src/main.rs:6:10"
    } else {
        l.input.as_str()
    };
    f.render_widget(
        Paragraph::new(format!("{input}{busy_mark}"))
            .block(Block::bordered().title(Span::styled("position", Style::new().bold()))),
        input_area,
    );

    if !l.err.is_empty() {
        f.render_widget(
            Paragraph::new(format!("error: {}", l.err)).style(Style::new().fg(Color::Red)),
            results_area,
        );
        return;
    }
    let items: Vec<ListItem> = l
        .locations
        .iter()
        .enumerate()
        .map(|(i, loc)| {
            let style = if i == l.selected {
                Style::new().bg(Color::Cyan).fg(Color::Black)
            } else {
                Style::new()
            };
            ListItem::new(
                Line::from(format!(
                    "{}:{}:{}  (col {})",
                    loc.file, loc.line, loc.character, loc.character
                ))
                .style(style),
            )
        })
        .collect();
    let title = format!("{} ({} hits)", l.kind.title(), l.locations.len());
    f.render_widget(
        List::new(items).block(Block::bordered().title(title)),
        results_area,
    );
}

fn draw_projects(f: &mut Frame, area: Rect, app: &App) {
    let [list_area, tip] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(2)]).areas(area);
    let items: Vec<ListItem> = app
        .projects
        .iter()
        .map(|p| {
            let active_mark = app
                .active
                .as_ref()
                .map(|a| a.to_string_lossy() == p.path)
                .unwrap_or(false);
            let head = Line::from(vec![
                if active_mark {
                    Span::styled("▶ ", Style::new().fg(Color::Green).bold())
                } else {
                    Span::raw("  ")
                },
                Span::styled(p.path.clone(), Style::new().bold()),
            ]);
            let mut detail = vec![
                Span::raw(format!("  files={} symbols={}", p.files, p.symbols)),
                Span::raw(format!("  vector={}", p.vector)),
            ];
            if p.vector_degraded {
                detail.push(Span::styled(
                    format!(" (degraded: {})", p.vector_degraded_reason),
                    Style::new().fg(Color::Yellow),
                ));
            }
            let lsps = if p.lsps.is_empty() {
                "-".to_string()
            } else {
                p.lsps.join(",")
            };
            let lsp_line = Line::from(vec![Span::styled(
                format!("  lsps={}", lsps),
                Style::new().fg(Color::DarkGray),
            )]);
            ListItem::new(vec![head, Line::from(detail), lsp_line, Line::from("")])
        })
        .collect();
    f.render_widget(
        List::new(items).block(Block::bordered().title("mounted projects")),
        list_area,
    );
    f.render_widget(
        Paragraph::new("s: sync  u: unmount (确认)  Enter: 刷新"),
        tip,
    );
}

/// 状态栏：挂载数 / VmRSS / tab / 快捷键摘要。
fn draw_status(f: &mut Frame, area: Rect, app: &App) {
    let rss = if app.rss_kb == 0 {
        "rss: n/a".to_string()
    } else {
        if app.rss_kb >= 1024 {
            format!("rss: {:.1}MB", app.rss_kb as f64 / 1024.0)
        } else {
            format!("rss: {}kB", app.rss_kb)
        }
    };
    let busy_mark = if app.mount_page.busy.is_some() || app.skeleton.busy || app.search.busy {
        format!("{} ", SPINNER[(app.tick as usize) % SPINNER.len()])
    } else {
        String::new()
    };
    let spans = vec![
        Span::styled(busy_mark, Style::new().fg(Color::Yellow)),
        Span::styled(
            format!(" mounted:{} ", app.projects.len()),
            Style::new().fg(Color::Black).bg(Color::Cyan),
        ),
        Span::styled(format!(" {rss} "), Style::new()),
        Span::styled(format!(" tab:{} ", app.tab.title()), Style::new().bold()),
        Span::styled(
            " F1-F5:tab  q:quit  ?:keys",
            Style::new().fg(Color::DarkGray),
        ),
    ];
    f.render_widget(
        Paragraph::new(Line::from(spans)).alignment(Alignment::Left),
        area,
    );
}

/// 弹层（确认 / 符号全文 / 帮助）。
fn draw_dialog(f: &mut Frame, full: Rect, app: &App) {
    match &app.dialog {
        Dialog::None => {}
        Dialog::ConfirmQuit => {
            let area = centered(full, 64, 3, 3);
            f.render_widget(Clear, area);
            f.render_widget(
                Paragraph::new("仍有挂载项目。确认退出？ (y: 退出 / n: 取消)")
                    .block(
                        Block::bordered()
                            .title("quit")
                            .style(Style::new().fg(Color::Red)),
                    )
                    .alignment(Alignment::Center),
                area,
            );
        }
        Dialog::ConfirmUnmount(path) => {
            let area = centered(full, 75, 3, 4);
            f.render_widget(Clear, area);
            f.render_widget(
                Paragraph::new(format!(
                    "确认卸载（释放内存并 kill 关联 LSP）？
{}",
                    path.display()
                ))
                .block(
                    Block::bordered()
                        .title("unmount")
                        .style(Style::new().fg(Color::Red)),
                )
                .alignment(Alignment::Center),
                area,
            );
        }
        Dialog::SymbolText(code) => {
            let area = centered(full, 90, 70, 3);
            f.render_widget(Clear, area);
            let title = format!(
                "{}  {}:{}-{}",
                code.name, code.file, code.start_line, code.end_line
            );
            f.render_widget(
                Paragraph::new(code.code.clone())
                    .wrap(Wrap { trim: false })
                    .block(
                        Block::bordered()
                            .title(title)
                            .style(Style::new().fg(Color::Cyan)),
                    )
                    .scroll((0, 0)),
                area,
            );
        }
        Dialog::Help => {
            let area = centered(full, 80, 60, 3);
            f.render_widget(Clear, area);
            let help = [
                "全局    F1-F5 tab  q 退出(挂载中二次确认)  ? 帮助  Ctrl+C 强制退出",
                "启动页  Enter 挂载  Ctrl+V enable_vector 开关  ↑↓ 从 HOME 目录填充路径",
                "Skeleton  ↑↓ 滚动  PgUp/PgDn token_budget ±500 (500-8000)",
                "Search  Tab 循环 keyword/hybrid/vector  +/- top_k  Enter 搜索",
                "        结果区 ↑↓ 选择  Enter 下钻符号全文  Esc 返回输入",
                "Outline  输入相对路径 Enter  Enter 下钻  Esc 返回",
                "Lsp     输入 rel:line:col  Enter definition  Ctrl+R references",
                "        首次调用冷启动需数秒",
                "Projects  s: sync  u: unmount(确认)  Enter: 刷新",
                "",
                "Esc 关闭本帮助",
            ];
            f.render_widget(
                Paragraph::new(help.join("\n")).block(
                    Block::bordered()
                        .title("快捷键速查")
                        .style(Style::new().fg(Color::Cyan)),
                ),
                area,
            );
        }
    }
}

/// 居中矩形（百分比弹层）。
fn centered(full: Rect, pct_w: u16, pct_h: u16, min_height: u16) -> Rect {
    let w = (full.width.saturating_mul(pct_w) / 100)
        .clamp(0, full.width)
        .max(10.min(full.width));
    let h = (full.height.saturating_mul(pct_h) / 100)
        .max(min_height)
        .min(full.height);
    let x = full.x + (full.width.saturating_sub(w)) / 2;
    let y = full.y + (full.height.saturating_sub(h)) / 2;
    Rect::new(x, y, w, h)
}

fn mode_name(m: SearchMode) -> &'static str {
    match m {
        SearchMode::Keyword => "keyword",
        SearchMode::Hybrid => "hybrid",
        SearchMode::Vector => "vector",
    }
}

/// snippet 压成单行（≤ 3 行文本入表格单元格）。
fn snippet_one_line(s: &str) -> String {
    let mut out = s.replace('\n', " ⏎ ");
    if let Some((end, _)) = out.char_indices().nth(120) {
        out.truncate(end);
        out.push('…');
    }
    out
}

/// 供 selftest / 单测断言用：渲染一帧并抽取全部可见文本。
pub fn render_text(app: &App, width: u16, height: u16) -> String {
    let backend = ratatui::backend::TestBackend::new(width, height);
    let mut terminal = ratatui::Terminal::new(backend).expect("test terminal");
    terminal.draw(|f| draw(f, app)).expect("draw");
    let mut buf = String::new();
    for row in terminal
        .backend()
        .buffer()
        .content
        .chunks(usize::from(width.max(1)))
    {
        let mut cells = row.iter();
        while let Some(cell) = cells.next() {
            let symbol = cell.symbol();
            buf.push_str(symbol);
            for _ in 1..Span::raw(symbol).width() {
                cells.next();
            }
        }
        buf.push('\n');
    }
    buf
}

/// 供类型断言使用的再导出（防止 unused import 误报）。
#[allow(dead_code)]
fn _ev_witness(_: Option<Ev>, _: Option<SymbolCode>, _: Option<LspKind>) {}
