use crate::{CallConfidence, CallSiteKind, Store};
use codeidx_ast::{parse_file_with_calls, Lang};

fn put(store: &Store, lang: Lang, file: &str, source: &str) {
    let (file, sites) = parse_file_with_calls(lang, file, source);
    store.replace_file_with_calls(&file, &sites).unwrap();
}

#[test]
fn unchanged_callers_track_definition_insert_ambiguity_and_deletion() {
    let store = Store::new(false).unwrap();
    put(
        &store,
        Lang::Rust,
        "caller.rs",
        "fn source() { run(); run(); }",
    );
    let run_sites = || {
        store
            .call_graph()
            .unwrap()
            .sites
            .into_iter()
            .filter(|site| site.name == "run")
            .collect::<Vec<_>>()
    };
    let original = run_sites();
    assert_eq!(original.len(), 2);
    assert_ne!(original[0].character, original[1].character);
    assert!(original
        .iter()
        .all(|site| site.confidence == CallConfidence::Unresolved));
    put(&store, Lang::Python, "foreign.py", "def run(): pass");
    assert_eq!(original, run_sites());
    put(&store, Lang::Rust, "one.rs", "fn run() {}");
    assert!(run_sites()
        .iter()
        .all(|site| site.confidence == CallConfidence::Unique));
    put(&store, Lang::Rust, "two.rs", "fn run() {}");
    assert!(run_sites()
        .iter()
        .all(|site| site.confidence == CallConfidence::Ambiguous && site.candidates.len() == 2));
    put(
        &store,
        Lang::Rust,
        "one.rs",
        "fn other() {}
fn run() {}",
    );
    let graph = store.call_graph().unwrap();
    for candidate in &run_sites()[0].candidates {
        assert_eq!(
            graph
                .symbols
                .iter()
                .find(|sym| sym.id == *candidate)
                .unwrap()
                .name,
            "run"
        );
    }
    store.remove_file("two.rs").unwrap();
    assert!(run_sites()
        .iter()
        .all(|site| site.confidence == CallConfidence::Unique));
    store.remove_file("one.rs").unwrap();
    assert!(run_sites()
        .iter()
        .all(|site| site.confidence == CallConfidence::Unresolved));
}

#[test]
fn recursive_calls_callbacks_and_dynamic_targets_have_distinct_evidence() {
    let store = Store::new(false).unwrap();
    put(
        &store,
        Lang::Rust,
        "both.rs",
        "fn dispatch() { dispatch(); receive(dispatch); (choose())(); }",
    );
    let graph = store.call_graph().unwrap();
    let id = graph.symbols[0].id;
    assert!(graph
        .sites
        .iter()
        .any(|s| s.kind == CallSiteKind::Call && s.caller_id == Some(id) && s.candidates == [id]));
    assert!(graph
        .sites
        .iter()
        .any(|s| s.kind == CallSiteKind::Callback && s.candidates == [id]));
    assert!(graph
        .sites
        .iter()
        .any(|s| s.kind == CallSiteKind::Call && s.name.is_empty() && s.candidates.is_empty()));
}
