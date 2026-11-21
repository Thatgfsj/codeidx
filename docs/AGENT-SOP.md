# AGENT-SOP — codeidx-mcp 渐进式披露 SOP（供写入 Agent 系统记忆）

> 核心诉求：**让 AI 遇到复杂项目先建骨架与索引，而不是持续挂载，仅开发时挂载。**
> 本 SOP 面向把 codeidx-mcp 作为 MCP 工具源的 AI Agent；第 6 节是可直接粘贴进系统提示词的紧凑模板。

## 1. 触发条件评估

拿到代码任务先估规模，再决定是否动用索引：

| 评估 | 判定 |
|---|---|
| 源码文件 < 30，且改动局部、依赖单文件 | 直接 `rg` / 读文件解决，**不挂载** |
| 源码文件 ≥ 30，或跨模块依赖复杂（多 crate / 多包 / 需要全局引用面） | 走 codeidx 工作流 |

快速数文件：`rg --files -g '!target' -g '!node_modules' | wc -l` 或等效 glob。

小任务挂载是浪费：挂载要为整个仓库建内存索引；读三五个文件能解决的事，不值得建索引，更不值得常驻。

## 2. 标准工作流（渐进式披露）

**严禁上来全量读文件。** 严格按"骨架 → 定位 → 结构 → 实现"逐层下钻，每层只取下一步需要的信息：

1. **一次性挂载**：`mount_project(path=<项目绝对路径>)`。内存态、不落盘；幂等，同一项目会话内只挂载一次。
   - 默认 `enable_vector=false`；仅当预感需要纯语义检索（自然语言描述找代码）才显式传 `enable_vector=true`。
2. **建全局认知**：`get_repo_map(path, token_budget=3000)`。得到 PageRank 加权骨架：核心模块与高频引用符号排前。先读骨架，不读源文件正文。
   - 排查收窄到某几个文件后，传 `focus_files=[...]` 提升聚焦文件的符号权重。
3. **定位**：`search_code(path, query, mode=..., top_k=8)`。
   - 符号名 / 唯一标识 → `mode=keyword`
   - 业务语义（"处理重试的逻辑"这类）→ `mode=hybrid`（默认）
   - 检查返回的 `mode` 字段：回显值 ≠ 请求值即发生了降级（见 §5），照常使用结果。
4. **看结构**：`get_file_outline(path, file_path)`。拿到符号 kind / name / 行号 / 签名，无函数体——先确认该文件是否值得精读。
5. **精读实现**：`read_symbol(path, file_path, symbol_name)`。只读命中的符号，不读整文件。
6. **精确引用面**（按需，成本最高）：`lsp_references` / `lsp_definition`（查类型用 `lsp_hover`）。位置参数 `line` 1-based、`character` 0-based；首次调用会冷启动对应语言的 LSP 子进程。

## 3. 变更后维护

完成一轮文件修改后，索引是旧的：继续跨文件排查前先 `sync_project(path)`，增量反映 added / modified / removed，避免基于过期索引误判。频繁小改可攒一轮再 sync。

## 4. 结束纪律

- 任务完成或切换项目：`unmount_project(path)` —— 销毁内存索引、kill 关联的 LSP 子进程，把常驻内存还给系统。
- 多项目并行时只保留当前活跃项目；不确定就 `list_mounted_projects()` 清点。
- Agent 进程退出前无需手动 unmount 兜底：SIGINT / SIGTERM 时服务端优雅清理；但主动 unmount 语义更清晰、更早释放。

## 5. 检索模式选择与降级

| mode | 适用 | 语义 |
|---|---|---|
| `keyword` | 符号名、唯一标识符（函数 / 类型 / 常量名） | 纯 FTS5 BM25，零模型开销 |
| `hybrid` | 默认；业务语义、不记得确切符号名 | FTS5 + 向量 RRF 融合，两路命中都进结果 |
| `vector` | 纯语义描述（自然语言找代码） | 仅向量 KNN；要求挂载时 `enable_vector=true` |

降级语义：向量未就绪（挂载时 `enable_vector=false` / 模型加载失败 / 环境变量 `CODEIDX_DISABLE_VECTOR=1`）时，vector / hybrid 请求**自动降级 keyword 且请求仍成功**；客户端唯一可靠判据是返回的 `mode` 回显实际执行模式——请求 hybrid 回显 keyword 即降级。

## 6. 系统提示词模板（可直接粘贴）

```markdown
# codeidx 代码探索纪律（渐进式披露）

先评估再动手：
- 源码文件 < 30 且局部改动 → 直接 rg / 读文件，禁止挂载。
- ≥ 30 或跨模块复杂 → 走 codeidx 工作流，严禁上来全量读文件。

codeidx 工作流（严格按序下钻）：
1. mount_project(path=绝对路径)：一次性挂载（内存态，不落盘）；
   默认 enable_vector=false，需要纯语义检索才显式 true。
2. get_repo_map(token_budget=3000) 建全局骨架认知，不读源文件正文；
   收窄后可用 focus_files 提权。
3. search_code 定位：符号名/唯一标识 → mode=keyword；
   业务语义 → mode=hybrid（默认）；纯语义描述 → mode=vector。
   返回 mode ≠ 请求值 = 已降级 keyword，接受并继续。
4. get_file_outline 看目标文件结构（无函数体）。
5. read_symbol 只精读命中的符号实现。
6. 需要精确类型/引用面才 lsp_definition / lsp_references
   （line 1-based、character 0-based；首次调用冷启动 LSP）。

修改后与收尾：
- 完成一轮修改 → sync_project 后再继续跨文件排查。
- 任务完成或切换项目 → unmount_project（释放内存 + 终止 LSP）。
```
