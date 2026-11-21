# COMPAT — 跨平台兼容矩阵（静态审计 + 契约声明）

> 本文件由优化菜单 #6 审计产出。本机为 Linux，macOS / Windows 结论为
> 静态审计 + 契约声明，非运行验证。审计方法：全仓静态 grep（cfg 门、
> mallopt 家族、/proc、signal、VmRSS、os::unix / nix / libc、kill /
> canonicalize）+ Linux 三件套编译实测。

## 1. 支持矩阵

| 能力 | Linux | macOS | Windows |
|---|---|---|---|
| workspace 编译 | ✓ 实测 | ✓ 理论（unix 家族，无 cfg 拦截点） | ✓ 理论（信号守卫修复后无编译拦截） |
| stdio JSON-RPC 主循环（11 工具） | ✓ 实测 | ✓ 理论 | ✓ 理论 |
| 挂载 / 增量 sync / 检索 / RepoMap | ✓ 实测 | ✓ 理论 | ✓ 理论 |
| SIGINT / SIGTERM 优雅退出（F1.12） | ✓ 实测 | ✓ 理论（同为 unix 信号模型） | ✗ SIGTERM 无等价物；CTRL_C 事件降级 |
| LSP 子进程桥 + idle reaper（F3.x） | ✓ 实测 | ✓ 理论 | △ 未实测（rust-analyzer 有 Windows 版） |
| VmRSS 内存估算 mem_est_mb（F1.14） | ✓ 实测 | ✗ 无 /proc → 0.0（不设门） | ✗ 同左 |
| malloc arena 上限（RSS 抑制） | ✓ 实测 | ✗ 无 glibc → 编译剔除 | ✗ 无 glibc → 编译剔除 |
| LSP 可执行文件探测 | ✓ 实测 | ✓ 理论 | 按 PATH 和 PATHEXT 解析可启动文件 |

## 2. 逐点审计表（文件 : 行号 : 耦合点 : 分类 : 处置）

| 文件 | 行号 | 耦合点 | 分类 | 处置 |
|---|---|---|---|---|
| crates/codeidx-mcp/src/main.rs | 31-46 | mallopt extern / M_ARENA_MAX / unsafe 调用 | linux-only cfg ✓ 已守卫 | 保持：glibc 专属 RSS 优化，非 Linux 编译剔除，无需降级 |
| crates/codeidx-mcp/src/server.rs | 11, 31-32, 55-62, 73-75 | tokio::signal::unix（SIGINT/SIGTERM 流） | ✗→✓ 原未守卫裸调用，本次修复 | 加 #[cfg(unix)]（run_loop_unix 保持原行为）；Windows 走 run_loop_ctrl_c（CTRL_C 事件）；SIGTERM 无系统等价物 → 已知不支持 |
| crates/codeidx-project/src/manager.rs | 172-181 | /proc/self/status VmRSS 读取 | ✓ 已守卫 | read 失败返回 None（mac/win 无此文件） |
| crates/codeidx-project/src/mount.rs | 27-28, 146-149 | mem_est_mb 差值计算 | ✓ 已兜底 | (rss_before, vm_rss_kb()) 双 Some 才计差值，否则 0.0 |
| crates/codeidx-lsp/src/detect.rs | — | LSP 可执行文件解析（#22） | 跨平台 | Unix 检查执行位，Windows 按 PATHEXT 仅接受 `.com`、`.exe`、`.bat`、`.cmd`，缺省或空值时用该顺序。两者都直接启动 PATH 命中的绝对路径。 |
| crates/codeidx-lsp/src/proto.rs | — | LSP file URI（#22） | 跨平台 | Windows 使用 `file:///C:/...` 和 `file://server/share/...`，移除盘符和 UNC 的 verbatim 前缀后转换。回包中的项目内路径统一用 `/`，项目外路径保留绝对路径。 |
| crates/codeidx-lsp/src/client.rs | 290-305 | 子进程终止（start_kill / try_wait 轮询） | ✓ 跨平台 API | tokio 底层 unix kill(SIGKILL) / Windows TerminateProcess；无需改动 |
| crates/codeidx-lsp/src/tests.rs | 197-204 | /proc 只读（孤儿断言辅助） | ✓ 已兜底 | read_dir 失败返回空 vec；非 Linux 该断言静默跳过（已知限制，仅影响测试） |
| crates/codeidx-project/src/manager.rs + crates/codeidx-lsp/src/bridge.rs | 75 / 55 | std::fs::canonicalize 键规范化 | ✓ 跨平台 API | Windows 可返回 verbatim 盘符或 verbatim UNC；项目键由同一 API 规范化，LSP URI 转换另行去除 verbatim 前缀 |

## 3. mem_est 兜底完整性结论

- manager.rs vm_rss_kb()：read_to_string("/proc/self/status").ok()? → 非 Linux 返回 None ✓
- mount.rs：match (rss_before, vm_rss_kb()) 双 Some 才计算，否则 0.0 ✓
- 结论：任务书要求的 fallback 0.0 兜底完整，无缺口，未做修改。

## 4. Windows 已知不支持项与替代方案建议

| 项 | 现状 | 建议替代方案（本次未实现） |
|---|---|---|
| SIGTERM 优雅退出 | ✗ 不支持（无系统等价物） | CTRL_C 事件已降级；如需 service 式停用建议 Job Object + CTRL_BREAK 或 Windows Service 控制码 |
| VmRSS 内存估算 | ✗ /proc 不存在 → mem_est_mb = 0.0 | GetProcessMemoryInfo / GlobalMemoryStatusEx；F1.14 仅记录不设门，0.0 可接受 |
| malloc arena 上限 | ✗ 无 glibc → 编译剔除 | Windows 默认堆即可；如需硬限用 Job Object 内存配额 |

## 5. macOS 未实测项清单

同 unix 信号模型 + PermissionsExt 路径理论上直接可用，以下无实机验证：

- F1.12 SIGTERM 优雅退出、F1.13 空闲 RSS < 15MB 门
- malloc arena 调优不存在（无 glibc；代码已 cfg 剔除，不产生错误）
- VmRSS → None → mem_est_mb = 0.0（F1.14 记录不受影响）
- LSP 子进程生命周期（rust-analyzer macOS 版理论可用）
