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
    let root = temp_project("mount-json space 中文");
    let canonical = fs::canonicalize(&root).expect("canonical fixture root");
    let value = json_of(mount_project(&json!({"path": canonical})));
    assert_eq!(value["mounted"], true);
    assert_eq!(value["files"], 6, "fixture has 6 source files");
    assert!(value["symbols"].as_u64().unwrap() >= 10);
    assert_eq!(value["vector"], false);
    assert_eq!(value["path"], canonical.to_str().unwrap());
    assert_eq!(value["skipped_unsupported"], 2, "Cargo.toml and README.md");
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
    let results = value["results"].as_str().expect("results compact text");
    let data_rows: Vec<&str> = results
        .lines()
        .filter(|l| !l.starts_with('#') && !l.starts_with("file\t"))
        .collect();
    assert!(!data_rows.is_empty(), "at least one data row: {results}");
    // F1.7 裁决已落地（store 精确标识符提升 EXACT_QUERY_BOOST）：
    // 完整 query 原样出现的定义块必居 top1（首个数据行）。
    assert!(
        data_rows[0].starts_with("src/lib.rs\tunicorn_marker_fn\t"),
        "top row must be unicorn definition: {}",
        data_rows[0]
    );
    assert!(value["elapsed_ms"].is_u64());

    // 产物语义硬化：keyword 无降级，degraded=false 且 reason 空串、
    // total_candidates 为正（固定字段集恒出现）。
    assert_eq!(value["degraded"], false, "keyword never degrades");
    assert_eq!(value["degraded_reason"], "");
    assert!(
        value["total_candidates"].as_u64().unwrap() > 0,
        "candidate pool: {}",
        value["total_candidates"]
    );

    // 缺省参数：本挂载未启用 vector，降级为 keyword 且 mode 如实
    // 回显实际执行模式（F2.5 降级可见性契约）。
    let value = json_of(search_code(
        &json!({"path": root, "query": "helper_spinner"}),
    ));
    assert_eq!(value["mode"], "keyword");
    assert!(
        value["results"]
            .as_str()
            .unwrap_or("")
            .contains("helper_spinner"),
        "degraded keyword still returns hits"
    );
    assert_eq!(value["degraded"], true, "hybrid degraded to keyword");
    assert_eq!(
        value["degraded_reason"], "vector not enabled for this mount",
        "reason must be explicit: {}",
        value["degraded_reason"]
    );

    // vector 模式未就绪：降级 keyword 成功返回并回显 keyword
    // （store 层 "vector search disabled" 错误保留给直接调用方）。
    let value = json_of(search_code(
        &json!({"path": root, "query": "spin", "mode": "vector"}),
    ));
    assert_eq!(value["mode"], "keyword");
    // query "spin" 的独立词命中在 src/main.rs（let spin = ...）。
    assert!(
        value["results"]
            .as_str()
            .unwrap_or("")
            .contains("src/main.rs"),
        "vector-degraded search still returns keyword hits"
    );
    assert_eq!(value["degraded"], true);
    assert_eq!(
        value["degraded_reason"],
        "vector not enabled for this mount"
    );
    unmount_project(&json!({"path": root})).expect("unmount");
}

/// top_k 硬 clamp：缺省 8、上限 50（传 1000 取 50）、显式小值照传。
#[test]
fn top_k_hard_clamped_to_fifty() {
    assert_eq!(clamp_top_k(None), 8, "default 8");
    assert_eq!(clamp_top_k(Some(1000)), 50, "over-limit clamps to 50");
    assert_eq!(clamp_top_k(Some(50)), 50, "boundary 50 stays 50");
    assert_eq!(clamp_top_k(Some(3)), 3, "small values pass through");
    assert_eq!(
        clamp_top_k(Some(0)),
        0,
        "explicit 0 is an empty-result request"
    );
}

/// token_budget 硬 clamp：缺省 1200、下限 200、上限 8000。
#[test]
fn token_budget_hard_clamped_to_range() {
    assert_eq!(clamp_token_budget(None), 1200, "default 1200");
    assert_eq!(clamp_token_budget(Some(100)), 200, "below floor clamps up");
    assert_eq!(
        clamp_token_budget(Some(99_999)),
        8000,
        "above ceiling clamps down"
    );
    assert_eq!(clamp_token_budget(Some(200)), 200, "floor boundary stays");
    assert_eq!(
        clamp_token_budget(Some(8000)),
        8000,
        "ceiling boundary stays"
    );
    assert_eq!(
        clamp_token_budget(Some(600)),
        600,
        "in-range passes through"
    );
}

/// hits_to_lines：元信息行 + 列名行 + 数据行；score 四位小数、
/// symbol 缺省 "-"、snippet 换行转义、预算截断计数真实。
#[test]
fn hits_to_lines_compact_rows_and_budget() {
    let hits = vec![
        SearchHit {
            file: "src/a.rs".into(),
            symbol: Some("f".into()),
            start_line: 1,
            end_line: 2,
            score: 0.5,
            snippet: "fn f() {
    inner"
                .into(),
        },
        SearchHit {
            file: "src/b.rs".into(),
            symbol: None,
            start_line: 3,
            end_line: 4,
            score: 1.23456789,
            snippet: "top level".into(),
        },
    ];
    let out = hits_to_lines(&hits, 1200);
    let lines: Vec<&str> = out.lines().collect();
    assert!(
        lines[0].starts_with("# search hits=2 truncated_hits=0 tokens≈")
            && lines[0].ends_with("/budget=1200"),
        "header: {}",
        lines[0]
    );
    assert_eq!(lines[1], "file	symbol	start	end	score	snippet");
    assert!(
        lines[2].starts_with("src/a.rs	f	1	2	0.5000	fn f() {"),
        "row: {}",
        lines[2]
    );
    // snippet 换行转义为字面 backslash-n 两字符，行结构完整。
    assert!(
        lines[2].contains("fn f() {\\n    inner"),
        "escaped: {}",
        lines[2]
    );
    // symbol 缺省 "-" 且 score 四位小数。
    assert!(
        lines[3].starts_with("src/b.rs	-	3	4	1.2346	"),
        "row: {}",
        lines[3]
    );

    // 预算闸：单行成本约 32 字符/4=8 tokens；预算 200 只放得下
    // 大量行——改用两条长行验证截断计数（第一条 ~260 tokens 即超 200）。
    let long_hits = vec![
        SearchHit {
            file: "src/long_a.rs".into(),
            symbol: Some("big".into()),
            start_line: 1,
            end_line: 9,
            score: 2.0,
            snippet: "x".repeat(900),
        },
        SearchHit {
            file: "src/long_b.rs".into(),
            symbol: Some("small".into()),
            start_line: 1,
            end_line: 9,
            score: 1.0,
            snippet: "y".repeat(50),
        },
    ];
    let truncated = hits_to_lines(&long_hits, 200);
    assert!(
        truncated.contains("# search hits=0 truncated_hits=2"),
        "budget cuts all rows: {truncated}"
    );
    // 只放第一条（第一条 ~240 tokens 已超 200）。
    let partial = hits_to_lines(&long_hits, 240);
    assert!(
        partial.contains("# search hits=1 truncated_hits=1"),
        "one row survives: {}",
        partial.lines().next().unwrap()
    );
    assert!(partial.contains("src/long_a.rs	"));

    // 空 hits：仅元信息行 + 列名行，计数真实为零。
    let empty = hits_to_lines(&[], 1200);
    assert_eq!(
        empty,
        "# search hits=0 truncated_hits=0 tokens≈0/budget=1200
file	symbol	start	end	score	snippet
"
    );
}

/// outline_to_text：头行计数 + tab 分列 + sig 内 tab 防御 + 空符号文件。
#[test]
fn outline_to_text_rows_and_tab_guard() {
    let entries = vec![
        OutlineEntry {
            kind: "fn".into(),
            name: "f".into(),
            start_line: 1,
            end_line: 2,
            sig: "fn f(a: &str)".into(),
        },
        OutlineEntry {
            kind: "struct".into(),
            name: "S".into(),
            start_line: 4,
            end_line: 5,
            sig: "struct S {	a: u32,	}".into(),
        },
    ];
    let out = outline_to_text("src/a.rs", &entries);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines[0], "# outline file=src/a.rs symbols=2");
    assert_eq!(lines[1], "fn	f	1	2	fn f(a: &str)");
    // sig 内 tab 替换空格（最后一列防错位）。
    assert_eq!(lines[2], "struct	S	4	5	struct S { a: u32, }");

    // 空符号文件：头行 symbols=0 后无数据行（真实标记空）。
    let empty = outline_to_text("src/empty.rs", &[]);
    assert_eq!(
        empty,
        "# outline file=src/empty.rs symbols=0
"
    );
}

/// MountReport 新固定字段端到端恒出现：skipped 三类计数 +
/// vector_degraded 零值态（mount 未请求向量）。
#[test]
fn mount_report_hardcoded_fields_end_to_end() {
    let root = temp_project("hardcode");
    let value = json_of(mount_project(&json!({"path": root})));
    for key in [
        "skipped_binary",
        "skipped_oversize",
        "skipped_unsupported",
        "vector_degraded",
        "vector_degraded_reason",
    ] {
        assert!(
            value.get(key).is_some(),
            "MountReport 恒出现字段缺失: {key} in {value}"
        );
    }
    assert_eq!(value["vector_degraded"], false);
    assert_eq!(value["vector_degraded_reason"], "");
    assert_eq!(value["skipped_binary"], 0, "fixture 无二进制");

    // list 的 ProjectInfo 同步携带降级态字段。
    let list_value = json_of(list_mounted_projects(&Value::Null));
    let projects = list_value["projects"].as_array().unwrap();
    let mine = projects
        .iter()
        .find(|p| {
            p["path"]
                .as_str()
                .unwrap_or("")
                .contains("mcp-test-hardcode")
        })
        .expect("project listed");
    assert_eq!(mine["vector_degraded"], false);
    assert_eq!(mine["vector_degraded_reason"], "");
    unmount_project(&json!({"path": root})).expect("unmount");
}

/// 端到端跳过计数：.md 计 unsupported、600KB 计 oversize、NUL 计
/// binary（真造三类文件；node_modules 属黑名单目录不计入三类）。
#[test]
fn mount_report_counts_three_skip_kinds() {
    let root = temp_project("skips");
    fs::write(root.join("notes.md"), "docs are unsupported").expect("write md");
    let big = format!("pub fn big() -> u32 {{ 1 }} {}", "// pad".repeat(90_000));
    fs::write(root.join("big.rs"), &big).expect("write big");
    let mut nul = b"pub fn n() -> u32 { 0 }".to_vec();
    nul.push(0u8);
    fs::write(root.join("nul.rs"), &nul).expect("write nul");

    let value = json_of(mount_project(&json!({"path": root})));
    assert_eq!(value["skipped_binary"], 1, "nul.rs counted: {value}");
    assert_eq!(value["skipped_oversize"], 1, "big.rs counted: {value}");
    // fixture 自带 Cargo.toml + README.md + 本用例的 notes.md = 3。
    assert_eq!(
        value["skipped_unsupported"], 3,
        "Cargo.toml + README.md + notes.md: {value}"
    );
    unmount_project(&json!({"path": root})).expect("unmount");
}

#[test]
fn outline_and_read_symbol_json() {
    let root = temp_project("outline");
    mount_project(&json!({"path": root})).expect("mount");

    // 紧凑行文本：首行元信息 + tab 分列数据行。
    let text = text_of(get_file_outline(
        &json!({"path": root, "file_path": "src/lib.rs"}),
    ));
    let header = text.lines().next().unwrap();
    assert!(
        header.starts_with("# outline file=src/lib.rs symbols="),
        "header: {header}"
    );
    assert!(
        text.contains("fn\tunicorn_marker_fn\t"),
        "unicorn row present: {text}"
    );
    assert!(text.contains("struct\tGoldConfig\t"), "GoldConfig row");
    // 数据行数与头行 symbols 计数一致。
    let declared: usize = header
        .rsplit('=')
        .next()
        .unwrap()
        .parse()
        .expect("symbols count numeric");
    let rows = text.lines().filter(|l| !l.starts_with('#')).count();
    assert_eq!(rows, declared, "rows match header count: {text}");

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

    // 3) 支持的语言但本机未装对应服务器（TypeScript：vtsls /
    //    typescript-language-server 均未装，F3.5 同型断言。多语言实装
    //    后 basedpyright 已在机，Python 语义由 bb_phase3 的 F3.6 与
    //    codeidx-lsp 的 polyglot 测试实测——teammate-f 修正，Lead 追认）。
    let err = lsp_dispatch(
        "lsp_definition",
        &json!({"path": root, "file_path": "index.ts", "line": 1, "character": 0}),
    )
    .await
    .expect_err("typescript server missing on this machine");
    assert!(
        err.message.contains("not found on PATH"),
        "{:?}",
        err.message
    );
    assert!(err.message.contains("typescript"), "{:?}", err.message);
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
