# codeidx-mcp

全局按需挂载的代码索引 MCP 服务（Rust 原生二进制，零外部脚本运行时依赖）。

- 全局轻量常驻（启动 <15MB、零扫描）， 按需挂载
- 三层索引：AST 骨架（Tree-sitter + PageRank）+ 混合检索（FTS5 + sqlite-vec 向量 + RRF）+ 按需 LSP 桥
- 纯内存态（SQLite :memory:）， 即释放并终止 LSP 子进程

License: GPL-3.0。架构见 [ARCHITECTURE.md](ARCHITECTURE.md)，验收线路见 [docs/ACCEPTANCE.md](docs/ACCEPTANCE.md)。
