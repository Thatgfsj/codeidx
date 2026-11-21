//! C / C++ 定义提取：Extractor 的方法扩展。
//!
//! 从 extract.rs 拆出以控制单文件行数（交付红线 ≤ 1000 行）。

use tree_sitter::Node;

use crate::types::SymbolKind;

use super::Extractor;

impl<'a> Extractor<'a> {
    pub(super) fn handle_c_def(&mut self, node: Node, kind: &str) {
        match kind {
            "function_definition" => {
                if let Some(name) = self.c_func_name(node) {
                    self.push_symbol(SymbolKind::Fn, &name, node, None, self.signature(node));
                }
            }
            "struct_specifier" => {
                if self.has_child_kind(node, "field_declaration_list") {
                    let name = self.name_text(node);
                    self.push_symbol(
                        SymbolKind::Struct,
                        &name.unwrap_or_default(),
                        node,
                        None,
                        self.signature(node),
                    );
                }
            }
            "enum_specifier" => {
                if self.has_child_kind(node, "enumerator_list") {
                    let name = self.name_text(node);
                    self.push_symbol(
                        SymbolKind::Enum,
                        &name.unwrap_or_default(),
                        node,
                        None,
                        self.signature(node),
                    );
                }
            }
            "type_definition" => {
                // typedef struct/enum {...} 中的类型名已由 struct_specifier 提取，
                // 这里只补充非 struct/enum 的 typedef 别名，避免同节点重复符号。
                if !self.contains_kind_deep(node, &["struct_specifier", "enum_specifier"]) {
                    if let Some(name) = self.c_typedef_name(node) {
                        self.push_symbol(SymbolKind::Type, &name, node, None, self.signature(node));
                    }
                }
            }
            "declaration" if self.is_const_declaration(node) => {
                for name in self.declarator_names(node) {
                    self.push_symbol(SymbolKind::Const, &name, node, None, self.signature(node));
                }
            }
            _ => {}
        }
    }

    pub(super) fn handle_cpp_def(&mut self, node: Node, kind: &str, parent: Option<String>) {
        match kind {
            "function_definition" => {
                if let Some((name, qual_parent)) = self.cxx_func_name(node) {
                    // qualified 名（Forge::spin）或类体内内联定义都视为成员方法。
                    let p = qual_parent.or(parent);
                    let k = if p.is_some() {
                        SymbolKind::Method
                    } else {
                        SymbolKind::Fn
                    };
                    self.push_symbol(k, &name, node, p, self.signature(node));
                }
            }
            "class_specifier" => {
                if self.has_child_kind(node, "field_declaration_list") {
                    let name = self.name_text(node);
                    self.push_symbol(
                        SymbolKind::Class,
                        &name.unwrap_or_default(),
                        node,
                        None,
                        self.signature(node),
                    );
                }
            }
            "struct_specifier" => {
                if self.has_child_kind(node, "field_declaration_list") {
                    let name = self.name_text(node);
                    self.push_symbol(
                        SymbolKind::Struct,
                        &name.unwrap_or_default(),
                        node,
                        None,
                        self.signature(node),
                    );
                }
            }
            "enum_specifier" => {
                if self.has_child_kind(node, "enumerator_list") {
                    let name = self.name_text(node);
                    self.push_symbol(
                        SymbolKind::Enum,
                        &name.unwrap_or_default(),
                        node,
                        None,
                        self.signature(node),
                    );
                }
            }
            "alias_declaration" => {
                let name = self.name_text(node);
                self.push_symbol(
                    SymbolKind::Type,
                    &name.unwrap_or_default(),
                    node,
                    None,
                    self.signature(node),
                );
            }
            "declaration" if self.is_const_declaration(node) => {
                for name in self.declarator_names(node) {
                    self.push_symbol(SymbolKind::Const, &name, node, None, self.signature(node));
                }
            }
            _ => {}
        }
    }

    // ---- C / C++ 名称辅助 ----

    fn c_func_name(&self, node: Node) -> Option<String> {
        self.cxx_func_name(node).map(|(n, _)| n)
    }

    /// C/C++ 函数名：进入 function_declarator 取 identifier；
    /// qualified_identifier（Forge::spin）返回（末段, Some(首段)）。
    fn cxx_func_name(&self, node: Node) -> Option<(String, Option<String>)> {
        let decl = self.find_function_declarator(node)?;
        let mut cursor = decl.walk();
        for child in decl.children(&mut cursor) {
            match child.kind() {
                "identifier" => return Some((self.text_of(child), None)),
                "field_identifier" => return Some((self.text_of(child), None)),
                "qualified_identifier" => {
                    let mut it = Vec::new();
                    let mut c2 = child.walk();
                    for g in child.children(&mut c2) {
                        if super::is_name_kind(g.kind()) {
                            it.push(self.text_of(g));
                        }
                    }
                    let name = it.last().cloned()?;
                    let parent = if it.len() >= 2 {
                        it.first().cloned()
                    } else {
                        None
                    };
                    return Some((name, parent));
                }
                "destructor_name" | "operator_name" => return None,
                _ => {}
            }
        }
        // 指针 / 引用返回值包裹时 declarator 更深一层。
        let mut c3 = decl.walk();
        for child in decl.children(&mut c3) {
            if matches!(
                child.kind(),
                "pointer_declarator" | "reference_declarator" | "parenthesized_declarator"
            ) {
                if let Some(r) = self.cxx_declarator_name(child) {
                    return Some(r);
                }
            }
        }
        None
    }

    fn cxx_declarator_name(&self, node: Node) -> Option<(String, Option<String>)> {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            match child.kind() {
                "identifier" => return Some((self.text_of(child), None)),
                "qualified_identifier" => {
                    let mut it = Vec::new();
                    let mut c2 = child.walk();
                    for g in child.children(&mut c2) {
                        if super::is_name_kind(g.kind()) {
                            it.push(self.text_of(g));
                        }
                    }
                    return Some((it.last()?.clone(), it.first().cloned()));
                }
                "function_declarator" => return self.cxx_func_name(child),
                _ => {}
            }
        }
        None
    }

    fn find_function_declarator<'t>(&self, node: Node<'t>) -> Option<Node<'t>> {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            match child.kind() {
                "function_declarator" => return Some(child),
                "pointer_declarator" | "reference_declarator" => {
                    let mut c2 = child.walk();
                    for g in child.children(&mut c2) {
                        if g.kind() == "function_declarator" {
                            return Some(g);
                        }
                    }
                }
                _ => {}
            }
        }
        None
    }

    fn c_typedef_name(&self, node: Node) -> Option<String> {
        if let Some(d) = node.child_by_field_name("declarator") {
            return Some(self.text_of(d));
        }
        None
    }

    /// const 声明里的每个初始化声明符名字。
    fn declarator_names(&self, node: Node) -> Vec<String> {
        let mut out = Vec::new();
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if matches!(child.kind(), "init_declarator" | "declarator") {
                if let Some(n) = self.cxx_declarator_name_simple(child) {
                    out.push(n);
                }
            }
        }
        out
    }

    fn cxx_declarator_name_simple(&self, node: Node) -> Option<String> {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "identifier" {
                return Some(self.text_of(child));
            }
            if matches!(
                child.kind(),
                "pointer_declarator" | "reference_declarator" | "init_declarator"
            ) {
                if let Some(n) = self.cxx_declarator_name_simple(child) {
                    return Some(n);
                }
            }
        }
        None
    }

    /// declaration 是否带 const 限定（C/C++ 常量定义）。
    fn is_const_declaration(&self, node: Node) -> bool {
        self.has_child_kind_deep(node, "const", 2)
    }

    fn has_child_kind_deep(&self, node: Node, kind: &str, max_depth: usize) -> bool {
        if max_depth == 0 {
            return false;
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == kind {
                return true;
            }
            if self.has_child_kind_deep(child, kind, max_depth - 1) {
                return true;
            }
        }
        false
    }

    fn contains_kind_deep(&self, node: Node, kinds: &[&str]) -> bool {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if kinds.contains(&child.kind()) {
                return true;
            }
            if self.contains_kind_deep(child, kinds) {
                return true;
            }
        }
        false
    }
}
