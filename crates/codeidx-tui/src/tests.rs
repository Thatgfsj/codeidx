//! 单测：渲染层（TestBackend 逐帧断言关键文本）+ 状态机（handle_key /
//! apply_ev 的 Cmd 纯函数断言）。全部无需 tty、无需真实引擎。

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use codeidx_protocol::tools::{
    MountReport, OutlineEntry, ProjectInfo, SearchHit, SearchMode, SymbolCode,
};

use crate::app::{self, App, Cmd, Dialog, Ev, Focus, LspKind, Tab};
use crate::ui;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn char_key(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
}

fn mounted_app() -> App {
    App {
        active: Some("/tmp/proj".into()),
        projects: vec![ProjectInfo {
            path: "/tmp/proj".into(),
            files: 3,
            symbols: 10,
            lsps: vec!["rust-analyzer".into()],
            vector: false,
            vector_degraded: false,
            vector_degraded_reason: String::new(),
        }],
        ..App::default()
    }
}

// ---------------------------------------------------------------- 渲染单测 --

#[test]
fn render_mount_page_shows_title_vector_toggle_and_dirs() {
    let mut app = App::default();
    app.mount_page.home_dirs = vec!["/home/u/a".into(), "/home/u/b".into()];
    app.mount_page.sel = 1;
    let text = ui::render_text(&app, 100, 30);
    assert!(text.contains("codeidx-tui"), "title missing");
    assert!(text.contains("vector: OFF"), "vector toggle missing");
    assert!(text.contains("HOME 目录"), "dir list block missing");
    assert!(text.contains("/home/u/b"), "selected dir missing");
}

#[test]
fn home_list_scrolls_with_selection_in_a_short_viewport() {
    let mut app = App::default();
    app.mount_page.home_dirs = (0..24).map(|i| format!("/home/u/folder-{i:02}")).collect();
    let list_contains = |app: &App, dir: &str| {
        let text = ui::render_text(app, 50, 13);
        let list = text.split_once("HOME 目录").expect("list title").1;
        list.lines()
            .any(|line| line.starts_with('│') && line.contains(dir))
    };
    assert!(list_contains(&app, "folder-00"));
    assert!(!list_contains(&app, "folder-23"));
    for i in 1..24 {
        app.handle_key(key(KeyCode::Down));
        assert_eq!(app.mount_page.sel, i);
        assert!(list_contains(&app, &format!("folder-{i:02}")));
    }
    assert!(!list_contains(&app, "folder-00"));
    for i in (0..23).rev() {
        app.handle_key(key(KeyCode::Up));
        assert_eq!(app.mount_page.sel, i);
        assert!(list_contains(&app, &format!("folder-{i:02}")));
    }
}

#[test]
fn render_mounting_spinner() {
    let mut app = App::default();
    app.mount_page.busy = Some("/tmp/proj".into());
    let text = ui::render_text(&app, 100, 30);
    assert!(text.contains("mounting"), "spinner line missing");
}

#[test]
fn render_skeleton_tab_with_map_text_and_budget() {
    let mut app = mounted_app();
    app.skeleton.text = "# repo_map symbols=3 shown=3 budget=3000 truncated=0
src/lib.rs:10 fn main
"
    .into();
    let text = ui::render_text(&app, 100, 30);
    assert!(text.contains("# repo_map"), "meta header missing");
    assert!(text.contains("fn main"), "map line missing");
    assert!(text.contains("budget=3000"), "budget in title missing");
}

#[test]
fn render_search_results_table_and_degraded_hint() {
    let mut app = mounted_app();
    app.tab = Tab::Search;
    app.search.query = "unicorn".into();
    app.search.focus = Focus::Results;
    app.search.results = vec![
        SearchHit {
            file: "src/lib.rs".into(),
            symbol: Some("unicorn_marker_fn".into()),
            start_line: 24,
            end_line: 31,
            score: 2.5,
            snippet: "pub fn unicorn_marker_fn(cfg: &GoldConfig)".into(),
        },
        SearchHit {
            file: "app.py".into(),
            symbol: None,
            start_line: 1,
            end_line: 3,
            score: 1.1,
            snippet: "def app():".into(),
        },
    ];
    app.search.selected = 0;
    app.search.outcome = Some(crate::app::SearchOutcome {
        hits: app.search.results.clone(),
        effective_mode: SearchMode::Keyword,
        degraded: true,
        degraded_reason: "vector not enabled for this mount".into(),
        elapsed_ms: 5,
    });
    let text = ui::render_text(&app, 110, 40);
    assert!(text.contains("src/lib.rs"), "file column missing");
    assert!(text.contains("unicorn_marker_fn"), "symbol column missing");
    assert!(text.contains("24-31"), "lines column missing");
    assert!(text.contains("2.500"), "score column missing");
    assert!(text.contains("degraded"), "degraded hint missing");
}

#[test]
fn render_outline_entries() {
    let mut app = mounted_app();
    app.tab = Tab::Outline;
    app.outline.file = "src/lib.rs".into();
    app.outline.entries = vec![OutlineEntry {
        kind: "struct".into(),
        name: "GoldConfig".into(),
        start_line: 8,
        end_line: 13,
        sig: "pub struct GoldConfig".into(),
    }];
    let text = ui::render_text(&app, 100, 30);
    assert!(text.contains("GoldConfig"), "outline name missing");
    assert!(text.contains("8-13"), "outline lines missing");
    assert!(text.contains("struct"), "outline kind missing");
}

#[test]
fn render_lsp_tab_coldstart_and_results() {
    let mut app = mounted_app();
    app.tab = Tab::Lsp;
    let text = ui::render_text(&app, 100, 30);
    assert!(text.contains("冷启动"), "cold start hint missing");
    assert!(text.contains("position"), "position input missing");

    app.lsp.ever_spawned = true;
    app.lsp.locations = vec![codeidx_protocol::tools::Location {
        file: "src/utils.rs".into(),
        line: 4,
        character: 10,
    }];
    let text = ui::render_text(&app, 100, 30);
    assert!(text.contains("已就绪"), "ready hint missing");
    assert!(text.contains("src/utils.rs"), "location missing");
    assert!(text.contains("definition"), "kind title missing");
}

#[test]
fn render_projects_cards_and_status_bar() {
    let mut app = mounted_app();
    app.tab = Tab::Projects;
    app.rss_kb = 15 * 1024;
    let text = ui::render_text(&app, 100, 30);
    assert!(text.contains("/tmp/proj"), "card path missing");
    assert!(text.contains("files=3"), "card files missing");
    assert!(text.contains("rust-analyzer"), "card lsps missing");
    assert!(text.contains("mounted:1"), "status count missing");
    assert!(text.contains("rss: 15.0MB"), "status rss missing");
}

#[test]
fn render_dialogs() {
    let mut app = mounted_app();
    app.dialog = Dialog::ConfirmQuit;
    assert!(ui::render_text(&app, 100, 30).contains("确认退出"));

    app.dialog = Dialog::ConfirmUnmount("/tmp/proj".into());
    let text = ui::render_text(&app, 100, 30);
    assert!(text.contains("确认卸载"), "unmount dialog missing");
    assert!(text.contains("/tmp/proj"), "unmount path missing");

    app.dialog = Dialog::SymbolText(SymbolCode {
        file: "src/lib.rs".into(),
        name: "forge".into(),
        start_line: 18,
        end_line: 23,
        code: "pub fn forge(label: &str) -> GoldConfig".into(),
    });
    let text = ui::render_text(&app, 100, 30);
    assert!(text.contains("pub fn forge"), "symbol code missing");
    assert!(text.contains("18-23"), "symbol lines missing");

    app.dialog = Dialog::Help;
    assert!(ui::render_text(&app, 100, 40).contains("快捷键速查"));
}

#[test]
fn render_toast_line() {
    let mut app = mounted_app();
    app.set_toast("sync: +1 ~0 -0 failed=0");
    let text = ui::render_text(&app, 100, 30);
    assert!(
        text.contains("sync: +1 ~0 -0 failed=0"),
        "toast text missing"
    );
}

// ---------------------------------------------------------------- 状态机单测 --

#[test]
fn mount_page_typing_and_enter_produce_mount_cmd() {
    let mut app = App::default();
    for c in "/tmp/proj".chars() {
        let _ = app.handle_key(char_key(c));
    }
    assert_eq!(app.mount_page.input, "/tmp/proj");
    let cmds = app.handle_key(key(KeyCode::Enter));
    assert!(matches!(
        cmds.first(),
        Some(Cmd::Mount { path, enable_vector: false }) if path.as_os_str() == "/tmp/proj"
    ));
    assert!(app.mount_page.busy.is_some(), "busy flag must be set");
}

#[test]
fn mount_page_empty_input_is_rejected() {
    let mut app = App::default();
    let cmds = app.handle_key(key(KeyCode::Enter));
    assert!(cmds.is_empty());
    assert!(!app.mount_page.err.is_empty(), "err hint expected");
}

#[test]
fn mount_page_ctrl_v_toggles_vector_without_swallowing_path_text() {
    let mut app = App::default();
    app.mount_page.home_dirs = vec!["/home/u/a".into(), "/home/u/b".into()];
    let _ = app.handle_key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::CONTROL));
    assert!(app.mount_page.enable_vector);
    let _ = app.handle_key(key(KeyCode::Down));
    assert_eq!(app.mount_page.input, "/home/u/b");
    let _ = app.handle_key(key(KeyCode::Up));
    assert_eq!(app.mount_page.input, "/home/u/a");
    let _ = app.handle_key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::CONTROL));
    assert!(!app.mount_page.enable_vector);
    let _ = app.handle_key(char_key('v'));
    assert_eq!(app.mount_page.input, "/home/u/av");
}

#[test]
fn digit_keys_switch_tabs_with_skeleton_prefetch() {
    let mut app = mounted_app();
    let cmds = app.handle_key(char_key('1'));
    assert_eq!(app.tab, Tab::Skeleton);
    assert!(
        matches!(cmds.first(), Some(Cmd::RepoMap { budget: 3000, .. })),
        "skeleton prefetch expected, got {cmds:?}"
    );
    app.skeleton.text = "x".into(); // 已有文本不再预取。
    let cmds = app.handle_key(char_key('1'));
    assert!(cmds.is_empty(), "no prefetch when text present");
    let _ = app.handle_key(char_key('5'));
    assert_eq!(app.tab, Tab::Projects);
    assert!(matches!(
        app.handle_key(char_key('5')).first(),
        Some(Cmd::RefreshProjects)
    ));
}

#[test]
fn skeleton_budget_paging_clamped_500_to_8000() {
    let mut app = mounted_app();
    app.skeleton.text = "x".into();
    let cmds = app.handle_key(key(KeyCode::PageDown));
    assert!(matches!(
        cmds.first(),
        Some(Cmd::RepoMap { budget: 3500, .. })
    ));
    app.skeleton.budget = 8000;
    let cmds = app.handle_key(key(KeyCode::PageDown));
    assert!(cmds.is_empty(), "budget must clamp at 8000");
    app.skeleton.budget = 500;
    let cmds = app.handle_key(key(KeyCode::PageUp));
    assert!(cmds.is_empty(), "budget must clamp at 500");
}

#[test]
fn search_mode_cycles_and_top_k_clamps() {
    let mut app = mounted_app();
    app.tab = Tab::Search;
    assert_eq!(app.search.mode, SearchMode::Hybrid);
    let _ = app.handle_key(key(KeyCode::Tab));
    assert_eq!(app.search.mode, SearchMode::Vector);
    let _ = app.handle_key(key(KeyCode::Tab));
    assert_eq!(app.search.mode, SearchMode::Keyword);
    let _ = app.handle_key(key(KeyCode::Tab));
    assert_eq!(app.search.mode, SearchMode::Hybrid);

    for _ in 0..60 {
        let _ = app.handle_key(char_key('+'));
    }
    assert_eq!(app.search.top_k, 50, "top_k clamps at 50");
    for _ in 0..60 {
        let _ = app.handle_key(char_key('-'));
    }
    assert_eq!(app.search.top_k, 1, "top_k clamps at 1");
}

#[test]
fn search_enter_emits_cmd_and_esc_toggles_focus() {
    let mut app = mounted_app();
    app.tab = Tab::Search;
    for c in "helper".chars() {
        let _ = app.handle_key(char_key(c));
    }
    let cmds = app.handle_key(key(KeyCode::Enter));
    assert!(matches!(
        cmds.first(),
        Some(Cmd::Search { query, mode: SearchMode::Hybrid, top_k: 8, .. })
            if query == "helper"
    ));
    // 回注后焦点在结果区，Esc 回输入区。
    let outcome = crate::app::SearchOutcome {
        hits: Vec::new(),
        effective_mode: SearchMode::Keyword,
        degraded: false,
        degraded_reason: String::new(),
        elapsed_ms: 1,
    };
    let _ = app.apply_ev(Ev::Search(Ok(outcome)));
    assert_eq!(app.search.focus, Focus::Results);
    let _ = app.handle_key(key(KeyCode::Esc));
    assert_eq!(app.search.focus, Focus::Input);
}

#[test]
fn search_empty_query_or_not_mounted_no_cmd() {
    let mut app = mounted_app();
    app.tab = Tab::Search;
    assert!(app.handle_key(key(KeyCode::Enter)).is_empty());
    app.search.query = "x".into();
    let mut bare = App {
        tab: Tab::Search,
        ..App::default()
    };
    bare.search.query = "x".into();
    // 无 active 时 Enter 无命令（挂载页逻辑覆盖此态）。
    assert!(bare.handle_key(key(KeyCode::Enter)).is_empty());
}

#[test]
fn search_drill_uses_symbol_name() {
    let mut app = mounted_app();
    app.tab = Tab::Search;
    app.search.focus = Focus::Results;
    app.search.results = vec![SearchHit {
        file: "src/lib.rs".into(),
        symbol: Some("forge".into()),
        start_line: 1,
        end_line: 2,
        score: 1.0,
        snippet: "...".into(),
    }];
    let cmds = app.handle_key(key(KeyCode::Enter));
    assert!(matches!(
        cmds.first(),
        Some(Cmd::ReadSymbol { file, name, .. }) if file == "src/lib.rs" && name == "forge"
    ));
    // 文件级命中给 toast 不给命令。
    app.search.results[0].symbol = None;
    let cmds = app.handle_key(key(KeyCode::Enter));
    assert!(cmds.is_empty());
    assert!(app.toast.is_some(), "file-level drill toast expected");
}

#[test]
fn quit_requires_confirmation_when_mounted() {
    let mut app = mounted_app();
    let cmds = app.handle_key(char_key('q'));
    assert!(cmds.is_empty(), "first q only opens dialog");
    assert!(matches!(app.dialog, Dialog::ConfirmQuit));
    let cmds = app.handle_key(char_key('y'));
    assert!(app.quit, "y must quit");
    assert!(matches!(cmds.first(), Some(Cmd::Quit)));

    // n 取消。
    let mut app2 = mounted_app();
    let _ = app2.handle_key(char_key('q'));
    let _ = app2.handle_key(char_key('n'));
    assert!(!app2.quit);
    assert_eq!(app2.dialog, Dialog::None);
}

#[test]
fn unmount_flow_dialog_to_cmd() {
    let mut app = mounted_app();
    app.tab = Tab::Projects;
    let _ = app.handle_key(char_key('u'));
    assert!(matches!(app.dialog, Dialog::ConfirmUnmount(_)));
    let cmds = app.handle_key(char_key('y'));
    assert!(matches!(cmds.first(), Some(Cmd::Unmount(_))));
}

#[test]
fn apply_mount_done_activates_and_prefetches() {
    let mut app = App::default();
    let cmds = app.apply_ev(Ev::Mount(Ok(MountReport {
        mounted: true,
        path: "/tmp/proj".into(),
        files: 6,
        symbols: 12,
        elapsed_ms: 120,
        mem_est_mb: 1.5,
        vector: false,
        skipped_binary: 1,
        skipped_oversize: 0,
        skipped_unsupported: 2,
        vector_degraded: false,
        vector_degraded_reason: String::new(),
    })));
    assert_eq!(app.active, Some("/tmp/proj".into()));
    assert!(
        cmds.iter()
            .any(|cmd| matches!(cmd, Cmd::RepoMap { budget: 3000, .. })),
        "mount done prefetches repo_map"
    );
    assert!(cmds.contains(&Cmd::RefreshProjects));
    assert!(app.toast.is_some(), "mount toast expected");
}

#[test]
fn apply_unmount_done_resets_to_mount_page() {
    let mut app = mounted_app();
    app.tab = Tab::Projects;
    app.skeleton.text = "x".into();
    app.search.query = "q".into();
    let cmds = app.apply_ev(Ev::Unmount(Ok(("/tmp/proj".into(), 1))));
    assert!(app.active.is_none());
    assert!(app.projects.is_empty(), "projects list must be cleared");
    assert!(app.on_mount_page(), "must return to mount page");
    assert!(matches!(cmds.first(), Some(Cmd::RefreshProjects)));
    assert_eq!(app.skeleton.text, "", "skeleton must reset");
    assert_eq!(app.search.query, "", "search must reset");
}

#[test]
fn apply_search_degraded_sets_toast() {
    let mut app = mounted_app();
    app.tab = Tab::Search;
    let outcome = crate::app::SearchOutcome {
        hits: vec![SearchHit {
            file: "a.rs".into(),
            symbol: None,
            start_line: 1,
            end_line: 1,
            score: 0.5,
            snippet: "s".into(),
        }],
        effective_mode: SearchMode::Keyword,
        degraded: true,
        degraded_reason: "vector not enabled for this mount".into(),
        elapsed_ms: 3,
    };
    let cmds = app.apply_ev(Ev::Search(Ok(outcome)));
    assert!(cmds.is_empty());
    assert_eq!(app.search.results.len(), 1);
    assert!(app.toast.is_some(), "degraded toast expected");
    assert_eq!(app.search.focus, Focus::Results);
}

#[test]
fn lsp_input_parsing() {
    assert_eq!(
        app::parse_lsp_input("src/main.rs:6:10"),
        Some(("src/main.rs".into(), 6, 10))
    );
    assert_eq!(app::parse_lsp_input("src/main.rs:6"), None);
    assert_eq!(
        app::parse_lsp_input("a.rs:0:5"),
        None,
        "line must be 1-based"
    );
    assert_eq!(app::parse_lsp_input("bad"), None);
    assert_eq!(
        app::parse_lsp_input("weird dir/file.rs:12:3 "),
        Some(("weird dir/file.rs".into(), 12, 3))
    );
}

#[test]
fn lsp_enter_emits_definition_cmd() {
    let mut app = mounted_app();
    app.tab = Tab::Lsp;
    for c in "src/main.rs:6:10".chars() {
        let _ = app.handle_key(char_key(c));
    }
    let cmds = app.handle_key(key(KeyCode::Enter));
    assert!(matches!(
        cmds.first(),
        Some(Cmd::LspDefinition { file, line, character, .. }) if file == "src/main.rs" && *line == 6 && *character == 10
    ));
    let references = KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL);
    assert!(app.handle_key(references).is_empty());
    app.apply_ev(Ev::Lsp(Ok((LspKind::Definition, Vec::new()))));
    let cmds = app.handle_key(references);
    assert!(
        matches!(cmds.first(), Some(Cmd::LspReferences { .. })),
        "Ctrl+R must emit references command, got {cmds:?}"
    );
    assert_eq!(app.lsp.kind, LspKind::References);
}

#[test]
fn ctrl_c_quits_unconditionally() {
    let mut app = App::default();
    let key = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
    let cmds = app.handle_key(key);
    assert!(app.quit);
    assert!(matches!(cmds.first(), Some(Cmd::Quit)));
}
