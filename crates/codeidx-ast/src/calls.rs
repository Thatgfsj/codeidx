//! Syntax-backed calls and callback argument candidates.

use tree_sitter::Node;

use crate::Lang;

/// Why a source position participates in the graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallSiteKind {
    /// The grammar identifies an invocation.
    Call,
    /// A callable name was passed as an argument. Execution is unknown.
    Callback,
    /// Legacy coarse symbol reference with no column or call syntax.
    Reference,
}

/// A site in the current source snapshot, addressed by an LSP position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallSite {
    /// Unqualified target name, or empty for an unnamed computed callee.
    pub name: String,
    /// One-based source line.
    pub line: u32,
    /// Zero-based UTF-16 character of the target token.
    pub character: u32,
    /// Syntactic evidence for this site.
    pub kind: CallSiteKind,
}

pub(crate) fn extract_calls(lang: Lang, root: Node<'_>, text: &str) -> Vec<CallSite> {
    let mut calls = Vec::new();
    if lang == Lang::Html {
        return calls;
    }
    let mut cursor = root.walk();
    loop {
        let node = cursor.node();
        if let Some(target) = invocation(lang, node) {
            let token = name_node(target);
            calls.push(site(
                token.unwrap_or(target),
                token,
                text,
                CallSiteKind::Call,
            ));
            if let Some(args) = field(node, &["arguments"]).or_else(|| {
                let mut walk = node.walk();
                let args = node.named_children(&mut walk).find(|n| {
                    matches!(n.kind(), "arguments" | "argument_list" | "value_arguments")
                });
                args
            }) {
                let mut walk = args.walk();
                for arg in args.named_children(&mut walk) {
                    let value = if matches!(
                        arg.kind(),
                        "argument" | "value_argument" | "keyword_argument"
                    ) {
                        field(arg, &["value", "expression"]).or_else(|| last_named(arg))
                    } else {
                        Some(arg)
                    };
                    if let Some(token) = value.and_then(name_node) {
                        calls.push(site(token, Some(token), text, CallSiteKind::Callback));
                    }
                }
            }
        }
        if cursor.goto_first_child() {
            continue;
        }
        loop {
            if cursor.goto_next_sibling() {
                break;
            }
            if !cursor.goto_parent() {
                calls.sort_by_key(|s| (s.line, s.character));
                calls.dedup();
                return calls;
            }
        }
    }
}

fn field<'a>(node: Node<'a>, names: &[&str]) -> Option<Node<'a>> {
    names
        .iter()
        .find_map(|name| node.child_by_field_name(*name))
}

fn first_named(node: Node<'_>) -> Option<Node<'_>> {
    let mut cursor = node.walk();
    let found = node
        .named_children(&mut cursor)
        .find(|child| !child.is_extra());
    found
}

fn last_named(node: Node<'_>) -> Option<Node<'_>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .filter(|child| !child.is_extra())
        .last()
}

fn invocation(lang: Lang, node: Node<'_>) -> Option<Node<'_>> {
    match (lang, node.kind()) {
        (Lang::Java, "method_invocation") => field(node, &["name"]),
        (
            Lang::Php,
            "member_call_expression" | "nullsafe_member_call_expression" | "scoped_call_expression",
        ) => field(node, &["name"]),
        (Lang::Php, "function_call_expression") => field(node, &["function"]),
        (Lang::CSharp, "invocation_expression") => field(node, &["function"]),
        (Lang::Python, "call") => field(node, &["function"]),
        (Lang::Kotlin, "call_expression") => first_named(node),
        (Lang::Dart, "argument_part") => {
            let selector = node.parent().filter(|p| p.kind() == "selector")?;
            selector.prev_named_sibling()
        }
        (_, "call_expression") => field(node, &["function"]),
        _ => None,
    }
    .filter(|target| !target.is_missing())
}

/// Only descend through wrappers with a conventional, syntactically named target.
fn name_node(mut node: Node<'_>) -> Option<Node<'_>> {
    while !node.is_missing() {
        node = match node.kind() {
            "identifier"
            | "simple_identifier"
            | "property_identifier"
            | "field_identifier"
            | "type_identifier"
            | "name" => return Some(node),
            "generic_function" | "generic_name" | "template_function" | "template_method" => {
                field(node, &["function", "name"]).or_else(|| first_named(node))?
            }
            "scoped_identifier"
            | "qualified_identifier"
            | "qualified_name"
            | "alias_qualified_name"
            | "field_expression"
            | "field_access"
            | "selector_expression"
            | "member_expression"
            | "member_access_expression"
            | "member_binding_expression"
            | "attribute"
            | "navigation_expression"
            | "navigation_suffix"
            | "selector"
            | "unconditional_assignable_selector"
            | "conditional_assignable_selector" => {
                field(node, &["name", "field", "property", "attribute"])
                    .or_else(|| last_named(node))?
            }
            _ => return None,
        };
    }
    None
}

fn site(position: Node<'_>, name: Option<Node<'_>>, text: &str, kind: CallSiteKind) -> CallSite {
    let start = position.start_position();
    let byte = position.start_byte();
    let prefix = text
        .get(byte.saturating_sub(start.column)..byte)
        .unwrap_or_default();
    CallSite {
        name: name
            .and_then(|n| text.get(n.byte_range()))
            .unwrap_or_default()
            .to_owned(),
        line: start.row as u32 + 1,
        character: prefix.encode_utf16().count() as u32,
        kind,
    }
}
