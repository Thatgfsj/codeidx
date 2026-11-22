//! Shared line-based attribution for code graphs.

/// Find the tightest closed interval containing a source line.
pub fn enclosing_symbol<T: Copy>(spans: &[(u32, u32, T)], line: u32) -> Option<T> {
    spans
        .iter()
        .filter(|(start, end, _)| *start <= line && line <= *end)
        .min_by_key(|(start, end, _)| end - start)
        .map(|(_, _, id)| *id)
}
