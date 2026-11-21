//! 符号 / 引用提取：tree-sitter 语法树 -> SymbolDef / SymbolRef。
//!
//! 设计约定：
//! - 两遍遍历：第一遍提取符号并记录定义位置（(name, line)），第二遍
//!   提取引用；引用行与同名定义行重合时丢弃，避免自环。
//! - 一切异常路径（超大文本、含 NUL、grammar 加载失败、解析返回 None）
//!   都降级为空结果，绝不 panic。
//! - 粗粒度策略：符号按各语言"定义节点类型"表识别；引用按
//!   调用 / 成员访问 / 类型标识符三类识别，跨文件同名匹配交给 PageRank。

use std::collections::HashSet;

use tree_sitter::{Node, Parser};

use crate::lang::{language_of, MAX_PARSE_BYTES};
use crate::types::{Lang, SymbolDef, SymbolKind, SymbolRef};

/// C / C++ 定义提取拆分至子模块 clang（见 extract/clang.rs）。
mod clang;

/// 提取单文件的符号与引用；任何失败返回空集，不 panic。
pub(crate) fn extract(lang: Lang, rel_path: &str, text: &str) -> (Vec<SymbolDef>, Vec<SymbolRef>) {
    if text.len() > MAX_PARSE_BYTES || text.contains('\0') {
        return (Vec::new(), Vec::new());
    }
    let mut parser = Parser::new();
    if parser.set_language(&language_of(lang)).is_err() {
        return (Vec::new(), Vec::new());
    }
    let Some(tree) = parser.parse(text, None) else {
        return (Vec::new(), Vec::new());
    };
    let mut ex = Extractor::new(lang, rel_path, text);
    ex.walk_defs(tree.root_node());
    ex.walk_refs(tree.root_node());
    ex.finish()
}

struct Extractor<'a> {
    lang: Lang,
    rel_path: &'a str,
    src: &'a str,
    symbols: Vec<SymbolDef>,
    refs: Vec<SymbolRef>,
    /// 当前嵌套容器名（class / struct / trait / impl 宿主 / interface）。
    containers: Vec<String>,
    /// 定义位置集合：(name, start_line)，用于引用自环排除。
    def_lines: HashSet<(String, u32)>,
}

impl<'a> Extractor<'a> {
    fn new(lang: Lang, rel_path: &'a str, src: &'a str) -> Self {
        Self {
            lang,
            rel_path,
            src,
            symbols: Vec::new(),
            refs: Vec::new(),
            containers: Vec::new(),
            def_lines: HashSet::new(),
        }
    }

    fn finish(self) -> (Vec<SymbolDef>, Vec<SymbolRef>) {
        (self.symbols, self.refs)
    }

    fn text_of(&self, node: Node) -> String {
        self.src
            .get(node.start_byte()..node.end_byte())
            .unwrap_or("")
            .to_string()
    }

    fn line_of(&self, node: Node) -> u32 {
        node.start_position().row as u32 + 1
    }

    fn end_line_of(&self, node: Node) -> u32 {
        node.end_position().row as u32 + 1
    }

    /// 当前容器名（栈顶），供方法 / 嵌套定义挂 parent。
    fn parent_of(&self) -> Option<String> {
        self.containers.last().cloned()
    }

    fn push_symbol(
        &mut self,
        kind: SymbolKind,
        name: &str,
        node: Node,
        parent: Option<String>,
        signature: String,
    ) {
        if name.is_empty() {
            return;
        }
        self.def_lines
            .insert((name.to_string(), self.line_of(node)));
        self.symbols.push(SymbolDef {
            kind,
            name: name.to_string(),
            rel_path: self.rel_path.to_string(),
            start_line: self.line_of(node),
            end_line: self.end_line_of(node),
            signature,
            parent,
        });
    }

    /// 第一遍：符号提取 + 容器栈维护。
    ///
    /// 符号先于容器入栈处理（容器自身的 parent 取外层栈），随后容器入栈
    /// 供其子树内的成员定义挂 parent。
    fn walk_defs(&mut self, node: Node) {
        let before = self.containers.len();
        self.handle_def(node);
        if let Some(host) = self.container_host(node) {
            self.containers.push(host);
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.walk_defs(child);
        }
        drop(cursor);
        if self.containers.len() > before {
            self.containers.pop();
        }
    }

    /// 节点是否充当"容器"（其子树内的定义要挂 parent）。
    fn container_host(&mut self, node: Node) -> Option<String> {
        match (self.lang, node.kind()) {
            (Lang::Rust, "trait_item") => self.name_text(node),
            (Lang::Rust, "impl_item") => node
                .child_by_field_name("type")
                .and_then(|t| self.plain_type_name(t)),
            (Lang::Python, "class_definition")
            | (Lang::TypeScript, "class_declaration")
            | (Lang::TypeScript, "abstract_class_declaration")
            | (Lang::JavaScript, "class_declaration")
            | (Lang::JavaScript, "abstract_class_declaration")
            | (Lang::Java, "class_declaration")
            | (Lang::Java, "interface_declaration")
            | (Lang::Java, "enum_declaration")
            | (Lang::Cpp, "class_specifier")
            | (Lang::Cpp, "struct_specifier") => self.name_text(node),
            _ => None,
        }
    }

    /// impl 宿主类型名：剥掉泛型参数与生命周期前缀。
    fn plain_type_name(&self, node: Node) -> Option<String> {
        if is_name_kind(node.kind()) {
            return Some(self.text_of(node));
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if let Some(n) = self.plain_type_name(child) {
                return Some(n);
            }
        }
        None
    }

    fn handle_def(&mut self, node: Node) {
        let kind = node.kind();
        let parent = self.parent_of();
        match self.lang {
            Lang::Rust => self.handle_rust_def(node, kind, parent),
            Lang::Python => self.handle_python_def(node, kind, parent),
            Lang::TypeScript | Lang::JavaScript => self.handle_ts_def(node, kind, parent),
            Lang::Go => self.handle_go_def(node, kind),
            Lang::Java => self.handle_java_def(node, kind, parent),
            Lang::C => self.handle_c_def(node, kind),
            Lang::Cpp => self.handle_cpp_def(node, kind, parent),
        }
    }

    fn handle_rust_def(&mut self, node: Node, kind: &str, parent: Option<String>) {
        match kind {
            "function_item" => {
                let name = self.name_text(node);
                let (k, p) = match parent.as_deref() {
                    Some(_) => (SymbolKind::Method, parent),
                    None => (SymbolKind::Fn, None),
                };
                self.push_symbol(k, &name.unwrap_or_default(), node, p, self.signature(node));
            }
            "function_signature_item" => {
                let name = self.name_text(node);
                self.push_symbol(
                    SymbolKind::Method,
                    &name.unwrap_or_default(),
                    node,
                    parent,
                    self.signature(node),
                );
            }
            "struct_item" | "union_item" => {
                let name = self.name_text(node);
                self.push_symbol(
                    SymbolKind::Struct,
                    &name.unwrap_or_default(),
                    node,
                    None,
                    self.signature(node),
                );
            }
            "enum_item" => {
                let name = self.name_text(node);
                self.push_symbol(
                    SymbolKind::Enum,
                    &name.unwrap_or_default(),
                    node,
                    None,
                    self.signature(node),
                );
            }
            "trait_item" => {
                let name = self.name_text(node);
                self.push_symbol(
                    SymbolKind::Trait,
                    &name.unwrap_or_default(),
                    node,
                    None,
                    self.signature(node),
                );
            }
            "type_item" => {
                let name = self.name_text(node);
                self.push_symbol(
                    SymbolKind::Type,
                    &name.unwrap_or_default(),
                    node,
                    None,
                    self.signature(node),
                );
            }
            "const_item" | "static_item" => {
                let name = self.name_text(node);
                self.push_symbol(
                    SymbolKind::Const,
                    &name.unwrap_or_default(),
                    node,
                    None,
                    self.signature(node),
                );
            }
            "mod_item" => {
                let name = self.name_text(node);
                self.push_symbol(
                    SymbolKind::Module,
                    &name.unwrap_or_default(),
                    node,
                    None,
                    self.signature(node),
                );
            }
            _ => {}
        }
    }

    fn handle_python_def(&mut self, node: Node, kind: &str, parent: Option<String>) {
        match kind {
            "class_definition" => {
                let name = self.name_text(node);
                self.push_symbol(
                    SymbolKind::Class,
                    &name.unwrap_or_default(),
                    node,
                    None,
                    self.signature(node),
                );
            }
            "function_definition" => {
                let name = self.name_text(node);
                let (k, p) = match parent.as_deref() {
                    Some(_) => (SymbolKind::Method, parent),
                    None => (SymbolKind::Fn, None),
                };
                self.push_symbol(k, &name.unwrap_or_default(), node, p, self.signature(node));
            }
            _ => {}
        }
    }

    fn handle_ts_def(&mut self, node: Node, kind: &str, parent: Option<String>) {
        match kind {
            "function_declaration" => {
                let name = self.name_text(node);
                self.push_symbol(
                    SymbolKind::Fn,
                    &name.unwrap_or_default(),
                    node,
                    None,
                    self.signature(node),
                );
            }
            "method_definition" => {
                let name = self.name_text(node);
                self.push_symbol(
                    SymbolKind::Method,
                    &name.unwrap_or_default(),
                    node,
                    parent,
                    self.signature(node),
                );
            }
            "class_declaration" | "abstract_class_declaration" => {
                let name = self.name_text(node);
                self.push_symbol(
                    SymbolKind::Class,
                    &name.unwrap_or_default(),
                    node,
                    parent,
                    self.signature(node),
                );
            }
            "interface_declaration" => {
                let name = self.name_text(node);
                self.push_symbol(
                    SymbolKind::Interface,
                    &name.unwrap_or_default(),
                    node,
                    None,
                    self.signature(node),
                );
            }
            "enum_declaration" => {
                let name = self.name_text(node);
                self.push_symbol(
                    SymbolKind::Enum,
                    &name.unwrap_or_default(),
                    node,
                    None,
                    self.signature(node),
                );
            }
            "type_alias_declaration" => {
                let name = self.name_text(node);
                self.push_symbol(
                    SymbolKind::Type,
                    &name.unwrap_or_default(),
                    node,
                    None,
                    self.signature(node),
                );
            }
            "lexical_declaration" | "variable_declaration" => self.handle_ts_binding(node),
            _ => {}
        }
    }

    /// TS 顶层 const/let：箭头函数 / 函数表达式初始化 -> Fn，其余 -> Const。
    fn handle_ts_binding(&mut self, node: Node) {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() != "variable_declarator" {
                continue;
            }
            let Some(name_node) = child.child_by_field_name("name") else {
                continue;
            };
            let name = self.text_of(name_node);
            let is_fn = child
                .child_by_field_name("value")
                .map(|v| {
                    matches!(
                        v.kind(),
                        "arrow_function" | "function" | "function_expression"
                    )
                })
                .unwrap_or(false);
            let k = if is_fn {
                SymbolKind::Fn
            } else {
                SymbolKind::Const
            };
            let sig = if is_fn {
                // 箭头函数 / 函数表达式：签名截到 value 起点之前。
                match child.child_by_field_name("value") {
                    Some(v) => {
                        let head = self
                            .src
                            .get(node.start_byte()..v.start_byte())
                            .unwrap_or("");
                        truncate_sig(&collapse_ws(&strip_comments(head)))
                    }
                    None => self.signature(node),
                }
            } else {
                self.signature(node)
            };
            self.push_symbol(k, &name, node, None, sig);
        }
    }

    fn handle_go_def(&mut self, node: Node, kind: &str) {
        match kind {
            "function_declaration" => {
                let name = self.name_text(node);
                self.push_symbol(
                    SymbolKind::Fn,
                    &name.unwrap_or_default(),
                    node,
                    None,
                    self.signature(node),
                );
            }
            "method_declaration" => {
                let name = self.name_text(node);
                let receiver = self.go_receiver_type(node);
                self.push_symbol(
                    SymbolKind::Method,
                    &name.unwrap_or_default(),
                    node,
                    receiver,
                    self.signature(node),
                );
            }
            "type_spec" => {
                let name = self.name_text(node);
                let k = if self.has_child_kind(node, "struct_type") {
                    SymbolKind::Struct
                } else if self.has_child_kind(node, "interface_type") {
                    SymbolKind::Interface
                } else {
                    SymbolKind::Type
                };
                self.push_symbol(
                    k,
                    &name.unwrap_or_default(),
                    node,
                    None,
                    self.signature(node),
                );
            }
            "const_spec" => {
                let name = self.name_text(node);
                self.push_symbol(
                    SymbolKind::Const,
                    &name.unwrap_or_default(),
                    node,
                    None,
                    self.signature(node),
                );
            }
            _ => {}
        }
    }

    fn go_receiver_type(&self, node: Node) -> Option<String> {
        let recv = node.child_by_field_name("receiver")?;
        let mut cursor = recv.walk();
        for child in recv.children(&mut cursor) {
            if child.kind() == "parameter_declaration" {
                if let Some(t) = child.child_by_field_name("type") {
                    return Some(self.text_of(t));
                }
            }
        }
        None
    }

    fn handle_java_def(&mut self, node: Node, kind: &str, parent: Option<String>) {
        match kind {
            "class_declaration" => {
                let name = self.name_text(node);
                self.push_symbol(
                    SymbolKind::Class,
                    &name.unwrap_or_default(),
                    node,
                    parent,
                    self.signature(node),
                );
            }
            "interface_declaration" => {
                let name = self.name_text(node);
                self.push_symbol(
                    SymbolKind::Interface,
                    &name.unwrap_or_default(),
                    node,
                    parent,
                    self.signature(node),
                );
            }
            "enum_declaration" => {
                let name = self.name_text(node);
                self.push_symbol(
                    SymbolKind::Enum,
                    &name.unwrap_or_default(),
                    node,
                    parent,
                    self.signature(node),
                );
            }
            "method_declaration" => {
                let name = self.name_text(node);
                self.push_symbol(
                    SymbolKind::Method,
                    &name.unwrap_or_default(),
                    node,
                    parent,
                    self.signature(node),
                );
            }
            _ => {}
        }
    }

    // ---- 名称与签名辅助 ----

    /// 优先取 name 字段，回退到第一个标识符类子节点。
    fn name_text(&self, node: Node) -> Option<String> {
        if let Some(n) = node.child_by_field_name("name") {
            return Some(self.text_of(n));
        }
        if let Some(n) = node.child_by_field_name("declarator") {
            return Some(self.text_of(n));
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if is_name_kind(child.kind()) {
                return Some(self.text_of(child));
            }
        }
        None
    }

    fn has_child_kind(&self, node: Node, kind: &str) -> bool {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == kind {
                return true;
            }
        }
        false
    }

    /// 归一化签名：定义头（去 body / 注释）压成单行，超 120 字符截断。
    fn signature(&self, node: Node) -> String {
        let start = node.start_byte();
        let cut = match node.child_by_field_name("body") {
            Some(body) => body.start_byte(),
            None => node.end_byte(),
        };
        let head = self.src.get(start..cut.max(start)).unwrap_or("");
        let cleaned = strip_comments(head);
        let collapsed = collapse_ws(&cleaned);
        truncate_sig(&collapsed)
    }

    // ---- 第二遍：引用提取 ----

    fn walk_refs(&mut self, node: Node) {
        self.try_ref(node);
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.walk_refs(child);
        }
    }

    fn try_ref(&mut self, node: Node) {
        let kind = node.kind();
        let name: Option<String> = match kind {
            "call_expression" | "call" | "method_invocation" => {
                let f = node
                    .child_by_field_name("function")
                    .or_else(|| node.child_by_field_name("name"));
                f.and_then(|fn_node| self.callee_base(fn_node))
            }
            "field_expression"
            | "member_expression"
            | "attribute"
            | "selector_expression"
            | "field_access" => {
                let f = node
                    .child_by_field_name("field")
                    .or_else(|| node.child_by_field_name("property"))
                    .or_else(|| node.child_by_field_name("attribute"))
                    .or_else(|| node.child_by_field_name("name"));
                f.map(|n| self.text_of(n))
            }
            "type_identifier" => Some(self.text_of(node)),
            _ => None,
        };
        let Some(name) = name else { return };
        if is_reserved(&name) {
            return;
        }
        let line = self.line_of(node);
        // 排除定义所在行自身的定义符，避免自环。
        if self.def_lines.contains(&(name.clone(), line)) {
            return;
        }
        // 同名同行相邻去重：调用与其成员访问子节点只记一次。
        if let Some(prev) = self.refs.last() {
            if prev.name == name && prev.line == line && prev.rel_path == self.rel_path {
                return;
            }
        }
        self.refs.push(SymbolRef {
            name,
            rel_path: self.rel_path.to_string(),
            line,
        });
    }

    /// 调用目标的基名：utils::helper_spinner -> helper_spinner。
    fn callee_base(&self, node: Node) -> Option<String> {
        if is_name_kind(node.kind()) {
            return Some(self.text_of(node));
        }
        match node.kind() {
            "field_expression"
            | "scoped_identifier"
            | "selector_expression"
            | "member_expression"
            | "attribute"
            | "field_access" => {
                let f = node
                    .child_by_field_name("field")
                    .or_else(|| node.child_by_field_name("property"))
                    .or_else(|| node.child_by_field_name("attribute"))
                    .or_else(|| node.child_by_field_name("name"));
                f.map(|n| self.text_of(n))
            }
            "qualified_identifier" => {
                let mut cursor = node.walk();
                let mut last = None;
                for child in node.children(&mut cursor) {
                    if is_name_kind(child.kind()) {
                        last = Some(self.text_of(child));
                    }
                }
                last
            }
            "parenthesized_expression" => {
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    if let Some(n) = self.callee_base(child) {
                        return Some(n);
                    }
                }
                None
            }
            _ => None,
        }
    }
}

/// 标识符类节点类型（名称提取目标）。
fn is_name_kind(kind: &str) -> bool {
    matches!(
        kind,
        "identifier"
            | "type_identifier"
            | "field_identifier"
            | "property_identifier"
            | "constant"
            | "namespace_identifier"
            | "package_identifier"
    )
}

/// 语言关键字 / 上下文保留字不作为引用。
fn is_reserved(name: &str) -> bool {
    matches!(name, "self" | "Self" | "super" | "this" | "crate" | "new")
}

/// 去掉头段里的行 / 块注释（按字符扫描，UTF-8 安全）。
fn strip_comments(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut iter = s.chars().peekable();
    while let Some(c) = iter.next() {
        if c == '/' {
            match iter.peek() {
                Some('/') => {
                    for n in iter.by_ref() {
                        if n == '\n' {
                            out.push('\n');
                            break;
                        }
                    }
                }
                Some('*') => {
                    iter.next();
                    let mut prev = '\0';
                    for n in iter.by_ref() {
                        if prev == '*' && n == '/' {
                            break;
                        }
                        prev = n;
                    }
                }
                _ => out.push('/'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// 空白折叠为单空格的单行文本。
fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate_sig(s: &str) -> String {
    let t = s.trim_end_matches(';').trim_end();
    if t.chars().count() > 120 {
        t.chars().take(120).collect()
    } else {
        t.to_string()
    }
}

#[cfg(test)]
mod tests;
