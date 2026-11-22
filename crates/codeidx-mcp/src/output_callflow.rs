//! Compact view of the engine's bounded call tree.

use std::fmt::Write as _;

use codeidx_project::callflow::{
    TraceBranch, TraceConfidence, TraceDirection, TraceNode, TraceReport,
};

pub(crate) fn compact(report: &TraceReport) -> String {
    let direction = match report.direction {
        TraceDirection::Down => "down",
        TraceDirection::Up => "up",
        TraceDirection::Both => "both",
    };
    let cut = &report.truncated;
    let mut text = format!(
        "flow roots={} dir={} depth<={} nodes={} edges={} lsp={}/{} degraded={} skipped={} cut_roots={} cut_depth={} cut_nodes={} cut_budget={} cut_candidates={}\n",
        report.roots.len(), direction, report.max_depth, report.nodes, report.edges,
        report.lsp_refined, report.lsp_attempted, u8::from(report.lsp_degraded),
        report.skipped, cut.roots, cut.depth, cut.nodes, cut.budget, cut.candidates,
    );
    for root in &report.roots {
        node_line(&mut text, root, 0, report.direction == TraceDirection::Both);
    }
    text
}

fn node_line(text: &mut String, node: &TraceNode, depth: usize, show_branch: bool) {
    let location = &node.location;
    let file = serde_json::to_string(&location.file).expect("path serializes");
    let name = &location.symbol;
    let symbol = if name
        .chars()
        .all(|c| c.is_alphanumeric() || matches!(c, '_' | ':' | '$'))
    {
        name.clone()
    } else {
        serde_json::to_string(name).expect("symbol serializes")
    };
    let _ = write!(text, "{:width$}", "", width = depth * 2);
    if show_branch {
        match node.branch {
            Some(TraceBranch::Down) => text.push_str("↓ "),
            Some(TraceBranch::Up) => text.push_str("↑ "),
            None => {}
        }
    }
    let _ = write!(text, "{symbol} @{file}:{}", location.line);
    match node.confidence {
        Some(TraceConfidence::Unique) => text.push_str(" unique"),
        Some(TraceConfidence::SameFile) => text.push_str(" same_file"),
        Some(TraceConfidence::Ambiguous) => {
            let _ = write!(text, " ?{}", node.candidates);
        }
        Some(TraceConfidence::Lsp) => text.push_str(" ✓"),
        None => {}
    }
    for (present, flag) in [
        (node.file_root, " file"),
        (node.inferred, " inferred"),
        (node.callback, " callback"),
        (node.dynamic, " ⇢"),
        (node.cycle, " ↻cycle"),
        (node.shared, " ↻shared"),
    ] {
        if present {
            text.push_str(flag);
        }
    }
    if node.truncated > 0 {
        let _ = write!(text, " …(+{})", node.truncated);
    }
    text.push('\n');
    for child in &node.children {
        node_line(text, child, depth + 1, show_branch);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::OutputFormat;
    use codeidx_project::callflow::{TraceLocation, TraceTruncation};
    use codeidx_protocol::{CallToolResult, Content};

    #[test]
    fn formats_return_one_body_and_share_the_tree() {
        let node = TraceNode {
            location: TraceLocation {
                file: "a\nb.rs".to_owned(),
                line: 1,
                symbol: "start".to_owned(),
            },
            confidence: Some(TraceConfidence::Ambiguous),
            branch: Some(TraceBranch::Up),
            candidates: 2,
            dynamic: false,
            callback: false,
            inferred: true,
            file_root: false,
            cycle: true,
            shared: false,
            truncated: 1,
            children: vec![],
        };
        let report = TraceReport {
            direction: TraceDirection::Both,
            roots: vec![node],
            max_depth: 1,
            nodes: 1,
            edges: 0,
            truncated: TraceTruncation::default(),
            lsp_attempted: 1,
            lsp_refined: 0,
            lsp_degraded: true,
            skipped: 1,
        };
        let default = OutputFormat::parse(&serde_json::json!({})).unwrap();
        let expected = compact(&report);
        assert!(expected.contains("↑ start @\"a\\nb.rs\":1 ?2 inferred ↻cycle …(+1)"));
        for format in [default, OutputFormat::Json, OutputFormat::Markdown] {
            let result: CallToolResult = format.render(&report, || compact(&report)).into();
            assert_eq!(result.content.len(), 1);
            let Content::Text { text } = &result.content[0];
            match format {
                OutputFormat::Compact => assert_eq!(text, &expected),
                OutputFormat::Markdown => assert_eq!(text, &format!("```text\n{expected}```\n")),
                OutputFormat::Json => {
                    let body: serde_json::Value = serde_json::from_str(text).unwrap();
                    assert_eq!(body["roots"][0]["location"]["symbol"], "start");
                    assert_eq!(body["roots"][0]["confidence"], "ambiguous");
                    assert_eq!(body["roots"][0]["cycle"], true);
                }
            }
        }
    }
}
