//! In-memory static call graph snapshots and incremental maintenance.

use std::collections::{BTreeSet, HashMap};

pub use codeidx_ast::CallSiteKind;
use codeidx_ast::{enclosing_symbol, CallSite, ParsedFile};
use codeidx_protocol::ToolError;
use rusqlite::{params, Connection};

use crate::store::{poisoned, sqlite_err};
use crate::Store;

const MAX_CANDIDATES: usize = 16;

#[cfg(test)]
#[path = "call_graph_tests.rs"]
mod tests;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallConfidence {
    Unique,
    SameFile,
    Ambiguous,
    Unresolved,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallSymbol {
    pub id: i64,
    pub name: String,
    pub rel_path: String,
    pub start_line: u32,
    pub end_line: u32,
    pub parent: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredCallSite {
    pub caller_id: Option<i64>,
    pub name: String,
    pub rel_path: String,
    pub line: u32,
    pub character: Option<u32>,
    pub kind: CallSiteKind,
    pub candidates: Vec<i64>,
    /// Count before applying the per-site candidate bound.
    pub candidate_count: usize,
    pub confidence: CallConfidence,
}

#[derive(Debug, Default, Clone)]
pub struct CallGraph {
    pub symbols: Vec<CallSymbol>,
    pub sites: Vec<StoredCallSite>,
}

impl Store {
    /// Clone graph data under a single lock. Row IDs only identify nodes in this snapshot.
    pub fn call_graph(&self) -> Result<CallGraph, ToolError> {
        let conn = self.conn.lock().map_err(|_| poisoned())?;
        snapshot(&conn).map_err(sqlite_err)
    }
}

fn snapshot(conn: &Connection) -> rusqlite::Result<CallGraph> {
    let mut symbols = conn.prepare(
        "SELECT id, name, rel_path, start_line, end_line, parent FROM symbols
         WHERE kind IN ('fn', 'method') ORDER BY rel_path, start_line, end_line, name, id",
    )?;
    let symbols = symbols
        .query_map([], |row| {
            Ok(CallSymbol {
                id: row.get(0)?,
                name: row.get(1)?,
                rel_path: row.get(2)?,
                start_line: row.get(3)?,
                end_line: row.get(4)?,
                parent: row.get(5)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut stmt = conn.prepare(
        "SELECT s.id, s.caller_id, s.name, s.rel_path, s.line, s.character, s.kind,
                s.candidate_count, s.confidence, e.callee_id
         FROM call_sites s LEFT JOIN call_edges e ON e.site_id = s.id
         WHERE s.kind = 'call' OR s.candidate_count <> 0
         ORDER BY s.rel_path, s.line, s.character, s.id, e.ordinal",
    )?;
    let mut rows = stmt.query([])?;
    let mut graph = CallGraph {
        symbols,
        sites: Vec::new(),
    };
    let mut previous = None;
    while let Some(row) = rows.next()? {
        let id: i64 = row.get(0)?;
        if previous != Some(id) {
            let kind = match row.get_ref(6)?.as_str()? {
                "call" => CallSiteKind::Call,
                "callback" => CallSiteKind::Callback,
                _ => CallSiteKind::Reference,
            };
            let confidence = match row.get_ref(8)?.as_str()? {
                "unique" => CallConfidence::Unique,
                "same_file" => CallConfidence::SameFile,
                "ambiguous" => CallConfidence::Ambiguous,
                _ => CallConfidence::Unresolved,
            };
            graph.sites.push(StoredCallSite {
                caller_id: row.get(1)?,
                name: row.get(2)?,
                rel_path: row.get(3)?,
                line: row.get(4)?,
                character: row.get(5)?,
                kind,
                candidates: Vec::new(),
                candidate_count: row.get::<_, i64>(7)? as usize,
                confidence,
            });
            previous = Some(id);
        }
        if let (Some(site), Some(callee)) = (graph.sites.last_mut(), row.get::<_, Option<i64>>(9)?)
        {
            site.candidates.push(callee);
        }
    }
    Ok(graph)
}

pub(crate) fn old_definitions(
    conn: &Connection,
    path: &str,
) -> rusqlite::Result<BTreeSet<(String, String)>> {
    let mut stmt = conn.prepare_cached(
        "SELECT source_lang, name FROM symbols WHERE rel_path = ?1 AND kind IN ('fn', 'method')",
    )?;
    let rows = stmt.query_map([path], |row| Ok((row.get(0)?, row.get(1)?)))?;
    rows.collect()
}

pub(crate) fn remove_sites(conn: &Connection, path: &str) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM call_edges WHERE site_id IN (SELECT id FROM call_sites WHERE rel_path = ?1)",
        [path],
    )?;
    conn.execute("DELETE FROM call_sites WHERE rel_path = ?1", [path])?;
    Ok(())
}

pub(crate) fn replace_sites(
    conn: &Connection,
    parsed: &ParsedFile,
    calls: Option<&[CallSite]>,
    lang: &str,
    mut dirty: BTreeSet<(String, String)>,
) -> rusqlite::Result<()> {
    let path = parsed.rel_path.as_str();
    remove_sites(conn, path)?;
    dirty.extend(old_definitions(conn, path)?);
    let mut spans = conn.prepare_cached(
        "SELECT start_line, end_line, id FROM symbols
         WHERE rel_path = ?1 AND kind IN ('fn', 'method') ORDER BY id",
    )?;
    let spans = spans
        .query_map([path], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
        .collect::<rusqlite::Result<Vec<(u32, u32, i64)>>>()?;
    let mut stmt = conn.prepare_cached(
        "INSERT INTO call_sites (caller_id, source_lang, name, rel_path, line, character, kind)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )?;
    let mut insert =
        |name: &str, line: u32, character: Option<u32>, kind: &str| -> rusqlite::Result<()> {
            let caller = enclosing_symbol(&spans, line);
            stmt.execute(params![caller, lang, name, path, line, character, kind])?;
            if !name.is_empty() {
                dirty.insert((lang.to_owned(), name.to_owned()));
            }
            Ok(())
        };
    if let Some(calls) = calls {
        for call in calls {
            let kind = match call.kind {
                CallSiteKind::Call => "call",
                CallSiteKind::Callback => "callback",
                CallSiteKind::Reference => "reference",
            };
            insert(&call.name, call.line, Some(call.character), kind)?;
        }
    } else {
        let mut seen = BTreeSet::new();
        for site in &parsed.refs {
            if seen.insert((&site.name, site.line)) {
                insert(&site.name, site.line, None, "reference")?;
            }
        }
    }
    rebuild_names(conn, dirty)
}

pub(crate) fn rebuild_names(
    conn: &Connection,
    names: BTreeSet<(String, String)>,
) -> rusqlite::Result<()> {
    let mut sites = conn.prepare_cached(
        "SELECT id, rel_path FROM call_sites WHERE source_lang = ?1 AND name = ?2",
    )?;
    let mut defs = conn.prepare_cached(
        "SELECT id, rel_path FROM symbols WHERE source_lang = ?1 AND name = ?2 AND kind IN ('fn', 'method')
         ORDER BY rel_path, start_line, end_line, parent, id",
    )?;
    let mut clear = conn.prepare_cached("DELETE FROM call_edges WHERE site_id = ?1")?;
    let mut update = conn.prepare_cached(
        "UPDATE call_sites SET candidate_count = ?1, confidence = ?2 WHERE id = ?3",
    )?;
    let mut write = conn.prepare_cached(
        "INSERT INTO call_edges (site_id, callee_id, ordinal) VALUES (?1, ?2, ?3)",
    )?;
    for (lang, name) in names {
        let mut sites = sites.query(params![lang, name])?;
        let Some(first) = sites.next()? else { continue };
        let mut all = Vec::new();
        let mut per_path: HashMap<String, Vec<i64>> = HashMap::new();
        let mut definitions = defs.query(params![lang, name])?;
        let mut total = 0usize;
        while let Some(definition) = definitions.next()? {
            let id = definition.get(0)?;
            let path = definition.get(1)?;
            total += 1;
            if all.len() < MAX_CANDIDATES {
                all.push(id);
            }
            let local = per_path.entry(path).or_default();
            if local.len() <= MAX_CANDIDATES {
                local.push(id);
            }
        }
        let mut current = Some(first);
        while let Some(row) = current {
            let site_id: i64 = row.get(0)?;
            let path = row.get_ref(1)?.as_str()?;
            let (confidence, targets, count) = match (total, per_path.get(path)) {
                (0, _) => ("unresolved", &all, 0),
                (1, _) => ("unique", &all, 1),
                (_, Some(local)) if local.len() == 1 => ("same_file", local, 1),
                // Multiple local definitions remain ambiguous. Retain all global candidates
                // to give LSP a chance to pick a definition outside the current file.
                _ => ("ambiguous", &all, total),
            };
            clear.execute([site_id])?;
            update.execute(params![count as i64, confidence, site_id])?;
            for (ordinal, target) in targets.iter().enumerate() {
                write.execute(params![site_id, target, ordinal as i64])?;
            }
            current = sites.next()?;
        }
    }
    Ok(())
}
