//! PageRank 加权骨架图（纯函数，无副作用）。
//!
//! 节点 = 符号（按 `name + rel_path` 唯一）；边 = 引用处符号 → 同名匹配的
//! 定义符号。迭代上限 20 轮、damping 0.85；focus 文件中的符号基准值 +2.0
//! （personalized PageRank 的提权形式，每轮基准保持，聚焦效果不随轮数衰减）。

use std::collections::HashMap;

use crate::types::{SymbolDef, SymbolRef};

/// 一个骨架图节点：`(name, rel_path)` 唯一的符号与其代表定义。
#[derive(Debug, Clone)]
pub struct RepoNode {
    pub name: String,
    pub rel_path: String,
    pub kind: crate::types::SymbolKind,
    pub start_line: u32,
}

/// damping 与迭代上限。
const DAMPING: f64 = 0.85;
const MAX_ITERATIONS: usize = 20;
/// focus 提权幅度。
const FOCUS_BOOST: f64 = 2.0;

/// 构建 PageRank 节点与分数，按分数降序返回（同分按路径稳定排序）。
pub fn pagerank(
    symbols: &[SymbolDef],
    refs: &[SymbolRef],
    focus_files: &[String],
) -> Vec<(RepoNode, f64)> {
    // 1) 节点唯一化：同一 (name, rel_path) 的多个定义合并到最早定义。
    let mut node_of: HashMap<(String, String), usize> = HashMap::new();
    let mut nodes: Vec<RepoNode> = Vec::new();
    for sym in symbols {
        let key = (sym.name.clone(), sym.rel_path.clone());
        match node_of.get(&key) {
            Some(&idx) => {
                if sym.start_line < nodes[idx].start_line {
                    nodes[idx].start_line = sym.start_line;
                }
            }
            None => {
                node_of.insert(key, nodes.len());
                nodes.push(RepoNode {
                    name: sym.name.clone(),
                    rel_path: sym.rel_path.clone(),
                    kind: sym.kind,
                    start_line: sym.start_line,
                });
            }
        }
    }
    if nodes.is_empty() {
        return Vec::new();
    }

    // 2) 名字 → 节点索引（粗粒度同名匹配）；文件 → 该文件符号区间。
    let mut ids_by_name: HashMap<&str, Vec<usize>> = HashMap::new();
    let mut spans_by_file: HashMap<&str, Vec<(u32, u32, usize)>> = HashMap::new();
    for sym in symbols {
        let idx = node_of[&(sym.name.clone(), sym.rel_path.clone())];
        ids_by_name.entry(sym.name.as_str()).or_default().push(idx);
        spans_by_file
            .entry(sym.rel_path.as_str())
            .or_default()
            .push((sym.start_line, sym.end_line, idx));
    }

    // 3) 边：引用处符号 → 同名定义符号（均分权重，跳过自环）。
    let mut edges: Vec<(usize, Vec<(usize, f64)>)> = vec![(0, Vec::new()); nodes.len()];
    for r in refs {
        let Some(targets) = ids_by_name.get(r.name.as_str()) else {
            continue;
        };
        let Some(src) = ref_site_symbol(&spans_by_file, &r.rel_path, r.line) else {
            continue;
        };
        let targets: Vec<usize> = targets.iter().copied().filter(|&t| t != src).collect();
        if targets.is_empty() {
            continue;
        }
        let w = 1.0 / targets.len() as f64;
        let out = &mut edges[src].1;
        for t in targets {
            if let Some(e) = out.iter_mut().find(|(tt, _)| *tt == t) {
                e.1 += w;
            } else {
                out.push((t, w));
            }
        }
    }

    // 4) personalized PageRank：focus 文件符号基准值 1.0 + 2.0。
    let focus: std::collections::HashSet<&str> = focus_files.iter().map(|s| s.as_str()).collect();
    let base: Vec<f64> = nodes
        .iter()
        .map(|n| {
            if focus.contains(n.rel_path.as_str()) {
                1.0 + FOCUS_BOOST
            } else {
                1.0
            }
        })
        .collect();
    let out_weight: Vec<f64> = edges
        .iter()
        .map(|(_, o)| o.iter().map(|(_, w)| w).sum())
        .collect();

    let mut rank = base.clone();
    for _ in 0..MAX_ITERATIONS {
        let mut next = vec![0.0f64; nodes.len()];
        for (src, outs) in &edges {
            if outs.is_empty() || out_weight[*src] <= 0.0 {
                continue;
            }
            let share = rank[*src] / out_weight[*src];
            for (dst, w) in outs {
                next[*dst] += share * w;
            }
        }
        let mut delta = 0.0f64;
        for i in 0..nodes.len() {
            next[i] = (1.0 - DAMPING) * base[i] + DAMPING * next[i];
            delta += (next[i] - rank[i]).abs();
        }
        rank = next;
        if delta < 1e-9 {
            break;
        }
    }

    let mut scored: Vec<(RepoNode, f64)> = nodes.into_iter().zip(rank).collect();
    scored.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.rel_path.cmp(&b.0.rel_path))
            .then_with(|| a.0.start_line.cmp(&b.0.start_line))
            .then_with(|| a.0.name.cmp(&b.0.name))
    });
    scored
}

/// 引用落在哪个符号体内（取覆盖它的最小区间符号，即最内层符号）。
fn ref_site_symbol(
    spans_by_file: &HashMap<&str, Vec<(u32, u32, usize)>>,
    rel_path: &str,
    line: u32,
) -> Option<usize> {
    let spans = spans_by_file.get(rel_path)?;
    spans
        .iter()
        .filter(|(s, e, _)| line >= *s && line <= *e)
        .min_by_key(|(s, e, _)| e - s)
        .map(|(_, _, idx)| *idx)
}
