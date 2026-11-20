# ARCHITECTURE — codeidx-mcp 架构契约

> 本文档是所有开发任务的唯一施工契约。任何子任务开始前先通读本文；与契约冲突的实现一律打回。

## 1. 产品定位

全局按需挂载的代码索引 MCP 服务：

- 单一 Rust 原生二进制，零 Python / Node.js 运行时依赖。
- 全局轻量常驻：启动零扫描、零模型、零 LSP，空闲 RSS < 15MB。
- 显式挂载：仅 `mount_project(path)` 后才初始化该项目内存索引。
- 纯内存态：一切索引存于 SQLite `:memory:`；目标项目目录不残留任何缓存文件。
- 即时释放：`unmount_project(path)` 销毁内存库并 kill 关联 LSP 子进程；SIGINT/SIGTERM 优雅清理全部子进程后退出。

## 2. Cargo Workspace 结构与依赖方向

```
crates/
  codeidx-mcp        (bin)  入口、stdio 主循环、信号处理、工具路由
  codeidx-protocol   (lib)  JSON-RPC 2.0 + MCP 类型 + 工具 DTO/Schema
  codeidx-project    (lib)  ProjectManager(DashMap) + walker(ignore) + 增量 sync
  codeidx-ast        (lib)  tree-sitter 语言驱动、符号/引用提取、PageRank RepoMap
  codeidx-store      (lib)  SQLite :memory: + FTS5 + 向量 + RRF 混合检索
  codeidx-lsp        (lib)  LSP 子进程桥、idle timeout、语言探测
```

依赖方向（禁止反向/环状依赖）：

```
mcp(bin) → protocol, project, ast, store, lsp
project → ast, store
store   → ast        // 复用 ast 中定义的核心数据类型
ast, lsp, protocol  // 叶子 crate，无内部依赖
```

核心数据类型（`codeidx-ast/src/types.rs`，骨架阶段冻结，后续修改需 Lead 审批）：

- `SymbolDef`：kind(fn/struct/class/trait/interface/enum/method/const...)、name、file、start_line、end_line、signature、parent(所属容器符号名，可空)
- `SymbolRef`：被引用符号名、引用处 file/line
- `Chunk`：file、symbol(所属符号，可空)、start_line、end_line、content_hash(blake3)、text(已注入路径/符号上下文)

## 3. 技术栈

- tokio（rt-multi-thread + io + process + signal）、dashmap、rayon
- ignore（gitignore 感知遍历）、blake3（内容哈希）
- tree-sitter + grammars：rust / python / typescript(tsx 含 js) / go / java / c / cpp（Phase 1 至少覆盖 rust、python、typescript/javascript；go/java/c/cpp 随 C 任务进度交付，不得阻塞 Phase 1 验收）
- rusqlite（features = ["bundled", "fts5"]）、sqlite-vec（Phase 2，静态链接）、fastembed（Phase 2，懒加载全局单例）
- LSP 子进程：tokio::process::Command + stdio JSON-RPC

## 4. MCP 工具契约（对外接口，黑盒验收基准）

传输：stdio，JSON-RPC 2.0，一行一条 JSON 消息（LSP 式 Content-Length 头不使用）。
**stdout 仅允许 JSON-RPC 协议字节；一切日志/tracing 走 stderr，违者验收直接失败。**

工具执行失败（如未挂载路径）：`tools/call` 返回 `result.isError = true` + `content[0].text = "error: <原因>"`，不使用 JSON-RPC error（协议级错误如 method not found 才用 -32601）。

行号约定：对外一律 1-based line、0-based character（UTF-16 code unit，LSP 对齐）。

| 工具 | 参数 | 成功返回（紧凑 JSON，省 token） |
|---|---|---|
| mount_project | path: string, enable_vector?: bool=false | `{mounted,path,files,symbols,elapsed_ms,mem_est_mb,vector}` |
| unmount_project | path: string | `{unmounted:true,killed_lsps:N}` |
| list_mounted_projects | — | `{projects:[{path,files,symbols,lsps:[..],vector}]}` |
| sync_project | path: string | `{added,modified,removed,files,elapsed_ms}` |
| get_repo_map | path, focus_files?: string[], token_budget?: number=3000 | 纯文本骨架：每行 `<rel_path>:<line> <kind> <name>`，PageRank 权重降序，超预算截断并附尾行 `(truncated, budget=<n>)` |
| search_code | path, query: string, mode?: "hybrid"|"keyword"|"vector"=hybrid, top_k?: number=8 | `{results:[{file,symbol,start_line,end_line,score,snippet}],mode,elapsed_ms}`，snippet ≤ 3 行 |
| get_file_outline | path, file_path: string | `{file,symbols:[{kind,name,start_line,end_line,sig}]}`，无函数体 |
| read_symbol | path, file_path, symbol_name | `{file,name,start_line,end_line,code}`，完整实现体 |
| lsp_definition | path, file_path, line, character | `{locations:[{file,line,character}]}` |
| lsp_references | path, file_path, line, character | `{locations:[{file,line,character}]}` |
| lsp_hover | path, file_path, line, character | `{contents}`（Phase 3） |

## 5. 硬性工程约束

1. **stdout 纯净**：见 §4，黑盒脚本逐字节校验。
2. **单文件 ≤ 1000 行**：超限必须拆分，无例外。
3. `cargo fmt --check` + `cargo clippy --workspace --all-targets -- -D warnings` + `cargo test --workspace` 全绿才可交付。
4. 大文件保护：> 512KB 跳过 AST 解析（FTS 仍可索引截断内容）；含 \0 视为二进制完全跳过；`.git`/`node_modules`/`target`/`build`/`dist` 永不索引。
5. 单文件解析失败必须跳过并记 stderr 警告，不允许 panic 中断整库挂载。
6. 并行：blake3 哈希与 tree-sitter 解析用 rayon；MCP IO 与 LSP 子进程用 tokio；不得在 async 上下文里做 CPU 密集解析（丢给 rayon/blocking）。
7. 内存：SQLite 一律 `:memory:`；fastembed 仅在首次启用 vector 时加载（进程内单例复用）；`CODEIDX_DISABLE_VECTOR=1` 强制纯 FTS5 模式。
8. 环境变量：`CODEIDX_LSP_IDLE_SECS`(默认300)、`CODEIDX_DISABLE_VECTOR`、`RUST_LOG`。

## 6. 阶段划分

- **Phase 1**：workspace + protocol + bin 路由 + project 生命周期/增量引擎 + ast(Rust/Python/TS) RepoMap/outline/read_symbol + store FTS5 keyword 检索。工具 1–8 全部可用（lsp_* 返回明确 `error: LSP bridge not available yet`）。
- **Phase 2**：AST-aware 分块 + sqlite-vec + fastembed 懒加载 + chunk 哈希缓存 + RRF 混合排序；`search_code(mode=vector/hybrid)` 可用且可优雅降级。
- **Phase 3**：通用 LSP 客户端（JSON-RPC over stdio）+ 语言二进制探测 + idle timeout + unmount/退出清理；rust-analyzer 为验收基准。
