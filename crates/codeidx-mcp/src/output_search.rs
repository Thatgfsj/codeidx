//! File-grouped compact text for search_code.

use std::collections::HashMap;

use codeidx_protocol::tools::{SearchHit, SearchMode};

pub(crate) struct SearchMeta<'a> {
    pub(crate) mode: SearchMode,
    pub(crate) elapsed_ms: u64,
    pub(crate) degraded: bool,
    pub(crate) degraded_reason: &'a str,
    pub(crate) total_candidates: u64,
}

/// Render hits supplied in engine rank order. Each file has a single header,
/// and each row contains rank, line range, score, symbol and snippet (tab-separated).
/// String fields use JSON string escaping, and an absent symbol is null.
///
/// Pack a prefix in rank order before emitting the groups, so a later hit in
/// an already visible file cannot displace an earlier hit in a new file.
/// The budget is ceil(final body UTF-8 bytes / 4), excluding the metadata line.
pub(crate) fn render_search(
    hits: &[SearchHit],
    token_budget: usize,
    meta: SearchMeta<'_>,
) -> String {
    let mut group_indices: HashMap<&str, usize> = HashMap::new();
    let mut groups: Vec<String> = Vec::new();
    let mut body_bytes = 0usize;
    let mut shown = 0usize;

    for (index, hit) in hits.iter().enumerate() {
        let rank = index + 1;
        let file = hit.file.as_str();
        let group_index = group_indices.get(file).copied();
        let header = group_index
            .is_none()
            .then(|| format!("# file={}\n", json_text(file)));
        let symbol = hit
            .symbol
            .as_deref()
            .map_or_else(|| "null".to_owned(), json_text);
        let row = format!(
            "{rank}\t{}-{}\t{:.4}\t{symbol}\t{}\n",
            hit.start_line,
            hit.end_line,
            hit.score,
            json_text(&hit.snippet)
        );
        let next_bytes = body_bytes + header.as_ref().map_or(0, String::len) + row.len();
        if next_bytes.div_ceil(4) > token_budget {
            break;
        }

        if let Some(group_index) = group_index {
            groups[group_index].push_str(&row);
        } else {
            group_indices.insert(file, groups.len());
            let mut group = header.expect("a new file has a header");
            group.push_str(&row);
            groups.push(group);
        }
        shown = rank;
        body_bytes = next_bytes;
    }

    let mode = match meta.mode {
        SearchMode::Keyword => "keyword",
        SearchMode::Vector => "vector",
        SearchMode::Hybrid => "hybrid",
    };
    let mut out = format!(
        "# search hits={shown} truncated_hits={} tokens≈{}/budget={token_budget} mode={mode} elapsed_ms={} degraded={} total_candidates={} degraded_reason={}\n",
        hits.len() - shown,
        body_bytes.div_ceil(4),
        meta.elapsed_ms,
        meta.degraded,
        meta.total_candidates,
        json_text(meta.degraded_reason)
    );
    out.reserve(body_bytes);
    for group in groups {
        out.push_str(&group);
    }
    out
}

fn json_text(value: &str) -> String {
    serde_json::to_string(value).expect("strings serialize to JSON")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(file: &str, snippet: &str) -> SearchHit {
        SearchHit {
            file: file.into(),
            symbol: None,
            start_line: 11,
            end_line: 13,
            score: 0.125,
            snippet: snippet.into(),
        }
    }

    fn meta() -> SearchMeta<'static> {
        SearchMeta {
            mode: SearchMode::Keyword,
            elapsed_ms: 3,
            degraded: false,
            degraded_reason: "",
            total_candidates: 17,
        }
    }

    #[test]
    fn groups_interleaved_hits_by_first_appearance_and_preserves_global_ranks() {
        let mut hits = [hit("z.py", "one"), hit("a.rs", "two"), hit("z.py", "three")];
        hits[1].symbol = Some("two".into());
        let out = render_search(&hits, 200, meta());
        let (header, body) = out.split_once('\n').unwrap();
        assert!(header.starts_with("# search hits=3 truncated_hits=0"));
        assert_eq!(
            body,
            concat!(
                "# file=\"z.py\"\n",
                "1\t11-13\t0.1250\tnull\t\"one\"\n",
                "3\t11-13\t0.1250\tnull\t\"three\"\n",
                "# file=\"a.rs\"\n",
                "2\t11-13\t0.1250\t\"two\"\t\"two\"\n"
            )
        );
    }

    #[test]
    fn round_trips_source_whitespace_and_field_delimiters() {
        let path = "sub\tdir/a\n\"file\".py";
        let symbol = "a\tb\n";
        let snippet = "\tif ok:\r\n\r\n    f('\\n')  \n\n";
        let mut entry = hit(path, snippet);
        entry.symbol = Some(symbol.into());
        let out = render_search(&[entry], 200, meta());
        let mut lines = out.lines().skip(1);
        let path_field = lines.next().unwrap().strip_prefix("# file=").unwrap();
        let columns: Vec<_> = lines.next().unwrap().split('\t').collect();
        assert_eq!(columns.len(), 5);
        for (field, expected) in [
            (path_field, path),
            (columns[3], symbol),
            (columns[4], snippet),
        ] {
            assert_eq!(serde_json::from_str::<String>(field).unwrap(), expected);
        }
        assert_eq!(lines.next(), None);
    }

    #[test]
    fn budgets_rendered_utf8_path_and_snippet_and_keeps_a_ranked_prefix() {
        let first = hit("共享.py", "    一\n\n\t二");
        let second = hit(&"深".repeat(40), "second");
        let third = hit("共享.py", "third");
        let same_file = render_search(&[first.clone(), third.clone()], 200, meta());
        let (_, body) = same_file.split_once('\n').unwrap();
        let budget = body.len().div_ceil(4);
        assert_ne!(budget, body.chars().count().div_ceil(4));
        assert!(same_file.contains(&format!("tokens≈{budget}/budget=200")));
        assert_eq!(
            render_search(&[first.clone(), third.clone()], budget, meta())
                .split_once('\n')
                .unwrap()
                .1,
            body
        );
        let below = render_search(&[first.clone(), third.clone()], budget - 1, meta());
        assert!(below.starts_with("# search hits=1 truncated_hits=1"));

        let out = render_search(&[first, second, third], budget, meta());
        assert!(out.starts_with("# search hits=1 truncated_hits=2"));
        assert_eq!(out.lines().count(), 3);
        assert!(!out.contains("second"));
        assert!(!out.contains("third"));
    }

    #[test]
    fn emits_metadata_when_no_hit_fits_or_no_hits_exist() {
        let degraded = || SearchMeta {
            degraded: true,
            degraded_reason: "model init failed:\n\"test\"",
            ..meta()
        };
        let over_budget = render_search(&[hit("a.rs", &"x".repeat(1000))], 200, degraded());
        let empty = render_search(&[], 200, degraded());
        assert_eq!(
            over_budget,
            "# search hits=0 truncated_hits=1 tokens≈0/budget=200 mode=keyword elapsed_ms=3 degraded=true total_candidates=17 degraded_reason=\"model init failed:\\n\\\"test\\\"\"\n"
        );
        assert_eq!(
            empty,
            over_budget.replace("truncated_hits=1", "truncated_hits=0")
        );
    }
}
