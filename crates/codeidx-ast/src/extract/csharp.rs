//! C# definitions and references for the tree-sitter-c-sharp grammar.

use tree_sitter::Node;

use super::{Extractor, SymbolKind};

fn type_kind(node: Node) -> Option<SymbolKind> {
    match node.kind() {
        "class_declaration" => Some(SymbolKind::Class),
        "record_declaration" => {
            let mut cursor = node.walk();
            let is_struct = node
                .children(&mut cursor)
                .any(|child| child.kind() == "struct");
            Some(if is_struct {
                SymbolKind::Struct
            } else {
                SymbolKind::Class
            })
        }
        "struct_declaration" => Some(SymbolKind::Struct),
        "interface_declaration" => Some(SymbolKind::Interface),
        "enum_declaration" => Some(SymbolKind::Enum),
        _ => None,
    }
}

impl Extractor<'_> {
    pub(super) fn csharp_container(&self, node: Node) -> Option<String> {
        if type_kind(node).is_some() || node.kind() == "namespace_declaration" {
            self.name_text(node)
        } else {
            None
        }
    }

    pub(super) fn handle_csharp_def(&mut self, node: Node, parent: Option<String>) {
        let (kind, body) = match node.kind() {
            "namespace_declaration" | "file_scoped_namespace_declaration" => {
                (SymbolKind::Module, node.child_by_field_name("body"))
            }
            "method_declaration" | "constructor_declaration" => {
                (SymbolKind::Method, node.child_by_field_name("body"))
            }
            "local_function_statement" => (SymbolKind::Fn, node.child_by_field_name("body")),
            "property_declaration" => (
                SymbolKind::Other,
                node.child_by_field_name("accessors")
                    .or_else(|| node.child_by_field_name("value")),
            ),
            "delegate_declaration" => (SymbolKind::Type, None),
            "field_declaration" => {
                self.handle_csharp_const(node, parent);
                return;
            }
            _ => {
                let Some(kind) = type_kind(node) else { return };
                (kind, node.child_by_field_name("body"))
            }
        };
        if let Some(name) = self.name_text(node) {
            self.push_symbol(kind, &name, node, parent, self.signature_before(node, body));
        }
    }

    fn handle_csharp_const(&mut self, node: Node, parent: Option<String>) {
        let mut cursor = node.walk();
        if !node.named_children(&mut cursor).any(|child| {
            child.kind() == "modifier"
                && child.child(0).is_some_and(|token| token.kind() == "const")
        }) {
            return;
        }
        let Some(declaration) = node
            .named_children(&mut cursor)
            .find(|child| child.kind() == "variable_declaration")
        else {
            return;
        };
        let mut cursor = declaration.walk();
        for declarator in declaration.named_children(&mut cursor) {
            if declarator.kind() == "variable_declarator" {
                if let Some(name) = self.name_text(declarator) {
                    self.push_symbol(
                        SymbolKind::Const,
                        &name,
                        node,
                        parent.clone(),
                        self.signature(node),
                    );
                }
            }
        }
    }

    pub(super) fn csharp_ref(&self, node: Node) -> Option<String> {
        let target = match node.kind() {
            "invocation_expression" => {
                let target = node.child_by_field_name("function")?;
                // 成员调用由其 member_access / member_binding 子节点记录。
                if matches!(
                    target.kind(),
                    "member_access_expression" | "conditional_access_expression"
                ) {
                    return None;
                }
                target
            }
            "member_access_expression" | "member_binding_expression" => {
                node.child_by_field_name("name")?
            }
            "object_creation_expression" => node.child_by_field_name("type")?,
            "identifier" | "generic_name" | "qualified_name" | "alias_qualified_name" => {
                let parent = node.parent()?;
                if !["type", "returns"]
                    .iter()
                    .any(|field| parent.child_by_field_name(field) == Some(node))
                    || parent.kind() == "object_creation_expression"
                {
                    return None;
                }
                node
            }
            _ => return None,
        };
        self.csharp_name_base(target)
    }

    fn csharp_name_base(&self, mut node: Node) -> Option<String> {
        loop {
            node = match node.kind() {
                "identifier" => return Some(self.text_of(node)),
                "generic_name" => node.named_child(0)?,
                "qualified_name" | "alias_qualified_name" => node.child_by_field_name("name")?,
                _ => return None,
            };
        }
    }
}
