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

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::runtime::{display_path, resolve_claude_home};

const SCHEMA_VERSION: &str = "1";
const MAX_FILES: usize = 20_000;
const MAX_FILE_BYTES: u64 = 2_000_000;
const MAX_CHUNK_BYTES: usize = 32_000;
const MAX_SEARCH_RESULTS: usize = 50;
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
    let raw_workspace = display_path(workspace_root);
    let slug = crate::utility::system_map::bounded_slug(&raw_workspace, 64);
    Ok(home
        .join("memories")
        .join("workspaces")
        .join(if slug.is_empty() { "workspace" } else { &slug })
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
    let mut connection = open_connection(&path)?;
    ensure_schema(&connection)?;
    let sources = collect_sources(&root)?;
    let existing = existing_file_metadata(&connection)?;
    let previous_commit = meta(&connection, "indexed_commit");
    let mut report = RefreshReport::default();

    let transaction = connection
        .transaction()
        .map_err(|error| format!("begin workspace index refresh: {error}"))?;
    let mut active_paths = BTreeSet::new();
    for source in &sources {
        active_paths.insert(source.path.clone());
        let unchanged = existing
            .get(&source.path)
            .map(|metadata| metadata == &(source.hash.clone(), source.size))
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

    transaction
        .execute("DELETE FROM edges", [])
        .map_err(|error| format!("clear workspace edges: {error}"))?;
    let path_set: BTreeSet<String> = sources.iter().map(|source| source.path.clone()).collect();
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
                            "INSERT OR IGNORE INTO edges(from_path, from_symbol_id, to_path, to_symbol_id, relation, evidence) VALUES (?1, ?2, ?3, ?4, 'calls', ?5)",
                            params![source.path, from_id, target_path, target_id, format!("{called_name}(")],
                        )
                        .map_err(|error| format!("insert call edge {}: {error}", source.path))?;
                    report.edges_indexed += 1;
                }
            }
        }
    }
    let indexed_commit = git_head(&root);
    let commit_changed = previous_commit.as_deref() != Some(indexed_commit.as_str());
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
            "INSERT OR REPLACE INTO meta(key, value) VALUES ('generation', ?1), ('indexed_commit', ?2), ('workspace_root', ?3), ('updated_at_millis', ?4)",
            params![generation.to_string(), indexed_commit, root.to_string_lossy().to_string(), now_millis().to_string()],
        )
        .map_err(|error| format!("stamp workspace index: {error}"))?;
    transaction
        .commit()
        .map_err(|error| format!("commit workspace index: {error}"))?;
    report.files_indexed = sources.len() as u64;
    report.generation = generation;
    report.indexed_commit = indexed_commit;
    Ok(report)
}

pub fn search(
    workspace_root: &Path,
    claude_home_flag: &str,
    query: &str,
    limit: usize,
) -> Result<Vec<SearchHit>, String> {
    if query.trim().is_empty() {
        return Ok(Vec::new());
    }
    refresh(workspace_root, claude_home_flag, false)?;
    let root = canonical_workspace_root(workspace_root)?;
    let path = database_path(&root, claude_home_flag)?;
    let connection = open_connection(&path)?;
    let terms = query_terms(query);
    if terms.is_empty() {
        return Ok(Vec::new());
    }
    let exact = exact_candidates(&connection, &terms)?;
    let channels = vec![
        exact.clone(),
        fts_candidates(&connection, &terms, limit.max(10))?,
        path_candidates(&connection, &terms, limit.max(10))?,
        graph_candidates(&connection, &exact, limit.max(10))?,
    ];
    Ok(fuse_candidates(channels, limit))
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
    })
}

pub fn render_map(workspace_root: &Path, claude_home_flag: &str) -> Result<String, String> {
    refresh(workspace_root, claude_home_flag, false)?;
    let root = canonical_workspace_root(workspace_root)?;
    let path = database_path(&root, claude_home_flag)?;
    let connection = open_connection(&path)?;
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
        "## Indexed Files".to_string(),
    ];
    let mut statement = connection
        .prepare("SELECT path, language FROM files ORDER BY path LIMIT 500")
        .map_err(|error| format!("prepare map files: {error}"))?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|error| format!("read map files: {error}"))?;
    for row in rows {
        let (path, language) = row.map_err(|error| format!("read map file row: {error}"))?;
        lines.push(format!("- `{path}` ({language})"));
    }
    lines.push(String::new());
    lines.push("## Indexed Symbols".to_string());
    let mut symbols = connection
        .prepare("SELECT path, kind, qualified_name, start_line, end_line FROM symbols ORDER BY path, start_line LIMIT 1000")
        .map_err(|error| format!("prepare map symbols: {error}"))?;
    let rows = symbols
        .query_map([], |row| {
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
        .prepare("SELECT from_path, to_path, relation, evidence FROM edges ORDER BY from_path, to_path LIMIT 1000")
        .map_err(|error| format!("prepare map edges: {error}"))?;
    let rows = edges
        .query_map([], |row| {
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
        .prepare("SELECT DISTINCT path FROM files WHERE lower(path) LIKE '%test%' OR lower(path) LIKE '%spec%' ORDER BY path LIMIT 200")
        .map_err(|error| format!("prepare map tests: {error}"))?;
    let test_rows = tests
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| format!("read map tests: {error}"))?;
    for row in test_rows {
        lines.push(format!(
            "- `{}`",
            row.map_err(|error| format!("read map test row: {error}"))?
        ));
    }
    lines.push(String::new());
    lines.push("## Ownership Sources".to_string());
    let mut owners = connection
        .prepare("SELECT path FROM files WHERE lower(path) LIKE '%agents.md' OR lower(path) LIKE '%claude.md' OR lower(path) LIKE '%codeowners' OR lower(path) LIKE '%contributing.md' ORDER BY path LIMIT 200")
        .map_err(|error| format!("prepare map ownership: {error}"))?;
    let owner_rows = owners
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| format!("read map ownership: {error}"))?;
    for row in owner_rows {
        lines.push(format!(
            "- `{}`",
            row.map_err(|error| format!("read map ownership row: {error}"))?
        ));
    }
    lines.push(String::new());
    lines.push("## Maintenance".to_string());
    lines.push("- Refresh: `keel code-index refresh`".to_string());
    lines.push("- Query: `keel code-search search --query \"...\"`".to_string());
    Ok(lines.join("\n"))
}

fn ensure_schema(connection: &Connection) -> Result<(), String> {
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
        .map_err(|error| format!("ensure workspace index schema: {error}"))?;
    let version = meta(connection, "schema_version").unwrap_or_default();
    if version != SCHEMA_VERSION {
        return Err(format!("unsupported workspace index schema {version:?}"));
    }
    Ok(())
}

fn open_connection(path: &Path) -> Result<Connection, String> {
    let connection = crate::utility::sqlite::open_connection(path)
        .map_err(|error| format!("open {}: {error}", display_path(path)))?;
    connection
        .busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|error| format!("set workspace index busy timeout: {error}"))?;
    connection
        .execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")
        .map_err(|error| format!("configure workspace index: {error}"))?;
    Ok(connection)
}

fn existing_file_metadata(
    connection: &Connection,
) -> Result<HashMap<String, (String, u64)>, String> {
    let mut statement = connection
        .prepare("SELECT path, hash, size FROM files")
        .map_err(|error| format!("prepare existing workspace files: {error}"))?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get(0)?, row.get::<_, String>(1)?, row.get::<_, i64>(2)?))
        })
        .map_err(|error| format!("read existing workspace files: {error}"))?;
    let mut result = HashMap::new();
    for row in rows {
        let (path, hash, size) =
            row.map_err(|error| format!("read existing workspace file: {error}"))?;
        result.insert(path, (hash, size.max(0) as u64));
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
        .query_map(
            params![query, limit.min(MAX_SEARCH_RESULTS) as i64],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                ))
            },
        )
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
            .query_map(
                params![pattern, limit.min(MAX_SEARCH_RESULTS) as i64],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
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

fn fuse_candidates(channels: Vec<Vec<Candidate>>, limit: usize) -> Vec<SearchHit> {
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
    hits.truncate(limit.min(MAX_SEARCH_RESULTS));
    hits
}

fn collect_sources(root: &Path) -> Result<Vec<SourceFile>, String> {
    let mut paths = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let entries = fs::read_dir(&directory)
            .map_err(|error| format!("read {}: {error}", display_path(&directory)))?;
        for entry in entries.flatten() {
            if paths.len() >= MAX_FILES {
                break;
            }
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if should_skip(&name, &path) {
                continue;
            }
            let file_type = entry
                .file_type()
                .map_err(|error| format!("read file type {}: {error}", display_path(&path)))?;
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                stack.push(path);
            } else if file_type.is_file() && is_indexable_file(&path) {
                paths.push(path);
            }
        }
    }
    paths.sort();
    let mut sources = Vec::new();
    for absolute_path in paths {
        let metadata = fs::metadata(&absolute_path)
            .map_err(|error| format!("stat {}: {error}", display_path(&absolute_path)))?;
        if metadata.len() > MAX_FILE_BYTES {
            continue;
        }
        let content = fs::read_to_string(&absolute_path)
            .map_err(|error| format!("read {}: {error}", display_path(&absolute_path)))?;
        let relative = absolute_path
            .strip_prefix(root)
            .unwrap_or(&absolute_path)
            .to_string_lossy()
            .replace('\\', "/");
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
    Ok(sources)
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
    let prefixes: &[(&str, &str)] = match language {
        "rust" => &[
            ("pub async fn ", "function"),
            ("async fn ", "function"),
            ("pub fn ", "function"),
            ("fn ", "function"),
            ("pub struct ", "struct"),
            ("struct ", "struct"),
            ("pub enum ", "enum"),
            ("enum ", "enum"),
            ("pub trait ", "trait"),
            ("trait ", "trait"),
            ("pub mod ", "module"),
            ("mod ", "module"),
        ],
        "javascript" | "typescript" => &[
            ("export async function ", "function"),
            ("async function ", "function"),
            ("export function ", "function"),
            ("function ", "function"),
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
        "go" => &[("func ", "function"), ("type ", "type")],
        _ => &[],
    };
    for (prefix, kind) in prefixes {
        if let Some(rest) = line.strip_prefix(prefix) {
            let name = rest
                .trim_start()
                .chars()
                .take_while(|character| character.is_alphanumeric() || *character == '_')
                .collect::<String>();
            if !name.is_empty() {
                return Some((kind, name));
            }
        }
    }
    None
}

fn extract_call_names(content: &str) -> Vec<String> {
    let mut calls = BTreeSet::new();
    let mut identifier = String::new();
    let chars = content.chars();
    for character in chars {
        if character.is_alphanumeric() || character == '_' {
            identifier.push(character);
            continue;
        }
        if character == '(' && !identifier.is_empty() && !is_call_keyword(&identifier) {
            calls.insert(identifier.clone());
        }
        identifier.clear();
    }
    calls.into_iter().collect()
}

fn is_call_keyword(value: &str) -> bool {
    matches!(
        value,
        "if" | "for" | "while" | "match" | "loop" | "fn" | "function" | "switch"
    )
}

fn symbol_end_line(lines: &[&str], start: usize, language: &str) -> usize {
    let start_line = lines[start];
    let balance = start_line
        .chars()
        .filter(|character| *character == '{')
        .count() as i32
        - start_line
            .chars()
            .filter(|character| *character == '}')
            .count() as i32;
    if balance > 0 {
        let mut current = balance;
        for (offset, line) in lines.iter().enumerate().skip(start + 1) {
            current += line.chars().filter(|character| *character == '{').count() as i32;
            current -= line.chars().filter(|character| *character == '}').count() as i32;
            if current <= 0 {
                return offset + 1;
            }
        }
    }
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
    }
    (start + 1).min(lines.len())
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
                | "hermes-agent"
                | "karpathy-skills-cmp"
                | "agent-tools"
                | "terminals"
                | "mcps"
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

    fn temp_workspace(label: &str) -> (PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!("keel-index-{label}-{}", now_millis()));
        let home = root.join("home");
        fs::create_dir_all(root.join("src")).expect("workspace");
        fs::create_dir_all(&home).expect("home");
        (root, home)
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
        let second = refresh(&root, &home.to_string_lossy(), false).expect("no-op refresh");
        assert_eq!(second.generation, report.generation);
    }

    #[test]
    fn search_fuses_exact_symbol_path_and_fts_results() {
        let (root, home) = temp_workspace("search");
        fs::write(root.join("src/main.rs"), "pub fn dispatch_request() {}\n").expect("source");
        let hits = search(&root, &home.to_string_lossy(), "dispatch_request", 10).expect("search");
        assert!(!hits.is_empty());
        assert_eq!(hits[0].path, "src/main.rs");
        assert_eq!(hits[0].symbol, "dispatch_request");
        assert!(hits[0].reason.contains("exact-symbol"));
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
        let hits = search(&root, &home.to_string_lossy(), "old", 10).expect("search");
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
}
