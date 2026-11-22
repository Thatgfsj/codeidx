//! In-memory static call graph snapshots and incremental maintenance.

use std::collections::{BTreeSet, HashMap};

pub use codeidx_ast::CallSiteKind;
use codeidx_ast::{enclosing_symbol, CallSite, CallTarget, ParsedFile};
use codeidx_protocol::ToolError;
use rusqlite::{params, Connection};

use crate::store::{poisoned, sqlite_err};
use crate::Store;

const MAX_CANDIDATES: usize = 16;

#[path = "call_targets.rs"]
mod targets;
use targets::{Candidates, Definition};

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
    pub owner_hint: Option<String>,
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
                s.candidate_count, s.confidence, e.callee_id,
                CASE WHEN s.target_kind = 'owner' THEN s.qualifier END
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
                owner_hint: row.get(10)?,
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
        "INSERT INTO call_sites (caller_id, source_lang, name, rel_path, line, character, kind, target_kind, qualifier)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
    )?;
    let mut insert = |name: &str,
                      line: u32,
                      character: Option<u32>,
                      kind: &str,
                      target: &str,
                      qualifier: Option<&str>|
     -> rusqlite::Result<()> {
        let caller = enclosing_symbol(&spans, line);
        stmt.execute(params![
            caller, lang, name, path, line, character, kind, target, qualifier
        ])?;
        if !name.is_empty() && target != "unresolved" {
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
            let (target, qualifier) = match &call.target {
                CallTarget::Bare => ("bare", None),
                CallTarget::Owner(owner) => ("owner", Some(owner.as_str())),
                CallTarget::Module(module) => ("module", Some(module.as_str())),
                CallTarget::Unresolved => ("unresolved", None),
            };
            insert(
                &call.name,
                call.line,
                Some(call.character),
                kind,
                target,
                qualifier,
            )?;
        }
    } else {
        let mut seen = BTreeSet::new();
        for site in &parsed.refs {
            if seen.insert((&site.name, site.line)) {
                insert(&site.name, site.line, None, "reference", "bare", None)?;
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
        "SELECT id, rel_path, target_kind, qualifier FROM call_sites
         WHERE source_lang = ?1 AND name = ?2 AND target_kind <> 'unresolved'",
    )?;
    let mut defs = conn.prepare_cached(
        "SELECT id, rel_path, kind, parent FROM symbols WHERE source_lang = ?1 AND name = ?2 AND kind IN ('fn', 'method')
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
        let definitions = defs
            .query_map(params![lang, name], |row| {
                Ok(Definition {
                    id: row.get(0)?,
                    path: row.get(1)?,
                    kind: row.get(2)?,
                    owner: row.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut resolved = HashMap::new();
        let mut current = Some(first);
        while let Some(row) = current {
            let site_id: i64 = row.get(0)?;
            let path = row.get_ref(1)?.as_str()?;
            let kind = row.get_ref(2)?.as_str()?;
            let qualifier = row.get::<_, Option<String>>(3)?;
            let (confidence, targets, count) = resolved
                .entry((path.to_owned(), kind.to_owned(), qualifier.clone()))
                .or_insert_with(|| {
                    Candidates::select(&definitions, path, kind, qualifier.as_deref())
                })
                .view();
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
