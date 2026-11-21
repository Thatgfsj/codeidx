use super::*;
use crate::parse_file;
use crate::types::ParsedFile;

fn fixture(rel: &str) -> String {
    let base = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/sample-project/"
    );
    std::fs::read_to_string(format!("{base}{rel}")).expect("fixture missing")
}

fn parse_fixture(rel: &str) -> ParsedFile {
    let text = fixture(rel);
    let lang = crate::detect_lang(rel).expect("lang detect");
    parse_file(lang, rel, &text)
}

fn find_symbol<'s>(pf: &'s ParsedFile, name: &str) -> Option<&'s SymbolDef> {
    pf.symbols.iter().find(|s| s.name == name)
}

fn actual_line(text: &str, needle: &str) -> u32 {
    text.lines()
        .position(|l| l.contains(needle))
        .map(|i| i as u32 + 1)
        .expect("needle in fixture")
}

#[test]
fn rust_fixture_symbols_and_lines() {
    let pf = parse_fixture("src/lib.rs");
    let text = fixture("src/lib.rs");

    let uni = find_symbol(&pf, "unicorn_marker_fn").expect("unicorn_marker_fn");
    assert_eq!(uni.kind, SymbolKind::Fn);
    assert_eq!(uni.parent, None);
    // 行号与文件实际定义行一致（动态定位，不受格式化漂移影响）。
    assert_eq!(
        uni.start_line,
        actual_line(&text, "pub fn unicorn_marker_fn")
    );
    assert!(uni.end_line > uni.start_line);
    assert!(uni
        .signature
        .contains("fn unicorn_marker_fn(cfg: &GoldConfig)"));

    let gold = find_symbol(&pf, "GoldConfig").expect("GoldConfig");
    assert_eq!(gold.kind, SymbolKind::Struct);
    assert_eq!(gold.parent, None);

    let forge = find_symbol(&pf, "forge").expect("forge");
    assert_eq!(forge.kind, SymbolKind::Method);
    assert_eq!(forge.parent, Some("GoldConfig".to_string()));
    assert!(forge.signature.contains("pub fn forge(label: &str)"));
}

#[test]
fn rust_fixture_helper_spinner_refs_at_least_three() {
    let pf = parse_fixture("src/lib.rs");
    let count = pf
        .refs
        .iter()
        .filter(|r| r.name == "helper_spinner")
        .count();
    assert!(count >= 3, "helper_spinner refs = {count}");
    // 引用行都在符号体内（unicorn_marker_fn L27-L32 内）。
    for r in pf.refs.iter().filter(|r| r.name == "helper_spinner") {
        assert!((27..=32).contains(&r.line), "ref line {}", r.line);
    }
}

#[test]
fn python_fixture_class_and_method() {
    let pf = parse_fixture("app.py");
    let vault = find_symbol(&pf, "VaultApp").expect("VaultApp");
    assert_eq!(vault.kind, SymbolKind::Class);
    let run = find_symbol(&pf, "run").expect("run");
    assert_eq!(run.kind, SymbolKind::Method);
    assert_eq!(run.parent, Some("VaultApp".to_string()));

    let utils = parse_fixture("utils.py");
    let spin = find_symbol(&utils, "helper_spinner").expect("py helper_spinner");
    assert_eq!(spin.kind, SymbolKind::Fn);
}

#[test]
fn ts_fixture_dispatch_and_store() {
    let pf = parse_fixture("index.ts");
    let dispatch = find_symbol(&pf, "dispatch").expect("dispatch");
    assert_eq!(dispatch.kind, SymbolKind::Fn);
    assert_eq!(dispatch.parent, None);
    let store = find_symbol(&pf, "RegistryStore").expect("RegistryStore");
    assert_eq!(store.kind, SymbolKind::Class);
    // 容器自身的 parent 不指向自己。
    assert_eq!(store.parent, None);
    let add = find_symbol(&pf, "add").expect("add");
    assert_eq!(add.kind, SymbolKind::Method);
    assert_eq!(add.parent, Some("RegistryStore".to_string()));
    let item = find_symbol(&pf, "StoreItem").expect("StoreItem");
    assert_eq!(item.kind, SymbolKind::Interface);
}

#[test]
fn huge_text_does_not_panic() {
    // 1MB 重复字符：超限路径必须降级为空结果，绝不 panic。
    let text = "a".repeat(1024 * 1024);
    let pf = parse_file(Lang::Rust, "src/big.rs", &text);
    assert!(pf.symbols.is_empty());
    assert!(pf.refs.is_empty());
    // 超限文件的兜底块仍可产出（分块不依赖 AST）。
    assert!(!pf.chunks.is_empty());
}

#[test]
fn nul_text_does_not_panic() {
    let text = "fn main() { let x = \u{0}; }";
    let pf = parse_file(Lang::Rust, "src/nul.rs", text);
    assert!(pf.symbols.is_empty());
}

#[test]
fn garbage_source_does_not_panic() {
    let text = "{{{{ %%% &&& fn fn fn @@@ ))) }}}}";
    let pf = parse_file(Lang::Go, "src/garbage.go", text);
    let _ = pf;
}

#[test]
fn signature_is_single_line_and_truncated() {
    // 120+ 字符签名被截断为单行。
    let long_params: Vec<String> = (0..40).map(|i| format!("arg_{i}: u32")).collect();
    let text = format!("pub fn long_fn({}) -> u32 {{ 1 }}", long_params.join(", "));
    let pf = parse_file(Lang::Rust, "src/long.rs", &text);
    let sym = find_symbol(&pf, "long_fn").expect("long_fn");
    assert!(!sym.signature.contains('\n'));
    assert!(sym.signature.chars().count() <= 120);
}

// ---- Dart 支持（tests/fixtures/dart-project） ----

fn dart_fixture(rel: &str) -> String {
    let base = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/dart-project/"
    );
    std::fs::read_to_string(format!("{base}{rel}")).expect("dart fixture missing")
}

fn parse_dart_fixture(rel: &str) -> ParsedFile {
    let text = dart_fixture(rel);
    parse_file(Lang::Dart, rel, &text)
}

#[test]
fn dart_fixture_symbols_kinds_lines_and_parents() {
    let pf = parse_dart_fixture("lib.dart");
    let text = dart_fixture("lib.dart");

    let marker = find_symbol(&pf, "dart_unique_marker_fn").expect("marker fn");
    assert_eq!(marker.kind, SymbolKind::Fn);
    assert_eq!(marker.parent, None);
    // 行号与文件实际定义行一致（动态定位，抗格式化漂移）。
    assert_eq!(
        marker.start_line,
        actual_line(&text, "void dart_unique_marker_fn")
    );
    assert!(marker
        .signature
        .contains("void dart_unique_marker_fn(VaultApp app)"));

    let vault = find_symbol(&pf, "VaultApp").expect("VaultApp");
    assert_eq!(vault.kind, SymbolKind::Class);
    assert_eq!(vault.parent, None);
    let run = find_symbol(&pf, "run").expect("run");
    assert_eq!(run.kind, SymbolKind::Method);
    assert_eq!(run.parent, Some("VaultApp".to_string()));
    assert!(run.signature.contains("int run(int depth)"));

    // mixin → Trait；extension → Other（两项申报映射，见交付报告）。
    let spin_host = find_symbol(&pf, "Spinnable").expect("Spinnable");
    assert_eq!(spin_host.kind, SymbolKind::Trait);
    let spin = find_symbol(&pf, "spin").expect("spin");
    assert_eq!(spin.kind, SymbolKind::Method);
    assert_eq!(spin.parent, Some("Spinnable".to_string()));

    let color = find_symbol(&pf, "Color").expect("Color");
    assert_eq!(color.kind, SymbolKind::Enum);
    let ext = find_symbol(&pf, "GoldExt").expect("GoldExt");
    assert_eq!(ext.kind, SymbolKind::Other);
    let forge = find_symbol(&pf, "forge").expect("forge");
    assert_eq!(forge.kind, SymbolKind::Method);
    assert_eq!(forge.parent, Some("GoldExt".to_string()));

    let alias = find_symbol(&pf, "VaultId").expect("VaultId");
    assert_eq!(alias.kind, SymbolKind::Type);
    let konst = find_symbol(&pf, "kVaultDepth").expect("kVaultDepth");
    assert_eq!(konst.kind, SymbolKind::Const);
}

#[test]
fn dart_fixture_helper_refs_at_least_three() {
    let pf = parse_dart_fixture("lib.dart");
    let count = pf
        .refs
        .iter()
        .filter(|r| r.name == "dart_helper_fn")
        .count();
    assert!(count >= 3, "dart_helper_fn refs = {count}");
    // utils.dart 里的定义本身为顶层 Fn。
    let utils = parse_dart_fixture("utils.dart");
    let helper = find_symbol(&utils, "dart_helper_fn").expect("helper fn");
    assert_eq!(helper.kind, SymbolKind::Fn);
}

#[test]
fn dart_garbage_does_not_panic() {
    let pf = parse_file(Lang::Dart, "a.dart", "{{{{ %%% ))) }");
    let _ = pf;
}
