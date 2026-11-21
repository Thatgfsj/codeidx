use codeidx_ast::{detect_lang, parse_file, Lang, SymbolKind};

const PHP: &str = include_str!("../../../tests/fixtures/php-html-project/catalog.php");
const HTML: &str = include_str!("../../../tests/fixtures/php-html-project/index.html");
const HTM: &str = include_str!("../../../tests/fixtures/php-html-project/about.htm");

#[test]
fn php_extracts_declarations_and_static_references_in_a_template() {
    let path = "catalog.php";
    assert_eq!(detect_lang(path), Some(Lang::Php));
    let parsed = parse_file(detect_lang(path).unwrap(), path, PHP);
    let symbols: Vec<_> = parsed
        .symbols
        .iter()
        .map(|s| (s.name.as_str(), s.kind, s.parent.as_deref()))
        .collect();
    use SymbolKind::*;
    assert_eq!(
        symbols,
        [
            ("Demo\\Catalog", Module, None),
            ("DEFAULT_LABEL", Const, Some("Demo\\Catalog")),
            ("FALLBACK_LABEL", Const, Some("Demo\\Catalog")),
            ("Labelled", Interface, Some("Demo\\Catalog")),
            ("label", Method, Some("Labelled")),
            ("FormatsLabels", Trait, Some("Demo\\Catalog")),
            ("formatLabel", Method, Some("FormatsLabels")),
            ("Stock", Enum, Some("Demo\\Catalog")),
            ("Ready", Const, Some("Stock")),
            ("Product", Class, Some("Demo\\Catalog")),
            ("MAX_ITEMS", Const, Some("Product")),
            ("$label", Other, Some("Product")),
            ("label", Method, Some("Product")),
            ("makeProduct", Fn, Some("Demo\\Catalog")),
            ("describeProduct", Fn, Some("Demo\\Catalog")),
        ]
    );
    let product = parsed.symbols.iter().find(|s| s.name == "Product").unwrap();
    assert_eq!((product.start_line, product.end_line), (16, 23));
    assert_eq!(product.signature, "class Product implements Labelled");
    let refs: Vec<_> = parsed
        .refs
        .iter()
        .map(|r| (r.name.as_str(), r.line))
        .collect();
    for expected in [
        ("formatLabel", 21),
        ("Product", 24),
        ("Product", 25),
        ("makeProduct", 28),
        ("label", 29),
    ] {
        assert!(refs.contains(&expected), "missing {expected:?} in {refs:?}");
    }
}

#[test]
fn html_indexes_real_element_ids_and_uses_element_ranges() {
    let path = "index.html";
    assert_eq!(detect_lang(path), Some(Lang::Html));
    let parsed = parse_file(detect_lang(path).unwrap(), path, HTML);
    let symbols: Vec<_> = parsed
        .symbols
        .iter()
        .map(|s| {
            (
                s.name.as_str(),
                s.kind,
                s.parent.as_deref(),
                s.start_line,
                s.end_line,
            )
        })
        .collect();
    assert_eq!(
        symbols,
        [
            ("catalog", SymbolKind::Other, None, 2, 14),
            ("Featured", SymbolKind::Other, Some("catalog"), 4, 6),
            ("thumbnail", SymbolKind::Other, Some("catalog"), 7, 7),
            ("client-script", SymbolKind::Other, Some("catalog"), 13, 13),
        ]
    );
    assert_eq!(parsed.symbols[1].signature, "<article ID='Featured'>");
    assert!(parsed.refs.is_empty());
    let chunk = parsed
        .chunks
        .iter()
        .find(|c| c.symbol.as_deref() == Some("Featured"))
        .unwrap();
    assert!(chunk.text.contains("<p>Featured products</p>"));
}

#[test]
fn htm_alias_uses_the_html_grammar() {
    let path = "about.htm";
    assert_eq!(detect_lang(path), Some(Lang::Html));
    let parsed = parse_file(detect_lang(path).unwrap(), path, HTM);
    assert_eq!(parsed.symbols.len(), 1);
    assert_eq!(parsed.symbols[0].name, "about");
}
