//! 以元素的显式 id 作为 HTML 结构符号。

use tree_sitter::Node;

use super::{collapse_ws, truncate_sig, Extractor, SymbolKind};

impl Extractor<'_> {
    pub(super) fn html_container(&self, node: Node) -> Option<String> {
        self.html_element_id(node).map(|(_, id)| id)
    }

    pub(super) fn handle_html_def(&mut self, node: Node, parent: Option<String>) {
        let Some((tag, id)) = self.html_element_id(node) else {
            return;
        };
        let signature = truncate_sig(&collapse_ws(&self.text_of(tag)));
        self.push_symbol(SymbolKind::Other, &id, node, parent, signature);
    }

    fn html_element_id<'tree>(&self, node: Node<'tree>) -> Option<(Node<'tree>, String)> {
        if !matches!(node.kind(), "element" | "script_element" | "style_element") {
            return None;
        }
        let mut cursor = node.walk();
        let tag = node
            .named_children(&mut cursor)
            .find(|child| matches!(child.kind(), "start_tag" | "self_closing_tag"))?;
        let mut cursor = tag.walk();
        let id = tag
            .named_children(&mut cursor)
            .filter(|child| child.kind() == "attribute")
            .find_map(|attr| self.html_id_value(attr))?;
        Some((tag, id))
    }

    fn html_id_value(&self, attr: Node) -> Option<String> {
        let name = attr.named_child(0)?;
        if name.kind() != "attribute_name" || !self.text_of(name).eq_ignore_ascii_case("id") {
            return None;
        }
        let value = attr.named_child(1)?;
        let value = match value.kind() {
            "attribute_value" => value,
            "quoted_attribute_value" => value.named_child(0)?,
            _ => return None,
        };
        let id = self.text_of(value);
        // HTML id 必须非空且不得包含 ASCII 空白。
        (!id.is_empty() && !id.bytes().any(|b| b.is_ascii_whitespace() || b == b'\x0c'))
            .then_some(id)
    }
}
