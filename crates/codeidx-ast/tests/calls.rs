use codeidx_ast::{parse_file_with_calls, CallSiteKind, Lang};

#[test]
fn direct_and_member_callees_have_target_coordinates() {
    let cases = [
        (Lang::Rust, "x.rs", "fn main() { target(); item.member(); }"),
        (Lang::Python, "x.py", "target(); item.member()"),
        (Lang::TypeScript, "x.ts", "target(); item.member();"),
        (Lang::JavaScript, "x.js", "target(); item.member();"),
        (
            Lang::Go,
            "x.go",
            "package main\nfunc main() { target(); item.member() }",
        ),
        (
            Lang::Java,
            "X.java",
            "class X { void go() { target(); item.member(); } }",
        ),
        (Lang::C, "x.c", "void f() { target(); item.member(); }"),
        (Lang::Cpp, "x.cpp", "void f() { target(); item.member(); }"),
        (
            Lang::Kotlin,
            "X.kt",
            "fun main() { target(); item.member() }",
        ),
        (
            Lang::CSharp,
            "X.cs",
            "class X { void Go() { target(); item.member(); } }",
        ),
        (
            Lang::Dart,
            "x.dart",
            "void main() { target(); item.member(); }",
        ),
        (
            Lang::Php,
            "x.php",
            "<?php function f() { target(); $item->member(); }",
        ),
    ];
    for (lang, file, source) in cases {
        let (_, calls) = parse_file_with_calls(lang, file, source);
        for expected in ["target", "member"] {
            let call = calls
                .iter()
                .find(|s| s.kind == CallSiteKind::Call && s.name == expected)
                .unwrap_or_else(|| panic!("{lang:?} {expected}: {calls:?}"));
            let line = source.lines().nth(call.line as usize - 1).unwrap();
            assert_eq!(
                call.character as usize,
                line.find(expected).unwrap(),
                "{lang:?}"
            );
        }
    }
}

#[test]
fn rust_macro_candidate_and_utf16_position_are_distinct_from_real_calls() {
    let source = r#"fn f() { println!("😀", inner()); outer(); }"#;
    let (_, calls) = parse_file_with_calls(Lang::Rust, "x.rs", source);
    for (name, kind) in [
        ("inner", CallSiteKind::Reference),
        ("outer", CallSiteKind::Call),
    ] {
        let call = calls
            .iter()
            .find(|s| s.name == name && s.kind == kind)
            .unwrap_or_else(|| panic!("{name}: {calls:?}"));
        assert_eq!(
            call.character as usize,
            source[..source.find(name).unwrap()].encode_utf16().count()
        );
    }
}
