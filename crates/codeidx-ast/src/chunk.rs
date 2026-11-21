//! AST 分块：按符号边界切块，符号外区域生成文件级兜底块。

use crate::lang::comment_prefix;
use crate::types::{Chunk, Lang, SymbolDef};

/// 单个兜底块的行数上限（超出可硬切）。
pub(crate) const FALLBACK_CHUNK_MAX_LINES: usize = 200;

/// 按符号边界切块：
/// - 每个符号一个整块（嵌套符号允许区间重叠，子块更精准、父块给全文上下文）；
/// - 符号未覆盖的行段生成文件级兜底块（`symbol = None`，上限 200 行硬切）；
/// - 每块头部注入一行上下文 `// file: <rel_path> symbol: <name>`（Python 用 `#`）。
pub(crate) fn chunk_file(
    lang: Lang,
    rel_path: &str,
    text: &str,
    symbols: &[SymbolDef],
) -> Vec<Chunk> {
    let lines: Vec<&str> = text.lines().collect();
    let total = lines.len() as u32;
    if total == 0 {
        return Vec::new();
    }
    let prefix = comment_prefix(lang);
    let mut chunks = Vec::new();

    // 1) 符号整块（按起始行稳定排序）。
    let mut sorted: Vec<&SymbolDef> = symbols.iter().collect();
    sorted.sort_by_key(|s| (s.start_line, s.end_line));
    let mut covered: Vec<(u32, u32)> = Vec::new();
    for sym in &sorted {
        let start = sym.start_line.max(1).min(total);
        let end = sym.end_line.max(start).min(total);
        covered.push((start, end));
        let body = &lines[(start - 1) as usize..end as usize];
        let header = format!("{prefix} file: {rel_path} symbol: {}", sym.name);
        let body_text = render_text(&header, body);
        chunks.push(Chunk {
            rel_path: rel_path.to_string(),
            symbol: Some(sym.name.clone()),
            start_line: start,
            end_line: end,
            content_hash: blake3_hex(&body_text),
            text: body_text,
        });
    }

    // 2) 符号外兜底块（补集区间，按上限硬切）。
    for (start, end) in uncovered_spans(&covered, total) {
        let mut s = start;
        while s <= end {
            let e = (s + FALLBACK_CHUNK_MAX_LINES as u32 - 1).min(end);
            let body = &lines[(s - 1) as usize..e as usize];
            let header = format!("{prefix} file: {rel_path} symbol: file");
            let body_text = render_text(&header, body);
            chunks.push(Chunk {
                rel_path: rel_path.to_string(),
                symbol: None,
                start_line: s,
                end_line: e,
                content_hash: blake3_hex(&body_text),
                text: body_text,
            });
            s = e + 1;
        }
    }

    chunks.sort_by_key(|c| (c.start_line, c.end_line));
    chunks
}

fn render_text(header: &str, body: &[&str]) -> String {
    let mut text = String::from(header);
    for line in body {
        text.push('\n');
        text.push_str(line);
    }
    text
}

/// 相对 [1, total] 求覆盖区间的补集。
fn uncovered_spans(covered: &[(u32, u32)], total: u32) -> Vec<(u32, u32)> {
    let mut spans: Vec<(u32, u32)> = covered
        .iter()
        .filter(|(s, e)| s <= e && *s <= total && *e >= 1)
        .map(|&(s, e)| (s.max(1), e.min(total)))
        .collect();
    spans.sort_unstable();
    let mut out = Vec::new();
    let mut cursor = 1u32;
    for (s, e) in spans {
        if s > cursor {
            out.push((cursor, (s - 1).min(total)));
        }
        cursor = cursor.max(e + 1);
    }
    if cursor <= total {
        out.push((cursor, total));
    }
    out
}

fn blake3_hex(text: &str) -> String {
    blake3::hash(text.as_bytes()).to_hex().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_file;

    fn fixture(rel: &str) -> String {
        let base = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/fixtures/sample-project/"
        );
        std::fs::read_to_string(format!("{base}{rel}")).expect("fixture missing")
    }

    #[test]
    fn symbol_chunks_cover_defs_and_inject_header() {
        let text = fixture("src/lib.rs");
        let pf = parse_file(Lang::Rust, "src/lib.rs", &text);
        let gold = pf
            .chunks
            .iter()
            .find(|c| c.symbol.as_deref() == Some("GoldConfig"))
            .expect("GoldConfig chunk");
        assert_eq!(gold.start_line, 9);
        assert_eq!(gold.end_line, 14);
        assert!(gold
            .text
            .starts_with("// file: src/lib.rs symbol: GoldConfig"));
        // 头部之后紧跟定义体第一行。
        assert!(gold.text.contains("\npub struct GoldConfig {"));
        // blake3 十六进制：64 字符。
        assert_eq!(gold.content_hash.len(), 64);
        assert!(gold.content_hash.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(gold.content_hash, blake3_hex(&gold.text));
    }

    #[test]
    fn python_chunks_use_hash_comment() {
        let text = fixture("utils.py");
        let pf = parse_file(Lang::Python, "utils.py", &text);
        let spin = pf
            .chunks
            .iter()
            .find(|c| c.symbol.as_deref() == Some("helper_spinner"))
            .expect("py helper_spinner chunk");
        assert!(spin
            .text
            .starts_with("# file: utils.py symbol: helper_spinner"));
        let fallback = pf
            .chunks
            .iter()
            .find(|c| c.symbol.is_none())
            .expect("fallback chunk");
        assert!(fallback.text.starts_with("# file: utils.py symbol: file"));
    }

    #[test]
    fn fallback_chunks_respect_max_lines() {
        // 250 行无符号文本：兜底块上限 200 行 → 切成 2 块。
        let text = (0..250)
            .map(|i| format!("let x{i} = {i};"))
            .collect::<Vec<_>>()
            .join("\n");
        let pf = parse_file(Lang::Rust, "src/plain.rs", &text);
        let fallbacks: Vec<_> = pf.chunks.iter().filter(|c| c.symbol.is_none()).collect();
        assert_eq!(fallbacks.len(), 2);
        assert_eq!(fallbacks[0].start_line, 1);
        assert_eq!(fallbacks[0].end_line, 200);
        assert_eq!(fallbacks[1].start_line, 201);
        assert_eq!(fallbacks[1].end_line, 250);
    }

    #[test]
    fn chunks_are_line_complete_and_sorted() {
        let text = fixture("src/main.rs");
        let pf = parse_file(Lang::Rust, "src/main.rs", &text);
        // 相邻块行号连续无缝（按 start_line 排序后首尾相接）。
        let mut last_end = 0u32;
        for c in &pf.chunks {
            assert!(c.start_line >= 1);
            assert!(c.end_line >= c.start_line);
            assert_eq!(c.start_line, last_end + 1);
            last_end = c.end_line;
        }
        assert_eq!(last_end, text.lines().count() as u32);
    }

    #[test]
    fn empty_text_yields_no_chunks() {
        let pf = parse_file(Lang::Python, "empty.py", "");
        assert!(pf.chunks.is_empty());
    }

    #[test]
    fn dart_chunks_use_slash_comment_header() {
        let base = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/fixtures/dart-project/"
        );
        let text =
            std::fs::read_to_string(format!("{base}lib.dart")).expect("dart fixture missing");
        let pf = parse_file(Lang::Dart, "lib.dart", &text);
        let marker = pf
            .chunks
            .iter()
            .find(|c| c.symbol.as_deref() == Some("dart_unique_marker_fn"))
            .expect("marker chunk");
        assert!(marker
            .text
            .starts_with("// file: lib.dart symbol: dart_unique_marker_fn"));
        assert_eq!(marker.content_hash.len(), 64);
        assert_eq!(marker.content_hash, blake3_hex(&marker.text));
        let fallback = pf
            .chunks
            .iter()
            .find(|c| c.symbol.is_none())
            .expect("fallback chunk");
        assert!(fallback.text.starts_with("// file: lib.dart symbol: file"));
    }
}
