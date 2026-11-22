//! Compact MCP navigation output, independent of project lookup and indexing.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

use codeidx_ast::RepoNode;
use codeidx_protocol::tools::OutlineEntry;
use serde::Serialize;
use serde_json::{json, Value};

const BYTES_PER_TOKEN: usize = 4;

#[derive(Serialize)]
struct MapSymbol<'a> {
    start_line: u32,
    kind: String,
    name: &'a str,
    rank: usize,
}

struct PathGroup<'a> {
    path: &'a str,
    heading: String,
    rows: String,
    symbols: Vec<MapSymbol<'a>>,
}

/// Select a prefix of the PageRank-ordered input, then group it for display.
/// The budget is measured on compact text body bytes, including each path
/// heading once and all symbol rows, with newlines. Metadata is excluded.
pub(crate) fn compact_repo_map(
    ranked: &[(RepoNode, f64)],
    focus_files: &[String],
    token_budget: usize,
) -> (String, Value) {
    let indexed_paths: HashSet<_> = ranked
        .iter()
        .map(|(node, _)| node.rel_path.as_str())
        .collect();
    let focus_ignored: Vec<_> = focus_files
        .iter()
        .map(String::as_str)
        .filter(|path| !indexed_paths.contains(path))
        .collect();

    let mut group_of_path: HashMap<&str, usize> = HashMap::new();
    let mut groups: Vec<PathGroup<'_>> = Vec::new();
    let mut shown = 0;
    let mut body_bytes: usize = 0;
    for (index, (node, _)) in ranked.iter().enumerate() {
        let path = node.rel_path.as_str();
        let kind = format!("{:?}", node.kind).to_ascii_lowercase();
        let row = format!("  {} {kind} {}\n", node.start_line, single_line(&node.name));
        let group_index = group_of_path.get(path).copied();
        let heading = group_index
            .is_none()
            .then(|| format!("{}:\n", single_line(path)));
        let next_bytes = body_bytes + row.len() + heading.as_ref().map_or(0, String::len);
        if next_bytes.div_ceil(BYTES_PER_TOKEN) > token_budget {
            break;
        }

        let group_index = group_index.unwrap_or_else(|| {
            let index = groups.len();
            group_of_path.insert(path, index);
            groups.push(PathGroup {
                path,
                heading: heading.expect("new groups have headings"),
                rows: String::new(),
                symbols: Vec::new(),
            });
            index
        });
        let group = &mut groups[group_index];
        group.rows.push_str(&row);
        group.symbols.push(MapSymbol {
            start_line: node.start_line,
            kind,
            name: &node.name,
            rank: index + 1,
        });
        body_bytes = next_bytes;
        shown += 1;
    }

    let symbols = ranked.len();
    let truncated = symbols - shown;
    let used_tokens = body_bytes.div_ceil(BYTES_PER_TOKEN);
    let mut text = format!(
        "# repo_map symbols={symbols} shown={shown} budget={token_budget} used_tokens≈{used_tokens} truncated={truncated}\n"
    );
    for path in &focus_ignored {
        writeln!(text, "# focus_files ignored: {}", single_line(path)).unwrap();
    }
    let mut files = Vec::with_capacity(groups.len());
    for group in groups {
        text.push_str(&group.heading);
        text.push_str(&group.rows);
        files.push(json!({"path": group.path, "symbols": group.symbols}));
    }
    if truncated > 0 {
        writeln!(text, "(truncated, budget={token_budget})").unwrap();
    }

    let data = json!({
        "symbols": symbols,
        "shown": shown,
        "budget": token_budget,
        "used_tokens": used_tokens,
        "truncated": truncated,
        "focus_ignored": focus_ignored,
        "files": files,
    });
    (text, data)
}

/// Preserve the store's symbol order and original values in JSON. The compact
/// outline flattens control characters to keep each symbol on one line.
pub(crate) fn compact_outline(file: &str, entries: &[OutlineEntry]) -> (String, Value) {
    let mut text = format!(
        "# outline file={} symbols={}\n",
        single_line(file),
        entries.len()
    );
    for entry in entries {
        let OutlineEntry {
            kind,
            name,
            start_line,
            end_line,
            sig,
        } = entry;
        write!(text, "{start_line}").unwrap();
        if start_line != end_line {
            write!(text, "-{end_line}").unwrap();
        }
        write!(text, " {} {}", single_line(kind), single_line(name)).unwrap();
        let signature = single_line(sig);
        if !signature.trim().is_empty() {
            write!(text, " {}", signature.trim()).unwrap();
        }
        text.push('\n');
    }
    (text, json!({"file": file, "symbols": entries}))
}

fn single_line(value: &str) -> Cow<'_, str> {
    let is_separator = |c: char| c.is_control() || matches!(c, '\u{2028}' | '\u{2029}');
    if value.contains(is_separator) {
        Cow::Owned(value.replace(is_separator, " "))
    } else {
        Cow::Borrowed(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codeidx_ast::SymbolKind;

    fn node(path: &str, line: u32, name: &str, score: f64) -> (RepoNode, f64) {
        (
            RepoNode {
                rel_path: path.into(),
                name: name.into(),
                start_line: line,
                kind: SymbolKind::Fn,
            },
            score,
        )
    }

    #[test]
    fn repo_map_admits_in_rank_order_and_groups_after_selection() {
        let ranked = [
            node("z.rs", 1, "high", 4.0),
            node("a.rs", 2, "second", 3.0),
            node("z.rs", 3, "third", 2.0),
            node("indexed.rs", 4, "low", 1.0),
        ];
        let focus = ["indexed.rs".into(), "missing.rs".into()];
        let body = "z.rs:\n  1 fn high\n  3 fn third\na.rs:\n  2 fn second\n";
        let budget = body.len().div_ceil(BYTES_PER_TOKEN);
        let (text, data) = compact_repo_map(&ranked, &focus, budget);
        assert!(text.contains(body));
        assert!(text.contains("# focus_files ignored: missing.rs\n"));
        assert!(!text.contains("indexed.rs"));
        assert_eq!(data["symbols"], 4);
        assert_eq!(data["shown"], 3);
        assert_eq!(data["used_tokens"], budget);
        assert_eq!(data["truncated"], 1);
        assert_eq!(data["focus_ignored"], json!(["missing.rs"]));
        assert_eq!(data["files"][0]["path"], "z.rs");
        assert_eq!(data["files"][0]["symbols"][1]["rank"], 3);
        assert_eq!(data["files"][1]["symbols"][0]["rank"], 2);
    }

    #[test]
    fn path_heading_counts_and_budget_does_not_skip_higher_ranks() {
        let ranked = [
            node("really-long-file-path.rs", 1, "top", 2.0),
            node("x", 2, "x", 1.0),
        ];
        let (text, data) = compact_repo_map(&ranked, &[], 4);
        assert!(!text.contains("rs:"));
        assert!(!text.contains("x:"));
        assert_eq!(data["shown"], 0);
        assert_eq!(data["used_tokens"], 0);
        assert_eq!(data["truncated"], 2);
        assert_eq!(data["files"], json!([]));
    }

    #[test]
    fn empty_repo_map_still_reports_ignored_focus() {
        let (text, data) = compact_repo_map(&[], &["missing.rs".into()], 0);
        assert!(text.contains("symbols=0 shown=0"));
        assert!(text.contains("# focus_files ignored: missing.rs\n"));
        assert_eq!(data["focus_ignored"], json!(["missing.rs"]));
        assert_eq!(data["files"], json!([]));
    }

    #[test]
    fn compact_outline_keeps_ranges_and_json_retains_raw_signatures() {
        let entries = [
            OutlineEntry {
                kind: "fn".into(),
                name: "f".into(),
                start_line: 1,
                end_line: 5,
                sig: "fn f(a:\t&str)\r\n".into(),
            },
            OutlineEntry {
                kind: "type".into(),
                name: "T".into(),
                start_line: 8,
                end_line: 8,
                sig: String::new(),
            },
        ];
        let (text, data) = compact_outline("src/a.rs", &entries);
        assert_eq!(
            text,
            "# outline file=src/a.rs symbols=2\n1-5 fn f fn f(a: &str)\n8 type T\n"
        );
        assert_eq!(data, json!({"file": "src/a.rs", "symbols": entries}));
    }
}
