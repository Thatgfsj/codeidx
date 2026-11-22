//! 11 个工具的 handler 接线（Phase 1-B）。
//!
//! - mount / unmount / list / sync：直连全局 `ProjectManager`（main.rs 单例）；
//! - get_repo_map / search_code / get_file_outline / read_symbol：
//!   `with_project` 拿上下文后调用 ast / store 真引擎；
//! - lsp_*：LSP 桥由 Phase 3 交付，无条件返回硬文案（契约不变）。

use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::de::DeserializeOwned;
use serde_json::Value;

use codeidx_ast::{detect_lang, pagerank, Lang};
use codeidx_protocol::tools::{
    HoverReport, Location, LocationsReport, LspPositionParams, MountParams, OutlineParams,
    PathParams, ProjectsReport, ReadSymbolParams, RepoMapParams, SearchHit, SearchMode,
    SearchParams, SearchResult,
};
use codeidx_protocol::{ToolError, ToolPayload};

use crate::output::OutputFormat;
use crate::{output_navigation, output_search, output_status, MANAGER};

/// 参数反序列化：失败统一转执行错误（isError）。
fn parse_args<T: DeserializeOwned>(args: &Value) -> Result<T, ToolError> {
    serde_json::from_value(args.clone())
        .map_err(|e| ToolError::new(format!("invalid arguments: {e}")))
}

/// mount_project：真挂载（walker 遍历 + rayon 并行解析 + 全量入库）。
pub fn mount_project(args: &Value) -> Result<ToolPayload, ToolError> {
    let format = OutputFormat::parse(args)?;
    let params: MountParams = parse_args(args)?;
    let enable_vector = params.enable_vector.unwrap_or(false);
    let report = MANAGER.mount_with_auto_sync(
        Path::new(&params.path),
        enable_vector,
        params.auto_sync.unwrap_or(true),
    )?;
    Ok(format.render(&report, || output_status::mount(&report)))
}

/// unmount_project：摘除上下文（store 随之 drop，内存即释放），
/// 并 SIGKILL 该 root 关联的全部 LSP 子进程（F3.4，报告真值）。
pub fn unmount_project(args: &Value) -> Result<ToolPayload, ToolError> {
    let format = OutputFormat::parse(args)?;
    let params: PathParams = parse_args(args)?;
    let root = Path::new(&params.path);
    let mut report = MANAGER.unmount(root)?;
    report.killed_lsps = crate::LSP.kill_for_root(root) as u32;
    Ok(format.render(&report, || output_status::unmount(&report)))
}

/// list_mounted_projects：manager.list 汇总 + LSP 桥的活动服务器名
pub fn list_mounted_projects(args: &Value) -> Result<ToolPayload, ToolError> {
    let format = OutputFormat::parse(args)?;
    let mut projects = MANAGER.list();
    for info in &mut projects {
        info.lsps = crate::LSP.active_lsps(Path::new(&info.path));
    }
    let report = ProjectsReport { projects };
    Ok(format.render(&report, || output_status::projects(&report)))
}

/// sync_project：mtime/size 快筛 + blake3 复核的增量重建。
pub fn sync_project(args: &Value) -> Result<ToolPayload, ToolError> {
    let format = OutputFormat::parse(args)?;
    let params: PathParams = parse_args(args)?;
    let report = MANAGER.sync(Path::new(&params.path))?;
    Ok(format.render(&report, || output_status::sync(&report)))
}

/// get_repo_map：parsed 全集 symbols+refs → ast::repo_map 纯文本骨架。
/// focus_files / token_budget 缺省：空聚焦、预算 3000。
pub fn get_repo_map(args: &Value) -> Result<ToolPayload, ToolError> {
    let format = OutputFormat::parse(args)?;
    let params: RepoMapParams = parse_args(args)?;
    MANAGER.with_project(Path::new(&params.path), |ctx| {
        let mut symbols = Vec::new();
        let mut refs = Vec::new();
        for pf in ctx.parsed.values() {
            symbols.extend(pf.symbols.iter().cloned());
            refs.extend(pf.refs.iter().cloned());
        }
        let focus = params.focus_files.clone().unwrap_or_default();
        let budget = params.token_budget.unwrap_or(3000) as usize;
        let ranked = pagerank(&symbols, &refs, &focus);
        let (text, json) = output_navigation::compact_repo_map(&ranked, &focus, budget);
        Ok(format.render(&json, || text))
    })
}

/// search_code：mode 分流。keyword → FTS5；vector → 向量；hybrid → RRF。
/// top_k 缺省 8。**降级可见性（F2.5）**：vector 未就绪（未启用/
/// CODEIDX_DISABLE_VECTOR/模型加载失败）时一律降级 keyword，且响应
/// mode 字段回显实际执行模式——客户端据 mode 即可感知降级。
/// top_k 硬 clamp（产物语义硬化：上下文防爆）：缺省 8、上限 50；
/// 传 >50 取 50；显式传 0 视为空结果请求照常下发。
fn clamp_top_k(raw: Option<u32>) -> usize {
    raw.unwrap_or(8).min(50) as usize
}

/// token_budget 硬 clamp（紧凑化第三闸）：缺省 1200、区间 200..8000。
fn clamp_token_budget(raw: Option<u32>) -> usize {
    raw.unwrap_or(1200).clamp(200, 8000) as usize
}

/// search 命中行文本（产物紧凑化）：首行元信息
/// `# search hits=<shown> truncated_hits=<k> tokens≈<t>/budget=<b>`，
/// 次行列名，数据行 `file<TAB>symbol<TAB>start<TAB>end<TAB>score<TAB>snippet`。
/// - score 一律四位小数（排序语义保留）；
/// - snippet 列内换行转义为字面 \n 两字符、tab 替换空格（保行结构）；
/// - symbol 缺省 "-" 占位（列不缺位）；
/// - 逐数据行累计 (len+1)/4，超出 token_budget 即停止追加并
///   truncated_hits 计数（元信息行与列名行不计入预算）。
fn hits_to_lines(hits: &[SearchHit], token_budget: usize) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut chars = 0usize;
    let mut truncated_hits = 0usize;
    for hit in hits {
        let symbol = hit.symbol.as_deref().unwrap_or("-");
        let snippet = hit.snippet.replace('\n', "\\n").replace('\t', " ");
        let line = format!(
            "{}\t{}\t{}\t{}\t{:.4}\t{}",
            hit.file, symbol, hit.start_line, hit.end_line, hit.score, snippet
        );
        let cost = (chars + line.len() + 1).div_ceil(4);
        if cost > token_budget {
            truncated_hits = hits.len() - lines.len();
            break;
        }
        chars += line.len() + 1;
        lines.push(line);
    }
    let tokens = chars.div_ceil(4);
    let mut out = format!(
        "# search hits={} truncated_hits={} tokens≈{}/budget={}\n",
        lines.len(),
        truncated_hits,
        tokens,
        token_budget
    );
    out.push_str("file\tsymbol\tstart\tend\tscore\tsnippet\n");
    for line in &lines {
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// 降级原因（产物语义硬化）：mount 时已降级 → 持久化原因
/// （CODEIDX_DISABLE_VECTOR=1 / model init failed: ...）；用户本就
/// 未请求向量 → 固定文案。keyword 模式不调用本函数（无降级）。
fn degrade_reason_of(ctx: &codeidx_project::ProjectContext) -> String {
    if ctx.vector_degraded {
        ctx.vector_degraded_reason.clone()
    } else {
        "vector not enabled for this mount".to_string()
    }
}

pub fn search_code(args: &Value) -> Result<ToolPayload, ToolError> {
    let format = OutputFormat::parse(args)?;
    let params: SearchParams = parse_args(args)?;
    let mode = params.mode.unwrap_or(SearchMode::Hybrid);
    let top_k = clamp_top_k(params.top_k);
    let token_budget = clamp_token_budget(params.token_budget);
    MANAGER.with_project(Path::new(&params.path), |ctx| {
        let started = Instant::now();
        let effective = if ctx.store.is_vector_ready() {
            mode
        } else {
            tracing::warn!(
                requested = ?mode,
                "vector not ready; degrading to keyword search"
            );
            SearchMode::Keyword
        };
        let degraded = effective != mode;
        let degraded_reason = if degraded {
            degrade_reason_of(ctx)
        } else {
            String::new()
        };
        let results = match effective {
            SearchMode::Keyword => ctx.store.search_keyword(&params.query, top_k),
            SearchMode::Vector => ctx.store.search_vector(&params.query, top_k),
            SearchMode::Hybrid => ctx.store.search_hybrid(&params.query, top_k),
        }?;
        // 候选块总数：parsed 缓存与 store 同步维护（mount/sync 一致性契约）。
        let total_candidates: u64 = ctx.parsed.values().map(|pf| pf.chunks.len() as u64).sum();
        // 紧凑行文本（产物紧凑化）：top_k 条数闸之后再过 token_budget 总量闸。
        let elapsed_ms = started.elapsed().as_millis() as u64;
        if format != OutputFormat::Json {
            return Ok(format.text(output_search::render_search(
                &results,
                token_budget,
                output_search::SearchMeta {
                    mode: effective,
                    elapsed_ms,
                    degraded,
                    degraded_reason: &degraded_reason,
                    total_candidates,
                },
            )));
        }
        let report = SearchResult {
            results: hits_to_lines(&results, token_budget),
            mode: effective,
            elapsed_ms,
            degraded,
            degraded_reason,
            total_candidates,
        };
        Ok(format.render(&report, String::new))
    })
}

/// get_file_outline：store.file_outline → 紧凑行文本（产物紧凑化）；
/// 文件未索引报执行错误（文案不变）。
pub fn get_file_outline(args: &Value) -> Result<ToolPayload, ToolError> {
    let format = OutputFormat::parse(args)?;
    let params: OutlineParams = parse_args(args)?;
    MANAGER.with_project(Path::new(&params.path), |ctx| {
        if !ctx.files.contains_key(&params.file_path) {
            return Err(ToolError::new(format!(
                "file not indexed: {}",
                params.file_path
            )));
        }
        let symbols = ctx.store.file_outline(&params.file_path)?;
        let (text, json) = output_navigation::compact_outline(&params.file_path, &symbols);
        Ok(format.render(&json, || text))
    })
}

/// read_symbol：store.read_symbol 完整实现体；未命中报执行错误。
pub fn read_symbol(args: &Value) -> Result<ToolPayload, ToolError> {
    let format = OutputFormat::parse(args)?;
    let params: ReadSymbolParams = parse_args(args)?;
    MANAGER.with_project(Path::new(&params.path), |ctx| {
        match ctx
            .store
            .read_symbol(&params.file_path, &params.symbol_name)?
        {
            Some(code) => Ok(format.render(&code, || {
                format!(
                    "{}:{}-{}\n{}",
                    code.file.escape_debug(),
                    code.start_line,
                    code.end_line,
                    code.code
                )
            })),
            None => Err(ToolError::new(format!(
                "symbol not found: {}",
                params.symbol_name
            ))),
        }
    })
}

// ------------------------------------------------------------------ LSP --
// Phase 3 接线：lsp_* 是 async 语义请求（LSP 子进程往返，冷启动秒级），
// server.rs 在 tools/call 层按工具名分流到 lsp_dispatch（本模块不提供
// 同步兜底——router 同步表已移除 lsp 三项）。

/// lsp_* 三工具的 async 分发入口（server 层按名分流到此）。
pub async fn lsp_dispatch(name: &str, args: &Value) -> Result<ToolPayload, ToolError> {
    use codeidx_protocol::tools::tool_name;
    match name {
        tool_name::LSP_DEFINITION => lsp_definition_async(args).await,
        tool_name::LSP_REFERENCES => lsp_references_async(args).await,
        tool_name::LSP_HOVER => lsp_hover_async(args).await,
        other => Err(ToolError::new(format!("unknown lsp tool: {other}"))),
    }
}

/// lsp_* 公共前置：挂载校验 → 语言探测 → 磁盘读全文。
///
/// 未挂载 → project not mounted: <path>（与其它工具同文案）；
/// 扩展名不受支持 → unsupported language for LSP: <file_path>；
/// 读盘失败 → cannot read file: <file_path>: <原因>。
async fn lsp_prepare(params: &LspPositionParams) -> Result<(PathBuf, Lang, String), ToolError> {
    let root = Path::new(&params.path);
    // 只做挂载校验，不消费索引内容（语义查询以磁盘文件 + LSP 分析为准）。
    MANAGER.with_project(root, |_| Ok(()))?;
    let Some(lang) = detect_lang(&params.file_path) else {
        return Err(ToolError::new(format!(
            "unsupported language for LSP: {}",
            params.file_path
        )));
    };
    let text = std::fs::read_to_string(root.join(&params.file_path))
        .map_err(|e| ToolError::new(format!("cannot read file {}: {e}", params.file_path)))?;
    Ok((root.to_path_buf(), lang, text))
}

/// lsp_definition：Location 列表（1-based line、0-based character）。
async fn lsp_definition_async(args: &Value) -> Result<ToolPayload, ToolError> {
    let format = OutputFormat::parse(args)?;
    let params: LspPositionParams = parse_args(args)?;
    let (root, lang, text) = lsp_prepare(&params).await?;
    let locations = crate::LSP
        .definition(
            &root,
            lang,
            &params.file_path,
            params.line,
            params.character,
            &text,
        )
        .await
        .map_err(|e| ToolError::new(e.to_string()))?;
    if format != OutputFormat::Json {
        return Ok(format.text(output_status::locations("lsp_definition", &locations)));
    }
    let report = LocationsReport {
        locations: to_protocol_locations(locations),
    };
    Ok(format.render(&report, String::new))
}

/// lsp_references：引用集（includeDeclaration=false）。
async fn lsp_references_async(args: &Value) -> Result<ToolPayload, ToolError> {
    let format = OutputFormat::parse(args)?;
    let params: LspPositionParams = parse_args(args)?;
    let (root, lang, text) = lsp_prepare(&params).await?;
    let locations = crate::LSP
        .references(
            &root,
            lang,
            &params.file_path,
            params.line,
            params.character,
            &text,
        )
        .await
        .map_err(|e| ToolError::new(e.to_string()))?;
    if format != OutputFormat::Json {
        return Ok(format.text(output_status::locations("lsp_references", &locations)));
    }
    let report = LocationsReport {
        locations: to_protocol_locations(locations),
    };
    Ok(format.render(&report, String::new))
}

/// lsp_hover：纯文本 contents；无信息时按契约返回空串。
async fn lsp_hover_async(args: &Value) -> Result<ToolPayload, ToolError> {
    let format = OutputFormat::parse(args)?;
    let params: LspPositionParams = parse_args(args)?;
    let (root, lang, text) = lsp_prepare(&params).await?;
    let contents = crate::LSP
        .hover(
            &root,
            lang,
            &params.file_path,
            params.line,
            params.character,
            &text,
        )
        .await
        .map_err(|e| ToolError::new(e.to_string()))?;
    if format != OutputFormat::Json {
        return Ok(format.text(contents));
    }
    Ok(format.render(
        &HoverReport {
            contents: Value::String(contents),
        },
        String::new,
    ))
}

/// 桥 Location → 协议 DTO（行号约定一致：1-based line、0-based character、
/// 根相对路径）。
fn to_protocol_locations(locations: Vec<codeidx_lsp::Location>) -> Vec<Location> {
    locations
        .into_iter()
        .map(|l| Location {
            file: l.file,
            line: l.line,
            character: l.character,
        })
        .collect()
}

#[cfg(test)]
#[path = "handlers_tests.rs"]
mod tests;
