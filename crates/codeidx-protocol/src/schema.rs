//! 11 个工具的元数据与 JSON Schema 字符串。
//!
//! [`TOOL_SPECS`] 是 tools/list 与路由表共用的事实来源：
//! schema 字符串在此一次定义，单测保证其可解析、结构合法。
//! 属性名一律蛇形，与 ARCHITECTURE §4 契约表格及 tools.rs 的输入 DTO 逐字一致。

use crate::mcp::ToolDescriptor;
use crate::tools::tool_name;

/// 单个工具的静态规格。
#[derive(Debug, Clone)]
pub struct ToolSpec {
    /// 工具名（tools/call 的 `name`）。
    pub name: &'static str,
    /// 工具描述（面向客户端 AI）。
    pub description: &'static str,
    /// 输入 JSON Schema（紧凑字符串；tools/list 时解析为对象下发）。
    pub schema_json: &'static str,
}

impl ToolSpec {
    /// 转为 MCP 工具描述符。
    pub fn to_descriptor(&self) -> ToolDescriptor {
        let mut input_schema: serde_json::Value = serde_json::from_str(self.schema_json)
            .expect("tool schema is valid JSON (guarded by unit tests)");
        input_schema["properties"]["format"] = serde_json::json!({
            "type": "string",
            "enum": ["compact", "json", "md"],
            "default": "compact",
            "description": "默认省略此参数，仅返回 compact 紧凑正文。仅在用户明确要求 JSON 时传 json，明确要求以 Markdown 查看时传 md（用代码块展示紧凑正文）。"
        });
        ToolDescriptor {
            name: self.name.to_owned(),
            description: self.description.to_owned(),
            input_schema,
        }
    }
}

/// 全部 11 个工具的规格表（顺序即 tools/list 下发顺序）。
pub const TOOL_SPECS: [ToolSpec; 11] = [
    ToolSpec {
        name: tool_name::MOUNT_PROJECT,
        description: "挂载项目并在内存中构建索引，默认监听文件变更自动同步；其余工具仅对已挂载项目可用。",
        schema_json: r#"{"type":"object","properties":{"path":{"type":"string","description":"项目根目录的绝对路径"},"enable_vector":{"type":"boolean","description":"是否启用向量检索（懒加载 embedding 模型）","default":false},"auto_sync":{"type":"boolean","description":"是否监听文件变更并自动同步索引","default":true}},"required":["path"]}"#,
    },
    ToolSpec {
        name: tool_name::UNMOUNT_PROJECT,
        description: "卸载项目：销毁内存索引并回收关联的 LSP 子进程。",
        schema_json: r#"{"type":"object","properties":{"path":{"type":"string","description":"已挂载项目的根目录绝对路径"}},"required":["path"]}"#,
    },
    ToolSpec {
        name: tool_name::LIST_MOUNTED_PROJECTS,
        description: "列出当前已挂载的项目及其索引规模。",
        schema_json: r#"{"type":"object","properties":{},"required":[]}"#,
    },
    ToolSpec {
        name: tool_name::SYNC_PROJECT,
        description: "对已挂载项目执行增量同步，反映文件系统变更。",
        schema_json: r#"{"type":"object","properties":{"path":{"type":"string","description":"已挂载项目的根目录绝对路径"}},"required":["path"]}"#,
    },
    ToolSpec {
        name: tool_name::GET_REPO_MAP,
        description: "按 PageRank 选择符号。紧凑正文按文件分组，缩进记录行号、类型和名称。",
        schema_json: r#"{"type":"object","properties":{"path":{"type":"string","description":"已挂载项目的根目录绝对路径"},"focus_files":{"type":"array","items":{"type":"string"},"description":"聚焦文件（相对路径，其符号权重显著提升）"},"token_budget":{"type":"integer","minimum":1,"description":"输出 token 预算","default":3000}},"required":["path"]}"#,
    },
    ToolSpec {
        name: tool_name::SEARCH_CODE,
        description: "检索代码（keyword/vector/hybrid）。紧凑结果先给实际模式、降级和截断，再按 @path 分组，命中含排名、行范围、分数、符号和片段。",
        schema_json: r#"{"type":"object","properties":{"path":{"type":"string","description":"已挂载项目的根目录绝对路径"},"query":{"type":"string","description":"检索词"},"mode":{"type":"string","enum":["hybrid","keyword","vector"],"description":"检索模式","default":"hybrid"},"top_k":{"type":"integer","minimum":1,"maximum":50,"description":"返回命中数上限","default":8},"token_budget":{"type":"integer","minimum":200,"maximum":8000,"description":"结果 token 预算（超预算数据行截断并计 truncated_hits）","default":1200}},"required":["path","query"]}"#,
    },
    ToolSpec {
        name: tool_name::GET_FILE_OUTLINE,
        description: "单文件符号大纲（无函数体）。紧凑正文按行给起止行、类型、名称和签名。",
        schema_json: r#"{"type":"object","properties":{"path":{"type":"string","description":"已挂载项目的根目录绝对路径"},"file_path":{"type":"string","description":"目标文件相对路径"}},"required":["path","file_path"]}"#,
    },
    ToolSpec {
        name: tool_name::READ_SYMBOL,
        description: "返回指定符号的完整实现代码。",
        schema_json: r#"{"type":"object","properties":{"path":{"type":"string","description":"已挂载项目的根目录绝对路径"},"file_path":{"type":"string","description":"符号所在文件相对路径"},"symbol_name":{"type":"string","description":"符号名"}},"required":["path","file_path","symbol_name"]}"#,
    },
    ToolSpec {
        name: tool_name::LSP_DEFINITION,
        description: "跳转到指定位置的符号定义（Phase 3 交付 LSP 桥）。",
        schema_json: r#"{"type":"object","properties":{"path":{"type":"string","description":"已挂载项目的根目录绝对路径"},"file_path":{"type":"string","description":"目标文件相对路径"},"line":{"type":"integer","minimum":1,"description":"行号（1-based）"},"character":{"type":"integer","minimum":0,"description":"列号（0-based）"}},"required":["path","file_path","line","character"]}"#,
    },
    ToolSpec {
        name: tool_name::LSP_REFERENCES,
        description: "列出指定位置符号的全部引用（Phase 3 交付 LSP 桥）。",
        schema_json: r#"{"type":"object","properties":{"path":{"type":"string","description":"已挂载项目的根目录绝对路径"},"file_path":{"type":"string","description":"目标文件相对路径"},"line":{"type":"integer","minimum":1,"description":"行号（1-based）"},"character":{"type":"integer","minimum":0,"description":"列号（0-based）"}},"required":["path","file_path","line","character"]}"#,
    },
    ToolSpec {
        name: tool_name::LSP_HOVER,
        description: "返回指定位置的 hover 信息（Phase 3 交付 LSP 桥）。",
        schema_json: r#"{"type":"object","properties":{"path":{"type":"string","description":"已挂载项目的根目录绝对路径"},"file_path":{"type":"string","description":"目标文件相对路径"},"line":{"type":"integer","minimum":1,"description":"行号（1-based）"},"character":{"type":"integer","minimum":0,"description":"列号（0-based）"}},"required":["path","file_path","line","character"]}"#,
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn specs_cover_11_unique_contract_tools() {
        assert_eq!(TOOL_SPECS.len(), 11);
        let names: Vec<_> = TOOL_SPECS.iter().map(|spec| spec.name).collect();
        let unique: std::collections::HashSet<_> = names.iter().copied().collect();
        assert_eq!(names.len(), unique.len(), "tool names must be unique");
        for expected in [
            "mount_project",
            "unmount_project",
            "list_mounted_projects",
            "sync_project",
            "get_repo_map",
            "search_code",
            "get_file_outline",
            "read_symbol",
            "lsp_definition",
            "lsp_references",
            "lsp_hover",
        ] {
            assert!(names.contains(&expected), "missing tool: {expected}");
        }
    }

    #[test]
    fn every_schema_is_a_valid_object_schema() {
        for spec in &TOOL_SPECS {
            let value: Value = serde_json::from_str(spec.schema_json).expect(spec.name);
            assert_eq!(value["type"], "object", "{}", spec.name);
            let props = value["properties"]
                .as_object()
                .unwrap_or_else(|| panic!("{}: properties must be object", spec.name));
            for required in value["required"].as_array().expect(spec.name) {
                let required = required.as_str().expect("required names are strings");
                assert!(
                    props.contains_key(required),
                    "{}: required '{required}' missing from properties",
                    spec.name
                );
            }
        }
    }

    /// 防回归：schema 属性名与 ARCHITECTURE §4 契约表格的蛇形参数逐字一致。
    #[test]
    fn schema_properties_use_snake_case() {
        let schemas: std::collections::HashMap<_, _> = TOOL_SPECS
            .iter()
            .map(|spec| (spec.name, spec.schema_json))
            .collect();

        assert!(schemas[tool_name::MOUNT_PROJECT].contains("\"enable_vector\""));
        let mount =
            serde_json::from_str::<serde_json::Value>(schemas[tool_name::MOUNT_PROJECT]).unwrap();
        assert_eq!(mount["properties"]["auto_sync"]["default"], true);
        assert!(schemas[tool_name::GET_REPO_MAP].contains("\"focus_files\""));
        assert!(schemas[tool_name::GET_REPO_MAP].contains("\"token_budget\""));
        assert!(schemas[tool_name::SEARCH_CODE].contains("\"top_k\""));
        assert!(schemas[tool_name::SEARCH_CODE].contains("\"token_budget\""));
        assert!(schemas[tool_name::SEARCH_CODE].contains("\"maximum\":50"));
        assert!(schemas[tool_name::GET_FILE_OUTLINE].contains("\"file_path\""));
        assert!(schemas[tool_name::READ_SYMBOL].contains("\"symbol_name\""));
        assert!(schemas[tool_name::LSP_DEFINITION].contains("\"file_path\""));
        for forbidden in [
            "topK",
            "filePath",
            "symbolName",
            "tokenBudget",
            "focusFiles",
            "enableVector",
        ] {
            for spec in &TOOL_SPECS {
                assert!(
                    !spec.schema_json.contains(forbidden),
                    "{}: schema contains camelCase property {forbidden}",
                    spec.name
                );
            }
        }
    }

    #[test]
    fn descriptors_carry_name_and_schema() {
        for spec in &TOOL_SPECS {
            let descriptor = spec.to_descriptor();
            assert_eq!(descriptor.name, spec.name);
            assert_eq!(descriptor.input_schema["type"], "object");
        }
    }
}
