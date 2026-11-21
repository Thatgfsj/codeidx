use codeidx_ast::{detect_lang, parse_file, repo_map, Lang, ParsedFile, SymbolDef, SymbolKind};

const KOTLIN: &str = include_str!("../../../tests/fixtures/kotlin-csharp-project/Catalog.kt");
const SCRIPT: &str = include_str!("../../../tests/fixtures/kotlin-csharp-project/catalog.kts");
const CSHARP: &str = include_str!("../../../tests/fixtures/kotlin-csharp-project/Catalog.cs");

fn parse(path: &str, expected_lang: Lang, text: &str) -> ParsedFile {
    let lang = detect_lang(path).expect("detect fixture language");
    assert_eq!(lang, expected_lang);
    let grammar = match lang {
        Lang::Kotlin => tree_sitter_kotlin_ng::LANGUAGE,
        Lang::CSharp => tree_sitter_c_sharp::LANGUAGE,
        _ => unreachable!(),
    };
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&grammar.into()).unwrap();
    let tree = parser.parse(text, None).unwrap();
    assert!(
        !tree.root_node().has_error(),
        "{}",
        tree.root_node().to_sexp()
    );
    parse_file(lang, path, text)
}

fn definition<'a>(
    file: &'a ParsedFile,
    name: &str,
    kind: SymbolKind,
    parent: Option<&str>,
) -> &'a SymbolDef {
    file.symbols
        .iter()
        .find(|sym| sym.name == name && sym.kind == kind && sym.parent.as_deref() == parent)
        .unwrap_or_else(|| {
            panic!(
                "missing {kind:?} {name} under {parent:?}: {:?}",
                file.symbols
            )
        })
}

fn assert_reference(file: &ParsedFile, name: &str, line: u32) {
    assert_eq!(
        file.refs
            .iter()
            .filter(|r| r.name == name && r.line == line && r.rel_path == file.rel_path)
            .count(),
        1,
        "{name}:{line}: {:?}",
        file.refs
    );
}

#[test]
fn kotlin_symbols_signatures_and_references() {
    use SymbolKind::*;
    let file = parse("Catalog.kt", Lang::Kotlin, KOTLIN);
    definition(&file, "DefaultSku", Const, None);
    definition(&file, "Sku", Type, None);
    definition(&file, "Inventory", Interface, None);
    definition(&file, "KotlinCatalog", Class, None);
    definition(&file, "InventoryState", Enum, None);
    definition(&file, "CatalogFormatter", Class, None);
    definition(&file, "Companion", Class, Some("KotlinCatalog"));
    definition(&file, "create", Method, Some("Companion"));
    definition(&file, "find", Method, Some("Inventory"));
    let method = definition(&file, "find", Method, Some("KotlinCatalog"));
    assert_eq!((method.start_line, method.end_line), (13, 15));
    assert_eq!(method.signature, "override fun find(sku: Sku): String");
    definition(&file, "loadCatalog", Fn, None);
    assert_reference(&file, "decorate", 14);
    assert_reference(&file, "Sku", 13);
    assert_reference(&file, "create", 26);
}

#[test]
fn csharp_symbols_signatures_and_references() {
    use SymbolKind::*;
    let file = parse("Catalog.cs", Lang::CSharp, CSHARP);
    definition(&file, "Fixture.Catalog", Module, None);
    definition(&file, "IInventory", Interface, Some("Fixture.Catalog"));
    definition(&file, "InventoryState", Enum, Some("Fixture.Catalog"));
    definition(&file, "CatalogKey", Struct, Some("Fixture.Catalog"));
    definition(&file, "SharpCatalog", Class, Some("Fixture.Catalog"));
    definition(&file, "SharpCatalog", Method, Some("SharpCatalog"));
    definition(&file, "Label", Other, Some("SharpCatalog"));
    definition(&file, "DefaultSku", Const, Some("SharpCatalog"));
    definition(&file, "EmptySku", Const, Some("SharpCatalog"));
    let method = definition(&file, "Find", Method, Some("SharpCatalog"));
    assert_eq!((method.start_line, method.end_line), (18, 21));
    assert_eq!(method.signature, "public string Find(string sku)");
    let create = definition(&file, "Create", Method, Some("SharpCatalog"));
    assert_eq!(create.signature, "public static SharpCatalog Create()");
    assert_reference(&file, "Decorate", 20);
    assert_reference(&file, "SharpCatalog", 23);
}

#[test]
fn kotlin_csharp_fixtures_feed_structural_retrieval() {
    let files = [
        parse("Catalog.kt", Lang::Kotlin, KOTLIN),
        parse("Catalog.cs", Lang::CSharp, CSHARP),
        parse("catalog.kts", Lang::Kotlin, SCRIPT),
    ];
    let script = &files[2];
    definition(script, "scriptCatalog", SymbolKind::Fn, None);
    assert_reference(script, "loadCatalog", 1);
    assert_reference(script, "scriptCatalog", 2);
    assert_reference(script, "find", 2);
    for (file, method, signature) in [
        (&files[0], "find", "override fun find"),
        (&files[1], "Find", "public string Find"),
    ] {
        assert!(
            file.chunks.iter().any(|chunk| {
                chunk.symbol.as_deref() == Some(method)
                    && chunk.text.starts_with("//")
                    && chunk.text.contains(signature)
            }),
            "{:?}",
            file.chunks
        );
    }
    let symbols = files
        .iter()
        .flat_map(|file| file.symbols.clone())
        .collect::<Vec<_>>();
    let refs = files
        .iter()
        .flat_map(|file| file.refs.clone())
        .collect::<Vec<_>>();
    let map = repo_map(&symbols, &refs, &[], 2048);
    for row in [
        "Catalog.kt:23 method decorate",
        "Catalog.cs:28 method Decorate",
        "catalog.kts:1 fn scriptCatalog",
    ] {
        assert!(
            map.lines().any(|line| line == row),
            "{row} missing from {map}"
        );
    }
}
