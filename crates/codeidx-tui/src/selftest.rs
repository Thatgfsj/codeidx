//! `--selftest`：无 tty 全状态机自检（Lead 黑盒验收入口，CI 友好）。
//!
//! 流程：复制 fixture 副本 → 真引擎驱动键序列（mount → 六个 tab → unmount）
//! → 每步以 TestBackend 渲染一帧并断言关键文本 → 全部 PASS 输出绿色清单，
//! 任一 FAIL 退出码 1。不依赖 HOME 目录列表、不拉起真实语言服务器。

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use codeidx_protocol::tools::SearchMode;

use crate::app::{App, Cmd, Ev, Tab};
use crate::engine::Engine;
use crate::ui;

/// 单项检查记录。
struct Checks {
    rows: Vec<(String, bool)>,
}

impl Checks {
    fn push(&mut self, name: &str, ok: bool) {
        println!("[{}] {}", if ok { "PASS" } else { "FAIL" }, name);
        self.rows.push((name.to_string(), ok));
    }

    fn all_pass(&self) -> bool {
        self.rows.iter().all(|(_, ok)| *ok)
    }
}

/// selftest 主入口：返回进程退出码。
pub fn run(fixture_arg: Option<&str>) -> u8 {
    let mut checks = Checks { rows: Vec::new() };
    let fixture_root: PathBuf = fixture_arg.map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/sample-project")
    });
    let tmp = std::env::temp_dir().join(format!("codeidx-tui-selftest-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    if let Err(e) = copy_tree(&fixture_root, &tmp) {
        println!("[FAIL] fixture copy: {e}");
        return 1;
    }
    checks.push(
        &format!("fixture 副本就绪: {}", tmp.display()),
        tmp.join("src/lib.rs").exists(),
    );

    let (tx, rx) = mpsc::channel::<Ev>();
    let eng = Engine::new(tx).expect("start test engine");
    let mut app = App::default();
    // selftest 不依赖真实 HOME 目录列表。
    app.mount_page.home_dirs = vec![format!("{}", tmp.display())];

    // 1) 启动页渲染。
    let text = ui::render_text(&app, 110, 44);
    checks.push("启动页: codeidx-tui 标题", text.contains("codeidx-tui"));
    checks.push("启动页: vector 开关可见", text.contains("vector: OFF"));
    checks.push("启动页: HOME 目录列表 block", text.contains("HOME 目录"));
    checks.push("启动页: 路径输入框", text.contains("path (Enter: mount)"));

    // 2) 输入路径 → Enter → Cmd::Mount。
    for c in tmp.display().to_string().chars() {
        let _ = app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    let cmds = app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    let mount_cmd_ok = matches!(
        cmds.first(),
        Some(Cmd::Mount { path, enable_vector: false }) if *path == tmp
    );
    checks.push("启动页: Enter 产生 Cmd::Mount{vector=false}", mount_cmd_ok);
    checks.push("启动页: busy spinner 状态", app.mount_page.busy.is_some());
    let text = ui::render_text(&app, 110, 44);
    checks.push("启动页: mounting spinner 渲染", text.contains("mounting"));

    // 3) 执行 mount → 回注。
    if let Some(cmd) = cmds.first().cloned() {
        let ev = exec(&eng, &rx, cmd);
        apply_tree(&eng, &rx, &mut app, ev);
    }
    let mounted = app.active.as_deref() == Some(tmp.as_path());
    checks.push("mount: active 指向副本根", mounted);
    let text = ui::render_text(&app, 110, 44);
    checks.push("mount: toast 展示 files/symbols", text.contains("mounted:"));
    checks.push("mount: 未再处于启动页", !app.on_mount_page());

    // 4) Skeleton tab + repo_map。
    for cmd in app.handle_key(KeyEvent::new(KeyCode::Char('1'), KeyModifiers::NONE)) {
        let ev = exec(&eng, &rx, cmd);
        apply_tree(&eng, &rx, &mut app, ev);
    }
    checks.push("skeleton: tab=1", app.tab == Tab::Skeleton);
    checks.push(
        "skeleton: repo_map 文本已回注",
        !app.skeleton.text.is_empty(),
    );
    let text = ui::render_text(&app, 110, 44);
    checks.push(
        "skeleton: 首行元信息 '# repo_map'",
        text.contains("# repo_map"),
    );
    checks.push(
        "skeleton: 验收符号 unicorn_marker_fn",
        text.contains("unicorn_marker_fn"),
    );
    checks.push(
        "skeleton: 高权重符号 helper_spinner",
        text.contains("helper_spinner"),
    );

    // 5) PgDown 调预算 3500 → 重新拉取。
    let cmds = app.handle_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE));
    let budget_cmd_ok = matches!(cmds.first(), Some(Cmd::RepoMap { budget: 3500, .. }));
    checks.push(
        "skeleton: PgDown → budget=3500 命令",
        budget_cmd_ok && app.skeleton.busy,
    );
    for cmd in cmds {
        let ev = exec(&eng, &rx, cmd);
        apply_tree(&eng, &rx, &mut app, ev);
    }
    let text = ui::render_text(&app, 110, 44);
    checks.push("skeleton: 预算行回显 3500", text.contains("budget=3500"));

    // 6) Search tab：keyword 检索 + 降级可见性。
    for cmd in app.handle_key(KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE)) {
        let _ = cmd;
    }
    checks.push("search: tab=2", app.tab == Tab::Search);
    for c in "unicorn".chars() {
        let _ = app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    let cmds = app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    let search_cmd_ok = matches!(
        cmds.first(),
        Some(Cmd::Search {
            mode: SearchMode::Hybrid,
            top_k: 8,
            ..
        })
    );
    checks.push(
        "search: Enter → Cmd::Search{hybrid, top_k=8}",
        search_cmd_ok,
    );
    for cmd in cmds {
        let ev = exec(&eng, &rx, cmd);
        apply_tree(&eng, &rx, &mut app, ev);
    }
    let degraded_ok = app
        .search
        .outcome
        .as_ref()
        .map(|o| o.degraded && o.effective_mode == SearchMode::Keyword && !o.hits.is_empty())
        .unwrap_or(false);
    checks.push("search: vector 未就绪 → 降级 keyword 且有命中", degraded_ok);
    let text = ui::render_text(&app, 110, 44);
    checks.push("search: 结果表含 src/lib.rs", text.contains("src/lib.rs"));
    checks.push("search: 降级原因提示", text.contains("degraded"));

    // 7) 按实际排名选择目标，再通过 Enter 下钻。
    if let Some(index) = app.search.results.iter().position(|hit| {
        hit.file.starts_with("src/")
            && hit
                .symbol
                .as_deref()
                .is_some_and(|name| name.ends_with("unicorn_marker_fn"))
    }) {
        for _ in 0..index {
            app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        }
    }
    let cmds = app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    let drill_ok = matches!(
        cmds.first(),
        Some(Cmd::ReadSymbol { path, file, name }) if path == &tmp && file.starts_with("src/") && name.ends_with("unicorn_marker_fn")
    );
    checks.push(
        "search: Enter 下钻 → Cmd::ReadSymbol(unicorn_marker_fn)",
        drill_ok,
    );
    for cmd in cmds {
        let ev = exec(&eng, &rx, cmd);
        apply_tree(&eng, &rx, &mut app, ev);
    }
    let text = ui::render_text(&app, 110, 44);
    checks.push(
        "search: 符号全文弹层渲染",
        text.contains("pub fn unicorn_marker_fn"),
    );
    // 8) Esc 关闭弹层。
    let _ = app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    checks.push(
        "search: Esc 关闭弹层",
        app.dialog == crate::app::Dialog::None,
    );

    // 9) Outline tab。
    let _ = app.handle_key(KeyEvent::new(KeyCode::Char('3'), KeyModifiers::NONE));
    checks.push("outline: tab=3", app.tab == Tab::Outline);
    for c in "src/lib.rs".chars() {
        let _ = app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    let cmds = app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    for cmd in cmds {
        let ev = exec(&eng, &rx, cmd);
        apply_tree(&eng, &rx, &mut app, ev);
    }
    checks.push("outline: 大纲条目已回注", !app.outline.entries.is_empty());
    let text = ui::render_text(&app, 110, 44);
    checks.push("outline: GoldConfig 可见", text.contains("GoldConfig"));
    checks.push("outline: forge 方法可见", text.contains("forge"));

    // 10) 通过方向键选择目标结构体，再下钻。
    if let Some(index) = app
        .outline
        .entries
        .iter()
        .position(|entry| entry.kind == "struct" && entry.name.ends_with("GoldConfig"))
    {
        for _ in 0..index {
            app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        }
    }
    let cmds = app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    for cmd in cmds {
        let ev = exec(&eng, &rx, cmd);
        apply_tree(&eng, &rx, &mut app, ev);
    }
    let text = ui::render_text(&app, 110, 44);
    checks.push(
        "outline: 下钻弹层渲染 (GoldConfig)",
        text.contains("pub struct GoldConfig"),
    );
    let _ = app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

    // 11) Lsp tab：渲染冷启动提示（不真拉语言服务器）。
    let _ = app.handle_key(KeyEvent::new(KeyCode::Char('4'), KeyModifiers::NONE));
    checks.push("lsp: tab=4", app.tab == Tab::Lsp);
    let text = ui::render_text(&app, 110, 44);
    checks.push("lsp: 冷启动提示可见", text.contains("冷启动"));
    checks.push("lsp: position 输入框", text.contains("position"));

    // Calls tab: query the real index in static mode so selftest needs no LSP binary.
    app.call_tree.refine = false;
    let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
    let cmds = app.handle_key(key(KeyCode::F(6)));
    checks.push(
        "calls: F6 发起静态追踪",
        app.tab == Tab::CallTree
            && matches!(cmds.as_slice(), [Cmd::TraceCalls { refine: false, .. }]),
    );
    for cmd in cmds {
        apply_tree(&eng, &rx, &mut app, exec(&eng, &rx, cmd));
    }
    let main = app.call_tree.rows().iter().position(|row| {
        row.node.location.symbol == "main" && row.node.location.file == "src/main.rs"
    });
    checks.push("calls: 从真实索引获得 main 入口", main.is_some());
    if let Some(main) = main {
        for _ in 0..main {
            app.handle_key(key(KeyCode::Down));
        }
        let text = ui::render_text(&app, 110, 44);
        checks.push(
            "calls: 树中渲染 main 和符号位置",
            text.contains("▾ main") && text.contains("src/main.rs:7"),
        );
        let expanded = app.call_tree.rows().len();
        app.handle_key(key(KeyCode::Left));
        let text = ui::render_text(&app, 110, 44);
        checks.push(
            "calls: 左键折叠并隐藏后代",
            text.contains("▸ main") && app.call_tree.rows().len() < expanded,
        );
        app.handle_key(key(KeyCode::Right));
        checks.push(
            "calls: 右键恢复后代",
            app.call_tree.rows().len() == expanded,
        );
        app.handle_key(key(KeyCode::Right));
        let selected = app
            .call_tree
            .rows()
            .get(app.call_tree.selected())
            .map(|r| r.node.location.clone());
        let cmds = app.handle_key(key(KeyCode::Char('t')));
        checks.push("calls: t 使用选中位置续查", matches!(cmds.as_slice(), [Cmd::TraceCalls{from: Some(from), ..}] if Some(from) == selected.as_ref()));
        for cmd in cmds {
            apply_tree(&eng, &rx, &mut app, exec(&eng, &rx, cmd));
        }
        checks.push(
            "calls: 续查以选中位置为根",
            app.call_tree.report().is_some_and(|r| {
                r.roots.len() == 1 && Some(&r.roots[0].location) == selected.as_ref()
            }),
        );
        let cmds = app.handle_key(key(KeyCode::Enter));
        for cmd in cmds {
            apply_tree(&eng, &rx, &mut app, exec(&eng, &rx, cmd));
        }
        checks.push("calls: Enter 打开选中定义的源码", matches!(&app.dialog, crate::app::Dialog::SymbolText(code) if selected.as_ref().is_some_and(|at| at.file == code.file && at.line == code.start_line && at.symbol == code.name) && !code.code.is_empty()));
        if app.dialog != crate::app::Dialog::None {
            app.handle_key(key(KeyCode::Esc));
        }
        app.handle_key(key(KeyCode::Esc));
        checks.push(
            "calls: Esc 返回前一个树视图",
            app.call_tree.rows().len() == expanded,
        );
    }

    // 12) Projects tab：刷新列表。
    let _ = app.handle_key(KeyEvent::new(KeyCode::Char('5'), KeyModifiers::NONE));
    checks.push("projects: tab=5", app.tab == Tab::Projects);
    let cmds = app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    for cmd in cmds {
        let ev = exec(&eng, &rx, cmd);
        apply_tree(&eng, &rx, &mut app, ev);
    }
    checks.push("projects: list 含副本根", {
        let p = app.projects.first().cloned();
        matches!(&p, Some(info) if Path::new(&info.path) == tmp)
    });
    let text = ui::render_text(&app, 110, 44);
    checks.push(
        "projects: 卡片渲染 files/symbols",
        text.contains("files=") && text.contains("symbols="),
    );
    checks.push("projects: vector 状态可见", text.contains("vector=false"));

    // 13) 状态栏：RSS 显示。
    app.rss_kb = 20 * 1024;
    let text = ui::render_text(&app, 110, 44);
    checks.push("status: 状态栏 VmRSS", text.contains("rss: 20.0MB"));
    checks.push("status: 挂载数", text.contains("mounted:1"));
    checks.push("status: 当前 tab 名", text.contains("5 Projects"));

    // 14) 帮助弹层。
    let _ = app.handle_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
    let text = ui::render_text(&app, 110, 44);
    checks.push("help: 快捷键速查弹层", text.contains("快捷键速查"));
    let _ = app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

    // 15) unmount 确认弹层 → y → Cmd::Unmount → 回注 → 回到启动页。
    let _ = app.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::NONE));
    let text = ui::render_text(&app, 110, 44);
    checks.push("projects: unmount 确认弹层", text.contains("确认卸载"));
    let cmds = app.handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE));
    let unmount_cmd_ok = matches!(cmds.first(), Some(Cmd::Unmount(p)) if *p == tmp);
    checks.push("projects: y → Cmd::Unmount(副本根)", unmount_cmd_ok);
    for cmd in cmds {
        let ev = exec(&eng, &rx, cmd);
        apply_tree(&eng, &rx, &mut app, ev);
    }
    checks.push("projects: unmount 后 active 清空", app.active.is_none());
    checks.push("projects: 回到启动页", app.on_mount_page());
    let text = ui::render_text(&app, 110, 44);
    checks.push("projects: 启动页复现", text.contains("codeidx-tui"));

    // 16) 退出确认弹层（有挂载时二次确认路径在此前已过；无挂载页直接退出路径）。
    checks.push("quit: 状态干净", app.projects.is_empty());

    eng.shutdown();
    let _ = std::fs::remove_dir_all(&tmp);
    let total = checks.rows.len();
    let passed = checks.rows.iter().filter(|(_, ok)| *ok).count();
    if checks.all_pass() {
        println!("\nselftest: {passed}/{total} PASS — codeidx-tui 状态机全绿");
        0
    } else {
        println!("\nselftest: {passed}/{total} — 存在失败项");
        1
    }
}

/// 同步执行单条命令并等待回注（selftest 阻塞等待，主循环是异步派发）。
fn exec(eng: &Engine, rx: &Receiver<Ev>, cmd: Cmd) -> Ev {
    eng.dispatch(cmd);
    rx.recv_timeout(Duration::from_secs(120))
        .expect("engine reply timeout")
}

/// 回注 + 级联命令（mount 后预取 repo_map / list 刷新等）。
fn apply_tree(eng: &Engine, rx: &Receiver<Ev>, app: &mut App, ev: Ev) {
    let cmds = app.apply_ev(ev);
    for cmd in cmds {
        let next = exec(eng, rx, cmd);
        apply_tree(eng, rx, app, next);
    }
}

/// 递归复制目录树。
fn copy_tree(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let to = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_tree(&entry.path(), &to)?;
        } else {
            std::fs::copy(entry.path(), &to)?;
        }
    }
    Ok(())
}
