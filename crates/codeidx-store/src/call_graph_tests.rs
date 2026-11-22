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
        "fn dispatch() { dispatch(); receive(dispatch, self::dispatch); (choose())(); }",
    );
    let graph = store.call_graph().unwrap();
    let id = graph.symbols[0].id;
    assert!(graph
        .sites
        .iter()
        .any(|s| s.kind == CallSiteKind::Call && s.caller_id == Some(id) && s.candidates == [id]));
    assert_eq!(
        graph
            .sites
            .iter()
            .filter(|s| s.kind == CallSiteKind::Callback && s.candidates == [id])
            .count(),
        1
    );
    assert!(graph
        .sites
        .iter()
        .any(|s| s.kind == CallSiteKind::Call && s.name.is_empty() && s.candidates.is_empty()));
}

#[test]
fn receiver_evidence_does_not_link_by_method_name_alone() {
    let store = Store::new(false).unwrap();
    put(
        &store,
        Lang::Rust,
        "src/foreign.rs",
        "struct Response;\nimpl Response { fn ok(&self) {} }\nfn ok() {}\nfn value() {}",
    );
    put(
        &store,
        Lang::Rust,
        "src/main.rs",
        r#"struct Local;
impl Local {
    fn ok(&self) {}
    fn launch(&self, item: &Local, value: i32) {
        self.ok();
        Self::ok(self);
        Local::ok(self);
        <Local>::ok(self);
        item.ok();
        { let item = something(); item.ok(); }
        result().ok();
        Result::ok(result());
        unknown.ok();
        receive(value);
        receive(Local::ok);
        let f = || {};
        f();
        ok();
    }
}
fn f() {}
"#,
    );
    let graph = store.call_graph().unwrap();
    let owner = |id| {
        graph
            .symbols
            .iter()
            .find(|d| d.id == id)
            .unwrap()
            .parent
            .as_deref()
    };
    let site = |line, name| {
        graph
            .sites
            .iter()
            .find(|s| s.line == line && s.name == name && s.rel_path == "src/main.rs")
            .unwrap()
    };
    for line in [5, 6, 7, 8, 9] {
        let call = site(line, "ok");
        assert_eq!(call.candidate_count, 1, "{line}: {call:?}");
        assert_eq!(owner(call.candidates[0]), Some("Local"));
    }
    for line in [10, 11, 12, 13] {
        let call = site(line, "ok");
        assert_eq!(call.confidence, CallConfidence::Unresolved, "{call:?}");
        assert!(call.candidates.is_empty());
        assert!(call.character.is_some());
    }
    let callback = site(15, "ok");
    assert_eq!(callback.kind, CallSiteKind::Callback);
    assert_eq!(owner(callback.candidates[0]), Some("Local"));
    assert!(!graph
        .sites
        .iter()
        .any(|s| s.kind == CallSiteKind::Callback && s.name == "value"));
    assert!(site(17, "f").candidates.is_empty());
    assert_eq!(owner(site(18, "ok").candidates[0]), None);
}

#[test]
fn local_constructor_does_not_link_to_unrelated_function() {
    let store = Store::new(false).unwrap();
    put(
        &store,
        Lang::Rust,
        "src/a.rs",
        "struct Pair(i32); fn caller() { Pair(1); Other::ok(); }",
    );
    put(
        &store,
        Lang::Rust,
        "crates/elsewhere/src/a.rs",
        "fn Pair(_: i32) {} struct Other; impl Other { fn ok() {} }",
    );
    let graph = store.call_graph().unwrap();
    for target in ["Pair", "ok"] {
        let call = graph.sites.iter().find(|site| site.name == target).unwrap();
        assert_eq!(call.confidence, CallConfidence::Unresolved);
        assert!(call.candidates.is_empty());
    }
}

#[test]
fn conventional_module_paths_filter_cross_crate_names_and_refresh() {
    let store = Store::new(false).unwrap();
    put(
        &store,
        Lang::Rust,
        "src/main.rs",
        "mod utils;\nfn caller() { utils::target(); other::target(); }",
    );
    put(
        &store,
        Lang::Rust,
        "crates/elsewhere/src/utils.rs",
        "pub fn target() {}",
    );
    put(&store, Lang::Rust, "src/elsewhere.rs", "pub fn target() {}");
    let sites = || store.call_graph().unwrap().sites;
    assert!(sites().iter().all(|s| s.candidates.is_empty()));
    put(&store, Lang::Rust, "src/utils.rs", "pub fn target() {}");
    assert_eq!(sites()[0].candidate_count, 1);
    assert!(sites()[1].candidates.is_empty());
    store.remove_file("src/utils.rs").unwrap();
    assert!(sites().iter().all(|s| s.candidates.is_empty()));
}

#[test]
fn unknown_java_and_python_members_stay_unresolved() {
    let store = Store::new(false).unwrap();
    put(
        &store,
        Lang::Python,
        "a.py",
        "def work(): pass\nthing.work()\nwork()\n",
    );
    put(
        &store,
        Lang::Java,
        "A.java",
        "class A { void work() {} void caller() { thing.work(); } }",
    );
    let graph = store.call_graph().unwrap();
    assert!(graph
        .sites
        .iter()
        .any(|s| s.rel_path == "A.java" && s.name == "work" && s.candidates.is_empty()));
    for (line, count) in [(2, 0), (3, 1)] {
        assert!(graph.sites.iter().any(|s| s.rel_path == "a.py"
            && s.line == line
            && s.name == "work"
            && s.candidate_count == count));
    }
}
