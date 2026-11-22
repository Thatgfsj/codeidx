//! Evidence for static call graph resolution; not a semantic type checker.

use tree_sitter::Node;

use crate::calls::{field, first_named};
use crate::{CallTarget, Lang};

pub(crate) fn classify(lang: Lang, call: Node<'_>, target: Node<'_>, text: &str) -> CallTarget {
    if lang == Lang::Rust {
        return rust_target(target, text);
    }
    // Java and PHP may expose a bare method name separately from the receiver.
    if matches!(
        call.kind(),
        "member_call_expression"
            | "nullsafe_member_call_expression"
            | "scoped_call_expression"
            | "cascade_call_expression"
    ) || (lang == Lang::Java && field(call, &["object"]).is_some())
    {
        return CallTarget::Unresolved;
    }
    if is_bare(target)
        || (matches!(
            target.kind(),
            "generic_function" | "generic_name" | "template_function"
        ) && first_named(target).is_some_and(is_bare))
    {
        CallTarget::Bare
    } else {
        CallTarget::Unresolved
    }
}

pub(crate) fn callback(lang: Lang, node: Node<'_>, text: &str) -> Option<CallTarget> {
    // Bare arguments may be unrelated variables. Accept only an explicit path,
    // and only if the store finds a callable compatible with that qualifier.
    if lang != Lang::Rust || node.kind() != "scoped_identifier" {
        return None;
    }
    let target = rust_target(node, text);
    matches!(target, CallTarget::Owner(_) | CallTarget::Module(_)).then_some(target)
}

fn rust_target(target: Node<'_>, text: &str) -> CallTarget {
    match target.kind() {
        "generic_function" => field(target, &["function"])
            .or_else(|| first_named(target))
            .map_or(CallTarget::Unresolved, |inner| rust_target(inner, text)),
        "scoped_identifier" => {
            let Some(path) = field(target, &["path"]) else {
                return CallTarget::Unresolved;
            };
            let Some(name) = source(path, text) else {
                return CallTarget::Unresolved;
            };
            if name == "Self" {
                return rust_owner(target, text).map_or(CallTarget::Unresolved, CallTarget::Owner);
            }
            if path.kind() == "bracketed_type" {
                // <Type as Trait> requires implementation identity the index lacks.
                return first_named(path)
                    .and_then(|ty| rust_type(ty, target, text))
                    .map_or(CallTarget::Unresolved, CallTarget::Owner);
            }
            if path.kind() == "generic_type" {
                return rust_type(path, target, text)
                    .map_or(CallTarget::Unresolved, CallTarget::Owner);
            }
            let parts: Vec<_> = name.split("::").map(str::trim).collect();
            if let [owner] = parts.as_slice() {
                if owner.starts_with(char::is_uppercase) && valid_ident(owner) {
                    return CallTarget::Owner((*owner).to_owned());
                }
            }
            if parts
                .iter()
                .all(|part| valid_ident(part) && !part.starts_with(char::is_uppercase))
            {
                CallTarget::Module(parts.join("::"))
            } else {
                CallTarget::Unresolved
            }
        }
        "field_expression" => {
            let Some(receiver) = field(target, &["value"]) else {
                return CallTarget::Unresolved;
            };
            if receiver.kind() == "self" {
                return rust_owner(target, text).map_or(CallTarget::Unresolved, CallTarget::Owner);
            }
            if receiver.kind() == "identifier" {
                return rust_binding(receiver, text)
                    .flatten()
                    .map_or(CallTarget::Unresolved, CallTarget::Owner);
            }
            CallTarget::Unresolved
        }
        _ if is_bare(target) && rust_binding(target, text).is_none() => CallTarget::Bare,
        _ => CallTarget::Unresolved,
    }
}

fn rust_owner(site: Node<'_>, text: &str) -> Option<String> {
    let function = ancestors(site).find(|n| n.kind() == "function_item")?;
    let host = function.parent()?.parent()?;
    if host.kind() != "impl_item" {
        return None;
    }
    simple_type(field(host, &["type"])?, text)
}

fn rust_type(ty: Node<'_>, site: Node<'_>, text: &str) -> Option<String> {
    let name = simple_type(ty, text)?;
    if name == "Self" {
        rust_owner(site, text)
    } else {
        Some(name)
    }
}

fn simple_type(ty: Node<'_>, text: &str) -> Option<String> {
    match ty.kind() {
        "type_identifier" => source(ty, text)
            .filter(|name| valid_ident(name))
            .map(str::to_owned),
        "reference_type" | "generic_type" => simple_type(field(ty, &["type"])?, text),
        _ => None,
    }
}

/// Some(None) marks an untyped binding. Never skip a nearer shadowing binding
/// to infer the type of an outer parameter.
fn rust_binding(token: Node<'_>, text: &str) -> Option<Option<String>> {
    let name = source(token, text)?;
    let mut child = token;
    for node in ancestors(token) {
        if node.kind() == "block" {
            let mut prev = child.prev_named_sibling();
            while let Some(binding) = prev {
                if binding.kind() == "let_declaration" {
                    if let Some(found) = binding_type(binding, name, token, text) {
                        return Some(found);
                    }
                }
                prev = binding.prev_named_sibling();
            }
        }
        if matches!(node.kind(), "function_item" | "closure_expression") {
            if let Some(params) = field(node, &["parameters"]) {
                let mut cursor = params.walk();
                for param in params.named_children(&mut cursor) {
                    if let Some(found) = binding_type(param, name, token, text) {
                        return Some(found);
                    }
                    if param.kind() != "parameter" && contains_ident(param, name, text) {
                        return Some(None);
                    }
                }
            }
            if node.kind() == "function_item" {
                break;
            }
        }
        if matches!(node.kind(), "match_arm" | "for_expression")
            && field(node, &["pattern"]).is_some_and(|p| contains_ident(p, name, text))
        {
            return Some(None);
        }
        if matches!(node.kind(), "if_expression" | "while_expression")
            && field(node, &["condition"])
                .is_some_and(|p| p.kind() == "let_condition" && contains_ident(p, name, text))
        {
            return Some(None);
        }
        child = node;
    }
    None
}

fn binding_type(
    binding: Node<'_>,
    name: &str,
    site: Node<'_>,
    text: &str,
) -> Option<Option<String>> {
    let pattern = field(binding, &["pattern"])?;
    if pattern.kind() == "identifier" && source(pattern, text) == Some(name) {
        Some(field(binding, &["type"]).and_then(|ty| rust_type(ty, site, text)))
    } else {
        contains_ident(pattern, name, text).then_some(None)
    }
}

fn contains_ident(root: Node<'_>, name: &str, text: &str) -> bool {
    let mut cursor = root.walk();
    loop {
        let node = cursor.node();
        if node.kind() == "identifier" && source(node, text) == Some(name) {
            return true;
        }
        if cursor.goto_first_child() {
            continue;
        }
        loop {
            if cursor.goto_next_sibling() {
                break;
            }
            if !cursor.goto_parent() {
                return false;
            }
        }
    }
}

pub(crate) fn bare_macro_token(node: Node<'_>) -> bool {
    node.prev_sibling()
        .is_none_or(|prev| !matches!(prev.kind(), "::" | "." | "!"))
}

fn ancestors(node: Node<'_>) -> impl Iterator<Item = Node<'_>> {
    std::iter::successors(node.parent(), |node| node.parent())
}

fn is_bare(node: Node<'_>) -> bool {
    matches!(node.kind(), "identifier" | "simple_identifier" | "name")
}

fn source<'a>(node: Node<'_>, text: &'a str) -> Option<&'a str> {
    text.get(node.byte_range())
}

fn valid_ident(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c == '_' || c.is_alphabetic())
        && chars.all(|c| c == '_' || c.is_alphanumeric())
}
