# ACCEPTANCE — 本地验收线路（Lead 执行）

所有子任务交付后由 Lead 按 `.github/workflows/ci.yml` 同样的门禁黑盒验收。CI 仅在构建阶段手动触发（workflow_dispatch），不在每个 PR 运行。

## 0. 通用门（每个交付必过）

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
awk 'length > 0 && FNR > 1000 { print FILENAME": "FNR" lines"; bad=1 } END { exit bad ? 1 : 0 }' $(git ls-files '*.rs')
```

## 1. 黑盒驱动

`scripts/bb_client.mjs`（Node，仅开发期验收工具，不属于发布物）：
通过 stdio 向目标二进制发送 JSON-RPC 消息序列，逐行解析 stdout，任何 stderr 输出只入日志。
**校验器同时断言 stdout 每一行都可 JSON.parse**（stdout 纯净性硬校验）。

运行：`node scripts/bb_phase1.mjs ./target/release/codeidx-mcp`（各 phase 一个脚本，退出码非 0 即失败）。

## 2. Phase 1 验收用例（tests/fixtures/sample-project，多语言迷你仓）

| # | 用例 | 断言 |
|---|---|---|
| F1.1 | initialize → initialized | protocolVersion/tools 能力返回，`tools.listChanged=false`，无 `instructions` |
| F1.2 | tools/list | 挂载前一次返回全部 11 个工具 |
| F1.3 | mount_project(fixtures) | mounted=true, files>0, symbols>0, elapsed_ms 记录 |
| F1.4 | list_mounted_projects | 含该 path，lsps 为空数组 |
| F1.5 | get_repo_map(budget=1000) | 非空；被多次引用的核心符号排前；截断标记生效 |
| F1.6 | get_file_outline(已知文件) | 符号 kind/name/行号与 fixture 手工标注一致 |
| F1.7 | search_code(mode=keyword, 唯一函数名) | 命中该函数，start/end 行号正确 |
| F1.8 | 挂载后直接调用 read_symbol | 返回已知符号的完整多行实现 |
| F1.9 | 关闭 auto_sync，改 1 增 1 删 1 文件后 sync_project | added=1 modified=1 removed=1；后续检索反映变更 |
| F1.10 | unmount_project | list 不再含该 path |
| F1.11 | 对未挂载路径 get_repo_map | isError=true，message 含 "not mounted" |
| F1.12 | mount 后立即 SIGTERM | 退出干净，无孤儿子进程 |
| F1.13 | 空闲启动(initialize 后 sleep 2s) | VmRSS < 15MB（/proc/<pid>/status） |
| F1.14 | 挂载后 RSS | 记录峰值供对比，不设硬门（Phase 2 设门） |
| F1.15 | get_repo_map focus_files | 聚焦文件符号权重显著提升 |

## 3. Phase 2 验收用例

| # | 用例 | 断言 |
|---|---|---|
| F2.1 | mount(enable_vector=true) | 懒加载成功；失败时优雅降级且 mount 仍成功（vector=false 并 stderr 警告） |
| F2.2 | search_code(mode=vector, 自然语言描述) | 语义命中目标函数 |
| F2.3 | search_code(mode=hybrid) | RRF 排序；keyword-only 结果与 vector-only 均被融合 |
| F2.4 | chunk 哈希缓存 | sync 未变更 chunk 不重新 embed（计数不变） |
| F2.5 | CODEIDX_DISABLE_VECTOR=1 | vector/hybrid 请求降级 keyword 且明确提示 |
| F2.6 | mount(enable_vector=false) vs true | 内存差记录（模型常驻约 +N MB） |

## 4. Phase 3 验收用例（rust-analyzer 基准）

| # | 用例 | 断言 |
|---|---|---|
| F3.1 | lsp_definition(调用点) | 跳转目标文件/行正确（与 fixture 手工标注一致） |
| F3.2 | lsp_references(定义点) | 引用集合与 fixture 标注一致 |
| F3.3 | CODEIDX_LSP_IDLE_SECS=10 | 10s 后 rust-analyzer 进程被 kill |
| F3.4 | unmount_project | 关联 rust-analyzer 进程消失（无孤儿） |
| F3.5 | 未装某语言 LSP 时调用 | isError 优雅提示，不崩溃 |
| F3.6 | lsp_definition(polyglot app.py 构造调用) | 跳 models.py 类定义行（basedpyright-langserver --stdio） |
| F3.7 | lsp_references(polyglot types.go 方法) | 引用集含 main.go 调用行（gopls，需 workspaceFolders + PATH 上有 go） |

## 5. 流程规范

- 任何仓库修改先建 GitHub Issue，完成后 Lead 从 main 开分支提交 PR 合并（队友共享工作目录，git 操作仅 Lead 执行）。
- 队友只写自己 crate 目录内文件（写域分离），跨 crate 接口变更须在任务书中申报并由 Lead 评审。
- 每阶段验收全记录进 `docs/acceptance-log.md`。

## 6. CI 门禁（GitHub Actions，构建阶段触发）

`.github/workflows/ci.yml` 仅 `workflow_dispatch` 手动触发：`gate`（fmt/clippy/test/行数门，Linux）、`blackbox`（release 构建 + bb_phase1/bb_phase3 + 模型缓存 bb_phase2 + TUI selftest 占位）、`windows-smoke`（windows-latest 构建 + 测试，跨平台基线）。Windows 需 MSVC 工具链（GNU 目标 ort-sys 无预编译）且 Windows SDK ≥ 10.0.19041（DXCORE.lib）。
