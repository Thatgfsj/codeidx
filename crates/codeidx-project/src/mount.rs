//! mount 引擎：walker 遍历 → rayon 并行解析 → 全量入库。
//!
//! 单文件失败（读取 / NUL / 超 512KB / 入库）一律 tracing 警告后跳过，
//! 不中断整库挂载（ARCHITECTURE §5.5）。
//!
//! 产物语义硬化：walker 三类跳过计数与向量降级原因随 BuiltContext 透传
//! 至 MountReport 固定字段；降级原因生成本模块单测覆盖（模型初始化以
//! 闭包注入，无需真实模型）。

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use rayon::prelude::*;

use crate::manager::{vm_rss_kb, FileRecord, ProjectContext};
use crate::walker;
use codeidx_ast::ParsedFile;
use codeidx_protocol::ToolError;
use codeidx_store::{Embedder, FastembedEmbedder, Store};

/// build_context 的构建产物。
pub(crate) struct BuiltContext {
    pub context: ProjectContext,
    /// store.stats().symbols（MountReport 的符号数）。
    pub symbols: u64,
    /// 挂载前后 /proc/self/status VmRSS 差值（MB）；读取失败为 0.0。
    pub mem_est_mb: f64,
    /// 真实生效的向量开关（降级后为 false，MountReport.vector 数据源）。
    pub vector: bool,
    /// walker 三类跳过计数（MountReport 固定字段）。
    pub skipped_binary: u64,
    pub skipped_oversize: u64,
    pub skipped_unsupported: u64,
    /// 请求向量但实际降级为纯 FTS5 时 true（MountReport / ProjectInfo）。
    pub vector_degraded: bool,
    /// 降级原因；未降级为空串。
    pub vector_degraded_reason: String,
}

/// CODEIDX_DISABLE_VECTOR 语义下的降级原因（协议契约文案）。
const REASON_ENV_DISABLED: &str = "CODEIDX_DISABLE_VECTOR=1";

/// CODEIDX_DISABLE_VECTOR 的 truthy 判定（ARCHITECTURE §5.8）。
fn is_truthy(v: &str) -> bool {
    matches!(
        v.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

/// mount 的真实向量开关与嵌入器注入（F2.1 / F2.5）：
/// - 请求 enable_vector=false → 纯 FTS5，不碰模型，不属降级；
/// - CODEIDX_DISABLE_VECTOR truthy → 降级，原因 "CODEIDX_DISABLE_VECTOR=1"；
/// - 模型初始化失败 → 降级，原因 "model init failed: ..."。
fn resolve_vector(enable_vector: bool) -> (bool, Option<Arc<dyn Embedder>>, String) {
    resolve_vector_with(enable_vector, || {
        FastembedEmbedder::global().map(|e| e as Arc<dyn Embedder>)
    })
}

/// 可测形态：模型初始化以闭包注入（单测覆盖全部降级分支与正常分支，
/// 不依赖真实模型缓存）。
fn resolve_vector_with(
    enable_vector: bool,
    model_init: impl FnOnce() -> Result<Arc<dyn Embedder>, String>,
) -> (bool, Option<Arc<dyn Embedder>>, String) {
    if !enable_vector {
        return (false, None, String::new());
    }
    if let Ok(v) = std::env::var("CODEIDX_DISABLE_VECTOR") {
        if is_truthy(&v) {
            tracing::warn!(
                value = %v,
                "CODEIDX_DISABLE_VECTOR set; vector disabled (F2.5)"
            );
            return (false, None, REASON_ENV_DISABLED.to_string());
        }
    }
    match model_init() {
        Ok(embedder) => (true, Some(embedder), String::new()),
        Err(err) => {
            let reason = format!("model init failed: {err}");
            tracing::warn!(
                reason = %reason,
                "fastembed init failed; mounting with vector disabled (F2.1)"
            );
            (false, None, reason)
        }
    }
}

/// 全量构建一个项目的索引上下文。
///
/// 流程：向量裁决（开关 + 环境变量 + 模型单例）→ VmRSS 采样 → walker
/// 收集（含三类跳过计数）→ rayon 并行加载（binary 计数在此收敛）→
/// Store 建库并逐文件 replace_file（单文件失败跳过）→ 汇总。
pub(crate) fn build_context(root: &Path, enable_vector: bool) -> Result<BuiltContext, ToolError> {
    // 真实向量开关（F2.1 / F2.5）：请求 + 环境变量 + 模型单例三重决定。
    let (vector, embedder, degraded_reason) = resolve_vector(enable_vector);
    let vector_degraded = enable_vector && !vector;
    let rss_before = vm_rss_kb();
    let outcome = walker::collect_with_counts(root);
    let sources = outcome.files;
    // 并行阶段：纯 CPU / IO 工作（读文件 + blake3 + tree-sitter）；
    // 跳过原因保留到计数收敛（binary / oversize）。
    let loaded: Vec<Result<walker::LoadedFile, walker::LoadSkip>> =
        sources.par_iter().map(walker::load_file).collect();

    let store = Store::new(vector)?;
    // 注入嵌入器（Store::new 建表后立即注入：逐文件 replace_file
    // 的向量写入依赖它；None = 纯 FTS5）。
    if let Some(e) = &embedder {
        store.set_embedder(Some(Arc::clone(e)));
    }
    let mut parsed: HashMap<String, ParsedFile> = HashMap::new();
    let mut files: HashMap<String, FileRecord> = HashMap::new();
    let mut skipped_binary = 0u64;
    let mut skipped_oversize = outcome.skipped_oversize;
    for lf in loaded {
        let lf = match lf {
            Ok(lf) => lf,
            Err(walker::LoadSkip::Binary) => {
                skipped_binary += 1;
                continue;
            }
            Err(walker::LoadSkip::Oversize) => {
                // collect 与读取之间文件增长：并入 oversize 计数。
                skipped_oversize += 1;
                continue;
            }
            Err(walker::LoadSkip::Read) => continue, // 已警告；不属三类计数
        };
        if let Err(err) = store.replace_file(&lf.parsed) {
            tracing::warn!(
                rel = %lf.parsed.rel_path,
                error = %err.message,
                "replace_file failed; skipping file"
            );
            continue;
        }
        files.insert(lf.parsed.rel_path.clone(), lf.record);
        parsed.insert(lf.parsed.rel_path.clone(), lf.parsed);
    }
    let symbols = store.stats().symbols;
    let mem_est_mb = match (rss_before, vm_rss_kb()) {
        (Some(before), Some(after)) => (after as f64 - before as f64) / 1024.0,
        _ => 0.0,
    };
    Ok(BuiltContext {
        context: ProjectContext {
            root: root.to_path_buf(),
            store,
            parsed,
            files,
            vector_degraded,
            vector_degraded_reason: degraded_reason.clone(),
        },
        symbols,
        mem_est_mb,
        vector,
        skipped_binary,
        skipped_oversize,
        skipped_unsupported: outcome.skipped_unsupported,
        vector_degraded,
        vector_degraded_reason: degraded_reason,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use codeidx_store::EMBED_DIM;

    /// CODEIDX_DISABLE_VECTOR 是进程级全局：三个 enable_vector=true 的
    /// 用例必须互斥执行，且持锁期间自行清理环境，杜绝并行污染。
    static VECTOR_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// 测试用嵌入器：embed_batch 返回固定零向量。
    struct MockEmbedder;

    impl Embedder for MockEmbedder {
        fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
            Ok(texts.iter().map(|_| vec![0f32; EMBED_DIM]).collect())
        }
        fn dim(&self) -> usize {
            EMBED_DIM
        }
    }

    /// 场景一：enable_vector=false → 不降级、不碰模型。
    #[test]
    fn resolve_vector_not_requested_no_degrade() {
        let (on, embedder, reason) =
            resolve_vector_with(false, || Err("must not be called".into()));
        assert!(!on);
        assert!(embedder.is_none());
        assert_eq!(reason, "", "未请求向量不属降级");
    }

    /// 场景二：CODEIDX_DISABLE_VECTOR=1 → 降级且原因固定文案。
    #[test]
    fn resolve_vector_env_disabled_degrades() {
        let _guard = VECTOR_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("CODEIDX_DISABLE_VECTOR", "1");
        let (on, embedder, reason) = resolve_vector_with(true, || Err("must not be called".into()));
        std::env::remove_var("CODEIDX_DISABLE_VECTOR");
        assert!(!on);
        assert!(embedder.is_none());
        assert_eq!(reason, "CODEIDX_DISABLE_VECTOR=1");
    }

    /// 场景三：模型初始化失败 → 降级且原因携带失败详情。
    #[test]
    fn resolve_vector_model_init_failure_degrades() {
        let _guard = VECTOR_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // 隔离其它用例残留的环境变量。
        std::env::remove_var("CODEIDX_DISABLE_VECTOR");
        let (on, embedder, reason) =
            resolve_vector_with(true, || Err("onnx session build failed".into()));
        assert!(!on);
        assert!(embedder.is_none());
        assert_eq!(reason, "model init failed: onnx session build failed");
    }

    /// 场景四：模型就绪 → 启用向量、无降级。
    #[test]
    fn resolve_vector_model_ready_enables() {
        let _guard = VECTOR_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("CODEIDX_DISABLE_VECTOR");
        let (on, embedder, reason) =
            resolve_vector_with(true, || Ok(Arc::new(MockEmbedder) as Arc<dyn Embedder>));
        assert!(on);
        assert!(embedder.is_some());
        assert_eq!(reason, "");
    }
}
