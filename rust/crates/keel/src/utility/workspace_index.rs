//! Persistent deterministic workspace index for code-aware retrieval.
//!
//! The index is the source of truth for code search and workspace navigation. It
//! stores file hashes, symbols, source chunks, and verified import edges in the
//! global per-workspace memory lane. Refresh is atomic and never falls back to a
//! live filesystem scan during retrieval.

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::Serialize;

use crate::runtime::{display_path, resolve_claude_home};

const SCHEMA_VERSION: &str = "1";
pub(crate) const MAX_FILES: usize = 20_000;
const MAX_FILE_BYTES: u64 = 2_000_000;
const MAX_CHUNK_BYTES: usize = 32_000;
const MAX_SEARCH_RESULTS: usize = 50;
const MAP_FILE_LIMIT: usize = 200;
const MAP_SYMBOL_LIMIT: usize = 1000;
const MAP_EDGE_LIMIT: usize = 1000;
const MAP_TEST_LIMIT: usize = 200;
const MAP_OWNER_LIMIT: usize = 200;
const RRF_K: f64 = 60.0;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SearchHit {
    pub path: String,
    pub symbol: String,
    pub kind: String,
    pub start_line: usize,
    pub end_line: usize,
    pub score: f64,
    pub reason: String,
    pub snippet: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RefreshReport {
    pub files_indexed: u64,
    pub files_discovered: u64,
    pub files_skipped_limit: u64,
    pub files_skipped_too_large: u64,
    pub files_skipped_unreadable: u64,
    /// True when the deterministic file ceiling excluded discovered files.
    pub truncated: bool,
    pub coverage_complete: bool,
    /// True when a concurrent writer prevented this refresh from publishing.
    pub lock_degraded: bool,
    pub files_added: u64,
    pub files_updated: u64,
    pub files_removed: u64,
    pub symbols_indexed: u64,
    pub chunks_indexed: u64,
    pub edges_indexed: u64,
    pub generation: u64,
    pub indexed_commit: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexStatus {
    pub database_path: PathBuf,
    pub workspace_root: PathBuf,
    pub indexed_commit: String,
    pub generation: u64,
    pub file_count: u64,
    pub symbol_count: u64,
    pub chunk_count: u64,
    pub edge_count: u64,
    pub stale: bool,
    pub files_discovered: u64,
    pub files_skipped_limit: u64,
    pub files_skipped_too_large: u64,
    pub files_skipped_unreadable: u64,
    pub truncated: bool,
    pub coverage_complete: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SearchResults {
    pub hits: Vec<SearchHit>,
    pub truncated: bool,
}

#[derive(Debug, Clone)]
struct SourceFile {
    path: String,
    language: String,
    hash: String,
    modified_at: u128,
    size: u64,
    content: String,
    imports: Vec<String>,
    symbols: Vec<ParsedSymbol>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StoredFileMetadata {
    hash: String,
    modified_at: u128,
    size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SourceSnapshot {
    path: String,
    modified_at: u128,
    size: u64,
}

#[derive(Debug, Default)]
struct SourcePathCollection {
    paths: Vec<PathBuf>,
    discovered: u64,
    skipped_limit: u64,
    skipped_unreadable: u64,
    unreadable_prefixes: BTreeSet<String>,
}

#[derive(Debug, Default)]
struct SnapshotCollection {
    snapshots: Vec<SourceSnapshot>,
    skipped_too_large: u64,
    skipped_unreadable: u64,
    unreadable_paths: BTreeSet<String>,
}

#[derive(Debug, Default)]
struct SourceCollection {
    sources: Vec<SourceFile>,
    unreadable_paths: BTreeSet<String>,
}

#[derive(Debug, Clone)]
struct ParsedSymbol {
    kind: String,
    name: String,
    qualified_name: String,
    signature: String,
    documentation: String,
    start_line: usize,
    end_line: usize,
}

#[derive(Debug, Clone)]
struct Candidate {
    key: String,
    hit: SearchHit,
}

pub fn database_path(workspace_root: &Path, claude_home_flag: &str) -> Result<PathBuf, String> {
    let home = resolve_claude_home(claude_home_flag)?;
    let canonical = canonical_workspace_root(workspace_root)?;
    let raw_workspace = display_path(&canonical);
    let slug = crate::utility::system_map::workspace_key(&raw_workspace);
    Ok(home
        .join("memories")
        .join("workspaces")
        .join(slug)
        .join("code-index")
        .join("workspace-index.sqlite3"))
}

pub fn refresh(
    workspace_root: &Path,
    claude_home_flag: &str,
    force: bool,
) -> Result<RefreshReport, String> {
    let root = canonical_workspace_root(workspace_root)?;
    let path = database_path(&root, claude_home_flag)?;
    crate::utility::sqlite::create_parent_directory(&path)
        .map_err(|error| format!("create index directory: {error}"))?;
    let mut connection = match open_connection(&path) {
        Ok(connection) => connection,
        Err(IndexAccessError::Locked) => {
            return Ok(degraded_refresh_report(&root));
        }
        Err(error) => return Err(error.to_string()),
    };
    if let Err(error) = ensure_schema(&connection) {
        if matches!(error, IndexAccessError::Locked) {
            return Ok(degraded_refresh_report(&root));
        }
        return Err(error.to_string());
    }
    let existing = match existing_file_metadata(&connection) {
        Ok(existing) => existing,
        Err(IndexAccessError::Locked) => return Ok(degraded_refresh_report(&root)),
        Err(error) => return Err(error.to_string()),
    };
    let previous_commit = meta(&connection, "indexed_commit");
    let indexed_commit = git_head(&root);
    let commit_changed = previous_commit.as_deref() != Some(indexed_commit.as_str());
    let SourcePathCollection {
        paths: source_paths,
        discovered,
        skipped_limit,
        skipped_unreadable: path_unreadable,
        unreadable_prefixes,
    } = collect_source_paths(&root)?;
    let SnapshotCollection {
        snapshots,
        skipped_too_large,
        skipped_unreadable,
        unreadable_paths: snapshot_unreadable_paths,
    } = collect_source_snapshots(&root, &source_paths);
    let mut unreadable_paths = snapshot_unreadable_paths;
    let mut report = RefreshReport {
        files_indexed: snapshots.len() as u64,
        files_discovered: discovered,
        files_skipped_limit: skipped_limit,
        files_skipped_too_large: skipped_too_large,
        files_skipped_unreadable: path_unreadable + skipped_unreadable,
        truncated: skipped_limit > 0,
        coverage_complete: skipped_limit == 0
            && skipped_too_large == 0
            && path_unreadable == 0
            && skipped_unreadable == 0,
        generation: meta(&connection, "generation")
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0),
        indexed_commit: indexed_commit.clone(),
        ..RefreshReport::default()
    };

    let metadata_unchanged = snapshots.len() == existing.len()
        && snapshots.iter().all(|snapshot| {
            existing.get(&snapshot.path).is_some_and(|stored| {
                stored.size == snapshot.size && stored.modified_at == snapshot.modified_at
            })
        });
    if !force && !commit_changed && metadata_unchanged {
        return Ok(report);
    }

    // Parse only files whose metadata changed; timestamp-only editor touches
    // otherwise made every refresh O(workspace).
    let dirty_paths: Vec<PathBuf> = snapshots
        .iter()
        .filter(|snapshot| {
            existing
                .get(&snapshot.path)
                .map(|stored| {
                    stored.size != snapshot.size || stored.modified_at != snapshot.modified_at
                })
                .unwrap_or(true)
        })
        .map(|snapshot| root.join(&snapshot.path))
        .collect();
    let dirty_collection = collect_sources_from_paths(&root, dirty_paths)?;
    let dirty_sources = dirty_collection.sources;
    unreadable_paths.extend(dirty_collection.unreadable_paths);
    report.files_skipped_unreadable = path_unreadable + unreadable_paths.len() as u64;
    report.coverage_complete =
        skipped_limit == 0 && skipped_too_large == 0 && report.files_skipped_unreadable == 0;
    let content_changed = dirty_sources.iter().any(|source| {
        existing
            .get(&source.path)
            .map(|stored| stored.hash != source.hash || stored.size != source.size)
            .unwrap_or(true)
    });
    let mut active_paths: BTreeSet<String> = snapshots
        .iter()
        .map(|snapshot| snapshot.path.clone())
        .collect();
    active_paths.extend(unreadable_paths.iter().cloned());
    for existing_path in existing.keys() {
        if unreadable_prefixes
            .iter()
            .any(|prefix| path_is_under_prefix(existing_path, prefix))
        {
            active_paths.insert(existing_path.clone());
        }
    }
    let has_stale_paths = existing.keys().any(|path| !active_paths.contains(path));

    // With no content change or deletion, update metadata in place and preserve
    // symbols/chunks/edges as the incremental fast path.
    if !force
        && !commit_changed
        && !content_changed
        && !has_stale_paths
        && unreadable_paths.is_empty()
    {
        let transaction = match connection.transaction_with_behavior(TransactionBehavior::Immediate)
        {
            Ok(transaction) => transaction,
            Err(error) if sqlite_lock_error(&error) => {
                report.lock_degraded = true;
                return Ok(report);
            }
            Err(error) => return Err(format!("begin workspace index metadata refresh: {error}")),
        };
        for snapshot in &snapshots {
            transaction
                .execute(
                    "UPDATE files SET modified_at = ?1, size = ?2 WHERE path = ?3",
                    params![
                        snapshot.modified_at.to_string(),
                        snapshot.size as i64,
                        snapshot.path
                    ],
                )
                .map_err(|error| format!("update indexed metadata {}: {error}", snapshot.path))?;
        }
        transaction
            .execute(
                "INSERT OR REPLACE INTO meta(key, value) VALUES
                 ('updated_at_millis', ?1),
                 ('files_discovered', ?2),
                 ('files_skipped_limit', ?3),
                 ('files_skipped_too_large', ?4),
                 ('files_skipped_unreadable', ?5),
                 ('truncated', ?6),
                 ('coverage_complete', ?7)",
                params![
                    now_millis().to_string(),
                    report.files_discovered.to_string(),
                    report.files_skipped_limit.to_string(),
                    report.files_skipped_too_large.to_string(),
                    report.files_skipped_unreadable.to_string(),
                    report.truncated.to_string(),
                    report.coverage_complete.to_string(),
                ],
            )
            .map_err(|error| format!("stamp metadata refresh: {error}"))?;
        if let Err(error) = transaction.commit() {
            if sqlite_lock_error(&error) {
                report.lock_degraded = true;
                return Ok(report);
            }
            return Err(format!("commit workspace index metadata refresh: {error}"));
        }
        return Ok(report);
    }

    // Content changes require the complete source set for relationship rebuilds;
    // unchanged files remain skipped by the record loop below.
    let source_collection = if content_changed || force || commit_changed || has_stale_paths {
        collect_sources_from_paths(
            &root,
            snapshots
                .iter()
                .map(|snapshot| root.join(&snapshot.path))
                .collect(),
        )?
    } else {
        SourceCollection {
            sources: dirty_sources,
            unreadable_paths: BTreeSet::new(),
        }
    };
    unreadable_paths.extend(source_collection.unreadable_paths);
    let sources = source_collection.sources;
    report.files_skipped_unreadable = path_unreadable + unreadable_paths.len() as u64;
    report.coverage_complete =
        skipped_limit == 0 && skipped_too_large == 0 && report.files_skipped_unreadable == 0;
    let files_changed = sources.iter().any(|source| {
        existing
            .get(&source.path)
            .map(|stored| stored.hash != source.hash || stored.size != source.size)
            .unwrap_or(true)
    }) || existing.keys().any(|path| !active_paths.contains(path));

    if !force && !files_changed && !commit_changed {
        return Ok(report);
    }

    let transaction = match connection.transaction_with_behavior(TransactionBehavior::Immediate) {
        Ok(transaction) => transaction,
        Err(error) if sqlite_lock_error(&error) => {
            report.lock_degraded = true;
            return Ok(report);
        }
        Err(error) => return Err(format!("begin workspace index refresh: {error}")),
    };
    for source in &sources {
        let unchanged = existing
            .get(&source.path)
            .map(|metadata| metadata.hash == source.hash && metadata.size == source.size)
            .unwrap_or(false);
        if unchanged && !force {
            continue;
        }
        delete_file_records(&transaction, &source.path)?;
        transaction
            .execute(
                "INSERT INTO files(path, language, hash, modified_at, size, imports) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    source.path,
                    source.language,
                    source.hash,
                    source.modified_at.to_string(),
                    source.size as i64,
                    source.imports.join("\n"),
                ],
            )
            .map_err(|error| format!("insert indexed file {}: {error}", source.path))?;
        let file_id = transaction.last_insert_rowid();
        for symbol in &source.symbols {
            transaction
                .execute(
                    "INSERT INTO symbols(file_id, path, kind, name, qualified_name, signature, documentation, start_line, end_line) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    params![
                        file_id,
                        source.path,
                        symbol.kind,
                        symbol.name,
                        symbol.qualified_name,
                        symbol.signature,
                        symbol.documentation,
                        symbol.start_line as i64,
                        symbol.end_line as i64,
                    ],
                )
                .map_err(|error| format!("insert symbol {}: {error}", symbol.name))?;
            let symbol_id = transaction.last_insert_rowid();
            insert_search_entry(
                &transaction,
                SearchEntry {
                    path: &source.path,
                    symbol: &symbol.name,
                    qualified_name: &symbol.qualified_name,
                    signature: &symbol.signature,
                    documentation: &symbol.documentation,
                    content: &symbol.signature,
                    kind: &symbol.kind,
                    start_line: symbol.start_line,
                    end_line: symbol.end_line,
                    entity_key: &format!("symbol:{symbol_id}"),
                },
            )?;
            report.symbols_indexed += 1;
        }
        let overview = truncate_utf8(&source.content, MAX_CHUNK_BYTES);
        transaction
            .execute(
                "INSERT INTO chunks(file_id, path, symbol_id, kind, start_line, end_line, content) VALUES (?1, ?2, NULL, 'file', 1, ?3, ?4)",
                params![file_id, source.path, source.content.lines().count() as i64, overview],
            )
            .map_err(|error| format!("insert file chunk {}: {error}", source.path))?;
        insert_search_entry(
            &transaction,
            SearchEntry {
                path: &source.path,
                symbol: "",
                qualified_name: "",
                signature: "",
                documentation: "",
                content: &overview,
                kind: "file",
                start_line: 1,
                end_line: source.content.lines().count().max(1),
                entity_key: &format!("file:{}", source.path),
            },
        )?;
        report.chunks_indexed += 1;
        for symbol in &source.symbols {
            let content = source
                .content
                .lines()
                .skip(symbol.start_line.saturating_sub(1))
                .take(
                    symbol
                        .end_line
                        .saturating_sub(symbol.start_line)
                        .saturating_add(1),
                )
                .collect::<Vec<_>>()
                .join("\n");
            let content = truncate_utf8(&content, MAX_CHUNK_BYTES);
            let symbol_id: i64 = transaction
                .query_row(
                    "SELECT id FROM symbols WHERE path = ?1 AND name = ?2 AND start_line = ?3 ORDER BY id DESC LIMIT 1",
                    params![source.path, symbol.name, symbol.start_line as i64],
                    |row| row.get(0),
                )
                .map_err(|error| format!("lookup symbol chunk {}: {error}", symbol.name))?;
            transaction
                .execute(
                    "INSERT INTO chunks(file_id, path, symbol_id, kind, start_line, end_line, content) VALUES (?1, ?2, ?3, 'symbol', ?4, ?5, ?6)",
                    params![file_id, source.path, symbol_id, symbol.start_line as i64, symbol.end_line as i64, content],
                )
                .map_err(|error| format!("insert symbol chunk {}: {error}", symbol.name))?;
            insert_search_entry(
                &transaction,
                SearchEntry {
                    path: &source.path,
                    symbol: &symbol.name,
                    qualified_name: &symbol.qualified_name,
                    signature: &symbol.signature,
                    documentation: &symbol.documentation,
                    content: &content,
                    kind: "symbol",
                    start_line: symbol.start_line,
                    end_line: symbol.end_line,
                    entity_key: &format!("chunk:{}:{}", source.path, symbol.start_line),
                },
            )?;
            report.chunks_indexed += 1;
        }
        if existing.contains_key(&source.path) {
            report.files_updated += 1;
        } else {
            report.files_added += 1;
        }
    }

    let stale_paths: Vec<String> = existing
        .keys()
        .filter(|path| !active_paths.contains(*path))
        .cloned()
        .collect();
    for path in stale_paths {
        delete_file_records(&transaction, &path)?;
        report.files_removed += 1;
    }

    // An incomplete walk must not discard relationships for files hidden by an
    // unreadable path; rebuild the full edge set only after a complete walk.
    if (force || files_changed) && unreadable_paths.is_empty() && unreadable_prefixes.is_empty() {
        transaction
            .execute("DELETE FROM edges", [])
            .map_err(|error| format!("clear workspace edges: {error}"))?;
    }
    let mut path_set: BTreeSet<String> = sources.iter().map(|source| source.path.clone()).collect();
    path_set.extend(active_paths.iter().cloned());
    for source in &sources {
        for import in &source.imports {
            if let Some(target) = resolve_import(&source.path, import, &path_set) {
                transaction
                    .execute(
                        "INSERT OR IGNORE INTO edges(from_path, from_symbol_id, to_path, to_symbol_id, relation, evidence) VALUES (?1, NULL, ?2, NULL, 'imports', ?3)",
                        params![source.path, target, import],
                    )
                    .map_err(|error| format!("insert import edge {}: {error}", source.path))?;
                report.edges_indexed += 1;
            }
        }
    }
    let mut symbol_locations: HashMap<(String, String, usize), i64> = HashMap::new();
    let mut symbols_by_name: HashMap<String, Vec<(String, i64)>> = HashMap::new();
    let mut symbol_rows = transaction
        .prepare("SELECT id, path, name, start_line FROM symbols")
        .map_err(|error| format!("prepare symbol relationships: {error}"))?;
    let rows = symbol_rows
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })
        .map_err(|error| format!("read symbol relationships: {error}"))?;
    for row in rows {
        let (id, path, name, start_line) =
            row.map_err(|error| format!("read symbol relationship row: {error}"))?;
        symbol_locations.insert((path.clone(), name.clone(), start_line.max(1) as usize), id);
        symbols_by_name.entry(name).or_default().push((path, id));
    }
    drop(symbol_rows);
    for source in &sources {
        for symbol in &source.symbols {
            let Some(from_id) = symbol_locations.get(&(
                source.path.clone(),
                symbol.name.clone(),
                symbol.start_line,
            )) else {
                continue;
            };
            for called_name in extract_call_names(
                &source
                    .content
                    .lines()
                    .skip(symbol.start_line.saturating_sub(1))
                    .take(
                        symbol
                            .end_line
                            .saturating_sub(symbol.start_line)
                            .saturating_add(1),
                    )
                    .collect::<Vec<_>>()
                    .join("\n"),
            ) {
                let Some(targets) = symbols_by_name.get(&called_name) else {
                    continue;
                };
                for (target_path, target_id) in targets.iter().take(4) {
                    if *target_id == *from_id {
                        continue;
                    }
                    transaction
                        .execute(
                            "INSERT OR IGNORE INTO edges(from_path, from_symbol_id, to_path, to_symbol_id, relation, evidence) VALUES (?1, ?2, ?3, ?4, 'calls-candidate', ?5)",
                            params![source.path, from_id, target_path, target_id, format!("{called_name}(")],
                        )
                        .map_err(|error| format!("insert call edge {}: {error}", source.path))?;
                    report.edges_indexed += 1;
                }
            }
        }
    }
    let has_changes = force
        || commit_changed
        || report.files_added > 0
        || report.files_updated > 0
        || report.files_removed > 0;
    let generation = if has_changes {
        next_generation(&transaction)?
    } else {
        transaction
            .query_row(
                "SELECT value FROM meta WHERE key = 'generation'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|error| format!("read generation: {error}"))?
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0)
    };
    transaction
        .execute(
            "INSERT OR REPLACE INTO meta(key, value) VALUES
             ('generation', ?1),
             ('indexed_commit', ?2),
             ('workspace_root', ?3),
             ('updated_at_millis', ?4),
             ('files_discovered', ?5),
             ('files_skipped_limit', ?6),
             ('files_skipped_too_large', ?7),
             ('files_skipped_unreadable', ?8),
             ('truncated', ?9),
             ('coverage_complete', ?10)",
            params![
                generation.to_string(),
                indexed_commit,
                root.to_string_lossy().to_string(),
                now_millis().to_string(),
                report.files_discovered.to_string(),
                report.files_skipped_limit.to_string(),
                report.files_skipped_too_large.to_string(),
                report.files_skipped_unreadable.to_string(),
                report.truncated.to_string(),
                report.coverage_complete.to_string(),
            ],
        )
        .map_err(|error| format!("stamp workspace index: {error}"))?;
    if let Err(error) = transaction.commit() {
        if sqlite_lock_error(&error) {
            report.lock_degraded = true;
            return Ok(report);
        }
        return Err(format!("commit workspace index: {error}"));
    }
    report.files_indexed = sources.len() as u64;
    report.generation = generation;
    report.indexed_commit = indexed_commit;
    Ok(report)
}

pub fn search_with_metadata(
    workspace_root: &Path,
    claude_home_flag: &str,
    query: &str,
    limit: usize,
) -> Result<SearchResults, String> {
    search_filtered_with_metadata(workspace_root, claude_home_flag, query, limit, None)
}

pub fn search_filtered(
    workspace_root: &Path,
    claude_home_flag: &str,
    query: &str,
    limit: usize,
    path_filter: Option<&str>,
) -> Result<Vec<SearchHit>, String> {
    Ok(
        search_filtered_with_metadata(workspace_root, claude_home_flag, query, limit, path_filter)?
            .hits,
    )
}

pub fn search_filtered_with_metadata(
    workspace_root: &Path,
    claude_home_flag: &str,
    query: &str,
    limit: usize,
    path_filter: Option<&str>,
) -> Result<SearchResults, String> {
    if query.trim().is_empty() {
        return Ok(SearchResults {
            hits: Vec::new(),
            truncated: false,
        });
    }
    refresh(workspace_root, claude_home_flag, false)?;
    let root = canonical_workspace_root(workspace_root)?;
    let path = database_path(&root, claude_home_flag)?;
    let connection = open_connection(&path)?;
    let terms = query_terms(query);
    if terms.is_empty() {
        return Ok(SearchResults {
            hits: Vec::new(),
            truncated: false,
        });
    }
    let normalized_filter = path_filter
        .map(|value| value.replace('\\', "/").to_ascii_lowercase())
        .filter(|value| !value.is_empty());
    let candidate_limit = if normalized_filter.is_some() {
        MAX_FILES
    } else {
        limit.max(10)
    };
    let mut exact = exact_candidates(&connection, &terms)?;
    let mut fts = fts_candidates(&connection, &terms, candidate_limit)?;
    let mut paths = path_candidates(&connection, &terms, candidate_limit)?;
    if let Some(path_filter) = normalized_filter.as_deref() {
        let matches = |candidate: &Candidate| {
            candidate
                .hit
                .path
                .to_ascii_lowercase()
                .contains(path_filter)
        };
        exact.retain(&matches);
        fts.retain(&matches);
        paths.retain(&matches);
    }
    let mut graph = graph_candidates(&connection, &exact, candidate_limit)?;
    if let Some(path_filter) = normalized_filter.as_deref() {
        graph.retain(|candidate| {
            candidate
                .hit
                .path
                .to_ascii_lowercase()
                .contains(path_filter)
        });
    }
    let channels = vec![exact, fts, paths, graph];
    let (hits, truncated) = fuse_candidates_with_status(channels, limit);
    Ok(SearchResults { hits, truncated })
}

pub fn status(workspace_root: &Path, claude_home_flag: &str) -> Result<IndexStatus, String> {
    let root = canonical_workspace_root(workspace_root)?;
    let path = database_path(&root, claude_home_flag)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("create index directory: {error}"))?;
    }
    let connection = open_connection(&path)?;
    ensure_schema(&connection)?;
    let indexed_commit = meta(&connection, "indexed_commit").unwrap_or_default();
    let generation = meta(&connection, "generation")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    let coverage_complete = meta(&connection, "coverage_complete")
        .map(|value| value == "true")
        .unwrap_or(false);
    let truncated = meta(&connection, "truncated")
        .map(|value| value == "true")
        .unwrap_or(false);
    Ok(IndexStatus {
        database_path: path,
        workspace_root: root.clone(),
        indexed_commit: indexed_commit.clone(),
        generation,
        file_count: count(&connection, "files")?,
        symbol_count: count(&connection, "symbols")?,
        chunk_count: count(&connection, "chunks")?,
        edge_count: count(&connection, "edges")?,
        stale: !indexed_commit.is_empty() && indexed_commit != git_head(&root),
        files_discovered: meta(&connection, "files_discovered")
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0),
        files_skipped_limit: meta(&connection, "files_skipped_limit")
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0),
        files_skipped_too_large: meta(&connection, "files_skipped_too_large")
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0),
        files_skipped_unreadable: meta(&connection, "files_skipped_unreadable")
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0),
        truncated,
        coverage_complete,
    })
}

pub fn render_map(workspace_root: &Path, claude_home_flag: &str) -> Result<String, String> {
    refresh(workspace_root, claude_home_flag, false)?;
    let root = canonical_workspace_root(workspace_root)?;
    let path = database_path(&root, claude_home_flag)?;
    let connection = open_connection(&path)?;
    let file_count = count(&connection, "files")?;
    let symbol_count = count(&connection, "symbols")?;
    let edge_count = count(&connection, "edges")?;
    let files_discovered = meta(&connection, "files_discovered")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(file_count);
    let files_skipped_limit = meta(&connection, "files_skipped_limit")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    let files_skipped_too_large = meta(&connection, "files_skipped_too_large")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    let files_skipped_unreadable = meta(&connection, "files_skipped_unreadable")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    let coverage_complete = meta(&connection, "coverage_complete")
        .map(|value| value == "true")
        .unwrap_or(false);
    let truncated = meta(&connection, "truncated")
        .map(|value| value == "true")
        .unwrap_or(false);
    let mut lines = vec![
        "# SYSTEM_MAP".to_string(),
        String::new(),
        format!("- workspace_root: {}", display_path(&root)),
        format!("- index_path: {}", display_path(&path)),
        format!(
            "- indexed_commit: {}",
            meta(&connection, "indexed_commit").unwrap_or_default()
        ),
        format!(
            "- generation: {}",
            meta(&connection, "generation").unwrap_or_default()
        ),
        String::new(),
        "## Index Coverage".to_string(),
        format!(
            "- coverage_complete: {} (truncated={}, discovered={}, indexed={}, skipped_limit={}, skipped_too_large={}, skipped_unreadable={})",
            coverage_complete,
            truncated,
            files_discovered,
            file_count,
            files_skipped_limit,
            files_skipped_too_large,
            files_skipped_unreadable,
        ),
        "- symbol extraction: Rust, JavaScript/TypeScript, Python, and Go (heuristic); Markdown, TOML, JSON, and YAML are indexed as file/chunk content only".to_string(),
        "- call relationships: candidate name matches only; verify source before treating an edge as a resolved call".to_string(),
        String::new(),
        "## Architecture and Ownership".to_string(),
        format!("- indexed symbols: {symbol_count}"),
        format!("- indexed relationships: {edge_count}"),
    ];

    let mut entry_points = connection
        .prepare("SELECT path FROM files WHERE lower(path) LIKE '%/main.%' OR lower(path) LIKE 'main.%' OR lower(path) LIKE '%/lib.%' OR lower(path) LIKE 'lib.%' OR lower(path) LIKE '%/index.%' OR lower(path) LIKE 'index.%' ORDER BY path LIMIT ?1")
        .map_err(|error| format!("prepare map files: {error}"))?;
    let rows = entry_points
        .query_map(params![MAP_OWNER_LIMIT as i64], |row| {
            row.get::<_, String>(0)
        })
        .map_err(|error| format!("read map entry points: {error}"))?;
    for row in rows {
        lines.push(format!(
            "- entry point: `{}`",
            row.map_err(|error| format!("read map entry point row: {error}"))?
        ));
    }
    let mut owners = connection
        .prepare("SELECT path FROM files WHERE lower(path) LIKE '%agents.md' OR lower(path) LIKE '%claude.md' OR lower(path) LIKE '%codeowners' OR lower(path) LIKE '%contributing.md' ORDER BY path LIMIT ?1")
        .map_err(|error| format!("prepare map ownership: {error}"))?;
    let owner_rows = owners
        .query_map(params![MAP_OWNER_LIMIT as i64], |row| {
            row.get::<_, String>(0)
        })
        .map_err(|error| format!("read map ownership: {error}"))?;
    for row in owner_rows {
        lines.push(format!(
            "- owner/instruction: `{}`",
            row.map_err(|error| format!("read map ownership row: {error}"))?
        ));
    }
    lines.push(String::new());
    lines.push("## Indexed Symbols".to_string());
    let mut symbols = connection
        .prepare("SELECT path, kind, qualified_name, start_line, end_line FROM symbols ORDER BY path, start_line LIMIT ?1")
        .map_err(|error| format!("prepare map symbols: {error}"))?;
    let rows = symbols
        .query_map(params![MAP_SYMBOL_LIMIT as i64], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })
        .map_err(|error| format!("read map symbols: {error}"))?;
    for row in rows {
        let (path, kind, name, start, end) =
            row.map_err(|error| format!("read map symbol row: {error}"))?;
        lines.push(format!("- `{name}` ({kind}) — `{path}:{start}-{end}`"));
    }
    lines.push(String::new());
    lines.push("## Indexed Relationships".to_string());
    let mut edges = connection
        .prepare("SELECT from_path, to_path, relation, evidence FROM edges ORDER BY from_path, to_path LIMIT ?1")
        .map_err(|error| format!("prepare map edges: {error}"))?;
    let rows = edges
        .query_map(params![MAP_EDGE_LIMIT as i64], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .map_err(|error| format!("read map edges: {error}"))?;
    for row in rows {
        let (from, to, relation, evidence) =
            row.map_err(|error| format!("read map edge row: {error}"))?;
        lines.push(format!("- `{from}` --[{relation}:{evidence}]--> `{to}`"));
    }
    lines.push(String::new());
    lines.push("## Indexed Tests".to_string());
    let mut tests = connection
        .prepare("SELECT DISTINCT path FROM files WHERE lower(path) LIKE '%test%' OR lower(path) LIKE '%spec%' ORDER BY path LIMIT ?1")
        .map_err(|error| format!("prepare map tests: {error}"))?;
    let test_rows = tests
        .query_map(params![MAP_TEST_LIMIT as i64], |row| {
            row.get::<_, String>(0)
        })
        .map_err(|error| format!("read map tests: {error}"))?;
    for row in test_rows {
        lines.push(format!(
            "- `{}`",
            row.map_err(|error| format!("read map test row: {error}"))?
        ));
    }
    lines.push(String::new());
    lines.push(format!(
        "## Indexed Files (first {MAP_FILE_LIMIT} of {file_count})"
    ));
    let mut files = connection
        .prepare("SELECT path, language FROM files ORDER BY path LIMIT ?1")
        .map_err(|error| format!("prepare map files: {error}"))?;
    let file_rows = files
        .query_map(params![MAP_FILE_LIMIT as i64], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|error| format!("read map files: {error}"))?;
    for row in file_rows {
        let (path, language) = row.map_err(|error| format!("read map file row: {error}"))?;
        lines.push(format!("- `{path}` ({language})"));
    }
    lines.push(String::new());
    lines.push("## Maintenance".to_string());
    lines.push("- Refresh: `keel code-index refresh`".to_string());
    lines.push("- Query: `keel code-search search --query \"...\"`".to_string());
    Ok(lines.join("\n"))
}

fn ensure_schema(connection: &Connection) -> Result<(), IndexAccessError> {
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS files(
                 id INTEGER PRIMARY KEY,
                 path TEXT NOT NULL UNIQUE,
                 language TEXT NOT NULL,
                 hash TEXT NOT NULL,
                 modified_at TEXT NOT NULL,
                 size INTEGER NOT NULL,
                 imports TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS symbols(
                 id INTEGER PRIMARY KEY,
                 file_id INTEGER NOT NULL,
                 path TEXT NOT NULL,
                 kind TEXT NOT NULL,
                 name TEXT NOT NULL,
                 qualified_name TEXT NOT NULL,
                 signature TEXT NOT NULL,
                 documentation TEXT NOT NULL,
                 start_line INTEGER NOT NULL,
                 end_line INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS symbols_name_idx ON symbols(name);
             CREATE INDEX IF NOT EXISTS symbols_path_idx ON symbols(path);
             CREATE TABLE IF NOT EXISTS chunks(
                 id INTEGER PRIMARY KEY,
                 file_id INTEGER NOT NULL,
                 path TEXT NOT NULL,
                 symbol_id INTEGER,
                 kind TEXT NOT NULL,
                 start_line INTEGER NOT NULL,
                 end_line INTEGER NOT NULL,
                 content TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS edges(
                 id INTEGER PRIMARY KEY,
                 from_path TEXT NOT NULL,
                 from_symbol_id INTEGER,
                 to_path TEXT NOT NULL,
                 to_symbol_id INTEGER,
                 relation TEXT NOT NULL,
                 evidence TEXT NOT NULL,
                 UNIQUE(from_path, from_symbol_id, to_path, to_symbol_id, relation)
             );
             CREATE INDEX IF NOT EXISTS edges_from_idx ON edges(from_path, relation);
             CREATE INDEX IF NOT EXISTS edges_to_idx ON edges(to_path, relation);
             CREATE VIRTUAL TABLE IF NOT EXISTS code_entries USING fts5(
                 path UNINDEXED,
                 symbol,
                 qualified_name,
                 signature,
                 documentation,
                 content,
                 kind UNINDEXED,
                 start_line UNINDEXED,
                 end_line UNINDEXED,
                 entity_key UNINDEXED,
                 tokenize = 'porter unicode61 remove_diacritics 2'
             );
             INSERT OR IGNORE INTO meta(key, value) VALUES('schema_version', '1');",
        )
        .map_err(|error| index_access_error("ensure workspace index schema", &error))?;
    let version = meta(connection, "schema_version").unwrap_or_default();
    if version != SCHEMA_VERSION {
        return Err(IndexAccessError::Other(format!(
            "unsupported workspace index schema {version:?}"
        )));
    }
    // Older indexes used `calls` for unresolved name matches. Preserve those
    // derived edges but make their uncertainty explicit after upgrading.
    connection
        .execute(
            "DELETE FROM edges
             WHERE relation = 'calls'
               AND EXISTS (
                   SELECT 1 FROM edges candidate
                   WHERE candidate.from_path = edges.from_path
                     AND candidate.from_symbol_id IS edges.from_symbol_id
                     AND candidate.to_path = edges.to_path
                     AND candidate.to_symbol_id IS edges.to_symbol_id
                     AND candidate.evidence = edges.evidence
                     AND candidate.relation = 'calls-candidate'
               )",
            [],
        )
        .map_err(|error| index_access_error("migrate duplicate candidate edges", &error))?;
    connection
        .execute(
            "UPDATE edges SET relation = 'calls-candidate' WHERE relation = 'calls'",
            [],
        )
        .map_err(|error| index_access_error("migrate candidate edge labels", &error))?;
    Ok(())
}

/// Why a workspace-index access failed. The lock condition stays typed all the
/// way to the degrade-versus-error decision, so a message that merely mentions
/// "busy" can never silently downgrade a real failure into a degraded report.
#[derive(Debug)]
pub(crate) enum IndexAccessError {
    /// Another process holds the SQLite write lock.
    Locked,
    /// Any other failure, already rendered for the operator.
    Other(String),
}

impl std::fmt::Display for IndexAccessError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Locked => write!(formatter, "workspace index is locked by another writer"),
            Self::Other(message) => write!(formatter, "{message}"),
        }
    }
}

impl From<IndexAccessError> for String {
    fn from(error: IndexAccessError) -> Self {
        error.to_string()
    }
}

/// Classify one rusqlite failure from its typed code, never its message.
fn index_access_error(context: &str, error: &rusqlite::Error) -> IndexAccessError {
    if sqlite_lock_error(error) {
        IndexAccessError::Locked
    } else {
        IndexAccessError::Other(format!("{context}: {error}"))
    }
}

fn open_connection(path: &Path) -> Result<Connection, IndexAccessError> {
    let connection = crate::utility::sqlite::open_connection(path).map_err(|error| {
        IndexAccessError::Other(format!("open {}: {error}", display_path(path)))
    })?;
    connection
        .busy_timeout(std::time::Duration::from_millis(250))
        .map_err(|error| index_access_error("set workspace index busy timeout", &error))?;
    for attempt in 0..20 {
        match connection.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;") {
            Ok(()) => break,
            // Retry a held lock, then report it as a lock rather than returning
            // a half-configured connection the caller would fail on later.
            Err(error) if sqlite_lock_error(&error) => {
                if attempt < 19 {
                    std::thread::sleep(std::time::Duration::from_millis(25));
                } else {
                    return Err(IndexAccessError::Locked);
                }
            }
            Err(error) => {
                return Err(index_access_error("configure workspace index", &error));
            }
        }
    }
    connection
        .busy_timeout(std::time::Duration::from_secs(1))
        .map_err(|error| index_access_error("set workspace index transaction timeout", &error))?;
    Ok(connection)
}

fn degraded_refresh_report(root: &Path) -> RefreshReport {
    RefreshReport {
        indexed_commit: git_head(root),
        coverage_complete: false,
        lock_degraded: true,
        ..RefreshReport::default()
    }
}

fn sqlite_lock_error(error: &rusqlite::Error) -> bool {
    matches!(
        error,
        rusqlite::Error::SqliteFailure(code, _)
            if matches!(
                code.code,
                rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
            )
    )
}

fn existing_file_metadata(
    connection: &Connection,
) -> Result<HashMap<String, StoredFileMetadata>, IndexAccessError> {
    let mut statement = connection
        .prepare("SELECT path, hash, modified_at, size FROM files")
        .map_err(|error| index_access_error("prepare existing workspace files", &error))?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })
        .map_err(|error| index_access_error("read existing workspace files", &error))?;
    let mut result = HashMap::new();
    for row in rows {
        let (path, hash, modified_at, size) =
            row.map_err(|error| index_access_error("read existing workspace file", &error))?;
        result.insert(
            path,
            StoredFileMetadata {
                hash,
                modified_at: modified_at.parse::<u128>().unwrap_or(0),
                size: size.max(0) as u64,
            },
        );
    }
    Ok(result)
}

fn delete_file_records(transaction: &rusqlite::Transaction<'_>, path: &str) -> Result<(), String> {
    transaction
        .execute("DELETE FROM code_entries WHERE path = ?1", params![path])
        .map_err(|error| format!("delete indexed search entries {path}: {error}"))?;
    transaction
        .execute("DELETE FROM chunks WHERE path = ?1", params![path])
        .map_err(|error| format!("delete indexed chunks {path}: {error}"))?;
    transaction
        .execute("DELETE FROM symbols WHERE path = ?1", params![path])
        .map_err(|error| format!("delete indexed symbols {path}: {error}"))?;
    transaction
        .execute(
            "DELETE FROM edges WHERE from_path = ?1 OR to_path = ?1",
            params![path],
        )
        .map_err(|error| format!("delete indexed edges {path}: {error}"))?;
    transaction
        .execute("DELETE FROM files WHERE path = ?1", params![path])
        .map_err(|error| format!("delete indexed file {path}: {error}"))?;
    Ok(())
}

struct SearchEntry<'a> {
    path: &'a str,
    symbol: &'a str,
    qualified_name: &'a str,
    signature: &'a str,
    documentation: &'a str,
    content: &'a str,
    kind: &'a str,
    start_line: usize,
    end_line: usize,
    entity_key: &'a str,
}

fn insert_search_entry(
    transaction: &rusqlite::Transaction<'_>,
    entry: SearchEntry<'_>,
) -> Result<(), String> {
    transaction
        .execute(
            "INSERT INTO code_entries(path, symbol, qualified_name, signature, documentation, content, kind, start_line, end_line, entity_key) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                entry.path,
                entry.symbol,
                entry.qualified_name,
                entry.signature,
                entry.documentation,
                entry.content,
                entry.kind,
                entry.start_line as i64,
                entry.end_line as i64,
                entry.entity_key,
            ],
        )
        .map_err(|error| format!("insert code search entry {}: {error}", entry.path))?;
    Ok(())
}

fn exact_candidates(connection: &Connection, terms: &[String]) -> Result<Vec<Candidate>, String> {
    let mut candidates = Vec::new();
    for term in terms {
        let mut statement = connection
            .prepare("SELECT id, path, kind, name, start_line, end_line, signature FROM symbols WHERE lower(name) = lower(?1) OR lower(qualified_name) = lower(?1) ORDER BY path, start_line")
            .map_err(|error| format!("prepare exact symbol search: {error}"))?;
        let rows = statement
            .query_map(params![term], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, String>(6)?,
                ))
            })
            .map_err(|error| format!("read exact symbol search: {error}"))?;
        for row in rows {
            let (id, path, kind, name, start, end, signature) =
                row.map_err(|error| format!("read exact symbol row: {error}"))?;
            candidates.push(Candidate {
                key: format!("symbol:{id}"),
                hit: SearchHit {
                    path,
                    symbol: name,
                    kind,
                    start_line: start.max(1) as usize,
                    end_line: end.max(start).max(1) as usize,
                    score: 0.0,
                    reason: "exact-symbol".to_string(),
                    snippet: signature,
                },
            });
        }
    }
    Ok(candidates)
}

fn fts_candidates(
    connection: &Connection,
    terms: &[String],
    limit: usize,
) -> Result<Vec<Candidate>, String> {
    let query = terms
        .iter()
        .map(|term| format!("\"{}\"*", term.replace('"', "")))
        .collect::<Vec<_>>()
        .join(" OR ");
    let mut statement = connection
        .prepare("SELECT path, symbol, kind, start_line, end_line, entity_key, snippet(code_entries, 5, '[', ']', '…', 20) FROM code_entries WHERE code_entries MATCH ?1 ORDER BY bm25(code_entries) LIMIT ?2")
        .map_err(|error| format!("prepare indexed code search: {error}"))?;
    let rows = statement
        .query_map(params![query, limit.min(MAX_FILES) as i64], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
            ))
        })
        .map_err(|error| format!("read indexed code search: {error}"))?;
    let mut candidates = Vec::new();
    for row in rows {
        let (path, symbol, kind, start, end, key, snippet) =
            row.map_err(|error| format!("read indexed code row: {error}"))?;
        candidates.push(Candidate {
            key,
            hit: SearchHit {
                path,
                symbol,
                kind,
                start_line: start.max(1) as usize,
                end_line: end.max(start).max(1) as usize,
                score: 0.0,
                reason: "fts5".to_string(),
                snippet,
            },
        });
    }
    Ok(candidates)
}

fn path_candidates(
    connection: &Connection,
    terms: &[String],
    limit: usize,
) -> Result<Vec<Candidate>, String> {
    let mut candidates = Vec::new();
    for term in terms {
        let pattern = format!("%{}%", term.to_ascii_lowercase());
        let mut statement = connection
            .prepare(
                "SELECT path, language FROM files WHERE lower(path) LIKE ?1 ORDER BY path LIMIT ?2",
            )
            .map_err(|error| format!("prepare indexed path search: {error}"))?;
        let rows = statement
            .query_map(params![pattern, limit.min(MAX_FILES) as i64], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|error| format!("read indexed path search: {error}"))?;
        for row in rows {
            let (path, language) =
                row.map_err(|error| format!("read indexed path row: {error}"))?;
            candidates.push(Candidate {
                key: format!("file:{path}"),
                hit: SearchHit {
                    path,
                    symbol: String::new(),
                    kind: language,
                    start_line: 1,
                    end_line: 1,
                    score: 0.0,
                    reason: "path".to_string(),
                    snippet: String::new(),
                },
            });
        }
    }
    Ok(candidates)
}
fn graph_candidates(
    connection: &Connection,
    exact: &[Candidate],
    limit: usize,
) -> Result<Vec<Candidate>, String> {
    let mut candidates = Vec::new();
    for candidate in exact.iter().take(limit) {
        let Some(symbol_id) = candidate.key.strip_prefix("symbol:") else {
            continue;
        };
        let mut statement = connection
            .prepare("SELECT to_path, relation, evidence FROM edges WHERE from_symbol_id = ?1 OR to_symbol_id = ?1 UNION ALL SELECT to_path, relation, evidence FROM edges WHERE from_path = (SELECT path FROM symbols WHERE id = ?1) AND relation = 'imports' ORDER BY relation, to_path LIMIT ?2")
            .map_err(|error| format!("prepare graph expansion: {error}"))?;
        let rows = statement
            .query_map(
                params![symbol_id.parse::<i64>().unwrap_or(-1), limit as i64],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .map_err(|error| format!("read graph expansion: {error}"))?;
        for row in rows {
            let (path, relation, evidence) =
                row.map_err(|error| format!("read graph row: {error}"))?;
            candidates.push(Candidate {
                key: format!("file:{path}"),
                hit: SearchHit {
                    path,
                    symbol: String::new(),
                    kind: relation.clone(),
                    start_line: 1,
                    end_line: 1,
                    score: 0.0,
                    reason: format!("graph-{relation}"),
                    snippet: evidence,
                },
            });
        }
    }
    Ok(candidates)
}

fn fuse_candidates_with_status(
    channels: Vec<Vec<Candidate>>,
    limit: usize,
) -> (Vec<SearchHit>, bool) {
    // Exact symbols are authoritative; graph expansion supplies context but
    // must not outrank a direct definition because one file can have many edges.
    let weights = [8.0, 2.0, 1.0, 0.4];
    let mut merged: HashMap<String, SearchHit> = HashMap::new();
    let mut reasons: HashMap<String, BTreeSet<String>> = HashMap::new();
    for (channel_index, channel) in channels.iter().enumerate() {
        for (rank, candidate) in channel.iter().enumerate() {
            let score =
                weights.get(channel_index).copied().unwrap_or(1.0) / (RRF_K + rank as f64 + 1.0);
            let entry = merged
                .entry(candidate.key.clone())
                .or_insert_with(|| candidate.hit.clone());
            entry.score += score;
            reasons
                .entry(candidate.key.clone())
                .or_default()
                .insert(candidate.hit.reason.clone());
            if entry.snippet.is_empty() && !candidate.hit.snippet.is_empty() {
                entry.snippet = candidate.hit.snippet.clone();
            }
            if entry.symbol.is_empty() && !candidate.hit.symbol.is_empty() {
                entry.symbol = candidate.hit.symbol.clone();
            }
        }
    }
    let mut hits: Vec<SearchHit> = merged
        .into_iter()
        .map(|(key, mut hit)| {
            hit.reason = reasons
                .remove(&key)
                .unwrap_or_default()
                .into_iter()
                .collect::<Vec<_>>()
                .join(",");
            hit
        })
        .collect();
    hits.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(left.path.cmp(&right.path))
            .then(left.start_line.cmp(&right.start_line))
    });
    let cap = limit.min(MAX_SEARCH_RESULTS);
    let truncated = hits.len() > cap;
    hits.truncate(cap);
    (hits, truncated)
}

fn collect_source_paths(root: &Path) -> Result<SourcePathCollection, String> {
    let mut collection = SourcePathCollection::default();
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(_) => {
                collection.skipped_unreadable += 1;
                collection
                    .unreadable_prefixes
                    .insert(relative_source_path(root, &directory));
                continue;
            }
        };
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => {
                    collection.skipped_unreadable += 1;
                    collection
                        .unreadable_prefixes
                        .insert(relative_source_path(root, &directory));
                    continue;
                }
            };
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if should_skip(&name, &path) {
                continue;
            }
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(_) => {
                    collection.skipped_unreadable += 1;
                    collection
                        .unreadable_prefixes
                        .insert(relative_source_path(root, &path));
                    continue;
                }
            };
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                stack.push(path);
            } else if file_type.is_file() && is_indexable_file(&path) {
                collection.discovered += 1;
                collection.paths.push(path);
            }
        }
    }
    collection.paths.sort();
    if collection.paths.len() > MAX_FILES {
        collection.skipped_limit = (collection.paths.len() - MAX_FILES) as u64;
        collection.paths.truncate(MAX_FILES);
    }
    Ok(collection)
}

fn collect_source_snapshots(root: &Path, paths: &[PathBuf]) -> SnapshotCollection {
    let mut collection = SnapshotCollection::default();
    for absolute_path in paths {
        let metadata = match fs::metadata(absolute_path) {
            Ok(metadata) => metadata,
            Err(error) => {
                if error.kind() != std::io::ErrorKind::NotFound {
                    collection.skipped_unreadable += 1;
                    collection
                        .unreadable_paths
                        .insert(relative_source_path(root, absolute_path));
                }
                continue;
            }
        };
        if metadata.len() > MAX_FILE_BYTES {
            collection.skipped_too_large += 1;
            continue;
        }
        collection.snapshots.push(SourceSnapshot {
            path: absolute_path
                .strip_prefix(root)
                .unwrap_or(absolute_path)
                .to_string_lossy()
                .replace('\\', "/"),
            modified_at: metadata
                .modified()
                .ok()
                .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
                .map(|value| value.as_millis())
                .unwrap_or(0),
            size: metadata.len(),
        });
    }
    collection
}

fn collect_sources_from_paths(
    root: &Path,
    paths: Vec<PathBuf>,
) -> Result<SourceCollection, String> {
    let mut sources = Vec::new();
    let mut unreadable_paths = BTreeSet::new();
    for absolute_path in paths {
        let metadata = match fs::metadata(&absolute_path) {
            Ok(meta) => meta,
            Err(error) => {
                if error.kind() != std::io::ErrorKind::NotFound {
                    unreadable_paths.insert(relative_source_path(root, &absolute_path));
                }
                continue;
            }
        };
        if metadata.len() > MAX_FILE_BYTES {
            continue;
        }
        let content = match fs::read_to_string(&absolute_path) {
            Ok(c) => c,
            Err(error) => {
                if error.kind() != std::io::ErrorKind::NotFound {
                    unreadable_paths.insert(relative_source_path(root, &absolute_path));
                }
                continue;
            }
        };
        let relative = relative_source_path(root, &absolute_path);
        let language = language_for(&absolute_path).to_string();
        let imports = extract_imports(&language, &content);
        let symbols = extract_symbols(&language, &content);
        sources.push(SourceFile {
            path: relative,
            language,
            hash: stable_hash(&content),
            modified_at: metadata
                .modified()
                .ok()
                .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
                .map(|value| value.as_millis())
                .unwrap_or(0),
            size: metadata.len(),
            content,
            imports,
            symbols,
        });
    }
    Ok(SourceCollection {
        sources,
        unreadable_paths,
    })
}

fn relative_source_path(root: &Path, absolute_path: &Path) -> String {
    absolute_path
        .strip_prefix(root)
        .unwrap_or(absolute_path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn path_is_under_prefix(path: &str, prefix: &str) -> bool {
    prefix.is_empty() || path == prefix || path.starts_with(&format!("{prefix}/"))
}

fn extract_symbols(language: &str, content: &str) -> Vec<ParsedSymbol> {
    let lines: Vec<&str> = content.lines().collect();
    let mut symbols = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        let Some((kind, name)) = symbol_prefix(language, trimmed) else {
            continue;
        };
        let end_line = symbol_end_line(&lines, index, language);
        let documentation = preceding_documentation(&lines, index);
        symbols.push(ParsedSymbol {
            kind: kind.to_string(),
            qualified_name: name.clone(),
            name,
            signature: truncate_utf8(trimmed, 4_000),
            documentation,
            start_line: index + 1,
            end_line,
        });
    }
    symbols
}

fn symbol_prefix(language: &str, line: &str) -> Option<(&'static str, String)> {
    if language == "rust" {
        return rust_symbol_prefix(line);
    }
    if language == "go" {
        if let Some(rest) = line.strip_prefix("func ") {
            let rest = if rest.starts_with('(') {
                rest.find(')')
                    .and_then(|end| rest.get(end + 1..))
                    .unwrap_or("")
                    .trim_start()
            } else {
                rest
            };
            if let Some(name) = symbol_name(rest) {
                return Some(("function", name));
            }
        }
        if let Some(rest) = line.strip_prefix("type ") {
            if let Some(name) = symbol_name(rest) {
                return Some(("type", name));
            }
        }
        return None;
    }
    let prefixes: &[(&str, &str)] = match language {
        "javascript" | "typescript" => &[
            ("export default async function ", "function"),
            ("export default function ", "function"),
            ("export async function ", "function"),
            ("async function ", "function"),
            ("export function ", "function"),
            ("function ", "function"),
            ("export default class ", "class"),
            ("export class ", "class"),
            ("class ", "class"),
            ("export const ", "constant"),
            ("const ", "constant"),
        ],
        "python" => &[
            ("async def ", "function"),
            ("def ", "function"),
            ("class ", "class"),
        ],
        _ => &[],
    };
    prefixes.iter().find_map(|(prefix, kind)| {
        line.strip_prefix(prefix)
            .and_then(symbol_name)
            .map(|name| (*kind, name))
    })
}

fn rust_symbol_prefix(mut line: &str) -> Option<(&'static str, String)> {
    if let Some(rest) = line.strip_prefix("pub(") {
        line = rest.get(rest.find(')')? + 1..)?.trim_start();
    } else if let Some(rest) = line.strip_prefix("pub ") {
        line = rest;
    }
    loop {
        let next = if let Some(rest) = line.strip_prefix("async ") {
            Some(rest)
        } else if let Some(rest) = line.strip_prefix("unsafe ") {
            Some(rest)
        } else if let Some(rest) = line.strip_prefix("const ") {
            Some(rest)
        } else if let Some(rest) = line.strip_prefix("extern ") {
            let quote = rest.find('"')?;
            let closing = rest.get(quote + 1..)?.find('"')? + quote + 2;
            Some(rest.get(closing..)?.trim_start())
        } else {
            None
        };
        let Some(next) = next else { break };
        line = next;
    }
    for (prefix, kind) in [
        ("fn ", "function"),
        ("struct ", "struct"),
        ("enum ", "enum"),
        ("trait ", "trait"),
        ("mod ", "module"),
    ] {
        if let Some(rest) = line.strip_prefix(prefix) {
            if let Some(name) = symbol_name(rest) {
                return Some((kind, name));
            }
        }
    }
    None
}

fn symbol_name(rest: &str) -> Option<String> {
    let name = rest
        .trim_start()
        .chars()
        .take_while(|character| character.is_alphanumeric() || *character == '_')
        .collect::<String>();
    (!name.is_empty()).then_some(name)
}

fn extract_call_names(content: &str) -> Vec<String> {
    let mut calls = BTreeSet::new();
    let chars: Vec<char> = content.chars().collect();
    let mut index = 0;
    while index < chars.len() {
        if chars[index] == '/' && chars.get(index + 1) == Some(&'/') {
            index += 2;
            while index < chars.len() && chars[index] != '\n' {
                index += 1;
            }
            continue;
        }
        if chars[index] == '/' && chars.get(index + 1) == Some(&'*') {
            index += 2;
            while index + 1 < chars.len() && !(chars[index] == '*' && chars[index + 1] == '/') {
                index += 1;
            }
            index = (index + 2).min(chars.len());
            continue;
        }
        if matches!(chars[index], '\'' | '"' | '`') {
            index = skip_quoted(&chars, index);
            continue;
        }
        if chars[index].is_alphanumeric() || chars[index] == '_' {
            let start = index;
            index += 1;
            while index < chars.len() && (chars[index].is_alphanumeric() || chars[index] == '_') {
                index += 1;
            }
            let identifier: String = chars[start..index].iter().collect();
            let mut next = index;
            while next < chars.len() && chars[next].is_whitespace() {
                next += 1;
            }
            if chars.get(next) == Some(&'(') && !is_call_keyword(&identifier) {
                calls.insert(identifier);
            }
            continue;
        }
        // Python comments and Rust attributes are not executable calls.
        if chars[index] == '#' {
            while index < chars.len() && chars[index] != '\n' {
                index += 1;
            }
            continue;
        }
        index += 1;
    }
    calls.into_iter().collect()
}

fn skip_quoted(chars: &[char], mut index: usize) -> usize {
    let quote = chars[index];
    index += 1;
    let mut escaped = false;
    while index < chars.len() {
        let character = chars[index];
        index += 1;
        if escaped {
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if character == quote {
            break;
        }
    }
    index
}

fn is_call_keyword(value: &str) -> bool {
    matches!(
        value,
        "if" | "for" | "while" | "match" | "loop" | "fn" | "function" | "switch"
    )
}

#[derive(Default)]
struct BraceScanState {
    depth: i32,
    saw_open: bool,
    block_comment: bool,
    quote: Option<char>,
    raw_hashes: Option<usize>,
    escaped: bool,
}

fn raw_string_hashes(chars: &[char], index: usize) -> Option<usize> {
    if chars.get(index) != Some(&'r') {
        return None;
    }
    let mut cursor = index + 1;
    let mut hashes = 0;
    while chars.get(cursor) == Some(&'#') {
        hashes += 1;
        cursor += 1;
    }
    (chars.get(cursor) == Some(&'"')).then_some(hashes)
}

fn scan_braces(line: &str, language: &str, state: &mut BraceScanState) {
    let chars: Vec<char> = line.chars().collect();
    let mut index = 0;
    while index < chars.len() {
        if let Some(hashes) = state.raw_hashes {
            if chars[index] == '"'
                && (0..hashes).all(|offset| chars.get(index + 1 + offset) == Some(&'#'))
            {
                state.raw_hashes = None;
                index += hashes + 1;
            } else {
                index += 1;
            }
            continue;
        }
        if let Some(quote) = state.quote {
            let character = chars[index];
            index += 1;
            if state.escaped {
                state.escaped = false;
            } else if character == '\\' {
                state.escaped = true;
            } else if character == quote {
                state.quote = None;
            }
            continue;
        }
        if state.block_comment {
            if chars[index] == '*' && chars.get(index + 1) == Some(&'/') {
                state.block_comment = false;
                index += 2;
            } else {
                index += 1;
            }
            continue;
        }
        if chars[index] == '/' && chars.get(index + 1) == Some(&'/') {
            break;
        }
        if chars[index] == '/' && chars.get(index + 1) == Some(&'*') {
            state.block_comment = true;
            index += 2;
            continue;
        }
        if language == "python" && chars[index] == '#' {
            break;
        }
        if language == "rust" {
            if let Some(hashes) = raw_string_hashes(&chars, index) {
                state.raw_hashes = Some(hashes);
                index += hashes + 2;
                continue;
            }
        }
        if matches!(chars[index], '\'' | '"' | '`') {
            state.quote = Some(chars[index]);
            state.escaped = false;
            index += 1;
            continue;
        }
        match chars[index] {
            '{' => {
                state.depth += 1;
                state.saw_open = true;
            }
            '}' if state.saw_open => state.depth -= 1,
            _ => {}
        }
        index += 1;
    }
}

fn symbol_end_line(lines: &[&str], start: usize, language: &str) -> usize {
    let start_line = lines[start];
    if language == "python" {
        let indent = start_line.len() - start_line.trim_start().len();
        for (offset, line) in lines.iter().enumerate().skip(start + 1) {
            if !line.trim().is_empty() {
                let next_indent = line.len() - line.trim_start().len();
                if next_indent <= indent {
                    return offset;
                }
            }
        }
        return lines.len();
    }

    let mut state = BraceScanState::default();
    for (offset, line) in lines.iter().enumerate().skip(start) {
        scan_braces(line, language, &mut state);
        if state.saw_open && state.depth <= 0 {
            return offset + 1;
        }
        if !state.saw_open && offset > start && symbol_prefix(language, line.trim_start()).is_some()
        {
            // A body-less declaration is one line; do not absorb later symbols
            // while searching for a brace that will never arrive.
            return offset;
        }
    }
    if state.saw_open {
        lines.len()
    } else {
        (start + 1).min(lines.len())
    }
}

fn preceding_documentation(lines: &[&str], start: usize) -> String {
    let mut docs = Vec::new();
    let mut index = start;
    while index > 0 {
        let line = lines[index - 1].trim();
        if line.starts_with("///") || line.starts_with("//!") || line.starts_with('#') {
            docs.push(line.to_string());
            index -= 1;
        } else {
            break;
        }
    }
    docs.reverse();
    docs.join("\n")
}

fn extract_imports(language: &str, content: &str) -> Vec<String> {
    let mut imports = BTreeSet::new();
    for line in content.lines().map(str::trim_start) {
        let candidate = match language {
            "rust" if line.starts_with("mod ") || line.starts_with("pub mod ") => {
                let name = line
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or("")
                    .trim_end_matches(';');
                if name.is_empty() {
                    String::new()
                } else {
                    format!("mod:{name}")
                }
            }
            "rust" if line.starts_with("use ") => line
                .trim_start_matches("use ")
                .split("::")
                .next()
                .unwrap_or("")
                .to_string(),
            "javascript" | "typescript" if line.starts_with("import ") => line.to_string(),
            "javascript" | "typescript" if line.contains("require(") => line.to_string(),
            "python" if line.starts_with("from ") => {
                line.split_whitespace().nth(1).unwrap_or("").to_string()
            }
            "python" if line.starts_with("import ") => {
                line.split_whitespace().nth(1).unwrap_or("").to_string()
            }
            "go" if line.starts_with('"') => line.trim_matches('"').to_string(),
            _ => String::new(),
        };
        if !candidate.is_empty() {
            imports.insert(
                candidate
                    .trim_matches(&['"', '\'', ';', '(', ')'][..])
                    .to_string(),
            );
        }
    }
    imports.into_iter().collect()
}

fn resolve_import(from: &str, import: &str, paths: &BTreeSet<String>) -> Option<String> {
    let import = import.trim();
    if import.is_empty() {
        return None;
    }
    let candidate = if import.starts_with('.') {
        let base = from
            .rsplit_once('/')
            .map(|(parent, _)| parent)
            .unwrap_or("");
        normalize_path(&format!("{base}/{import}"))
    } else if import.starts_with("mod:") {
        let name = import.trim_start_matches("mod:");
        let base = from
            .rsplit_once('/')
            .map(|(parent, _)| parent)
            .unwrap_or("");
        normalize_path(&format!("{base}/{name}"))
    } else {
        normalize_path(import)
    };
    [
        candidate.clone(),
        format!("{candidate}.rs"),
        format!("{candidate}.py"),
        format!("{candidate}.js"),
        format!("{candidate}.ts"),
        format!("{candidate}/mod.rs"),
        format!("{candidate}/index.ts"),
    ]
    .into_iter()
    .find(|option| paths.contains(option))
}

fn normalize_path(value: &str) -> String {
    let normalized = value.replace('\\', "/");
    let mut parts = Vec::new();
    for part in normalized.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    parts.join("/")
}

fn language_for(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
    {
        "rs" => "rust",
        "js" | "jsx" | "mjs" | "cjs" => "javascript",
        "ts" | "tsx" => "typescript",
        "py" => "python",
        "go" => "go",
        "md" => "markdown",
        "toml" => "toml",
        "json" => "json",
        "yaml" | "yml" => "yaml",
        _ => "text",
    }
}

fn is_indexable_file(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|value| value.to_str())
            .unwrap_or(""),
        "rs" | "js"
            | "jsx"
            | "mjs"
            | "cjs"
            | "ts"
            | "tsx"
            | "py"
            | "go"
            | "md"
            | "toml"
            | "json"
            | "yaml"
            | "yml"
    )
}

fn should_skip(name: &str, path: &Path) -> bool {
    if name.starts_with('.') && name != ".github" {
        return true;
    }
    if path.is_dir() {
        matches!(
            name,
            "node_modules"
                | "target"
                | "vendor"
                | ".venv"
                | "venv"
                | "build"
                | "dist"
                | "tmp"
                | "coverage"
                | ".git"
                | ".idea"
                | ".vscode"
                | "__pycache__"
        )
    } else {
        name.ends_with(".lock")
            || name.ends_with(".min.js")
            || name.ends_with(".map")
            || name.ends_with(".log")
    }
}

fn canonical_workspace_root(root: &Path) -> Result<PathBuf, String> {
    fs::canonicalize(root)
        .map_err(|error| format!("canonicalize workspace {}: {error}", display_path(root)))
}

fn stable_hash(value: &str) -> String {
    crate::utility::hashing::fnv1a64_hex(value)
}

fn truncate_utf8(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n[truncated]", &value[..end])
}

fn query_terms(query: &str) -> Vec<String> {
    query
        .split(|character: char| !character.is_alphanumeric() && character != '_')
        .filter_map(|term| {
            let lowered = term.to_ascii_lowercase();
            if lowered.len() >= 2 {
                Some(lowered)
            } else {
                None
            }
        })
        .collect()
}

fn meta(connection: &Connection, key: &str) -> Option<String> {
    connection
        .query_row(
            "SELECT value FROM meta WHERE key = ?1",
            params![key],
            |row| row.get(0),
        )
        .optional()
        .ok()
        .flatten()
}

fn count(connection: &Connection, table: &str) -> Result<u64, String> {
    let sql = format!("SELECT COUNT(*) FROM {table}");
    connection
        .query_row(&sql, [], |row| row.get::<_, i64>(0))
        .map(|value| value.max(0) as u64)
        .map_err(|error| format!("count {table}: {error}"))
}

fn next_generation(transaction: &rusqlite::Transaction<'_>) -> Result<u64, String> {
    let current = transaction
        .query_row(
            "SELECT value FROM meta WHERE key = 'generation'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|error| format!("read generation: {error}"))?
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    Ok(current.saturating_add(1))
}

fn git_head(root: &Path) -> String {
    Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};

    fn temp_workspace(label: &str) -> (crate::test_support::TestTempDir, PathBuf) {
        let root = crate::test_support::unique_temp_dir(&format!("keel-index-{label}"));
        let home = root.join("home");
        fs::create_dir_all(root.join("src")).expect("workspace");
        fs::create_dir_all(&home).expect("home");
        (root, home)
    }

    /// The degrade-versus-error decision must come from the typed SQLite code.
    /// A message that merely contains "busy" is not a lock, so classifying by
    /// message text would silently downgrade a real failure.
    #[test]
    fn lock_classification_uses_the_typed_code_not_the_message_text() {
        let busy = rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
            Some("database is locked".to_string()),
        );
        assert!(matches!(
            index_access_error("ctx", &busy),
            IndexAccessError::Locked
        ));

        let locked = rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_LOCKED),
            None,
        );
        assert!(matches!(
            index_access_error("ctx", &locked),
            IndexAccessError::Locked
        ));

        // Non-lock failures must stay errors even when the text mentions "busy".
        let misleading = rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_ERROR),
            Some("the writer is busy rebuilding this index".to_string()),
        );
        match index_access_error("ctx", &misleading) {
            IndexAccessError::Locked => panic!("a non-lock failure must not degrade as a lock"),
            IndexAccessError::Other(message) => assert!(message.contains("busy rebuilding")),
        }
    }

    /// §17/§29 concurrency: a second refresh while a writer holds the index lock
    /// must report a degraded, non-authoritative index rather than either
    /// succeeding silently or failing the caller.
    #[test]
    fn concurrent_refresh_degrades_instead_of_failing_while_a_writer_holds_the_lock() {
        let (root, home) = temp_workspace("lock-degrade");
        fs::write(root.join("src/main.rs"), "pub fn only() {}\n").expect("source");
        let home_flag = home.to_string_lossy().to_string();
        let first = refresh(&root, &home_flag, true).expect("initial refresh");
        assert!(first.coverage_complete);

        // Hold the write lock from an independent connection for the assertion
        // window, exactly as a concurrent reindex would.
        let index_path = database_path(&root, &home_flag).expect("index path");
        let holder = crate::utility::sqlite::open_connection(&index_path).expect("holder open");
        holder
            .busy_timeout(std::time::Duration::from_millis(50))
            .expect("holder busy timeout");
        holder
            .execute_batch("BEGIN IMMEDIATE")
            .expect("holder takes the write lock");

        let contended = refresh(&root, &home_flag, false).expect("contended refresh must not fail");

        holder.execute_batch("ROLLBACK").expect("holder releases");

        assert!(
            contended.lock_degraded,
            "a contended refresh must report the degraded lock state: {contended:?}"
        );
        assert!(
            !contended.coverage_complete,
            "a degraded refresh must not claim complete coverage: {contended:?}"
        );

        // The index stays usable and a later uncontended refresh recovers fully.
        let recovered = refresh(&root, &home_flag, false).expect("recovered refresh");
        assert!(!recovered.lock_degraded);
        assert!(recovered.coverage_complete);
    }

    #[test]
    fn refresh_indexes_symbols_chunks_and_edges() {
        let (root, home) = temp_workspace("build");
        fs::write(
            root.join("src/main.rs"),
            "mod helper;\nfn main() { helper(); }\n",
        )
        .expect("main");
        fs::write(root.join("src/helper.rs"), "pub fn helper() {}\n").expect("helper");
        let report = refresh(&root, &home.to_string_lossy(), true).expect("refresh");
        assert_eq!(report.files_indexed, 2);
        assert!(report.symbols_indexed >= 2);
        assert!(report.chunks_indexed >= 2);
        assert!(report.edges_indexed >= 2);
        let status = status(&root, &home.to_string_lossy()).expect("status");
        assert_eq!(status.file_count, 2);
        assert!(!status.stale);
        let index_path = database_path(&root, &home.to_string_lossy()).expect("index path");
        let first_updated = meta(
            &open_connection(&index_path).expect("open index"),
            "updated_at_millis",
        );
        let second = refresh(&root, &home.to_string_lossy(), false).expect("no-op refresh");
        let second_updated = meta(
            &open_connection(&index_path).expect("reopen index"),
            "updated_at_millis",
        );
        assert_eq!(second.generation, report.generation);
        assert_eq!(second.files_added, 0);
        assert_eq!(second.files_updated, 0);
        assert_eq!(second.files_removed, 0);
        assert_eq!(second.edges_indexed, 0);
        assert_eq!(first_updated, second_updated);
    }

    #[test]
    fn deleting_a_source_rebuilds_relationships_without_stale_edges() {
        let (root, home) = temp_workspace("delete-source");
        fs::write(
            root.join("src/main.rs"),
            "mod helper;\nfn main() { helper::helper(); }\n",
        )
        .expect("main");
        fs::write(root.join("src/helper.rs"), "pub fn helper() {}\n").expect("helper");
        refresh(&root, &home.to_string_lossy(), true).expect("initial refresh");
        let index_path = database_path(&root, &home.to_string_lossy()).expect("index path");
        let connection = open_connection(&index_path).expect("open index");
        let edges_before: i64 = connection
            .query_row("SELECT COUNT(*) FROM edges", [], |row| row.get(0))
            .expect("edge count");
        assert!(edges_before > 0);
        drop(connection);

        fs::remove_file(root.join("src/helper.rs")).expect("remove helper");
        let report = refresh(&root, &home.to_string_lossy(), false).expect("delete refresh");
        assert_eq!(report.files_removed, 1);
        let connection = open_connection(&index_path).expect("reopen index");
        let stale_edges: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM edges WHERE to_path = 'src/helper.rs'",
                [],
                |row| row.get(0),
            )
            .expect("edge count after deletion");
        assert_eq!(stale_edges, 0);
    }

    #[test]
    fn database_path_uses_the_canonical_workspace_lane() {
        let (root, home) = temp_workspace("canonical-lane");
        let canonical_root = canonical_workspace_root(&root).expect("canonical root");
        let path = database_path(&canonical_root, &home.to_string_lossy()).expect("index path");
        let actual_lane = path
            .parent()
            .and_then(Path::parent)
            .and_then(Path::file_name)
            .and_then(|value| value.to_str())
            .expect("workspace lane");
        let expected = crate::utility::system_map::workspace_key(&display_path(&canonical_root));

        assert_eq!(actual_lane, expected);
    }

    #[test]
    fn concurrent_refreshes_wait_for_the_active_writer() {
        let (root, home) = temp_workspace("concurrent-refresh");
        for index in 0..40 {
            fs::write(
                root.join("src").join(format!("item-{index:02}.rs")),
                format!("pub fn item_{index:02}() {{}}\n"),
            )
            .expect("source");
        }
        let root_path = root.to_path_buf();
        let writers = 8;
        let barrier = Arc::new(Barrier::new(writers));
        let mut threads = Vec::new();
        for _ in 0..writers {
            let thread_root = root_path.clone();
            let thread_home = home.clone();
            let thread_barrier = Arc::clone(&barrier);
            threads.push(std::thread::spawn(move || {
                thread_barrier.wait();
                refresh(&thread_root, &thread_home.to_string_lossy(), true)
            }));
        }
        for thread in threads {
            let result = thread.join().expect("refresh thread");
            assert!(result.is_ok(), "parallel refresh failed: {result:?}");
        }
    }

    #[test]
    fn search_fuses_exact_symbol_path_and_fts_results() {
        let (root, home) = temp_workspace("search");
        fs::write(root.join("src/main.rs"), "pub fn dispatch_request() {}\n").expect("source");
        let hits = search_filtered(&root, &home.to_string_lossy(), "dispatch_request", 10, None)
            .expect("search");
        assert!(!hits.is_empty());
        assert_eq!(hits[0].path, "src/main.rs");
        assert_eq!(hits[0].symbol, "dispatch_request");
        assert!(hits[0].reason.contains("exact-symbol"));
    }

    #[test]
    fn filtered_search_finds_match_below_unfiltered_result_cap() {
        let (root, home) = temp_workspace("filtered-search");
        for index in 0..60 {
            fs::write(
                root.join("src").join(format!("rank-{index:02}.rs")),
                "// deep_filter_token\n",
            )
            .expect("ranked source");
        }
        fs::create_dir_all(root.join("zzz")).expect("filtered directory");
        fs::write(root.join("zzz/target.rs"), "// deep_filter_token\n").expect("filtered source");

        let unfiltered = search_filtered(
            &root,
            &home.to_string_lossy(),
            "deep_filter_token",
            MAX_SEARCH_RESULTS,
            None,
        )
        .expect("unfiltered search");
        assert!(
            unfiltered.iter().all(|hit| hit.path != "zzz/target.rs"),
            "fixture must place the target below the unfiltered cap"
        );

        let hits = search_filtered(
            &root,
            &home.to_string_lossy(),
            "deep_filter_token",
            10,
            Some("zzz/"),
        )
        .expect("filtered search");

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "zzz/target.rs");
    }

    #[test]
    fn refresh_updates_only_changed_file_and_removes_deleted_file() {
        let (root, home) = temp_workspace("refresh");
        fs::write(root.join("src/old.rs"), "pub fn old() {}\n").expect("old");
        let first = refresh(&root, &home.to_string_lossy(), true).expect("first");
        assert_eq!(first.files_added, 1);
        fs::remove_file(root.join("src/old.rs")).expect("remove");
        fs::write(root.join("src/new.rs"), "pub fn new() {}\n").expect("new");
        let second = refresh(&root, &home.to_string_lossy(), false).expect("second");
        assert_eq!(second.files_removed, 1);
        assert_eq!(second.files_added, 1);
        let hits =
            search_filtered(&root, &home.to_string_lossy(), "old", 10, None).expect("search");
        assert!(hits.is_empty());
    }

    #[test]
    fn map_contains_exact_symbol_ranges_and_generation() {
        let (root, home) = temp_workspace("map");
        fs::write(root.join("src/main.rs"), "pub fn main() {}\n").expect("source");
        let map = render_map(&root, &home.to_string_lossy()).expect("map");
        assert!(map.contains("src/main.rs"));
        assert!(map.contains("main"));
        assert!(map.contains("generation:"));
    }

    #[test]
    fn multiline_symbol_ranges_include_body_but_ignore_braces_in_text_and_comments() {
        let content = "pub(crate) fn multi(\n    value: usize,\n) {\n    let text = \"} {\";\n    let raw = r#\"} {\"#;\n    // } {\n}\n\npub fn next() {}\n";
        let symbols = extract_symbols("rust", content);

        let multi = symbols
            .iter()
            .find(|symbol| symbol.name == "multi")
            .expect("multiline function indexed");
        assert_eq!(multi.start_line, 1);
        assert_eq!(multi.end_line, 7);
        assert_eq!(
            symbols
                .iter()
                .find(|symbol| symbol.name == "next")
                .unwrap()
                .start_line,
            9
        );
    }

    #[test]
    fn symbol_prefix_covers_visibility_qualifiers_and_go_methods() {
        assert_eq!(
            symbol_prefix("rust", "pub(crate) async fn load_data()"),
            Some(("function", "load_data".to_string()))
        );
        assert_eq!(
            symbol_prefix("rust", "pub(in crate::api) fn endpoint()"),
            Some(("function", "endpoint".to_string()))
        );
        assert_eq!(
            symbol_prefix("go", "func (s *Server) Serve() error"),
            Some(("function", "Serve".to_string()))
        );
    }

    #[test]
    fn call_name_extraction_ignores_comments_and_string_literals() {
        let calls = extract_call_names(
            "helper(); // fake_call()\nlet text = \"other_call()\"; /* hidden_call() */\n",
        );

        assert_eq!(calls, vec!["helper".to_string()]);
    }

    #[test]
    fn call_edges_are_labeled_as_candidates() {
        let (root, home) = temp_workspace("candidate-edges");
        fs::write(
            root.join("src/main.rs"),
            "pub fn main() { helper(); let _ = \"helper()\"; }\n",
        )
        .expect("main");
        fs::write(root.join("src/helper.rs"), "pub fn helper() {}\n").expect("helper");
        refresh(&root, &home.to_string_lossy(), true).expect("refresh");
        let index_path = database_path(&root, &home.to_string_lossy()).expect("index path");
        let connection = open_connection(&index_path).expect("open index");
        let relation: String = connection
            .query_row(
                "SELECT relation FROM edges WHERE relation LIKE 'calls-%' LIMIT 1",
                [],
                |row| row.get(0),
            )
            .expect("candidate call edge");
        assert_eq!(relation, "calls-candidate");
    }

    #[test]
    fn search_metadata_reports_result_truncation() {
        let candidates = (0..(MAX_SEARCH_RESULTS + 1))
            .map(|index| Candidate {
                key: format!("key-{index}"),
                hit: SearchHit {
                    path: format!("src/{index}.rs"),
                    symbol: String::new(),
                    kind: "file".to_string(),
                    start_line: 1,
                    end_line: 1,
                    score: 0.0,
                    reason: "test".to_string(),
                    snippet: String::new(),
                },
            })
            .collect::<Vec<_>>();
        let result = fuse_candidates_with_status(vec![candidates], 80);

        assert!(result.1, "capped results must report truncation");
        assert_eq!(result.0.len(), MAX_SEARCH_RESULTS);
    }

    #[test]
    fn refresh_reports_oversized_files_incomplete() {
        let (root, home) = temp_workspace("coverage");
        fs::write(root.join("src/ok.rs"), "pub fn ok() {}\n").expect("ok source");
        fs::write(
            root.join("src/oversized.rs"),
            vec![b'x'; (MAX_FILE_BYTES + 1) as usize],
        )
        .expect("oversized source");

        let report = refresh(&root, &home.to_string_lossy(), true).expect("refresh");

        assert_eq!(report.files_discovered, 2);
        assert_eq!(report.files_skipped_too_large, 1);
        assert!(!report.coverage_complete);
    }

    #[test]
    fn map_places_architecture_before_large_file_inventory() {
        let (root, home) = temp_workspace("map-order");
        fs::write(root.join("src/main.rs"), "pub fn main() {}\n").expect("source");
        let map = render_map(&root, &home.to_string_lossy()).expect("map");

        assert!(
            map.find("## Indexed Symbols").expect("symbols section")
                < map.find("## Indexed Files").expect("files section")
        );
        assert!(map.contains("## Index Coverage"));
    }
}
