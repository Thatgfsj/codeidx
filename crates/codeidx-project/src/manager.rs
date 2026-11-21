//! ProjectManager：多项目挂载表（DashMap）与生命周期入口。
//!
//! 键为 canonicalize 后的项目根；重复 mount 同一路径 = 卸载旧上下文后
//! 重建（幂等，且新上下文构建成功后才替换，失败不破坏既有索引）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use codeidx_ast::ParsedFile;
use codeidx_protocol::tools::{MountReport, ProjectInfo, SyncReport, UnmountReport};
use codeidx_protocol::ToolError;
use codeidx_store::Store;
use dashmap::DashMap;

use crate::{mount, sync, walker};

/// 单项目索引上下文：内存库 + 解析缓存 + 文件记录表。
pub struct ProjectContext {
    /// 项目根（canonicalize 后的绝对路径）。
    pub root: PathBuf,
    /// 内存索引库（SQLite :memory:，drop 即释放）。
    pub store: Store,
    /// rel_path -> 最新 ParsedFile（repo_map 需要 symbols+refs 全集）。
    pub parsed: HashMap<String, ParsedFile>,
    /// rel_path -> 文件记录（mtime 秒级 + size + blake3 十六进制）。
    pub files: HashMap<String, FileRecord>,
}

/// 单文件索引记录（增量 sync 的 mtime+size 快筛依据）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRecord {
    /// mtime（UNIX epoch 秒）。
    pub mtime: u64,
    /// 字节长度。
    pub size: u64,
    /// 内容 blake3 哈希（十六进制）。
    pub hash: String,
}

/// 多项目管理器：并发安全挂载表，全部方法 `&self`。
pub struct ProjectManager {
    projects: DashMap<PathBuf, ProjectContext>,
}

impl Default for ProjectManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ProjectManager {
    /// 新建空挂载表（全局单例由 codeidx-mcp 的 main.rs 持有）。
    pub fn new() -> ProjectManager {
        ProjectManager {
            projects: DashMap::new(),
        }
    }

    /// 未挂载错误文案（F1.11 契约：`project not mounted: <path>`）。
    fn not_mounted(path: &Path) -> ToolError {
        ToolError::new(format!("project not mounted: {}", path.display()))
    }

    /// 规范化路径键；路径不存在 → `path not found: <path>`。
    fn canonical_root(path: &Path) -> Result<PathBuf, ToolError> {
        std::fs::canonicalize(path)
            .map_err(|_| ToolError::new(format!("path not found: {}", path.display())))
    }

    /// 挂载：walker 遍历 → rayon 并行解析 → 全量入库。
    /// 重复 mount 同一路径 = 卸载旧上下文后重建（幂等）。
    pub fn mount(&self, path: &Path, enable_vector: bool) -> Result<MountReport, ToolError> {
        let started = Instant::now();
        let root = Self::canonical_root(path)?;
        // 新上下文全部构建成功后，才替换旧上下文（失败不破坏既有索引）。
        let built = mount::build_context(&root, enable_vector)?;
        let files = built.context.files.len() as u64;
        let symbols = built.symbols;
        self.projects.remove(&root); // drop 旧 store，内存即时释放
        self.projects.insert(root.clone(), built.context);
        Ok(MountReport {
            mounted: true,
            path: root.to_string_lossy().into_owned(),
            files,
            symbols,
            elapsed_ms: started.elapsed().as_millis() as u64,
            mem_est_mb: built.mem_est_mb,
            vector: enable_vector,
        })
    }

    /// 卸载：移除上下文（store 随之 drop，内存即释放）。
    /// killed_lsps Phase 1 恒 0，Phase 3 接 LSP 清理。
    pub fn unmount(&self, path: &Path) -> Result<UnmountReport, ToolError> {
        let root = Self::canonical_root(path).map_err(|_| Self::not_mounted(path))?;
        if self.projects.remove(&root).is_none() {
            return Err(Self::not_mounted(path));
        }
        Ok(UnmountReport {
            unmounted: true,
            killed_lsps: 0,
        })
    }

    /// 汇总当前已挂载项目（按路径排序，输出确定）。
    pub fn list(&self) -> Vec<ProjectInfo> {
        let mut infos: Vec<ProjectInfo> = self
            .projects
            .iter()
            .map(|entry| {
                let ctx = entry.value();
                ProjectInfo {
                    path: ctx.root.to_string_lossy().into_owned(),
                    files: ctx.files.len() as u64,
                    symbols: ctx.store.stats().symbols,
                    lsps: Vec::new(), // Phase 3 接 LSP 生命周期；Phase 1 恒空。
                    vector: ctx.store.is_vector_enabled(),
                }
            })
            .collect();
        infos.sort_by(|a, b| a.path.cmp(&b.path));
        infos
    }

    /// 增量同步：mtime+size 快筛 → 变更候选 blake3 复核 → 单文件事务重建。
    /// 未挂载报错同 with_project。
    pub fn sync(&self, path: &Path) -> Result<SyncReport, ToolError> {
        let started = Instant::now();
        let root = Self::canonical_root(path).map_err(|_| Self::not_mounted(path))?;
        // walk 在锁外完成，尽量缩短 DashMap 写锁持有时间。
        let present = walker::collect(&root);
        let mut ctx = self
            .projects
            .get_mut(&root)
            .ok_or_else(|| Self::not_mounted(path))?;
        Ok(sync::reconcile(ctx.value_mut(), present, started))
    }

    /// 查询入口：未挂载返回 `project not mounted: <path>`（对齐 F1.11）。
    /// 闭包只读上下文，不得访问 ProjectManager 自身（DashMap 读锁纪律）。
    pub fn with_project<T>(
        &self,
        path: &Path,
        f: impl FnOnce(&ProjectContext) -> Result<T, ToolError>,
    ) -> Result<T, ToolError> {
        let root = Self::canonical_root(path).map_err(|_| Self::not_mounted(path))?;
        let ctx = self
            .projects
            .get(&root)
            .ok_or_else(|| Self::not_mounted(path))?;
        f(ctx.value())
    }
}

/// 读取 /proc/self/status 的 VmRSS（KB）；不可用（非 Linux 等）返回 None。
pub(crate) fn vm_rss_kb() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            let kb = rest.trim().trim_end_matches("kB").trim();
            return kb.parse::<u64>().ok();
        }
    }
    None
}
