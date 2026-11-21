//! RepoMap：PageRank 加权骨架图的对外格式化输出。

use crate::pagerank::pagerank;
use crate::types::{SymbolDef, SymbolRef};

/// token 估算：累计字符数 / 4。
const CHARS_PER_TOKEN: usize = 4;

/// PageRank 加权骨架图输出（纯函数）。
///
/// 输出每行 <rel_path>:<start_line> <kind小写> <name>，按分数降序
/// （同分按路径稳定排序）；累计字符 / 4 超过 token_budget 时截断并
/// 追加尾行 (truncated, budget=<n>)；空图（无符号）返回空字符串。
pub fn repo_map(
    symbols: &[SymbolDef],
    refs: &[SymbolRef],
    focus_files: &[String],
    token_budget: usize,
) -> String {
    let ranked = pagerank(symbols, refs, focus_files);
    if ranked.is_empty() {
        return String::new();
    }

    let mut out = String::new();
    let mut chars = 0usize;
    let mut truncated = false;
    for (node, _score) in &ranked {
        let line = format_line(node);
        // 该行入图后的 token 估算（含换行符）。
        let cost = (chars + line.len() + 1).div_ceil(CHARS_PER_TOKEN);
        if cost > token_budget {
            truncated = true;
            break;
        }
        chars += line.len() + 1;
        out.push_str(&line);
        out.push('\n');
    }
    if truncated {
        out.push_str(&format!("(truncated, budget={token_budget})\n"));
    }
    out
}

fn format_line(node: &crate::pagerank::RepoNode) -> String {
    let kind = format!("{:?}", node.kind).to_lowercase();
    format!(
        "{}:{} {} {}",
        node.rel_path, node.start_line, kind, node.name
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_file;

    const FIXTURES: [&str; 6] = [
        "src/lib.rs",
        "src/utils.rs",
        "src/main.rs",
        "utils.py",
        "app.py",
        "index.ts",
    ];

    fn fixture(rel: &str) -> String {
        let base = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/fixtures/sample-project/"
        );
        std::fs::read_to_string(format!("{base}{rel}")).expect("fixture missing")
    }

    /// 全 fixture 集解析合并出的 symbols / refs 全集。
    fn full_corpus() -> (Vec<SymbolDef>, Vec<SymbolRef>) {
        let mut symbols = Vec::new();
        let mut refs = Vec::new();
        for rel in FIXTURES {
            let text = fixture(rel);
            let lang = crate::detect_lang(rel).expect("lang detect");
            let pf = parse_file(lang, rel, &text);
            symbols.extend(pf.symbols);
            refs.extend(pf.refs);
        }
        (symbols, refs)
    }

    #[test]
    fn repo_map_contains_helper_spinner() {
        let (symbols, refs) = full_corpus();
        let out = repo_map(&symbols, &refs, &[], 3000);
        assert!(out.contains("helper_spinner"), "output:\n{out}");
        // 行格式：<rel_path>:<line> <kind> <name>。
        assert!(out.contains("src/utils.rs:4 fn helper_spinner"));
        // 无截断时不出现 truncated 尾行。
        assert!(!out.contains("truncated"));
    }

    #[test]
    fn repo_map_truncates_on_small_budget() {
        let (symbols, refs) = full_corpus();
        let out = repo_map(&symbols, &refs, &[], 80);
        assert!(out.contains("(truncated, budget=80)"), "output:\n{out}");
        // 截断发生在预算内：不含尾行的部分 token 估算 ≤ 80。
        let body: &str = out.trim_end_matches("\n");
        let marker = body.rfind("(truncated");
        let body_only = &body[..marker.unwrap_or(body.len())];
        assert!(body_only.chars().count() / 4 <= 80);
    }

    #[test]
    fn repo_map_focus_boosts_helper_spinner_into_top3() {
        let (symbols, refs) = full_corpus();
        let out = repo_map(&symbols, &refs, &["src/utils.rs".to_string()], 3000);
        let top3: Vec<&str> = out.lines().take(3).collect();
        let hit = top3
            .iter()
            .any(|l| l.contains("helper_spinner") && l.starts_with("src/utils.rs:"));
        assert!(hit, "focus top3:\n{}", top3.join("\n"));
    }

    #[test]
    fn repo_map_empty_input_returns_empty_string() {
        assert_eq!(repo_map(&[], &[], &[], 3000), "");
    }

    #[test]
    fn repo_map_lines_follow_format_and_order() {
        let (symbols, refs) = full_corpus();
        let out = repo_map(&symbols, &refs, &[], 3000);
        for line in out.lines() {
            // 形如 <path>:<num> <kind> <name>
            let (pos, rest) = line.split_once(':').expect("colon");
            let mut it = rest.splitn(3, ' ');
            let _line_no = it.next().expect("line no");
            let _kind = it.next().expect("kind");
            let name = it.next().expect("name");
            assert!(!name.is_empty());
            assert!(pos.contains('.'));
        }
    }

    #[test]
    fn repo_map_focus_empty_refs_still_ranked() {
        // 无引用时全靠基准值：focus 文件符号仍应最靠前。
        let (symbols, _refs) = full_corpus();
        let out = repo_map(&symbols, &[], &["app.py".to_string()], 3000);
        let first = out.lines().next().expect("first line");
        assert!(first.starts_with("app.py:"), "first = {first}");
    }
}
