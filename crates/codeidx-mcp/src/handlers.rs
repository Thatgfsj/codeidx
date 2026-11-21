//! 11 个工具的 handler 接线（Phase 1-B）。
//!
//! - mount / unmount / list / sync：直连全局 `ProjectManager`（main.rs 单例）；
//! - get_repo_map / search_code / get_file_outline / read_symbol：
//!   `with_project` 拿上下文后调用 ast / store 真引擎；
//! - lsp_*：LSP 桥由 Phase 3 交付，无条件返回硬文案（契约不变）。

use std::path::Path;
use std::time::Instant;

use serde::de::DeserializeOwned;
use serde_json::Value;

use codeidx_ast::repo_map;
use codeidx_protocol::tools::{
    FileOutline, MountParams, OutlineParams, PathParams, ProjectsReport, ReadSymbolParams,
    RepoMapParams, SearchMode, SearchParams, SearchResult,
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

/// unmount_project：摘除上下文（store 随之 drop，内存即释放）。
pub fn unmount_project(args: &Value) -> Result<ToolPayload, ToolError> {
    let params: PathParams = parse_args(args)?;
    let report = MANAGER.unmount(Path::new(&params.path))?;
    Ok(report_to_value(&report))
}

/// list_mounted_projects：manager.list 汇总（无参数，忽略 arguments）。
pub fn list_mounted_projects(_args: &Value) -> Result<ToolPayload, ToolError> {
    Ok(report_to_value(&ProjectsReport {
        projects: MANAGER.list(),
    }))
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

/// search_code：mode 分流。keyword → FTS5；vector → 向量（Phase 1 报
/// 未启用）；hybrid → RRF（Phase 1 降级 keyword）。top_k 缺省 8。
/// 响应 mode 字段回显实际执行的模式。
pub fn search_code(args: &Value) -> Result<ToolPayload, ToolError> {
    let params: SearchParams = parse_args(args)?;
    let mode = params.mode.unwrap_or(SearchMode::Hybrid);
    let top_k = params.top_k.unwrap_or(8) as usize;
    MANAGER.with_project(Path::new(&params.path), |ctx| {
        let started = Instant::now();
        let results = match mode {
            SearchMode::Keyword => ctx.store.search_keyword(&params.query, top_k),
            SearchMode::Vector => ctx.store.search_vector(&params.query, top_k),
            SearchMode::Hybrid => ctx.store.search_hybrid(&params.query, top_k),
        }?;
        Ok(report_to_value(&SearchResult {
            results,
            mode,
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

/// lsp_definition：LSP 桥由 Phase 3 交付，此前无条件报不可用。
pub fn lsp_definition(_args: &Value) -> Result<ToolPayload, ToolError> {
    Err(ToolError::new("LSP bridge not available yet"))
}

/// lsp_references：同 lsp_definition。
pub fn lsp_references(_args: &Value) -> Result<ToolPayload, ToolError> {
    Err(ToolError::new("LSP bridge not available yet"))
}

/// lsp_hover：同 lsp_definition。
pub fn lsp_hover(_args: &Value) -> Result<ToolPayload, ToolError> {
    Err(ToolError::new("LSP bridge not available yet"))
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

        // 缺省参数：mode 回显 hybrid（Phase 1 内部降级 keyword）。
        let value = json_of(search_code(
            &json!({"path": root, "query": "helper_spinner"}),
        ));
        assert_eq!(value["mode"], "hybrid");
        assert!(!value["results"].as_array().unwrap().is_empty());

        // vector 模式：Phase 1 固定报未启用（isError 文案）。
        let err = search_code(&json!({"path": root, "query": "spin", "mode": "vector"}))
            .expect_err("vector disabled in Phase 1");
        assert_eq!(err.message, "vector search disabled (Phase 2)");
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

    #[test]
    fn lsp_tools_unconditionally_unavailable() {
        for result in [
            lsp_definition(&Value::Null),
            lsp_references(&Value::Null),
            lsp_hover(&Value::Null),
        ] {
            let err = result.expect_err("lsp stub must fail");
            assert_eq!(err.message, "LSP bridge not available yet");
        }
    }
}
