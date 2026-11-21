# Acceptance Log — 交付验收记录

> 每次子任务交付由 Lead 在此追加一节，格式固定：交付物 → 通用门 → 黑盒用例 → 结论（PASS / 打回原因）。

## 2026-11-21 Phase1-A workspace+protocol+bin 骨架 — teammate-a

- 交付物: 30 文件（workspace Cargo.toml、codeidx-protocol×5 模块、codeidx-ast/types.rs 冻结契约、codeidx-mcp bin×4 模块、3 个占位 crate、tests/fixtures/sample-project）
- 通用门: fmt=PASS clippy=PASS(-D warnings 0) test=41 passed/0 failed lines-gate=PASS(最大 tools.rs 512)
- 黑盒: 冒烟 15/15（initialize/tools/list 11 工具+蛇形 schema/mount stub/unmount/未挂载错误/lsp 硬文案/未知工具/-32601/垃圾行/stdout 零污染/SIGTERM exit 0/空闲 VmRSS 5.8MB < 15MB）
- 审查过程: 2 轮打回 — ①DTO camelCase 违反蛇形契约（已改蛇形+防回归单测×2）②SIGTERM 挂死（tokio stdin 常驻 spawn_blocking 线程 + Runtime Drop 无限等待；shutdown_timeout(1s) 修复，真实 SIGTERM 路径复现验证 EXIT 0）
- 结论: **PASS**（7 项申报偏离全部裁定接受；MCP 规范字段 camelCase 保留正确）

## 2026-11-21 Phase1-D codeidx-store 内存 SQLite+FTS5 — teammate-d

- 交付物: crates/codeidx-store（7 文件 990 行：schema/tokens/store/search/tests）+ Cargo 依赖
- 通用门: fmt=PASS clippy=PASS(-D warnings 0) test=62 passed/0 failed（新增 21，既有 41 无回归）；主工作区集成后复跑同样全绿
- 黑盒抽查: FTS5 探针实证（bm25 取负对齐 DTO 语义）；camelCase/snake_case 分词辅助列严格判据单测；事务幂等/并发冒烟过
- 审查过程: 1 次实例启动失败（Lead 重建隔离工作区补发任务书后正常交付）；5 项申报偏离全部裁定接受（store→protocol 依赖已同步 ARCHITECTURE §2、is_vector_enabled 访问器、stats 兜底、头剥离规则、u64 转换）
- 结论: **PASS**

## 2026-11-21 Phase1-C codeidx-ast Tree-sitter+PageRank — teammate-c

- 交付物: crates/codeidx-ast（10 文件 1577 行：lang/extract 722+clang 293/chunk/pagerank/repomap/tests；types.rs 冻结逐字节一致）
- 通用门: fmt=PASS clippy=PASS test=64 passed/0 failed（新增 23）；主工作区集成后 85 passed/0 failed
- 黑盒抽查: 8 grammar 全实证（节点类型表交付）；行号 0-based→1-based 换算复核；截断格式与验收断言逐字一致
- 集成链路冒烟（Lead 主导，/tmp/bb-crate-chain）: parse_file→Store.replace_file→search/ read_symbol（头注入-剥离链路兼容）/outline/repo_map 全通；focus 提权实测 helper_spinner 升至 top1；budget 截断生效 → CHAIN_OK
- 审查过程: 隔离快照缺失由子代理只读自救（Lead 复核无违规）；5 项申报偏离全部裁定接受（personalized PageRank 每轮基准提权数学正确、Module 符号保留、嵌套块重叠、C typedef 降级、引用粗粒度噪音）
- 结论: **PASS**

## 2026-11-21 Phase1-B codeidx-project 管理器+增量引擎+handlers 真接线 — teammate-b

- 交付物: crates/codeidx-project（walker/mount/sync/manager/tests 855 行）+ codeidx-mcp 真接线（handlers 414/main LazyLock/router）
- 通用门: fmt=PASS clippy=PASS test=98 passed/0 failed（project 9 + mcp 14 新增）；主工作区集成后同样 98/0
- 黑盒: bb_phase1 17/17（F1.1–F1.15 + stdout-clean + exit-0）；空闲 VmRSS≈7.7MB<15MB、挂载后≈16.2MB
- 审查过程: F1.7 交付时 16/17，根因定位 bm25 长度归一化短块偏置（use 兜底块 6.13 压过定义块 5.97）。Lead 裁决并亲自修复（三轮收口）：①精确标识符 boost → ②发现 use 行含函数名字面、收紧为仅 symbol==query 提升 → ③补 score 降序重排 + LIMIT 放宽 max(64,4×top_k) 防截断。最终 17/17
- 申报偏离裁定: project→protocol 依赖接受（与 store 同例）；walker require_git(false) 修复 ignore 默认不读 .gitignore 的真实缺陷（探针实证）；sync touch 不计数语义接受；mount 幂等先建后换接受
- 结论: **PASS**（Phase 1 全部收官：A/B/C/D 集成后主工作区 98 单测 + 黑盒 17/17 全绿）

## 2026-11-21 Phase3-第一步 codeidx-lsp LSP 桥 crate — teammate-f

- 交付物: crates/codeidx-lsp（7 文件 1406 行：proto 分帧/client/detect 探测/bridge 池化门面/tests 含实机 e2e）
- 通用门: fmt=PASS clippy=PASS test=109 passed/0 failed（新增 11：definition 行号转换/references 集/idle kill/未装语言）；主区集成后同样全绿
- 实机断言: rust-analyzer 1.98.1（子代理发现本机仅有 rustup shim 缺组件，已 rustup component add 修复）；definition main.rs 调用点→lib.rs:27 端到端；references {28,29,30}=3；idle 2s kill /proc 验证无孤儿；kill_for_root 返回 1；Go 未装报 not found on PATH
- 审查过程: 8 项申报偏离全部裁定接受（lsp→ast 消费 Lang 已同步 ARCHITECTURE §2、pyright langserver 形态、from_env、冷启动 warmup 重试实证必需、hover 空串、项目外目标保留、start_kill+try_wait 收割、active_lsps 二进制名）
- 结论: **PASS**（Phase 3 第二步接线待派）

## 2026-11-21 Phase3-第二步 LSP 工具接线与生命周期集成 — teammate-f

- 交付物: main.rs(+6 全局 LazyLock 桥)/server.rs(+29 async 分流+spawn_reaper+kill_all)/handlers.rs(+195 lsp_dispatch 三实现+unmount 真值 killed_lsps+list lsps)/bridge.rs(+15 kill_all)/client.rs warm 修复；scripts/bb_phase3.mjs（Lead）
- 通用门: fmt=PASS clippy=PASS test=110 passed/0 failed；主区集成后同样全绿
- 黑盒: bb_phase3 8/8（F3.0–F3.5 全含：definition 跳转/references 集/unmount kill 无孤儿/idle reaper 8s 杀净/未装语言优雅报错/stdout 纯净）；bb_phase1 回归 17/17（SIGTERM kill_all 路径 + 空闲 VmRSS 8.5MB<15MB 不回归）
- 审查过程: 会话中断一次（实例崩溃）由 Lead 唤醒恢复；0.52s×2 偶发失败根因修复——warm 信号缺陷（语法级 publishDiagnostics 误当 workspace 就绪，空 definition 被放行），修复为仅实质响应置 warm，5 轮复现循环全绿；grep 模式误报疑云澄清（crate::LSP 静态引用）
- 申报偏离裁定: a) router lsp stub 保留不可达兼容路径接受（记入待办清理）；b) spawn_reaper 在 serve() 首行（runtime 上下文要求）接受；c/d 接受
- 结论: **PASS**（Phase 3 全部完成；router lsp stub 清理记入待办）
