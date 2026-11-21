//! 11 个工具的 handler 接线（Phase 1-B）。
//!
//! - mount / unmount / list / sync：直连全局 `ProjectManager`（main.rs 单例）；
//! - get_repo_map / search_code / get_file_outline / read_symbol：
//!   `with_project` 拿上下文后调用 ast / store 真引擎；
//! - lsp_*：LSP 桥由 Phase 3 交付，无条件返回硬文案（契约不变）。

use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::de::DeserializeOwned;
use serde_json::Value;

use codeidx_ast::{detect_lang, repo_map, Lang};
use codeidx_protocol::tools::{
    FileOutline, HoverReport, Location, LocationsReport, LspPositionParams, MountParams,
    OutlineParams, PathParams, ProjectsReport, ReadSymbolParams, RepoMapParams, SearchMode,
    SearchParams, SearchResult,
};
use codeidx_protocol::{ToolError, ToolPayload};

use crate::MANAGER;

/// 参数反序列化：失败统一转执行错误（isError）。
fn parse_args<T: DeserializeOwned>(args: &Value) -> Result<T, ToolError> {
    serde_json::from_value(args.clone())
        .map_err(|e| ToolError::new(format!("invalid arguments: {e}")))
}

/// DTO → 紧凑 JSON 载荷。
fn report_to_value<T: serde::Serialize>(report: &T) -> ToolPayload {
    ToolPayload::Json(serde_json::to_value(report).expect("report DTO serializes"))
}

/// mount_project：真挂载（walker 遍历 + rayon 并行解析 + 全量入库）。
pub fn mount_project(args: &Value) -> Result<ToolPayload, ToolError> {
    let params: MountParams = parse_args(args)?;
    let enable_vector = params.enable_vector.unwrap_or(false);
    let report = MANAGER.mount(Path::new(&params.path), enable_vector)?;
    Ok(report_to_value(&report))
}

/// unmount_project：摘除上下文（store 随之 drop，内存即释放），
/// 并 SIGKILL 该 root 关联的全部 LSP 子进程（F3.4，报告真值）。
pub fn unmount_project(args: &Value) -> Result<ToolPayload, ToolError> {
    let params: PathParams = parse_args(args)?;
    let root = Path::new(&params.path);
    let mut report = MANAGER.unmount(root)?;
    report.killed_lsps = crate::LSP.kill_for_root(root) as u32;
    Ok(report_to_value(&report))
}

/// list_mounted_projects：manager.list 汇总 + LSP 桥的活动服务器名
/// （无参数，忽略 arguments）。
pub fn list_mounted_projects(_args: &Value) -> Result<ToolPayload, ToolError> {
    let mut projects = MANAGER.list();
    for info in &mut projects {
        info.lsps = crate::LSP.active_lsps(Path::new(&info.path));
    }
    Ok(report_to_value(&ProjectsReport { projects }))
}

/// sync_project：mtime/size 快筛 + blake3 复核的增量重建。
pub fn sync_project(args: &Value) -> Result<ToolPayload, ToolError> {
    let params: PathParams = parse_args(args)?;
    let report = MANAGER.sync(Path::new(&params.path))?;
    Ok(report_to_value(&report))
}

/// get_repo_map：parsed 全集 symbols+refs → ast::repo_map 纯文本骨架。
/// focus_files / token_budget 缺省：空聚焦、预算 3000。
pub fn get_repo_map(args: &Value) -> Result<ToolPayload, ToolError> {
    let params: RepoMapParams = parse_args(args)?;
    MANAGER.with_project(Path::new(&params.path), |ctx| {
        let mut symbols = Vec::new();
        let mut refs = Vec::new();
        for pf in ctx.parsed.values() {
            symbols.extend(pf.symbols.iter().cloned());
            refs.extend(pf.refs.iter().cloned());
        }
        let focus = params.focus_files.clone().unwrap_or_default();
        let budget = params.token_budget.unwrap_or(3000) as usize;
        Ok(ToolPayload::Text(repo_map(&symbols, &refs, &focus, budget)))
    })
}

/// search_code：mode 分流。keyword → FTS5；vector → 向量；hybrid → RRF。
/// top_k 缺省 8。**降级可见性（F2.5）**：vector 未就绪（未启用/
/// CODEIDX_DISABLE_VECTOR/模型加载失败）时一律降级 keyword，且响应
/// mode 字段回显实际执行模式——客户端据 mode 即可感知降级。
pub fn search_code(args: &Value) -> Result<ToolPayload, ToolError> {
    let params: SearchParams = parse_args(args)?;
    let mode = params.mode.unwrap_or(SearchMode::Hybrid);
    let top_k = params.top_k.unwrap_or(8) as usize;
    MANAGER.with_project(Path::new(&params.path), |ctx| {
        let started = Instant::now();
        let effective = if ctx.store.is_vector_ready() {
            mode
        } else {
            tracing::warn!(
                requested = ?mode,
                "vector not ready; degrading to keyword search"
            );
            SearchMode::Keyword
        };
        let results = match effective {
            SearchMode::Keyword => ctx.store.search_keyword(&params.query, top_k),
            SearchMode::Vector => ctx.store.search_vector(&params.query, top_k),
            SearchMode::Hybrid => ctx.store.search_hybrid(&params.query, top_k),
        }?;
        Ok(report_to_value(&SearchResult {
            results,
            mode: effective,
            elapsed_ms: started.elapsed().as_millis() as u64,
        }))
    })
}

/// get_file_outline：store.file_outline；文件未索引报执行错误。
pub fn get_file_outline(args: &Value) -> Result<ToolPayload, ToolError> {
    let params: OutlineParams = parse_args(args)?;
    MANAGER.with_project(Path::new(&params.path), |ctx| {
        if !ctx.files.contains_key(&params.file_path) {
            return Err(ToolError::new(format!(
                "file not indexed: {}",
                params.file_path
            )));
        }
        let symbols = ctx.store.file_outline(&params.file_path)?;
        Ok(report_to_value(&FileOutline {
            file: params.file_path.clone(),
            symbols,
        }))
    })
}

/// read_symbol：store.read_symbol 完整实现体；未命中报执行错误。
pub fn read_symbol(args: &Value) -> Result<ToolPayload, ToolError> {
    let params: ReadSymbolParams = parse_args(args)?;
    MANAGER.with_project(Path::new(&params.path), |ctx| {
        match ctx
            .store
            .read_symbol(&params.file_path, &params.symbol_name)?
        {
            Some(code) => Ok(report_to_value(&code)),
            None => Err(ToolError::new(format!(
                "symbol not found: {}",
                params.symbol_name
            ))),
        }
    })
}

// ------------------------------------------------------------------ LSP --
// Phase 3 接线：lsp_* 是 async 语义请求（LSP 子进程往返，冷启动秒级），
// server.rs 在 tools/call 层按工具名分流到 lsp_dispatch（本模块不提供
// 同步兜底——router 同步表已移除 lsp 三项）。

/// lsp_* 三工具的 async 分发入口（server 层按名分流到此）。
pub async fn lsp_dispatch(name: &str, args: &Value) -> Result<ToolPayload, ToolError> {
    use codeidx_protocol::tools::tool_name;
    match name {
        tool_name::LSP_DEFINITION => lsp_definition_async(args).await,
        tool_name::LSP_REFERENCES => lsp_references_async(args).await,
        tool_name::LSP_HOVER => lsp_hover_async(args).await,
        other => Err(ToolError::new(format!("unknown lsp tool: {other}"))),
    }
}

/// lsp_* 公共前置：挂载校验 → 语言探测 → 磁盘读全文。
///
/// 未挂载 → project not mounted: <path>（与其它工具同文案）；
/// 扩展名不受支持 → unsupported language for LSP: <file_path>；
/// 读盘失败 → cannot read file: <file_path>: <原因>。
async fn lsp_prepare(params: &LspPositionParams) -> Result<(PathBuf, Lang, String), ToolError> {
    let root = Path::new(&params.path);
    // 只做挂载校验，不消费索引内容（语义查询以磁盘文件 + LSP 分析为准）。
    MANAGER.with_project(root, |_| Ok(()))?;
    let Some(lang) = detect_lang(&params.file_path) else {
        return Err(ToolError::new(format!(
            "unsupported language for LSP: {}",
            params.file_path
        )));
    };
    let text = std::fs::read_to_string(root.join(&params.file_path))
        .map_err(|e| ToolError::new(format!("cannot read file {}: {e}", params.file_path)))?;
    Ok((root.to_path_buf(), lang, text))
}

/// lsp_definition：Location 列表（1-based line、0-based character）。
async fn lsp_definition_async(args: &Value) -> Result<ToolPayload, ToolError> {
    let params: LspPositionParams = parse_args(args)?;
    let (root, lang, text) = lsp_prepare(&params).await?;
    let locations = crate::LSP
        .definition(
            &root,
            lang,
            &params.file_path,
            params.line,
            params.character,
            &text,
        )
        .await
        .map_err(|e| ToolError::new(e.to_string()))?;
    Ok(report_to_value(&LocationsReport {
        locations: to_protocol_locations(locations),
    }))
}

/// lsp_references：引用集（includeDeclaration=false）。
async fn lsp_references_async(args: &Value) -> Result<ToolPayload, ToolError> {
    let params: LspPositionParams = parse_args(args)?;
    let (root, lang, text) = lsp_prepare(&params).await?;
    let locations = crate::LSP
        .references(
            &root,
            lang,
            &params.file_path,
            params.line,
            params.character,
            &text,
        )
        .await
        .map_err(|e| ToolError::new(e.to_string()))?;
    Ok(report_to_value(&LocationsReport {
        locations: to_protocol_locations(locations),
    }))
}

/// lsp_hover：纯文本 contents；无信息时按契约返回空串。
async fn lsp_hover_async(args: &Value) -> Result<ToolPayload, ToolError> {
    let params: LspPositionParams = parse_args(args)?;
    let (root, lang, text) = lsp_prepare(&params).await?;
    let contents = crate::LSP
        .hover(
            &root,
            lang,
            &params.file_path,
            params.line,
            params.character,
            &text,
        )
        .await
        .map_err(|e| ToolError::new(e.to_string()))?;
    Ok(report_to_value(&HoverReport {
        contents: Value::String(contents),
    }))
}

/// 桥 Location → 协议 DTO（行号约定一致：1-based line、0-based character、
/// 根相对路径）。
fn to_protocol_locations(locations: Vec<codeidx_lsp::Location>) -> Vec<Location> {
    locations
        .into_iter()
        .map(|l| Location {
            file: l.file,
            line: l.line,
            character: l.character,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// 测试副本序号：全局 MANAGER 跨测试共享，tempdir 路径必须唯一。
    static SEQ: AtomicU64 = AtomicU64::new(0);

    fn fixture_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/sample-project")
    }

    fn copy_dir(src: &Path, dst: &Path) {
        fs::create_dir_all(dst).expect("create tempdir");
        for entry in fs::read_dir(src).expect("read fixture dir") {
            let entry = entry.expect("fixture entry");
            let ty = entry.file_type().expect("fixture entry type");
            let to = dst.join(entry.file_name());
            if ty.is_dir() {
                copy_dir(&entry.path(), &to);
            } else {
                fs::copy(entry.path(), &to).expect("copy fixture file");
            }
        }
    }

    /// fixture 的独立 tempdir 副本（handler 测试需要真实可挂载目录）。
    fn temp_project(tag: &str) -> PathBuf {
        let seq = SEQ.fetch_add(1, Ordering::SeqCst);
        let dst = std::env::temp_dir().join(format!(
            "codeidx-mcp-test-{tag}-{}-{seq}",
            std::process::id()
        ));
        copy_dir(&fixture_root(), &dst);
        dst
    }

    fn json_of(result: Result<ToolPayload, ToolError>) -> Value {
        match result.expect("handler ok") {
            ToolPayload::Json(value) => value,
            ToolPayload::Text(_) => panic!("expected Json payload"),
        }
    }

    fn text_of(result: Result<ToolPayload, ToolError>) -> String {
        match result.expect("handler ok") {
            ToolPayload::Text(s) => s,
            ToolPayload::Json(_) => panic!("expected Text payload"),
        }
    }

    #[test]
    fn mount_returns_real_stats_json() {
        let root = temp_project("mount-json");
        let value = json_of(mount_project(&json!({"path": root})));
        assert_eq!(value["mounted"], true);
        assert_eq!(value["files"], 6, "fixture has 6 source files");
        assert!(value["symbols"].as_u64().unwrap() >= 10);
        assert_eq!(value["vector"], false);
        assert!(value["path"].is_string());
        assert!(value["elapsed_ms"].is_u64());

        let list_value = json_of(list_mounted_projects(&Value::Null));
        let projects = list_value["projects"].as_array().unwrap();
        assert!(projects.iter().any(|p| p["path"]
            .as_str()
            .unwrap_or("")
            .contains("mcp-test-mount-json")));

        let sync_value = json_of(sync_project(&json!({"path": root})));
        assert_eq!(sync_value["added"], 0);
        assert_eq!(sync_value["modified"], 0);
        assert_eq!(sync_value["removed"], 0);
        assert_eq!(sync_value["files"], 6);

        let un_value = json_of(unmount_project(&json!({"path": root})));
        assert_eq!(un_value["unmounted"], true);
        assert_eq!(un_value["killed_lsps"], 0);
    }

    #[test]
    fn mount_echoes_enable_vector() {
        let root = temp_project("vector-flag");
        let value = json_of(mount_project(&json!({"path": root, "enable_vector": true})));
        assert_eq!(value["vector"], true);
        unmount_project(&json!({"path": root})).expect("unmount");
    }

    #[test]
    fn mount_missing_dir_reports_path_not_found() {
        let err = mount_project(&json!({"path": "/tmp/codeidx-mcp-test-missing-dir-xyz"}))
            .expect_err("missing dir must fail");
        assert!(err.message.contains("path not found"), "{:?}", err.message);
    }

    #[test]
    fn mount_with_missing_path_argument_is_execution_error() {
        let err = mount_project(&json!({"enable_vector": true})).expect_err("path is required");
        assert!(
            err.message.contains("invalid arguments"),
            "{:?}",
            err.message
        );
    }

    #[test]
    fn analysis_tools_require_mount() {
        let missing = "/tmp/codeidx-mcp-test-not-mounted-xyz";
        for result in [
            get_repo_map(&json!({"path": missing})),
            search_code(&json!({"path": missing, "query": "x"})),
            get_file_outline(&json!({"path": missing, "file_path": "src/a.rs"})),
            read_symbol(&json!({
                "path": missing,
                "file_path": "src/a.rs",
                "symbol_name": "f"
            })),
            sync_project(&json!({"path": missing})),
        ] {
            let err = result.expect_err("must require mount");
            assert!(err.message.contains("not mounted"), "{:?}", err.message);
        }
    }

    #[test]
    fn unmount_twice_reports_not_mounted() {
        let root = temp_project("unmount-twice");
        mount_project(&json!({"path": root})).expect("mount");
        let value = json_of(unmount_project(&json!({"path": root})));
        assert_eq!(value["unmounted"], true);
        let err = unmount_project(&json!({"path": root})).expect_err("double unmount");
        assert!(err.message.contains("not mounted"), "{:?}", err.message);
    }

    #[test]
    fn repo_map_text_payload_with_truncation_and_focus() {
        let root = temp_project("repomap");
        mount_project(&json!({"path": root})).expect("mount");
        let text = text_of(get_repo_map(&json!({"path": root, "token_budget": 3000})));
        assert!(text.contains("helper_spinner"), "map: {text}");
        assert!(text.contains("src/utils.rs:"));

        let tiny = text_of(get_repo_map(&json!({"path": root, "token_budget": 80})));
        assert!(tiny.contains("truncated"), "tiny map: {tiny}");

        // focus_files 聚焦：src/utils.rs 的符号应进入前三行。
        let focused = text_of(get_repo_map(&json!({
            "path": root,
            "focus_files": ["src/utils.rs"],
            "token_budget": 3000
        })));
        let top3: Vec<&str> = focused.lines().take(3).collect();
        assert!(
            top3.iter().any(|l| l.starts_with("src/utils.rs")),
            "focus top3: {top3:?}"
        );
        unmount_project(&json!({"path": root})).expect("unmount");
    }

    #[test]
    fn search_code_modes_dispatch() {
        let root = temp_project("search");
        mount_project(&json!({"path": root})).expect("mount");

        let value = json_of(search_code(&json!({
            "path": root,
            "query": "unicorn_marker_fn",
            "mode": "keyword",
            "top_k": 8
        })));
        assert_eq!(value["mode"], "keyword");
        let hits = value["results"].as_array().unwrap();
        assert!(!hits.is_empty());
        // F1.7 裁决已落地（store 精确标识符提升 EXACT_QUERY_BOOST）：
        // 完整 query 原样出现的定义块必居 top1，bm25 短块偏置不再压过定义。
        assert!(
            hits.first()
                .is_some_and(|h| h["file"] == "src/lib.rs" && h["symbol"] == "unicorn_marker_fn"),
            "unicorn chunk must rank top-1: {hits:?}"
        );
        assert!(value["elapsed_ms"].is_u64());

        // 缺省参数：本挂载未启用 vector，降级为 keyword 且 mode 如实
        // 回显实际执行模式（F2.5 降级可见性契约）。
        let value = json_of(search_code(
            &json!({"path": root, "query": "helper_spinner"}),
        ));
        assert_eq!(value["mode"], "keyword");
        assert!(!value["results"].as_array().unwrap().is_empty());

        // vector 模式未就绪：降级 keyword 成功返回并回显 keyword
        // （store 层 "vector search disabled" 错误保留给直接调用方）。
        let value = json_of(search_code(
            &json!({"path": root, "query": "spin", "mode": "vector"}),
        ));
        assert_eq!(value["mode"], "keyword");
        assert!(!value["results"].as_array().unwrap().is_empty());
        unmount_project(&json!({"path": root})).expect("unmount");
    }

    #[test]
    fn outline_and_read_symbol_json() {
        let root = temp_project("outline");
        mount_project(&json!({"path": root})).expect("mount");

        let value = json_of(get_file_outline(
            &json!({"path": root, "file_path": "src/lib.rs"}),
        ));
        assert_eq!(value["file"], "src/lib.rs");
        let symbols = value["symbols"].as_array().unwrap();
        let unicorn = symbols
            .iter()
            .find(|s| s["name"] == "unicorn_marker_fn")
            .expect("unicorn_marker_fn in outline");
        assert_eq!(unicorn["kind"], "fn");
        assert!(unicorn["start_line"].as_u64().unwrap() >= 1);

        let value = json_of(read_symbol(&json!({
            "path": root,
            "file_path": "src/lib.rs",
            "symbol_name": "unicorn_marker_fn"
        })));
        assert_eq!(value["name"], "unicorn_marker_fn");
        assert!(value["code"]
            .as_str()
            .unwrap()
            .contains("pub fn unicorn_marker_fn"));

        let err = read_symbol(&json!({
            "path": root,
            "file_path": "src/lib.rs",
            "symbol_name": "no_such_symbol"
        }))
        .expect_err("unknown symbol");
        assert!(
            err.message.contains("symbol not found"),
            "{:?}",
            err.message
        );

        let err = get_file_outline(&json!({"path": root, "file_path": "not_indexed.rs"}))
            .expect_err("unindexed file");
        assert!(
            err.message.contains("file not indexed"),
            "{:?}",
            err.message
        );
        unmount_project(&json!({"path": root})).expect("unmount");
    }

    /// Phase 3 重接线：旧硬文案不复存在，错误路径走真实校验。
    #[tokio::test]
    async fn lsp_error_paths_require_mount_lang_and_server() {
        // 1) 未挂载。
        let missing = "/tmp/codeidx-mcp-test-not-mounted-lsp-xyz";
        let err = lsp_dispatch(
            "lsp_definition",
            &json!({"path": missing, "file_path": "src/a.rs", "line": 1, "character": 0}),
        )
        .await
        .expect_err("unmounted must fail");
        assert!(err.message.contains("not mounted"), "{:?}", err.message);

        // 2) 已挂载但扩展名不是 LSP 支持语言。
        let root = temp_project("lsp-err");
        mount_project(&json!({ "path": root })).expect("mount");
        let err = lsp_dispatch(
            "lsp_definition",
            &json!({"path": root, "file_path": "README.md", "line": 1, "character": 0}),
        )
        .await
        .expect_err(".md is not an LSP language");
        assert!(
            err.message
                .contains("unsupported language for LSP: README.md"),
            "{:?}",
            err.message
        );

        // 3) 支持的语言但本机未装对应服务器（Python：basedpyright /
        //    pyright 均未装，F3.5 同型断言；装有则本条会失败，与
        //    bb_phase3 的 F3.5 同命运）。
        let err = lsp_dispatch(
            "lsp_definition",
            &json!({"path": root, "file_path": "app.py", "line": 1, "character": 0}),
        )
        .await
        .expect_err("python server missing on this machine");
        assert!(
            err.message.contains("not found on PATH"),
            "{:?}",
            err.message
        );
        // 全程未拉起任何子进程：unmount 应报 killed_lsps=0。
        let value = json_of(unmount_project(&json!({ "path": root })));
        assert_eq!(value["killed_lsps"], 0, "no server spawned");
    }

    /// 轻量接线冒烟：async 分发 → 真机 rust-analyzer definition 命中
    /// 目标定位；unmount 报告真值 killed_lsps=1（F3.4）。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn lsp_dispatch_smoke_definition_on_real_rust_analyzer() {
        let root = temp_project("lsp-smoke");
        mount_project(&json!({ "path": root })).expect("mount");
        let main_text = fs::read_to_string(root.join("src/main.rs")).expect("read main.rs");
        let mut call_line = 0u32;
        let mut call_char = 0u32;
        for (i, line) in main_text.lines().enumerate() {
            if line.contains("unicorn_marker_fn(&cfg)") {
                call_line = (i + 1) as u32;
                call_char = line.find("unicorn_marker_fn").expect("marker col") as u32;
                break;
            }
        }
        assert!(call_line > 0, "fixture marker present");

        let payload = lsp_dispatch(
            "lsp_definition",
            &json!({
                "path": root,
                "file_path": "src/main.rs",
                "line": call_line,
                "character": call_char,
            }),
        )
        .await
        .expect("definition via real rust-analyzer");
        let value = json_of(Ok(payload));
        let locations = value["locations"].as_array().expect("locations array");
        assert!(
            locations
                .iter()
                .any(|l| l["file"] == "src/lib.rs" && l["line"] == 27),
            "jumps to src/lib.rs:27, got {locations:?}"
        );

        let un = json_of(unmount_project(&json!({ "path": root })));
        assert_eq!(un["killed_lsps"], 1, "one rust-analyzer reaped");
    }
}
