//! codeidx-tui 状态机：交互渲染层薄、业务状态纯函数化的可测核心。
//!
//! 纪律（Lead 任务书 §架构纪律2）：
//! - `App::handle_key` / `App::apply_ev` 均为纯函数——不碰 IO、不碰引擎；
//!   副作用以 `Cmd` 列表交由 main 循环派发，引擎结果以 `Ev` 回注。
//! - 渲染层（ui.rs）只读 App；App 不依赖 ratatui。
//! - 全部交互在 ui/tests.rs 的 TestBackend 单测与 --selftest 状态机下可复现。

use std::path::PathBuf;

use codeidx_project::callflow::{TraceDirection, TraceLocation, TraceReport};
use codeidx_protocol::tools::{
    Location, MountReport, OutlineEntry, ProjectInfo, SearchHit, SearchMode, SymbolCode, SyncReport,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::call_tree::CallTree;

/// 功能 tab（数字键 1-6 切换）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Skeleton,
    Search,
    Outline,
    Lsp,
    Projects,
    CallTree,
}

impl Tab {
    /// 数字键 → tab。
    pub fn from_digit(d: char) -> Option<Tab> {
        match d {
            '1' => Some(Tab::Skeleton),
            '2' => Some(Tab::Search),
            '3' => Some(Tab::Outline),
            '4' => Some(Tab::Lsp),
            '5' => Some(Tab::Projects),
            '6' => Some(Tab::CallTree),
            _ => None,
        }
    }

    /// 状态栏显示名。
    pub fn title(self) -> &'static str {
        match self {
            Tab::Skeleton => "1 Skeleton",
            Tab::Search => "2 Search",
            Tab::Outline => "3 Outline",
            Tab::Lsp => "4 Lsp",
            Tab::Projects => "5 Projects",
            Tab::CallTree => "6 Calls",
        }
    }
}

/// 输入框 / 结果表焦点。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Input,
    Results,
}

/// 弹层。
#[derive(Debug, Clone, PartialEq)]
pub enum Dialog {
    None,
    /// q 退出二次确认（有挂载时）。
    ConfirmQuit,
    /// u 卸载确认（携带项目根）。
    ConfirmUnmount(PathBuf),
    /// 符号全文下钻（search / outline 的 Enter）。
    SymbolText(SymbolCode),
    /// ? 快捷键速查。
    Help,
}

/// 引擎命令：handle_key / apply_ev 的全部副作用（main 循环派发给 engine）。
#[derive(Debug, Clone, PartialEq)]
pub enum Cmd {
    Mount {
        path: PathBuf,
        enable_vector: bool,
    },
    Unmount(PathBuf),
    Sync(PathBuf),
    RefreshProjects,
    RepoMap {
        path: PathBuf,
        budget: u32,
    },
    Search {
        path: PathBuf,
        query: String,
        mode: SearchMode,
        top_k: u32,
    },
    Outline {
        path: PathBuf,
        file: String,
    },
    ReadSymbol {
        path: PathBuf,
        file: String,
        name: String,
    },
    LspDefinition {
        path: PathBuf,
        file: String,
        line: u32,
        character: u32,
    },
    LspReferences {
        path: PathBuf,
        file: String,
        line: u32,
        character: u32,
    },
    TraceCalls {
        path: PathBuf,
        id: u64,
        from: Option<TraceLocation>,
        direction: TraceDirection,
        refine: bool,
    },
    ReadTraceSymbol {
        path: PathBuf,
        location: TraceLocation,
    },
    Quit,
}

/// 引擎回注事件（engine 工作线程 → main → apply_ev）。
#[derive(Debug, Clone)]
pub enum Ev {
    Mount(Result<MountReport, String>),
    Unmount(Result<(PathBuf, u32), String>),
    Sync(Result<SyncReport, String>),
    Projects(Vec<ProjectInfo>),
    /// (token_budget, repo_map 文本)。
    RepoMap(Result<(u32, String), String>),
    Search(Result<SearchOutcome, String>),
    Outline(Result<Vec<OutlineEntry>, String>),
    Symbol(Result<Option<SymbolCode>, String>),
    /// (查询类别, 结果位置集)。
    Lsp(Result<(LspKind, Vec<Location>), String>),
    TraceCalls(u64, Option<TraceLocation>, Result<TraceReport, String>),
    TraceSymbol(PathBuf, Result<SymbolCode, String>),
}

/// search 结果包（对齐 SearchResult 的降级可见性语义，结构化自渲染）。
#[derive(Debug, Clone, PartialEq)]
pub struct SearchOutcome {
    pub hits: Vec<SearchHit>,
    /// 实际生效模式（请求 vector 未就绪时降级 keyword）。
    pub effective_mode: SearchMode,
    pub degraded: bool,
    pub degraded_reason: String,
    pub elapsed_ms: u64,
}

/// LSP 查询类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LspKind {
    Definition,
    References,
}

impl LspKind {
    pub fn title(self) -> &'static str {
        match self {
            LspKind::Definition => "definition",
            LspKind::References => "references",
        }
    }
}

/// 短暂提示（sync 结果、降级警告等），tick 递减自动消失。
#[derive(Debug, Clone, PartialEq)]
pub struct Toast {
    pub text: String,
    pub ttl: u32,
}

/// 未挂载页状态。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MountPage {
    pub input: String,
    /// HOME 目录列表（main 侧 IO 填充注入；sel 变更时回填 input）。
    pub home_dirs: Vec<String>,
    pub sel: usize,
    pub enable_vector: bool,
    /// mounting 进行中（路径为待挂载根），驱动 spinner。
    pub busy: Option<PathBuf>,
    pub err: String,
}

/// Skeleton tab 状态。
#[derive(Debug, Clone, PartialEq)]
pub struct SkeletonState {
    /// 当前展示文本对应的 token_budget。
    pub budget: u32,
    pub text: String,
    pub scroll: u16,
    /// 拉取进行中（防抖：重复 PgDn 不重复发请求）。
    pub busy: bool,
    pub err: String,
}

impl Default for SkeletonState {
    fn default() -> Self {
        SkeletonState {
            budget: 3000,
            text: String::new(),
            scroll: 0,
            busy: false,
            err: String::new(),
        }
    }
}

/// Search tab 状态。
#[derive(Debug, Clone, PartialEq)]
pub struct SearchState {
    pub query: String,
    pub mode: SearchMode,
    pub top_k: u32,
    pub results: Vec<SearchHit>,
    pub selected: usize,
    pub focus: Focus,
    pub outcome: Option<SearchOutcome>,
    pub busy: bool,
    pub err: String,
}

impl Default for SearchState {
    fn default() -> Self {
        SearchState {
            query: String::new(),
            mode: SearchMode::Hybrid,
            top_k: 8,
            results: Vec::new(),
            selected: 0,
            focus: Focus::Input,
            outcome: None,
            busy: false,
            err: String::new(),
        }
    }
}

/// Outline tab 状态。
#[derive(Debug, Clone, PartialEq)]
pub struct OutlineState {
    pub file: String,
    pub entries: Vec<OutlineEntry>,
    pub selected: usize,
    pub focus: Focus,
    pub busy: bool,
    pub err: String,
}

impl Default for OutlineState {
    fn default() -> Self {
        OutlineState {
            file: String::new(),
            entries: Vec::new(),
            selected: 0,
            focus: Focus::Input,
            busy: false,
            err: String::new(),
        }
    }
}

/// Lsp tab 状态。
#[derive(Debug, Clone, PartialEq)]
pub struct LspState {
    /// 输入格式 `rel_path:line:character`（line 1-based、character 0-based）。
    pub input: String,
    pub kind: LspKind,
    pub locations: Vec<Location>,
    pub selected: usize,
    /// 本会话是否已成功拉起过语言服务器（首次调用冷启动提示依据）。
    pub ever_spawned: bool,
    pub busy: bool,
    pub err: String,
}

impl Default for LspState {
    fn default() -> Self {
        LspState {
            input: String::new(),
            kind: LspKind::Definition,
            locations: Vec::new(),
            selected: 0,
            ever_spawned: false,
            busy: false,
            err: String::new(),
        }
    }
}

/// TUI 全量状态（渲染唯一事实来源）。
#[derive(Debug, Clone)]
pub struct App {
    pub quit: bool,
    pub tab: Tab,
    /// 当前操作的项目根（mount 成功后指向新项目；Projects Enter 可切换）。
    pub active: Option<PathBuf>,
    pub projects: Vec<ProjectInfo>,
    pub toast: Option<Toast>,
    pub dialog: Dialog,
    pub mount_page: MountPage,
    pub skeleton: SkeletonState,
    pub search: SearchState,
    pub outline: OutlineState,
    pub lsp: LspState,
    pub call_tree: CallTree,
    /// 状态栏 VmRSS（KB，main 侧 IO 注入）。
    pub rss_kb: u64,
    /// spinner 帧计数（mounting 时驱动动画）。
    pub tick: u32,
}

impl Default for App {
    fn default() -> Self {
        App {
            quit: false,
            tab: Tab::Skeleton,
            active: None,
            projects: Vec::new(),
            toast: None,
            dialog: Dialog::None,
            mount_page: MountPage::default(),
            skeleton: SkeletonState::default(),
            search: SearchState::default(),
            outline: OutlineState::default(),
            lsp: LspState::default(),
            call_tree: CallTree::new(),
            rss_kb: 0,
            tick: 0,
        }
    }
}

impl App {
    /// 是否处于未挂载首页（尚无任何项目且无进行中的挂载）。
    pub fn on_mount_page(&self) -> bool {
        self.active.is_none() && self.mount_page.busy.is_none() && self.projects.is_empty()
    }

    /// toast 设置（默认 ttl 按文本长度放宽）。
    pub fn set_toast(&mut self, text: impl Into<String>) {
        let text = text.into();
        let ttl = 40 + text.len() as u32 / 20;
        self.toast = Some(Toast { text, ttl });
    }

    /// tick 推进：toast 递减、spinner 帧递增。返回 true 表示界面需要重绘。
    pub fn on_tick(&mut self) -> bool {
        self.tick = self.tick.wrapping_add(1);
        if let Some(t) = &mut self.toast {
            t.ttl = t.ttl.saturating_sub(1);
            if t.ttl == 0 {
                self.toast = None;
            }
        }
        self.toast.is_some() || self.mount_page.busy.is_some()
    }

    /// 键事件 → 状态变化 + 引擎命令（纯函数）。
    pub fn handle_key(&mut self, key: KeyEvent) -> Vec<Cmd> {
        // Ctrl+C 无条件退出。
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.quit = true;
            return vec![Cmd::Quit];
        }
        // 弹层优先消费按键。
        if self.dialog != Dialog::None {
            return self.handle_dialog_key(key);
        }
        let mut cmds = Vec::new();
        if !self.on_mount_page() {
            if let KeyCode::F(n @ 1..=6) = key.code {
                self.tab = Tab::from_digit(char::from(b'0' + n)).expect("valid tab key");
                return self.enter_tab_cmds();
            }
        }
        // 全局键：q / ? / 数字 tab 切换（挂载页也允许直接看 Projects？未挂载时
        // tab 区无意义，跳过）。
        if !self.on_mount_page() {
            match key.code {
                KeyCode::Char('q') => {
                    self.dialog = Dialog::ConfirmQuit;
                    return cmds;
                }
                KeyCode::Char('?') => {
                    self.dialog = Dialog::Help;
                    return cmds;
                }
                KeyCode::Char(c) => {
                    let typing_lsp_position = self.tab == Tab::Lsp && !self.lsp.input.is_empty();
                    if let Some(tab) = Tab::from_digit(c).filter(|_| !typing_lsp_position) {
                        self.tab = tab;
                        cmds.extend(self.enter_tab_cmds());
                        return cmds;
                    }
                }
                _ => {}
            }
            cmds.extend(self.handle_tab_key(key));
        } else if let Some(busy) = self.mount_page.busy.clone() {
            // mounting 进行中：只响应 Ctrl+C（上面已处理）。
            let _ = busy;
        } else {
            cmds.extend(self.handle_mount_key(key));
        }
        cmds
    }

    /// 弹层键处理：Esc 关闭；确认弹层 y/n。
    fn handle_dialog_key(&mut self, key: KeyEvent) -> Vec<Cmd> {
        let mut cmds = Vec::new();
        match (&self.dialog, key.code) {
            (Dialog::ConfirmQuit, KeyCode::Char('y') | KeyCode::Enter) => {
                self.dialog = Dialog::None;
                self.quit = true;
                cmds.push(Cmd::Quit);
            }
            (Dialog::ConfirmQuit, _) => {
                self.dialog = Dialog::None;
            }
            (Dialog::ConfirmUnmount(path), KeyCode::Char('y') | KeyCode::Enter) => {
                let path = path.clone();
                self.dialog = Dialog::None;
                cmds.push(Cmd::Unmount(path));
            }
            (Dialog::ConfirmUnmount(_), _) => {
                self.dialog = Dialog::None;
            }
            (Dialog::SymbolText(_) | Dialog::Help, _) => {
                self.dialog = Dialog::None;
            }
            (Dialog::None, _) => {}
        }
        cmds
    }

    /// 未挂载页键处理：路径输入、目录选择、v 开关、Enter 挂载。
    fn handle_mount_key(&mut self, key: KeyEvent) -> Vec<Cmd> {
        let page = &mut self.mount_page;
        match key.code {
            KeyCode::Char(c) => {
                if c == 'v' && key.modifiers.contains(KeyModifiers::CONTROL) {
                    page.enable_vector = !page.enable_vector;
                } else if !key.modifiers.contains(KeyModifiers::CONTROL) {
                    page.input.push(c);
                    page.err.clear();
                }
            }
            KeyCode::Backspace => {
                page.input.pop();
            }
            KeyCode::Up => {
                if !page.home_dirs.is_empty() {
                    page.sel = page.sel.saturating_sub(1);
                    page.input = page.home_dirs[page.sel].clone();
                }
            }
            KeyCode::Down => {
                if !page.home_dirs.is_empty() && page.sel + 1 < page.home_dirs.len() {
                    page.sel += 1;
                    page.input = page.home_dirs[page.sel].clone();
                }
            }
            KeyCode::Enter => {
                let path = PathBuf::from(page.input.trim());
                if path.as_os_str().is_empty() {
                    page.err = "input a project path first".into();
                } else {
                    page.err.clear();
                    page.busy = Some(path.clone());
                    return vec![Cmd::Mount {
                        path,
                        enable_vector: page.enable_vector,
                    }];
                }
            }
            _ => {}
        }
        Vec::new()
    }

    /// tab 内键处理。
    fn handle_tab_key(&mut self, key: KeyEvent) -> Vec<Cmd> {
        match self.tab {
            Tab::Skeleton => self.handle_skeleton_key(key),
            Tab::Search => self.handle_search_key(key),
            Tab::Outline => self.handle_outline_key(key),
            Tab::Lsp => self.handle_lsp_key(key),
            Tab::Projects => self.handle_projects_key(key),
            Tab::CallTree => self
                .active
                .as_deref()
                .map_or_else(Vec::new, |path| self.call_tree.handle(path, key.code)),
        }
    }

    /// 切入 tab 时的预取（Skeleton 文本缺失时拉取）。
    fn enter_tab_cmds(&mut self) -> Vec<Cmd> {
        let mut cmds = Vec::new();
        if let Some(path) = self.active.clone() {
            match self.tab {
                Tab::Skeleton => {
                    if self.skeleton.text.is_empty() && !self.skeleton.busy {
                        self.skeleton.busy = true;
                        cmds.push(Cmd::RepoMap {
                            path,
                            budget: self.skeleton.budget,
                        });
                    }
                }
                Tab::Search | Tab::Outline | Tab::Lsp => {}
                Tab::CallTree => return self.call_tree.enter(path),
                Tab::Projects => {
                    cmds.push(Cmd::RefreshProjects);
                }
            }
        }
        cmds
    }

    fn handle_skeleton_key(&mut self, key: KeyEvent) -> Vec<Cmd> {
        match key.code {
            KeyCode::Up => {
                self.skeleton.scroll = self.skeleton.scroll.saturating_sub(1);
            }
            KeyCode::Down => {
                self.skeleton.scroll = self.skeleton.scroll.saturating_add(1);
            }
            KeyCode::PageUp | KeyCode::PageDown => {
                let delta: i64 = if key.code == KeyCode::PageDown {
                    500
                } else {
                    -500
                };
                let next = (self.skeleton.budget as i64 + delta).clamp(500, 8000) as u32;
                if next != self.skeleton.budget && !self.skeleton.busy {
                    self.skeleton.budget = next;
                    self.skeleton.busy = true;
                    let path = self.active.clone().expect("active on tab");
                    return vec![Cmd::RepoMap { path, budget: next }];
                }
            }
            _ => {}
        }
        Vec::new()
    }

    fn handle_search_key(&mut self, key: KeyEvent) -> Vec<Cmd> {
        let s = &mut self.search;
        match (key.code, s.focus) {
            (KeyCode::Esc, Focus::Results) => s.focus = Focus::Input,
            (KeyCode::Tab, _) => {
                s.mode = match s.mode {
                    SearchMode::Keyword => SearchMode::Hybrid,
                    SearchMode::Hybrid => SearchMode::Vector,
                    SearchMode::Vector => SearchMode::Keyword,
                };
            }
            (KeyCode::Char('+') | KeyCode::Char('='), _) => {
                s.top_k = s.top_k.saturating_add(1).min(50);
            }
            (KeyCode::Char('-'), _) => {
                s.top_k = s.top_k.saturating_sub(1).max(1);
            }
            (KeyCode::Char(c), Focus::Input) => {
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && c != '+'
                    && c != '='
                    && c != '-'
                {
                    s.query.push(c);
                }
            }
            (KeyCode::Backspace, Focus::Input) => {
                s.query.pop();
            }
            (KeyCode::Enter, Focus::Input) => {
                if !s.query.trim().is_empty() && !s.busy {
                    let query = s.query.trim().to_string();
                    let (mode, top_k) = (s.mode, s.top_k);
                    if let Some(path) = self.active.clone() {
                        s.busy = true;
                        s.err.clear();
                        return vec![Cmd::Search {
                            path,
                            query,
                            mode,
                            top_k,
                        }];
                    }
                }
            }
            (KeyCode::Up, Focus::Results) => {
                s.selected = s.selected.saturating_sub(1);
            }
            (KeyCode::Down, Focus::Results) => {
                if s.selected + 1 < s.results.len() {
                    s.selected += 1;
                }
            }
            (KeyCode::Enter, Focus::Results) => {
                let hit = s.results.get(s.selected).cloned();
                if let Some(hit) = hit {
                    match hit.symbol.as_deref() {
                        Some(name) if !name.is_empty() => {
                            if let Some(path) = self.active.clone() {
                                let file = hit.file.clone();
                                let name = name.to_string();
                                return vec![Cmd::ReadSymbol { path, file, name }];
                            }
                        }
                        _ => {
                            self.set_toast("file-level hit: no symbol to drill");
                        }
                    }
                }
            }
            _ => {}
        }
        Vec::new()
    }

    fn handle_outline_key(&mut self, key: KeyEvent) -> Vec<Cmd> {
        let o = &mut self.outline;
        match (key.code, o.focus) {
            (KeyCode::Esc, Focus::Results) => o.focus = Focus::Input,
            (KeyCode::Char(c), Focus::Input) => {
                if !key.modifiers.contains(KeyModifiers::CONTROL) {
                    o.file.push(c);
                    o.err.clear();
                }
            }
            (KeyCode::Backspace, Focus::Input) => {
                o.file.pop();
            }
            (KeyCode::Enter, Focus::Input) => {
                if !o.file.trim().is_empty() && !o.busy {
                    let file = o.file.trim().to_string();
                    if let Some(path) = self.active.clone() {
                        o.busy = true;
                        return vec![Cmd::Outline { path, file }];
                    }
                }
            }
            (KeyCode::Up, Focus::Results) => {
                o.selected = o.selected.saturating_sub(1);
            }
            (KeyCode::Down, Focus::Results) => {
                if o.selected + 1 < o.entries.len() {
                    o.selected += 1;
                }
            }
            (KeyCode::Enter, Focus::Results) => {
                let entry = o.entries.get(o.selected).cloned();
                if let Some(entry) = entry {
                    if let Some(path) = self.active.clone() {
                        let file = o.file.trim().to_string();
                        let name = entry.name.clone();
                        return vec![Cmd::ReadSymbol { path, file, name }];
                    }
                }
            }
            _ => {}
        }
        Vec::new()
    }

    fn handle_lsp_key(&mut self, key: KeyEvent) -> Vec<Cmd> {
        let l = &mut self.lsp;
        match key.code {
            KeyCode::Char(c) => {
                if !key.modifiers.contains(KeyModifiers::CONTROL) {
                    l.input.push(c);
                    l.err.clear();
                } else if c == 'r' && !l.busy {
                    if let Some((file, line, ch)) = parse_lsp_input(&l.input) {
                        if let Some(path) = self.active.clone() {
                            l.busy = true;
                            l.err.clear();
                            l.kind = LspKind::References;
                            return vec![Cmd::LspReferences {
                                path,
                                file,
                                line,
                                character: ch,
                            }];
                        }
                    }
                }
            }
            KeyCode::Backspace => {
                l.input.pop();
            }
            KeyCode::Enter => {
                if !l.busy {
                    if let Some((file, line, ch)) = parse_lsp_input(&l.input) {
                        if let Some(path) = self.active.clone() {
                            l.busy = true;
                            l.err.clear();
                            l.kind = LspKind::Definition;
                            return vec![Cmd::LspDefinition {
                                path,
                                file,
                                line,
                                character: ch,
                            }];
                        }
                    }
                }
            }
            KeyCode::Up => {
                l.selected = l.selected.saturating_sub(1);
            }
            KeyCode::Down if l.selected + 1 < l.locations.len() => {
                l.selected += 1;
            }
            _ => {}
        }
        Vec::new()
    }

    fn handle_projects_key(&mut self, key: KeyEvent) -> Vec<Cmd> {
        // projects tab 的选中以 active 为准（单选模型）。
        match key.code {
            KeyCode::Enter => {
                // 切换 active 到选中项（当前实现：选中即 active，此处刷新）。
                return vec![Cmd::RefreshProjects];
            }
            KeyCode::Char('s') => {
                if let Some(path) = self.active.clone() {
                    return vec![Cmd::Sync(path)];
                }
            }
            KeyCode::Char('u') => {
                if let Some(path) = self.active.clone() {
                    self.dialog = Dialog::ConfirmUnmount(path);
                }
            }
            _ => {}
        }
        Vec::new()
    }

    /// 引擎结果回注（纯函数）。返回需要继续派发的 Cmd（如挂载后预取）。
    pub fn apply_ev(&mut self, ev: Ev) -> Vec<Cmd> {
        let mut cmds = Vec::new();
        match ev {
            Ev::Mount(Ok(report)) => {
                let path = PathBuf::from(&report.path);
                if let Some(_pending) = self.mount_page.busy.take() {
                    self.mount_page.busy = None;
                }
                self.active = Some(path.clone());
                self.call_tree.reset();
                self.set_toast(format!(
                    "mounted: {} files, {} symbols, {} ms{}",
                    report.files,
                    report.symbols,
                    report.elapsed_ms,
                    if report.vector_degraded {
                        format!(" (vector degraded: {})", report.vector_degraded_reason)
                    } else {
                        String::new()
                    }
                ));
                cmds.push(Cmd::RefreshProjects);
                // Skeleton 预取。
                self.skeleton.busy = true;
                cmds.push(Cmd::RepoMap {
                    path,
                    budget: self.skeleton.budget,
                });
            }
            Ev::Mount(Err(e)) => {
                self.mount_page.busy = None;
                self.mount_page.err = e;
            }
            Ev::Unmount(Ok((path, killed))) => {
                self.set_toast(format!("unmounted, killed_lsps={killed}"));
                if self.active.as_deref() == Some(path.as_path()) {
                    self.active = None;
                }
                self.projects
                    .retain(|p| std::path::Path::new(&p.path) != path);
                self.skeleton = SkeletonState::default();
                self.search = SearchState::default();
                self.outline = OutlineState::default();
                self.lsp = LspState::default();
                self.call_tree.reset();
                cmds.push(Cmd::RefreshProjects);
            }
            Ev::Unmount(Err(e)) => {
                self.set_toast(format!("unmount failed: {e}"));
            }
            Ev::Sync(Ok(report)) => {
                self.set_toast(format!(
                    "sync: +{} ~{} -{} failed={} ({} ms)",
                    report.added, report.modified, report.removed, report.failed, report.elapsed_ms
                ));
                cmds.push(Cmd::RefreshProjects);
            }
            Ev::Sync(Err(e)) => {
                self.set_toast(format!("sync failed: {e}"));
            }
            Ev::Projects(list) => {
                self.projects = list;
            }
            Ev::RepoMap(Ok((budget, text))) => {
                self.skeleton.busy = false;
                self.skeleton.budget = budget;
                self.skeleton.text = text;
                self.skeleton.scroll = 0;
            }
            Ev::RepoMap(Err(e)) => {
                self.skeleton.busy = false;
                self.skeleton.err = e;
            }
            Ev::Search(Ok(outcome)) => {
                self.search.busy = false;
                self.search.results = outcome.hits.clone();
                self.search.selected = 0;
                self.search.focus = Focus::Results;
                self.search.outcome = Some(outcome.clone());
                if outcome.degraded {
                    self.set_toast(format!("vector degraded: {}", outcome.degraded_reason));
                }
            }
            Ev::Search(Err(e)) => {
                self.search.busy = false;
                self.search.err = e;
            }
            Ev::Outline(Ok(entries)) => {
                self.outline.busy = false;
                self.outline.entries = entries;
                self.outline.selected = 0;
                self.outline.focus = Focus::Results;
            }
            Ev::Outline(Err(e)) => {
                self.outline.busy = false;
                self.outline.err = e;
            }
            Ev::Symbol(Ok(Some(code))) => {
                self.dialog = Dialog::SymbolText(code);
            }
            Ev::Symbol(Ok(None)) => {
                self.set_toast("symbol not found");
            }
            Ev::Symbol(Err(e)) => {
                self.set_toast(format!("read_symbol failed: {e}"));
            }
            Ev::TraceCalls(id, from, result) => self.call_tree.apply(id, from, result),
            Ev::TraceSymbol(path, result) => {
                if self.active.as_ref() == Some(&path) && self.tab == Tab::CallTree {
                    match result {
                        Ok(code) => self.dialog = Dialog::SymbolText(code),
                        Err(e) => self.set_toast(e),
                    }
                }
            }
            Ev::Lsp(Ok((kind, locations))) => {
                self.lsp.busy = false;
                self.lsp.ever_spawned = true;
                self.lsp.kind = kind;
                self.lsp.locations = locations;
                self.lsp.selected = 0;
            }
            Ev::Lsp(Err(e)) => {
                self.lsp.busy = false;
                self.lsp.err = e;
            }
        }
        cmds
    }
}

/// 解析 LSP 位置输入 `rel_path:line:character`。
/// line 1-based、character 0-based（对齐 LspPositionParams）。
pub fn parse_lsp_input(input: &str) -> Option<(String, u32, u32)> {
    let input = input.trim();
    let (rest, ch) = input.rsplit_once(':')?;
    let (file, line) = rest.rsplit_once(':')?;
    if file.is_empty() {
        return None;
    }
    let line: u32 = line.trim().parse().ok()?;
    let ch: u32 = ch.trim().parse().ok()?;
    if line == 0 {
        return None;
    }
    Some((file.to_string(), line, ch))
}
