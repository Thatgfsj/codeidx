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

// ------------------------------------------------------------------ Phase 2 --
// 向量层单测（Issue #5）：确定性 MockEmbedder，禁用真模型，秒级完成。

use std::sync::atomic::{AtomicUsize, Ordering};

use codeidx_protocol::SearchHit;

use crate::embedder::Embedder;
use crate::tokens::split_tokens;
use crate::vec;

/// 确定性 mock 嵌入器：词袋叠加到固定维度的确定性位置后归一化。
///
/// 位置规则（手工可推算，测试断言完全确定）：
/// 词 w → 首字符 c 的两个位置 (c % dim) 与 ((c*7+3) % dim)；
/// 全文本按长度落一个 0.01 微扰位。
/// 语义可构造：共享首字符词越多的文本 cosine 越高（相似文本相近向量）；
/// 同一文本必得同一向量。
struct MockEmbedder {
    dim: usize,
    /// 累计嵌入文本条数（F2.4 计数断言锚点）。
    texts_embedded: AtomicUsize,
}

impl MockEmbedder {
    fn new() -> MockEmbedder {
        MockEmbedder {
            dim: 512,
            texts_embedded: AtomicUsize::new(0),
        }
    }

    fn mock_vec(&self, text: &str) -> Vec<f32> {
        let mut v = vec![0f32; self.dim];
        for w in split_tokens(text) {
            let c = w.chars().next().unwrap_or('a') as usize;
            v[c % self.dim] += 1.0;
            v[(c * 7 + 3) % self.dim] += 0.5;
        }
        // 全文本指纹微扰：词集相同但长度不同的文本仍可区分。
        v[text.len() % self.dim] += 0.01;
        let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        if n > f32::EPSILON {
            for x in &mut v {
                *x /= n;
            }
        }
        v
    }
}

impl Embedder for MockEmbedder {
    fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
        self.texts_embedded.fetch_add(texts.len(), Ordering::SeqCst);
        Ok(texts.iter().map(|t| self.mock_vec(t)).collect())
    }
    fn dim(&self) -> usize {
        self.dim
    }
}

/// 嵌入恒失败的 mock（replace_file 自愈路径断言用）。
struct FailEmbedder;

impl Embedder for FailEmbedder {
    fn embed_batch(&self, _texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
        Err("mock embed always fails".into())
    }
    fn dim(&self) -> usize {
        512
    }
}

/// 向量测试用单符号 ParsedFile。
fn one_chunk(
    rel: &str,
    symbol: Option<&str>,
    start: u32,
    end: u32,
    hash: &str,
    text: &str,
) -> ParsedFile {
    ParsedFile {
        rel_path: rel.into(),
        lang: Lang::Rust,
        symbols: vec![],
        refs: vec![],
        chunks: vec![Chunk {
            rel_path: rel.into(),
            symbol: symbol.map(str::to_string),
            start_line: start,
            end_line: end,
            content_hash: hash.into(),
            text: text.into(),
        }],
    }
}

/// mock 确定性：同一文本同向量、不同文本异向量、共享词 cosine 更高。
#[test]
fn mock_embedder_deterministic_and_distinct() {
    let m = MockEmbedder::new();
    let a1 = m.mock_vec("alpha beta gamma");
    let a2 = m.mock_vec("alpha beta gamma");
    assert_eq!(a1, a2, "同一文本必须同向量");
    assert_eq!(m.embed_batch(&["alpha".into()]).unwrap().len(), 1);
    let b = m.mock_vec("unrelated zulu yankee");
    assert_ne!(a1, b, "不同文本必须异向量");
    // 语义可构造：共享词的文本 cosine 高于无共享词文本。
    let near = m.mock_vec("alpha beta gamma extra");
    assert!(vec::cosine(&a1, &near) > vec::cosine(&a1, &b));
    // 归一化约定（Embedder 契约）。
    let n: f32 = a1.iter().map(|x| x * x).sum::<f32>().sqrt();
    assert!((n - 1.0).abs() < 1e-5, "L2 范数应为 1: {n}");
}

/// 未启用 / 未注入 embedder 的错误路径文案（F2.5 锚点）。
#[test]
fn vector_disabled_and_missing_embedder_error_paths() {
    // enable_vector=false：注入了 embedder 也必须报 disabled（vec 表未建）。
    let store = Store::new(false).unwrap();
    store.set_embedder(Some(Arc::new(MockEmbedder::new())));
    let err = store.search_vector("unicorn", 8).unwrap_err();
    assert!(
        err.message.contains("vector search disabled"),
        "文案不符: {err:?}"
    );
    // enable_vector=true 但未注入 embedder：同样 disabled。
    let store2 = Store::new(true).unwrap();
    let err2 = store2.search_vector("unicorn", 8).unwrap_err();
    assert!(
        err2.message.contains("vector search disabled (Phase 2)"),
        "文案不符: {err2:?}"
    );
    // 无 embedder 的 hybrid 降级为纯 keyword（逐字节一致）。
    store2.replace_file(&lib_rs()).unwrap();
    assert_eq!(
        store2.search_hybrid("unicorn", 8).unwrap(),
        store2.search_keyword("unicorn", 8).unwrap()
    );
}

/// vector 路入库后命中且 top1 语义最近；score 即 cosine、降序。
#[test]
fn vector_search_hits_semantic_top1() {
    let store = Store::new(true).unwrap();
    let mock = Arc::new(MockEmbedder::new());
    store.set_embedder(Some(mock));
    store
        .replace_file(&one_chunk(
            "src/a.rs",
            Some("alpha_fn"),
            1,
            4,
            "h1",
            "pub fn alpha_fn() { alpha beta }",
        ))
        .unwrap();
    store
        .replace_file(&one_chunk(
            "src/b.rs",
            Some("zulu_fn"),
            1,
            4,
            "h2",
            "pub fn zulu_fn() { zulu yankee }",
        ))
        .unwrap();
    let hits = store.search_vector("alpha beta", 8).unwrap();
    assert!(!hits.is_empty());
    assert_eq!(
        hits[0].symbol.as_deref(),
        Some("alpha_fn"),
        "top1 必须是语义最近 chunk: {hits:?}"
    );
    assert!(hits[0].score > 0.0 && hits[0].score <= 1.0);
    assert!(hits.windows(2).all(|w| w[0].score >= w[1].score));
    // 空 / 纯标点 query 与 keyword 路语义一致返回空。
    assert!(store.search_vector("", 8).unwrap().is_empty());
    assert!(store.search_vector("!?:;", 8).unwrap().is_empty());
}

/// RRF 精确数学：keyword-only 与 vector-only 名次错位场景。
/// kw_fn 双路第一 = 1/(60+1) + 1/(60+1) = 2/61；
/// vo_fn 仅 vector 第二 = 1/(60+2) = 1/62。
#[test]
fn rrf_fusion_exact_math() {
    let store = Store::new(true).unwrap();
    let mock = Arc::new(MockEmbedder::new());
    store.set_embedder(Some(mock));
    store
        .replace_file(&one_chunk(
            "src/kw.rs",
            Some("kw_fn"),
            1,
            4,
            "k1",
            "pub fn kw_fn() { alpha beta }",
        ))
        .unwrap();
    store
        .replace_file(&one_chunk(
            "src/vo.rs",
            Some("vo_fn"),
            1,
            4,
            "k2",
            "pub fn vo_fn() { unicorn marker }",
        ))
        .unwrap();
    let hits = store.search_hybrid("alpha", 8).unwrap();
    assert_eq!(
        hits.len(),
        2,
        "keyword-only 与 vector-only 命中者都应进入融合结果: {hits:?}"
    );
    assert_eq!(hits[0].symbol.as_deref(), Some("kw_fn"));
    assert!(
        (hits[0].score - 2.0 / 61.0).abs() < 1e-9,
        "kw_fn RRF = 2/61, got {}",
        hits[0].score
    );
    assert_eq!(hits[1].symbol.as_deref(), Some("vo_fn"));
    assert!(
        (hits[1].score - 1.0 / 62.0).abs() < 1e-9,
        "vo_fn RRF = 1/62, got {}",
        hits[1].score
    );
}

/// rrf_fuse 纯单元数学（不依赖 SQL）：双路 / 单路名次的融合分精确值。
#[test]
fn rrf_fuse_unit_math() {
    let hit = |line: u32| SearchHit {
        file: "a.rs".into(),
        symbol: None,
        start_line: line,
        end_line: line,
        score: 0.0,
        snippet: String::new(),
    };
    // keyword 名次 [id1, id2]；vector 名次 [id3, id1]。
    let kw = vec![(1i64, hit(1)), (2, hit(2))];
    let vc = vec![(3i64, hit(3)), (1, hit(1))];
    let fused = vec::rrf_fuse(kw, vc, 8);
    assert_eq!(fused.len(), 3);
    let by_line: std::collections::HashMap<u32, f64> =
        fused.iter().map(|h| (h.start_line, h.score)).collect();
    // id1：kw 第一 + vec 第二 = 1/61 + 1/62。
    assert!((by_line[&1] - (1.0 / 61.0 + 1.0 / 62.0)).abs() < 1e-12);
    // id2：kw 第二 only = 1/62。
    assert!((by_line[&2] - 1.0 / 62.0).abs() < 1e-12);
    // id3：vec 第一 only = 1/61。
    assert!((by_line[&3] - 1.0 / 61.0).abs() < 1e-12);
    // 降序输出。
    assert!(fused.windows(2).all(|w| w[0].score >= w[1].score));
}

/// search_hybrid 与"两路运行时名次的独立 RRF 计算"逐项一致。
#[test]
fn hybrid_matches_runtime_rrf_computation() {
    let store = Store::new(true).unwrap();
    let mock = Arc::new(MockEmbedder::new());
    store.set_embedder(Some(mock));
    let cases = [
        ("src/a.rs", "a_fn", "pub fn a_fn() { alpha beta }"),
        ("src/b.rs", "b_fn", "pub fn b_fn() { alpha alpha alpha }"),
        (
            "src/c.rs",
            "c_fn",
            "pub fn c_fn() { alpha zulu yankee xray }",
        ),
    ];
    for (i, (rel, sym, text)) in cases.iter().enumerate() {
        store
            .replace_file(&one_chunk(rel, Some(sym), 1, 4, &format!("h{i}"), text))
            .unwrap();
    }
    let kw = store.search_keyword_hits("alpha", 8).unwrap();
    let vc = store.search_vector_hits("alpha", 8).unwrap();
    // 独立按 RRF 公式计算期望融合分（不复用 rrf_fuse 实现）。
    let mut want: std::collections::HashMap<i64, f64> = std::collections::HashMap::new();
    for (rank, (id, _)) in kw.iter().enumerate() {
        *want.entry(*id).or_insert(0.0) += 1.0 / (60.0 + rank as f64 + 1.0);
    }
    for (rank, (id, _)) in vc.iter().enumerate() {
        *want.entry(*id).or_insert(0.0) += 1.0 / (60.0 + rank as f64 + 1.0);
    }
    // 两路名次应真实存在（避免空路假绿）。
    assert_eq!(kw.len(), 3);
    assert_eq!(vc.len(), 3);
    let got = store.search_hybrid("alpha", 8).unwrap();
    assert_eq!(got.len(), want.len());
    // id → file 映射后逐项对分。
    let id_of_file: std::collections::HashMap<String, i64> = kw
        .iter()
        .chain(vc.iter())
        .map(|(id, h)| (h.file.clone(), *id))
        .collect();
    for h in &got {
        let id = id_of_file[&h.file];
        assert!(
            (h.score - want[&id]).abs() < 1e-12,
            "file {} RRF 不符: got {} want {}",
            h.file,
            h.score,
            want[&id]
        );
    }
    assert!(got.windows(2).all(|w| w[0].score >= w[1].score));
}

/// F2.4 chunk 哈希缓存：同内容二次 replace 零重 embed；改一行只重 embed 变更 chunk。
#[test]
fn chunk_hash_cache_reuses_embeddings() {
    let store = Store::new(true).unwrap();
    let mock = Arc::new(MockEmbedder::new());
    store.set_embedder(Some(Arc::clone(&mock) as Arc<dyn Embedder>));
    let mk_chunk = |sym: &str, line: u32, hash: &str, text: &str| Chunk {
        rel_path: "src/lib.rs".into(),
        symbol: Some(sym.into()),
        start_line: line,
        end_line: line + 3,
        content_hash: hash.into(),
        text: text.into(),
    };
    let v1 = ParsedFile {
        rel_path: "src/lib.rs".into(),
        lang: Lang::Rust,
        symbols: vec![],
        refs: vec![],
        chunks: vec![
            mk_chunk("f1", 1, "h1", "pub fn f1() { alpha }"),
            mk_chunk("f2", 6, "h2", "pub fn f2() { beta }"),
        ],
    };
    store.replace_file(&v1).unwrap();
    let after_first = mock.texts_embedded.load(Ordering::SeqCst);
    assert_eq!(after_first, 2, "首次入库两个 chunk 都要 embed");
    // 同内容二次 replace：两个 chunk 哈希全命中，零重 embed（增量毫秒级契约）。
    store.replace_file(&v1).unwrap();
    assert_eq!(
        mock.texts_embedded.load(Ordering::SeqCst),
        after_first,
        "未变 chunk 不允许重 embed（F2.4）"
    );
    // 改 f1 内容（hash 变）后 replace：只有 f1 重 embed，f2 复用。
    // （计数断言必须先于 search_vector：query 嵌入同样计入计数器。）
    let mut v2 = v1.clone();
    v2.chunks[0].content_hash = "h1-modified".into();
    v2.chunks[0].text = "pub fn f1() { alpha gamma }".into();
    store.replace_file(&v2).unwrap();
    assert_eq!(
        mock.texts_embedded.load(Ordering::SeqCst),
        after_first + 1,
        "只有变更 chunk 重 embed"
    );
    // 复用的向量行仍然可检（旧向量真实写回；query 嵌入此时才计入）。
    let hits = store.search_vector("alpha", 8).unwrap();
    assert_eq!(
        hits.first().and_then(|h| h.symbol.clone()),
        Some("f1".into())
    );
}

/// embed 失败不破坏 FTS 入库；同哈希重 replace 时自愈重嵌入。
#[test]
fn embed_failure_keeps_fts_and_heals_on_retry() {
    let store = Store::new(true).unwrap();
    let fail = Arc::new(FailEmbedder);
    store.set_embedder(Some(Arc::clone(&fail) as Arc<dyn Embedder>));
    store
        .replace_file(&one_chunk(
            "src/a.rs",
            Some("a_fn"),
            1,
            4,
            "h1",
            "pub fn a_fn() { alpha }",
        ))
        .unwrap();
    // FTS 正常可检（embed 失败仅损失向量列）。
    assert!(store
        .search_keyword("alpha", 8)
        .unwrap()
        .iter()
        .any(|h| h.file == "src/a.rs"));
    // 查询路同样失败：FailEmbedder 的 query 嵌入直接报错。
    let qerr = store.search_vector("alpha", 8).unwrap_err();
    assert!(
        qerr.message.contains("embed query failed"),
        "查询嵌入失败文案不符: {qerr:?}"
    );
    // 换正常 embedder 后同内容 replace：哈希匹配但旧向量缺失 → 自愈重 embed。
    let mock = Arc::new(MockEmbedder::new());
    store.set_embedder(Some(Arc::clone(&mock) as Arc<dyn Embedder>));
    store
        .replace_file(&one_chunk(
            "src/a.rs",
            Some("a_fn"),
            1,
            4,
            "h1",
            "pub fn a_fn() { alpha }",
        ))
        .unwrap();
    assert_eq!(
        mock.texts_embedded.load(Ordering::SeqCst),
        1,
        "缺向量的同哈希 chunk 应重新嵌入（自愈）"
    );
    assert!(!store.search_vector("alpha", 8).unwrap().is_empty());
}

/// remove_file 同步删除向量行（检索不再命中）。
#[test]
fn remove_file_purges_vector_rows() {
    let store = Store::new(true).unwrap();
    let mock = Arc::new(MockEmbedder::new());
    store.set_embedder(Some(mock));
    store
        .replace_file(&one_chunk(
            "src/a.rs",
            Some("a_fn"),
            1,
            4,
            "h1",
            "pub fn a_fn() { alpha }",
        ))
        .unwrap();
    assert!(!store.search_vector("alpha", 8).unwrap().is_empty());
    store.remove_file("src/a.rs").unwrap();
    assert!(
        store.search_vector("alpha", 8).unwrap().is_empty(),
        "向量行应随 remove_file 同步删除"
    );
    // 幂等删除。
    store.remove_file("src/a.rs").unwrap();
}

/// 向量层并发冒烟：检索与写入并行互不阻塞出错误结果（沿用既有锁纪律）。
#[test]
fn concurrent_vector_search_smoke() {
    let store = Arc::new(Store::new(true).unwrap());
    let mock = Arc::new(MockEmbedder::new());
    store.set_embedder(Some(mock));
    store
        .replace_file(&one_chunk(
            "src/a.rs",
            Some("alpha_fn"),
            1,
            4,
            "h1",
            "pub fn alpha_fn() { alpha beta }",
        ))
        .unwrap();
    let mut handles = Vec::new();
    for _ in 0..4 {
        let s = Arc::clone(&store);
        handles.push(thread::spawn(move || {
            for _ in 0..25 {
                let hits = s.search_vector("alpha", 8).unwrap();
                assert_eq!(
                    hits.first().and_then(|h| h.symbol.clone()),
                    Some("alpha_fn".into())
                );
                let hy = s.search_hybrid("alpha", 8).unwrap();
                assert!(!hy.is_empty());
            }
        }));
    }
    for h in handles {
        h.join().expect("并发线程不应 panic");
    }
}
