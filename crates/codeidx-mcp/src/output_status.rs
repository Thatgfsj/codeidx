//! Compact text rendering for project reports and LSP point locations.
//!
//! Header string values use JSON quoting. TSV text cells escape backslash,
//! tab, CR and LF as \\, \t, \r and \n; the lsps cell is a compact JSON array.

use std::borrow::Cow;
use std::fmt::Write as _;

use codeidx_lsp::Location;
use codeidx_protocol::tools::{MountReport, ProjectsReport, SyncReport, UnmountReport};

fn tsv_cell(text: &str) -> Cow<'_, str> {
    if !text.contains(['\\', '\t', '\r', '\n']) {
        return Cow::Borrowed(text);
    }
    let mut escaped = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '\\' => escaped.push_str("\\\\"),
            '\t' => escaped.push_str("\\t"),
            '\r' => escaped.push_str("\\r"),
            '\n' => escaped.push_str("\\n"),
            _ => escaped.push(ch),
        }
    }
    Cow::Owned(escaped)
}

pub(crate) fn mount(report: &MountReport) -> String {
    let path = serde_json::to_string(&report.path).expect("path serializes");
    let reason = serde_json::to_string(&report.vector_degraded_reason).expect("reason serializes");
    format!(
        "# mount_project mounted={} path={path} files={} symbols={} elapsed_ms={} mem_est_mb={} \
         vector={} skipped_binary={} skipped_oversize={} skipped_unsupported={} \
         vector_degraded={} vector_degraded_reason={reason}\n",
        report.mounted,
        report.files,
        report.symbols,
        report.elapsed_ms,
        report.mem_est_mb,
        report.vector,
        report.skipped_binary,
        report.skipped_oversize,
        report.skipped_unsupported,
        report.vector_degraded,
    )
}

pub(crate) fn unmount(report: &UnmountReport) -> String {
    format!(
        "# unmount_project unmounted={} killed_lsps={}\n",
        report.unmounted, report.killed_lsps,
    )
}

pub(crate) fn projects(report: &ProjectsReport) -> String {
    let mut out = format!(
        "# list_mounted_projects projects={}\n",
        report.projects.len()
    );
    out.push_str("path\tfiles\tsymbols\tvector\tvector_degraded\tvector_degraded_reason\tlsps\n");
    for project in &report.projects {
        let lsps = serde_json::to_string(&project.lsps).expect("LSP names serialize");
        writeln!(
            out,
            "{}\t{}\t{}\t{}\t{}\t{}\t{lsps}",
            tsv_cell(&project.path),
            project.files,
            project.symbols,
            project.vector,
            project.vector_degraded,
            tsv_cell(&project.vector_degraded_reason),
        )
        .expect("writing to String is infallible");
    }
    out
}

pub(crate) fn sync(report: &SyncReport) -> String {
    let mut out = format!(
        "# sync_project added={} modified={} removed={} files={} elapsed_ms={} \
         failed={} failed_files={} failed_files_truncated={}\n",
        report.added,
        report.modified,
        report.removed,
        report.files,
        report.elapsed_ms,
        report.failed,
        report.failed_files.len(),
        report.failed_files_truncated,
    );
    if !report.failed_files.is_empty() {
        out.push_str("file\n");
        for file in &report.failed_files {
            writeln!(out, "{}", tsv_cell(file)).expect("writing to String is infallible");
        }
    }
    out
}

/// The bridge exposes start points (1-based lines, 0-based UTF-16 columns).
pub(crate) fn locations(tool_name: &str, locations: &[Location]) -> String {
    let mut out = format!("# {tool_name} locations={}\n", locations.len());
    out.push_str("file\tline\tcharacter\n");
    for location in locations {
        writeln!(
            out,
            "{}\t{}\t{}",
            tsv_cell(&location.file),
            location.line,
            location.character,
        )
        .expect("writing to String is infallible");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_sync_keeps_truncation_and_escapes_paths() {
        let report = SyncReport {
            added: 1,
            modified: 2,
            removed: 3,
            files: 4,
            elapsed_ms: 5,
            failed: 21,
            failed_files: (0..20).map(|i| format!("dir\\src\t{i}\r\n.rs")).collect(),
            failed_files_truncated: true,
        };
        let result = sync(&report);
        let lines: Vec<_> = result.lines().collect();
        assert!(lines[0].ends_with("failed=21 failed_files=20 failed_files_truncated=true"));
        assert_eq!(lines[1], "file");
        assert_eq!(lines.len(), 22);
        assert_eq!(lines[2], r"dir\\src\t0\r\n.rs");
        assert_eq!(lines[21], r"dir\\src\t19\r\n.rs");
    }

    #[test]
    fn compact_mount_reports_skips_and_degradation() {
        let report = MountReport {
            mounted: true,
            path: "/test repo".into(),
            files: 3,
            symbols: 4,
            elapsed_ms: 5,
            mem_est_mb: 0.25,
            vector: false,
            skipped_binary: 6,
            skipped_oversize: 7,
            skipped_unsupported: 8,
            vector_degraded: true,
            vector_degraded_reason: "model init failed:\nno model".into(),
        };
        assert_eq!(
            mount(&report),
            "# mount_project mounted=true path=\"/test repo\" files=3 symbols=4 elapsed_ms=5 \
             mem_est_mb=0.25 vector=false skipped_binary=6 skipped_oversize=7 skipped_unsupported=8 \
             vector_degraded=true vector_degraded_reason=\"model init failed:\\nno model\"\n",
        );
    }

    #[test]
    fn compact_locations_keep_tabs_out_of_paths() {
        let location = Location {
            file: "src/a\tb.rs".into(),
            line: 7,
            character: 0,
        };
        assert_eq!(
            locations("lsp_definition", &[location]),
            "# lsp_definition locations=1\nfile\tline\tcharacter\nsrc/a\\tb.rs\t7\t0\n",
        );
        assert_eq!(
            locations("lsp_references", &[]),
            "# lsp_references locations=0\nfile\tline\tcharacter\n",
        );
    }
}
