//! PHP 声明和静态可识别的引用。

use tree_sitter::Node;

use super::{Extractor, SymbolKind};

impl Extractor<'_> {
    pub(super) fn php_container(&self, node: Node) -> Option<String> {
        match node.kind() {
            "class_declaration"
            | "interface_declaration"
            | "trait_declaration"
            | "enum_declaration"
            | "namespace_definition" => self.php_declared_name(node),
            _ => None,
        }
    }

    fn php_declared_name(&self, node: Node) -> Option<String> {
        let name = node.child_by_field_name("name")?;
        (!name.is_missing()).then(|| self.text_of(name))
    }

    pub(super) fn handle_php_def(&mut self, node: Node, parent: Option<String>) {
        use SymbolKind::*;
        let (kind, name) = match node.kind() {
            "namespace_definition" => (Module, self.php_declared_name(node)),
            "class_declaration" => (Class, self.php_declared_name(node)),
            "interface_declaration" => (Interface, self.php_declared_name(node)),
            "trait_declaration" => (Trait, self.php_declared_name(node)),
            "enum_declaration" => (Enum, self.php_declared_name(node)),
            "function_definition" => (Fn, self.php_declared_name(node)),
            "method_declaration" => (Method, self.php_declared_name(node)),
            "property_element" => (Other, self.php_declared_name(node)),
            "enum_case" => (Const, self.php_declared_name(node)),
            "const_element" => {
                // const_element 的标识符是首个非注释命名子节点，没有 name 字段。
                let mut cursor = node.walk();
                let name = node
                    .named_children(&mut cursor)
                    .find(|child| !child.is_extra())
                    .and_then(|child| self.php_static_name(child));
                (Const, name)
            }
            _ => return,
        };
        if let Some(name) = name {
            self.push_symbol(kind, &name, node, parent, self.signature(node));
        }
    }

    pub(super) fn php_ref(&self, node: Node) -> Option<String> {
        let name = match node.kind() {
            "function_call_expression" => node.child_by_field_name("function"),
            "member_call_expression"
            | "nullsafe_member_call_expression"
            | "scoped_call_expression"
            | "member_access_expression"
            | "nullsafe_member_access_expression" => node.child_by_field_name("name"),
            "object_creation_expression" | "named_type" => {
                let mut cursor = node.walk();
                let first = node
                    .named_children(&mut cursor)
                    .find(|child| !child.is_extra());
                first
            }
            _ => None,
        }?;
        self.php_static_name(name)
    }

    fn php_static_name(&self, node: Node) -> Option<String> {
        if node.is_missing() {
            return None;
        }
        match node.kind() {
            "name" => Some(self.text_of(node)),
            // 匹配未限定的符号定义名，不把动态表达式当成静态名称。
            "qualified_name" | "relative_name" => {
                let mut cursor = node.walk();
                let name = node
                    .named_children(&mut cursor)
                    .filter(|n| n.kind() == "name")
                    .last()?;
                self.php_static_name(name)
            }
            _ => None,
        }
    }
}
