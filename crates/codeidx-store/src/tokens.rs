//! 分词辅助：camelCase / snake_case 拆词、FTS5 MATCH 表达式生成、
//! 上下文头剥离与 snippet 截取。
//!
//! FTS5 默认 unicode61 分词器把下划线等标点视为分隔符（snake_case 天然
//! 可拆），但无法拆分驼峰标识符（`GoldConfig` 是单 token `goldconfig`）。
//! 写入侧的 `tokens` 辅助列把块内全部文本拆成小写词流，使 "gold
//! config" 这类自然词查询也能命中；查询侧用同一规则拆词后拼 OR 匹配。

/// 把文本拆成小写词流。
///
/// 规则：先按非字母数字字符切段；段内在大写边界开新词——
/// - 前一字符为小写或数字（`GoldConfig` → `gold config`）；
/// - 或"连续大写后跟小写"（缩写词边界，`HTTPServer` → `http server`）。
///
/// 示例：`unicorn_marker_fn` → `unicorn marker fn`；`GoldConfig` →
/// `gold config`；`HTTPServer` → `http server`。
pub(crate) fn split_tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for seg in text.split(|c: char| !c.is_alphanumeric()) {
        if seg.is_empty() {
            continue;
        }
        expand_camel(seg, &mut out);
    }
    out
}

/// 单段标识符内的驼峰拆分，结果追加到 `out`（全小写）。
fn expand_camel(seg: &str, out: &mut Vec<String>) {
    let chars: Vec<char> = seg.chars().collect();
    let mut cur = String::new();
    for (i, &c) in chars.iter().enumerate() {
        let boundary = !cur.is_empty()
            && c.is_uppercase()
            && (chars[i - 1].is_lowercase()
                || chars[i - 1].is_numeric()
                || (chars[i - 1].is_uppercase()
                    && chars.get(i + 1).is_some_and(|&n| n.is_lowercase())));
        if boundary {
            out.push(cur.to_lowercase());
            cur.clear();
        }
        cur.push(c);
    }
    if !cur.is_empty() {
        out.push(cur.to_lowercase());
    }
}

/// `tokens` 辅助列内容：全部小写词以空格连接。
pub(crate) fn tokens_column(text: &str) -> String {
    split_tokens(text).join(" ")
}

/// 查询预处理：同样规则拆词，词间 OR（优先召回），每词加双引号字面量化。
///
/// 词内只剩字母数字（拆分规则保证），双引号包裹防御 FTS5 保留字。
/// 拆不出任何词（空 / 纯标点 query）返回 `None`，调用方直接返回空结果。
pub(crate) fn match_expr(query: &str) -> Option<String> {
    let words = split_tokens(query);
    if words.is_empty() {
        return None;
    }
    Some(
        words
            .iter()
            .map(|w| format!("\"{w}\""))
            .collect::<Vec<_>>()
            .join(" OR "),
    )
}

/// 剥离 chunk.text 开头注入的上下文头行。
///
/// Phase 1-C（AST 提取）会在块文本前注入 `// file: <rel_path>` 形式的
/// 上下文注释（见 Issue #4 契约与单测断言"snippet 不含 // file: 头"）。
/// 判定规则：从首行起，行是注释（`//` / `#` / `/*` / `*` / `<!--` 开头）
/// 且包含 `file:` 或 `symbol:` 标记即跳过；遇到第一个不满足的行即停，
/// 后续正文（含正文内自己的注释）原样保留。无注入头时返回原文。
pub(crate) fn strip_context_header(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut idx = 0;
    while idx < lines.len() {
        let t = lines[idx].trim_start();
        let is_comment = t.starts_with("//")
            || t.starts_with('#')
            || t.starts_with("/*")
            || t.starts_with('*')
            || t.starts_with("<!--");
        let lower = t.to_ascii_lowercase();
        let marks_header = lower.contains("file:") || lower.contains("symbol:");
        if is_comment && marks_header {
            idx += 1;
        } else {
            break;
        }
    }
    lines[idx..].join("\n")
}

/// snippet：先剥上下文头，再取前 `max_lines` 行以换行连接。
pub(crate) fn snippet_of(text: &str, max_lines: usize) -> String {
    strip_context_header(text)
        .lines()
        .take(max_lines)
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_snake_case() {
        assert_eq!(
            split_tokens("unicorn_marker_fn"),
            vec!["unicorn", "marker", "fn"]
        );
    }

    #[test]
    fn splits_camel_case() {
        assert_eq!(split_tokens("GoldConfig"), vec!["gold", "config"]);
        assert_eq!(split_tokens("HTTPServer"), vec!["http", "server"]);
    }

    #[test]
    fn splits_mixed_sentence() {
        // 标点切段 + 段内驼峰拆分；数字并入前词。
        assert_eq!(
            split_tokens("pub fn forge(label: &str) -> GoldConfig"),
            vec!["pub", "fn", "forge", "label", "str", "gold", "config"]
        );
    }

    #[test]
    fn match_expr_quotes_and_ors() {
        assert_eq!(
            match_expr("gold config"),
            Some("\"gold\" OR \"config\"".to_string())
        );
        assert_eq!(
            match_expr("GoldConfig"),
            Some("\"gold\" OR \"config\"".to_string())
        );
        assert_eq!(match_expr(""), None);
        assert_eq!(match_expr("   "), None);
        assert_eq!(match_expr("!!!"), None);
    }

    #[test]
    fn strips_leading_context_header_only() {
        let text = "// file: src/lib.rs (symbol: forge)\n// symbol: forge\npub fn forge() {\n    // file: not a header\n}";
        assert_eq!(
            strip_context_header(text),
            "pub fn forge() {\n    // file: not a header\n}"
        );
        // 无注入头：原文不动。
        let plain = "let x = 1;";
        assert_eq!(strip_context_header(plain), plain);
        // Python 风格头同样剥离。
        assert_eq!(strip_context_header("# file: app.py\nx = 1"), "x = 1");
    }

    #[test]
    fn snippet_takes_first_lines_after_header() {
        let text = "// file: a.rs\nl1\nl2\nl3\nl4";
        assert_eq!(snippet_of(text, 3), "l1\nl2\nl3");
    }
}
