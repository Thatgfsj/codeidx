//! Kotlin definitions and references for the tree-sitter-kotlin-ng grammar.

use tree_sitter::Node;

use super::{Extractor, SymbolKind};

fn child_kind<'tree>(node: Node<'tree>, kind: &str) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    let child = node
        .children(&mut cursor)
        .find(|child| child.kind() == kind);
    child
}

impl Extractor<'_> {
    fn kotlin_container_name(&self, node: Node) -> Option<String> {
        self.name_text(node)
            .or_else(|| (node.kind() == "companion_object").then(|| "Companion".to_owned()))
    }

    pub(super) fn kotlin_container(&self, node: Node) -> Option<String> {
        match node.kind() {
            "class_declaration" | "object_declaration" | "companion_object" => {
                self.kotlin_container_name(node)
            }
            _ => None,
        }
    }

    pub(super) fn handle_kotlin_def(&mut self, node: Node, parent: Option<String>) {
        let (kind, name, body) = match node.kind() {
            "class_declaration" | "object_declaration" | "companion_object" => {
                let enum_body = child_kind(node, "enum_class_body");
                let kind = if child_kind(node, "interface").is_some() {
                    SymbolKind::Interface
                } else if enum_body.is_some()
                    || child_kind(node, "modifiers")
                        .and_then(|modifiers| child_kind(modifiers, "class_modifier"))
                        .is_some_and(|modifier| child_kind(modifier, "enum").is_some())
                {
                    SymbolKind::Enum
                } else {
                    SymbolKind::Class
                };
                (
                    kind,
                    self.kotlin_container_name(node),
                    enum_body.or_else(|| child_kind(node, "class_body")),
                )
            }
            "function_declaration" => {
                let kind = if node
                    .parent()
                    .is_some_and(|p| matches!(p.kind(), "class_body" | "enum_class_body"))
                {
                    SymbolKind::Method
                } else {
                    SymbolKind::Fn
                };
                (
                    kind,
                    self.name_text(node),
                    child_kind(node, "function_body"),
                )
            }
            "type_alias" => (
                SymbolKind::Type,
                node.child_by_field_name("type")
                    .map(|name| self.text_of(name)),
                None,
            ),
            "property_declaration" => {
                let Some(modifier) = child_kind(node, "modifiers")
                    .and_then(|modifiers| child_kind(modifiers, "property_modifier"))
                else {
                    return;
                };
                if child_kind(modifier, "const").is_none() {
                    return;
                }
                (
                    SymbolKind::Const,
                    child_kind(node, "variable_declaration").and_then(|decl| self.name_text(decl)),
                    child_kind(node, "="),
                )
            }
            _ => return,
        };
        if let Some(name) = name {
            self.push_symbol(kind, &name, node, parent, self.signature_before(node, body));
        }
    }

    pub(super) fn kotlin_ref(&self, node: Node) -> Option<String> {
        let name = match node.kind() {
            "call_expression" => {
                let mut cursor = node.walk();
                let target = node
                    .named_children(&mut cursor)
                    .find(|child| !child.is_extra())?;
                if target.kind() != "identifier" {
                    // 成员调用由其 navigation_expression 子节点记录。
                    return None;
                }
                target
            }
            "navigation_expression" | "user_type" => {
                let mut cursor = node.walk();
                let name = node
                    .named_children(&mut cursor)
                    .filter(|child| child.kind() == "identifier")
                    .last()?;
                name
            }
            _ => return None,
        };
        Some(self.text_of(name))
    }
}
