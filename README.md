# codeidx-mcp

全局按需挂载的代码索引 MCP 服务 —— 单一 Rust 原生二进制，零 Python/Node 运行时依赖。

## 核心特性

- **全局轻量常驻**：启动时零扫描、零模型、零 LSP，内存占用 < 15MB
- **按需挂载**：仅当 AI 调用 `mount_project(path)` 时才在内存中初始化索引
- **纯内存态**：AST 符号表 / 依赖图 / FTS5 / 向量索引全部在 SQLite `:memory:`，不在项目目录残留缓存
- **三层能力**：
  1. AST 骨架（Tree-sitter + PageRank RepoMap）
  2. 混合检索（FTS5 BM25 + sqlite-vec 向量 + RRF 融合，fastembed 懒加载可选）
  3. 按需 LSP 桥（rust-analyzer 等，idle timeout 自动回收）
- **即时释放**：`unmount_project` 销毁内存库并 kill 关联 LSP；SIGINT/SIGTERM 优雅清理

## 文档

- 架构契约：[ARCHITECTURE.md](ARCHITECTURE.md)
- 验收线路：[docs/ACCEPTANCE.md](docs/ACCEPTANCE.md)

## License

GPL-3.0-or-later
