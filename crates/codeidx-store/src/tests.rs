//! Store 行为单测（Issue #4 契约逐条断言）。样本为手工构造的 ParsedFile。

use std::sync::Arc;
use std::thread;

use codeidx_ast::{Chunk, Lang, ParsedFile, SymbolDef, SymbolKind, SymbolRef};

use crate::Store;

/// 样本：src/lib.rs —— unicorn_marker_fn（带注入上下文头）+ forge 方法。
fn lib_rs() -> ParsedFile {
    ParsedFile {
        rel_path: "src/lib.rs".into(),
        lang: Lang::Rust,
        symbols: vec![
            SymbolDef {
                kind: SymbolKind::Fn,
                name: "unicorn_marker_fn".into(),
                rel_path: "src/lib.rs".into(),
                start_line: 3,
                end_line: 9,
                signature: "fn unicorn_marker_fn(cfg: &GoldConfig) -> String".into(),
                parent: None,
            },
            SymbolDef {
                kind: SymbolKind::Method,
                name: "forge".into(),
                rel_path: "src/lib.rs".into(),
                start_line: 11,
                end_line: 14,
                signature: "pub fn forge(label: &str) -> GoldConfig".into(),
                parent: Some("GoldConfig".into()),
            },
        ],
        refs: vec![
            SymbolRef {
                name: "GoldConfig".into(),
                rel_path: "src/lib.rs".into(),
                line: 5,
            },
            SymbolRef {
                name: "helper_spinner".into(),
                rel_path: "src/lib.rs".into(),
                line: 6,
            },
        ],
        chunks: vec![
            Chunk {
                rel_path: "src/lib.rs".into(),
                symbol: Some("unicorn_marker_fn".into()),
                start_line: 3,
                end_line: 9,
                content_hash: "aa01".into(),
                text: concat!(
                    "// file: src/lib.rs (symbol: unicorn_marker_fn)\n",
                    "pub fn unicorn_marker_fn(cfg: &GoldConfig) -> String {\n",
                    "    let m = helper_spinner();\n",
                    "    let out = format!(\"{m}\");\n",
                    "    out\n",
                    "}"
                )
                .into(),
            },
            Chunk {
                rel_path: "src/lib.rs".into(),
                symbol: Some("forge".into()),
                start_line: 11,
                end_line: 14,
                content_hash: "aa02".into(),
                text: concat!(
                    "// file: src/lib.rs (symbol: forge)\n",
                    "pub fn forge(label: &str) -> GoldConfig {\n",
                    "    GoldConfig { name: label }\n",
                    "}"
                )
                .into(),
            },
        ],
    }
}

/// 样本：src/config.rs —— GoldConfig struct（无注入头，纯代码文本）。
fn config_rs() -> ParsedFile {
    ParsedFile {
        rel_path: "src/config.rs".into(),
        lang: Lang::Rust,
        symbols: vec![SymbolDef {
            kind: SymbolKind::Struct,
            name: "GoldConfig".into(),
            rel_path: "src/config.rs".into(),
            start_line: 1,
            end_line: 4,
            signature: "struct GoldConfig { name: String }".into(),
            parent: None,
        }],
        refs: vec![],
        chunks: vec![
            Chunk {
                rel_path: "src/config.rs".into(),
                symbol: Some("GoldConfig".into()),
                start_line: 1,
                end_line: 4,
                content_hash: "bb01".into(),
                text: concat!("pub struct GoldConfig {\n", "    pub name: String,\n", "}").into(),
            },
            Chunk {
                rel_path: "src/config.rs".into(),
                symbol: None,
                start_line: 6,
                end_line: 8,
                content_hash: "bb02".into(),
                text: concat!("let x = 1;\n", "let y = 2;").into(),
            },
        ],
    }
}

/// snake_case 天然可拆（unicode61 视下划线为分隔符，tokens 列同样命中）：
/// "unicorn" 查询命中 unicorn_marker_fn chunk，行号字段回读正确。
#[test]
fn keyword_hits_snake_case_chunk_and_lines() {
    let store = Store::new(false).unwrap();
    store.replace_file(&lib_rs()).unwrap();
    let hits = store.search_keyword("unicorn", 8).unwrap();
    assert!(!hits.is_empty(), "应命中 unicorn chunk: {hits:?}");
    let hit = hits
        .iter()
        .find(|h| h.symbol.as_deref() == Some("unicorn_marker_fn"))
        .expect("unicorn_marker_fn chunk 必须命中");
    assert_eq!(hit.file, "src/lib.rs");
    assert_eq!(hit.start_line, 3);
    assert_eq!(hit.end_line, 9);
    assert!(hit.score.is_finite() && hit.score >= 0.0);
}

/// camelCase 拆词（tokens 辅助列生效的判据）：body 列里 "GoldConfig" 是
/// 单 token，"gold config" 词查询只能经 tokens 列命中。
#[test]
fn keyword_hits_camel_case_via_tokens() {
    let store = Store::new(false).unwrap();
    store.replace_file(&lib_rs()).unwrap();
    store.replace_file(&config_rs()).unwrap();
    let hits = store.search_keyword("gold config", 8).unwrap();
    assert!(
        hits.iter()
            .any(|h| h.symbol.as_deref() == Some("GoldConfig")),
        "拆词后应命中 GoldConfig struct chunk: {hits:?}"
    );
    // 整词 camel 查询同样拆词命中。
    let hits2 = store.search_keyword("GoldConfig", 8).unwrap();
    assert!(!hits2.is_empty());
}

/// snippet ≤3 行且不含 "// file:" 上下文头。
#[test]
fn snippet_at_most_three_lines_without_header() {
    let store = Store::new(false).unwrap();
    store.replace_file(&lib_rs()).unwrap();
    let hits = store.search_keyword("unicorn", 8).unwrap();
    assert!(!hits.is_empty());
    for h in &hits {
        assert!(h.snippet.lines().count() <= 3, "snippet 行数超限");
        assert!(!h.snippet.contains("// file:"), "snippet 泄漏上下文头");
    }
    let hit = hits
        .iter()
        .find(|h| h.symbol.as_deref() == Some("unicorn_marker_fn"))
        .unwrap();
    assert_eq!(
        hit.snippet,
        concat!(
            "pub fn unicorn_marker_fn(cfg: &GoldConfig) -> String {\n",
            "    let m = helper_spinner();\n",
            "    let out = format!(\"{m}\");"
        )
    );
}

/// 同文件二次 replace_file 不产生重复行（stats 与检索均不变）。
#[test]
fn replace_file_is_idempotent() {
    let store = Store::new(false).unwrap();
    store.replace_file(&lib_rs()).unwrap();
    let s1 = store.stats();
    let hits1 = store.search_keyword("unicorn", 16).unwrap();
    store.replace_file(&lib_rs()).unwrap();
    let s2 = store.stats();
    assert_eq!(s1, s2);
    let hits2 = store.search_keyword("unicorn", 16).unwrap();
    assert_eq!(hits1.len(), hits2.len());
    assert_eq!(
        hits2
            .iter()
            .filter(|h| h.symbol.as_deref() == Some("unicorn_marker_fn"))
            .count(),
        1,
        "unicorn chunk 不允许重复命中: {hits2:?}"
    );
    let outline = store.file_outline("src/lib.rs").unwrap();
    assert_eq!(outline.len(), 2);
    assert_eq!(
        outline
            .iter()
            .filter(|e| e.name == "unicorn_marker_fn")
            .count(),
        1
    );
}

/// remove_file 后该文件检索不再命中，outline / read_symbol 清空。
#[test]
fn remove_file_clears_all_rows() {
    let store = Store::new(false).unwrap();
    store.replace_file(&lib_rs()).unwrap();
    store.replace_file(&config_rs()).unwrap();
    store.remove_file("src/lib.rs").unwrap();
    assert!(store.search_keyword("unicorn", 8).unwrap().is_empty());
    assert!(store.search_keyword("spinner", 8).unwrap().is_empty());
    let hits = store.search_keyword("gold", 8).unwrap();
    assert!(!hits.is_empty());
    assert!(hits.iter().all(|h| h.file == "src/config.rs"));
    assert!(store.file_outline("src/lib.rs").unwrap().is_empty());
    assert!(store
        .read_symbol("src/lib.rs", "unicorn_marker_fn")
        .unwrap()
        .is_none());
    let s = store.stats();
    assert_eq!(s.files, 1);
    assert_eq!(s.symbols, 1);
    // 不存在的文件再次删除幂等。
    store.remove_file("src/lib.rs").unwrap();
}

/// read_symbol 命中分支（code 去头、行号一致）与未命中分支。
#[test]
fn read_symbol_hit_and_miss() {
    let store = Store::new(false).unwrap();
    store.replace_file(&lib_rs()).unwrap();
    let code = store
        .read_symbol("src/lib.rs", "unicorn_marker_fn")
        .unwrap()
        .expect("read_symbol 必须命中");
    assert_eq!(code.file, "src/lib.rs");
    assert_eq!(code.name, "unicorn_marker_fn");
    assert_eq!(code.start_line, 3);
    assert_eq!(code.end_line, 9);
    assert!(!code.code.contains("// file:"));
    assert!(code.code.starts_with("pub fn unicorn_marker_fn"));
    assert!(code.code.ends_with('}'));
    assert_eq!(
        code.code.lines().count(),
        5,
        "完整实现体 5 行不应被截断: {code:?}"
    );
    assert!(store
        .read_symbol("src/lib.rs", "nope_fn")
        .unwrap()
        .is_none());
    assert!(store
        .read_symbol("src/none.rs", "unicorn_marker_fn")
        .unwrap()
        .is_none());
}

/// file_outline 按 start_line 升序，kind 为小写字符串。
#[test]
fn file_outline_sorted_lowercase_kinds() {
    let store = Store::new(false).unwrap();
    store.replace_file(&lib_rs()).unwrap();
    store.replace_file(&config_rs()).unwrap();
    let o = store.file_outline("src/lib.rs").unwrap();
    assert_eq!(o.len(), 2);
    assert_eq!(o[0].name, "unicorn_marker_fn");
    assert_eq!(o[0].kind, "fn");
    assert_eq!(o[0].start_line, 3);
    assert_eq!(o[0].end_line, 9);
    assert_eq!(o[0].sig, "fn unicorn_marker_fn(cfg: &GoldConfig) -> String");
    assert_eq!(o[1].name, "forge");
    assert_eq!(o[1].kind, "method");
    assert_eq!(o[1].start_line, 11);
    assert!(o[0].start_line < o[1].start_line);
    let o2 = store.file_outline("src/config.rs").unwrap();
    assert_eq!(o2.len(), 1);
    assert_eq!(o2[0].kind, "struct");
    assert!(store.file_outline("src/none.rs").unwrap().is_empty());
}

/// search_vector 始终返回 "vector search disabled (Phase 2)"。
#[test]
fn search_vector_reports_phase2_disabled() {
    let store = Store::new(true).unwrap();
    let err = store.search_vector("anything", 8).unwrap_err();
    assert!(
        err.message.contains("vector search disabled"),
        "错误文案不符: {}",
        err.message
    );
}

/// Phase 1 search_hybrid 降级为 search_keyword，结果逐字节一致。
#[test]
fn search_hybrid_equals_keyword() {
    let store = Store::new(false).unwrap();
    store.replace_file(&lib_rs()).unwrap();
    store.replace_file(&config_rs()).unwrap();
    for q in ["unicorn", "gold config", "GoldConfig", "spinner", "forge"] {
        assert_eq!(
            store.search_hybrid(q, 8).unwrap(),
            store.search_keyword(q, 8).unwrap(),
            "hybrid 应与 keyword 结果一致: q={q}"
        );
    }
}

/// 空 / 纯标点 query 与 top_k=0 一律返回空。
#[test]
fn empty_or_degenerate_queries_return_empty() {
    let store = Store::new(false).unwrap();
    store.replace_file(&lib_rs()).unwrap();
    assert!(store.search_keyword("", 8).unwrap().is_empty());
    assert!(store.search_keyword("   \t ", 8).unwrap().is_empty());
    assert!(store.search_keyword("!?;::", 8).unwrap().is_empty());
    assert!(store.search_keyword("gold", 0).unwrap().is_empty());
}

/// stats：files = distinct rel_path，symbols = symbols 行数。
#[test]
fn stats_counts_files_and_symbols() {
    let store = Store::new(false).unwrap();
    let s0 = store.stats();
    assert_eq!((s0.files, s0.symbols), (0, 0));
    store.replace_file(&lib_rs()).unwrap();
    store.replace_file(&config_rs()).unwrap();
    let s = store.stats();
    assert_eq!(s.files, 2);
    assert_eq!(s.symbols, 3);
}

/// BM25 结果按 score 降序。
#[test]
fn keyword_results_sorted_by_score_desc() {
    let store = Store::new(false).unwrap();
    store.replace_file(&lib_rs()).unwrap();
    store.replace_file(&config_rs()).unwrap();
    let hits = store.search_keyword("gold config", 8).unwrap();
    assert!(hits.len() >= 2);
    assert!(
        hits.windows(2).all(|w| w[0].score >= w[1].score),
        "hits 应按 score 降序: {hits:?}"
    );
}

/// 并发冒烟：4 线程同时对同一 Store 检索，不死锁不 panic。
#[test]
fn concurrent_search_smoke_no_deadlock() {
    let store = Arc::new(Store::new(false).unwrap());
    store.replace_file(&lib_rs()).unwrap();
    store.replace_file(&config_rs()).unwrap();
    let mut handles = Vec::new();
    for _ in 0..4 {
        let s = Arc::clone(&store);
        handles.push(thread::spawn(move || {
            for _ in 0..50 {
                let hits = s.search_keyword("gold config", 8).unwrap();
                assert!(!hits.is_empty());
                let _ = s.stats();
                let _ = s.file_outline("src/lib.rs").unwrap();
            }
        }));
    }
    for h in handles {
        h.join().expect("并发线程不应 panic");
    }
}

/// enable_vector 仅记录开关（含访问器）。
#[test]
fn vector_flag_recorded() {
    assert!(!Store::new(false).unwrap().is_vector_enabled());
    assert!(Store::new(true).unwrap().is_vector_enabled());
}
