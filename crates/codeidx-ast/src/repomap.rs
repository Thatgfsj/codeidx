//! RepoMap：PageRank 加权骨架图的对外格式化输出。
//!
//! 产物语义硬化：首行元信息头（symbols / shown / budget / used_tokens /
//! truncated 固定键集），AI 消费方无需逐行计数即可判读预算消耗与截断量；
//! focus_files 中未索引路径以 `# focus_files ignored:` 真实标记，静默
//! 忽略不再发生；空图单行头与"无数据"明确区分。

use crate::pagerank::pagerank;
use crate::types::{SymbolDef, SymbolRef};

/// token 估算：累计字符数 / 4。
const CHARS_PER_TOKEN: usize = 4;

/// PageRank 加权骨架图输出（纯函数）。
///
/// 输出结构（行序固定）：
/// 1. 首行元信息头：`# repo_map symbols=<总> shown=<行数> budget=<n>
///    used_tokens≈<u> truncated=<未展示符号数>`；
/// 2. focus_files 未索引路径：每个一行 `# focus_files ignored: <path>`；
/// 3. 骨架行：每行 `<rel_path>:<start_line> <kind小写> <name>`，PageRank
///    分数降序（同分按路径稳定排序），累计字符 / 4 超过 token_budget
///    时截断；头行 / ignored 行 / 尾行为元信息不计入预算；
/// 4. truncated > 0 时尾行 `(truncated, budget=<n>)`；
/// 5. 空图（无符号）返回单行 `# repo_map symbols=0 (no indexable
///    definitions found)`——与执行错误（isError 路径）明确区分。
pub fn repo_map(
    symbols: &[SymbolDef],
    refs: &[SymbolRef],
    focus_files: &[String],
    token_budget: usize,
) -> String {
    let ranked = pagerank(symbols, refs, focus_files);
    if ranked.is_empty() {
        return "# repo_map symbols=0 (no indexable definitions found)
"
        .to_string();
    }

    // 1) 预算内选行：骨架行本身计入 used_tokens。
    let mut shown: Vec<String> = Vec::new();
    let mut chars = 0usize;
    let mut truncated = 0usize;
    for (node, _score) in &ranked {
        let line = format_line(node);
        // 该行入图后的 token 估算（含换行符）。
        let cost = (chars + line.len() + 1).div_ceil(CHARS_PER_TOKEN);
        if cost > token_budget {
            truncated = ranked.len() - shown.len();
            break;
        }
        chars += line.len() + 1;
        shown.push(line);
    }
    let used_tokens = chars.div_ceil(CHARS_PER_TOKEN);

    // 2) focus_files 未索引路径：路径在骨架图中无任何符号即静默忽略，
    //    此处逐个真实标记（AI 可核对聚焦为何未生效）。
    let mut out = format!(
        "# repo_map symbols={} shown={} budget={} used_tokens≈{} truncated={}
",
        ranked.len(),
        shown.len(),
        token_budget,
        used_tokens,
        truncated
    );
    for f in focus_files {
        if !ranked.iter().any(|(node, _)| &node.rel_path == f) {
            out.push_str(&format!(
                "# focus_files ignored: {f}
"
            ));
        }
    }

    // 3) 骨架行 + 截断尾行。
    for line in &shown {
        out.push_str(line);
        out.push('\n');
    }
    if truncated > 0 {
        out.push_str(&format!(
            "(truncated, budget={token_budget})
"
        ));
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

    /// 解析首行元信息头为键值对。
    fn parse_header(out: &str) -> std::collections::HashMap<String, String> {
        let header = out.lines().next().expect("header line");
        assert!(
            header.starts_with("# repo_map symbols="),
            "header: {header}"
        );
        header
            .trim_start_matches("# repo_map ")
            .split(' ')
            .map(|kv| {
                // used_tokens 与值以 ≈ 连接（契约原文），其余键用 =。
                let (k, v) = kv
                    .split_once('=')
                    .or_else(|| kv.split_once('≈'))
                    .expect("k=v or k≈v");
                (k.to_string(), v.to_string())
            })
            .collect()
    }

    /// 骨架行（非 # / 非 (truncated 行）。
    fn skeleton_lines(out: &str) -> Vec<&str> {
        out.lines()
            .filter(|l| !l.starts_with('#') && !l.starts_with("(truncated"))
            .collect()
    }

    #[test]
    fn repo_map_contains_helper_spinner() {
        let (symbols, refs) = full_corpus();
        let out = repo_map(&symbols, &refs, &[], 3000);
        assert!(
            out.contains("helper_spinner"),
            "output:
{out}"
        );
        // 行格式：<rel_path>:<line> <kind> <name>。
        assert!(out.contains("src/utils.rs:4 fn helper_spinner"));
        // 无截断时不出现截断尾行（头行 truncated=0 属正常元信息）。
        assert!(
            !out.contains("(truncated, budget="),
            "output:
{out}"
        );
    }

    #[test]
    fn repo_map_truncates_on_small_budget() {
        let (symbols, refs) = full_corpus();
        let out = repo_map(&symbols, &refs, &[], 80);
        assert!(
            out.contains("(truncated, budget=80)"),
            "output:
{out}"
        );
        let header = parse_header(&out);
        let used: usize = header["used_tokens"].parse().expect("used_tokens numeric");
        assert!(used <= 80, "预算内截断：used_tokens={used} > 80");
        let truncated: usize = header["truncated"].parse().expect("truncated numeric");
        assert!(truncated > 0);
    }

    #[test]
    fn repo_map_focus_boosts_helper_spinner_into_top3() {
        let (symbols, refs) = full_corpus();
        let out = repo_map(&symbols, &refs, &["src/utils.rs".to_string()], 3000);
        let top3: Vec<&str> = skeleton_lines(&out).into_iter().take(3).collect();
        let hit = top3
            .iter()
            .any(|l| l.contains("helper_spinner") && l.starts_with("src/utils.rs:"));
        assert!(
            hit,
            "focus top3:
{}",
            top3.join(
                "
"
            )
        );
        // 已索引的聚焦文件不得出现在 ignored 标记里。
        assert!(!out.contains("# focus_files ignored: src/utils.rs"));
    }

    #[test]
    fn repo_map_empty_input_returns_single_header_line() {
        let out = repo_map(&[], &[], &["some/file.rs".to_string()], 3000);
        // 空图：单行头，明确区分"无索引数据"与执行错误；不再输出 ignored 行。
        assert_eq!(
            out,
            "# repo_map symbols=0 (no indexable definitions found)
"
        );
    }

    #[test]
    fn repo_map_lines_follow_format_and_order() {
        let (symbols, refs) = full_corpus();
        let out = repo_map(&symbols, &refs, &[], 3000);
        // 头行键值形态 + 全量入图；used_tokens 与骨架行一致由
        // normal_state / truncated_state 两个专门用例分别校验。
        let header = parse_header(&out);
        assert_eq!(header["symbols"], symbols.len().to_string());
        assert_eq!(header["shown"], symbols.len().to_string());
        assert_eq!(header["budget"], "3000");
        for line in skeleton_lines(&out) {
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
        let lines = skeleton_lines(&out);
        let first = lines.first().expect("first skeleton line");
        assert!(first.starts_with("app.py:"), "first = {first}");
    }

    /// 元信息头四态之一：正常态——头行键集完整、truncated=0、无尾行。
    #[test]
    fn repo_map_header_normal_state_full_key_set() {
        let (symbols, refs) = full_corpus();
        let out = repo_map(&symbols, &refs, &[], 3000);
        let header = parse_header(&out);
        for key in ["symbols", "shown", "budget", "used_tokens", "truncated"] {
            assert!(
                header.contains_key(key),
                "header missing key {key}: {header:?}"
            );
        }
        assert_eq!(header["budget"], "3000");
        assert_eq!(header["truncated"], "0");
        assert_eq!(header["symbols"], header["shown"], "全量入图");
        assert!(!out.contains("(truncated, budget="));
    }

    /// 元信息头四态之二：截断态——shown < symbols、truncated>0、尾行存在。
    #[test]
    fn repo_map_header_truncated_state() {
        let (symbols, refs) = full_corpus();
        let out = repo_map(&symbols, &refs, &[], 60);
        let header = parse_header(&out);
        assert_eq!(header["budget"], "60");
        let shown: usize = header["shown"].parse().unwrap();
        let total: usize = header["symbols"].parse().unwrap();
        let truncated: usize = header["truncated"].parse().unwrap();
        assert!(shown < total, "shown={shown} total={total}");
        assert_eq!(shown + truncated, total, "shown+truncated 必须等于总数");
        assert!(out.contains("(truncated, budget=60)"));
    }

    /// 元信息头四态之三：空图态——单行头且带明确"无符号"语义。
    #[test]
    fn repo_map_header_empty_state() {
        let out = repo_map(&[], &[], &[], 3000);
        assert_eq!(
            out,
            "# repo_map symbols=0 (no indexable definitions found)
"
        );
    }

    /// 元信息头四态之四：focus ignored 态——未索引路径逐个标记。
    #[test]
    fn repo_map_header_focus_ignored_state() {
        let (symbols, refs) = full_corpus();
        let focus = vec![
            "src/utils.rs".to_string(),
            "no/such/file.rs".to_string(),
            "also/missing.ts".to_string(),
        ];
        let out = repo_map(&symbols, &refs, &focus, 3000);
        assert!(
            out.contains(
                "# focus_files ignored: no/such/file.rs
"
            ),
            "output:
{out}"
        );
        assert!(out.contains(
            "# focus_files ignored: also/missing.ts
"
        ));
        assert!(!out.contains("ignored: src/utils.rs"));
        // ignored 行在头行之后、骨架行之前。
        let lines: Vec<&str> = out.lines().collect();
        let ignored_pos = lines
            .iter()
            .position(|l| l.starts_with("# focus_files ignored: no/such"))
            .unwrap();
        assert!(ignored_pos == 1, "ignored 行紧跟头行：pos={ignored_pos}");
        assert!(lines[0].starts_with("# repo_map symbols="));
    }
}
