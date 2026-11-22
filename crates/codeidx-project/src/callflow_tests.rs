use super::*;
use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};

struct Fixture(PathBuf);
use std::path::PathBuf;

impl Fixture {
    fn create() -> Self {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "codeidx-callflow-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("main.rs"),
            "fn main() { a(); ambiguous(); }
fn a() { b(); }
fn b() { a(); }
",
        )
        .unwrap();
        fs::write(
            dir.join("first.rs"),
            "pub fn ambiguous() {}
",
        )
        .unwrap();
        fs::write(
            dir.join("second.rs"),
            "pub fn ambiguous() {}
",
        )
        .unwrap();
        Self(dir)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[tokio::test]
#[ignore = "requires a rust-analyzer installation"]
async fn rust_definition_refines_an_ambiguous_site() {
    let fixture = Fixture::create();
    let root = &fixture.0;
    fs::write(root.join("Cargo.toml"), "[package]\nname = \"trace_fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[lib]\npath = \"entry.rs\"\n").unwrap();
    fs::write(
        root.join("entry.rs"),
        "mod first;\nmod second;\npub fn caller() { first::ambiguous(); }\n",
    )
    .unwrap();
    let pm = ProjectManager::new();
    pm.mount_with_auto_sync(root, false, false).unwrap();
    let bridge = LspBridge::new(60);
    let options = TraceOptions {
        symbol: Some("caller".into()),
        ..Default::default()
    };
    let static_result = trace_calls(&pm, None, root.to_str().unwrap(), options.clone())
        .await
        .unwrap();
    assert_eq!(static_result.roots[0].children.len(), 2);
    let result = trace_calls(&pm, Some(&bridge), root.to_str().unwrap(), options)
        .await
        .unwrap();
    bridge.kill_all();
    assert_eq!(result.lsp_refined, 1, "{result:?}");
    assert_eq!(result.roots[0].children.len(), 1);
    assert_eq!(result.roots[0].children[0].location.file, "first.rs");
    assert_eq!(
        result.roots[0].children[0].confidence,
        Some(TraceConfidence::Lsp)
    );
}

#[tokio::test]
async fn directions_cycles_continuation_and_degradation() {
    let f = Fixture::create();
    let pm = ProjectManager::new();
    pm.mount_with_auto_sync(&f.0, false, false).unwrap();
    let path = f.0.to_str().unwrap();
    let report = trace_calls(&pm, None, path, TraceOptions::default())
        .await
        .unwrap();
    let main = report
        .roots
        .iter()
        .find(|n| n.location.symbol == "main")
        .unwrap();
    let a = main
        .children
        .iter()
        .find(|n| n.location.symbol == "a")
        .unwrap();
    assert_eq!(a.children[0].location.symbol, "b");
    assert!(a.children[0].children[0].cycle);
    assert!(main
        .children
        .iter()
        .any(|n| n.candidates == 2 && n.confidence == Some(TraceConfidence::Ambiguous)));
    assert!(report.lsp_degraded);
    let up = TraceOptions {
        from: Some(a.location.clone()),
        direction: TraceDirection::Both,
        max_depth: 1,
        ..Default::default()
    };
    let next = trace_calls(&pm, None, path, up).await.unwrap();
    assert!(next.roots[0]
        .children
        .iter()
        .any(|n| n.branch == Some(TraceBranch::Up) && n.location.symbol == "main"));
    assert!(next.roots[0]
        .children
        .iter()
        .any(|n| n.branch == Some(TraceBranch::Down) && n.location.symbol == "b"));
    assert!(next.truncated.depth > 0);
    let bound = TraceOptions {
        symbol: Some("main".into()),
        max_nodes: 1,
        ..Default::default()
    };
    let short = trace_calls(&pm, None, path, bound).await.unwrap();
    assert_eq!(short.nodes, 1);
    assert_eq!(short.lsp_attempted, 0);
    assert!(short.truncated.nodes > 0);
    fs::remove_file(f.0.join("second.rs")).unwrap();
    pm.sync(&f.0).unwrap();
    let repaired = trace_calls(
        &pm,
        None,
        path,
        TraceOptions {
            symbol: Some("main".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(
        repaired.roots[0]
            .children
            .iter()
            .any(|n| n.location.symbol == "ambiguous"
                && n.confidence == Some(TraceConfidence::Unique))
    );
}
