# sample-project（黑盒验收夹具）

codeidx-mcp 的多语言迷你验收对象（Rust / Python / TypeScript）。

## 重要约定

- **验收一律按符号名断言，不按行号**：源文件经格式化后符号所在行号会漂移。
- 所有标记符号在仓内保持唯一命名，检索 / 跳转 / outline 断言据此手工可验证。

## 符号清单

| 语言 | 文件 | 符号 |
|---|---|---|
| Rust | src/main.rs | `fn main` |
| Rust | src/lib.rs | `pub struct GoldConfig`、`GoldConfig::forge`、`pub fn unicorn_marker_fn`、`pub fn forge_config` |
| Rust | src/utils.rs | `pub fn helper_spinner`（被 src/lib.rs 引用 3 次以上，PageRank 权重应排前） |
| Python | app.py | `class VaultApp`、`VaultApp.run`（引用 `utils.helper_spinner`） |
| Python | utils.py | `def helper_spinner` |
| TypeScript | index.ts | `function dispatch`、`class RegistryStore`、`interface StoreItem` |

本目录是一个独立的合法 crate（含空的 `[workspace]` 标记），与主仓库 workspace 互不干扰。
