//! codeidx-tui —— 给人用的本地代码索引终端前端（独立入口，直连引擎层）。
//!
//! 与 codeidx-mcp（AI 的 stdio 入口）平级：进程内自建 ProjectManager /
//! LspBridge，不连接 MCP 服务。架构纪律：
//! - mount 等秒级~分钟级引擎工作全部在引擎线程池执行（engine.rs），
//!   UI 循环以 16ms poll 驱动 spinner 与引擎结果轮询，保持 60fps 响应；
//! - `--selftest`：无 tty 全状态机自检（挂载 fixture → 五 tab 渲染断言
//!   → unmount），供 CI / Lead 黑盒验收。

mod app;
mod engine;
mod selftest;
mod ui;

#[cfg(test)]
mod tests;

use std::io;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
    MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::{backend::CrosstermBackend, Terminal};

use crate::app::{App, Cmd, Ev, Tab};
use crate::engine::Engine;

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().skip(1).any(|a| a == "--selftest") {
        let fixture = args.iter().skip(2).find(|a| *a != "--selftest").cloned();
        let code = selftest::run(fixture.as_deref());
        return std::process::ExitCode::from(code);
    }
    match run_tui() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("codeidx-tui: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

/// 全屏 TUI 主循环。
fn run_tui() -> io::Result<()> {
    let (tx, rx) = mpsc::channel::<Ev>();
    let eng = Engine::new(tx)?;
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    let mut terminal = match execute!(stdout, EnterAlternateScreen, EnableMouseCapture)
        .and_then(|_| Terminal::new(CrosstermBackend::new(stdout)))
    {
        Ok(terminal) => terminal,
        Err(error) => {
            let _ = disable_raw_mode();
            let _ = execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture);
            return Err(error);
        }
    };
    let mut app = App::default();
    // HOME 目录列表注入（IO 在 UI 线程启动前一次性完成）。
    app.mount_page.home_dirs = engine::list_home_dirs();

    let result = run_loop(&mut terminal, &mut app, &eng, &rx);

    // teardown（先还终端再杀 LSP，避免子进程竞争期间的画面残留）。
    let raw_cleanup = disable_raw_mode();
    let screen_cleanup = execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    );
    eng.shutdown();
    result.and(raw_cleanup).and(screen_cleanup)
}

/// 事件循环：16ms poll 保帧率；引擎 Ev 非阻塞 drain；1s 刷新 VmRSS。
fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    eng: &Engine,
    rx: &mpsc::Receiver<Ev>,
) -> io::Result<()> {
    let mut last_rss = Instant::now();
    loop {
        terminal.draw(|f| ui::draw(f, app))?;

        // 引擎结果回注（可能级联产生新命令，如 mount 后的 repo_map 预取）。
        let mut pending: Vec<Ev> = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            pending.push(ev);
        }
        for ev in pending {
            for cmd in app.apply_ev(ev) {
                dispatch(eng, app, &cmd);
            }
        }
        if app.quit {
            return Ok(());
        }

        if crossterm::event::poll(Duration::from_millis(16))? {
            match crossterm::event::read()? {
                Event::Key(key) => {
                    if key.kind == KeyEventKind::Press {
                        for cmd in app.handle_key(key) {
                            dispatch(eng, app, &cmd);
                        }
                    }
                }
                // 滚轮在 Skeleton tab 等价 PgUp/PgDn 调预算。
                Event::Mouse(m) => {
                    let is_scroll = matches!(
                        m.kind,
                        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
                    );
                    if is_scroll && app.tab == Tab::Skeleton {
                        let code = if matches!(m.kind, MouseEventKind::ScrollUp) {
                            KeyCode::PageUp
                        } else {
                            KeyCode::PageDown
                        };
                        let key = KeyEvent::new(code, KeyModifiers::NONE);
                        for cmd in app.handle_key(key) {
                            dispatch(eng, app, &cmd);
                        }
                    }
                }
                _ => {}
            }
        } else {
            app.on_tick();
        }

        if last_rss.elapsed() >= Duration::from_secs(1) {
            if let Some(kb) = engine::vm_rss_kb() {
                app.rss_kb = kb;
            }
            last_rss = Instant::now();
        }
    }
}

/// 派发命令：Quit 只置退出标记，不进引擎线程。
fn dispatch(eng: &Engine, app: &App, cmd: &Cmd) {
    if *cmd == Cmd::Quit {
        let _ = app;
        return;
    }
    eng.dispatch(cmd.clone());
}
