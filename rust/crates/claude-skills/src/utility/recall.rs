//! Purpose: SQLite FTS5-backed full-text search over the local Markdown memory stores.
//! Caller: `utility::memory::run_memory_command` for the `recall` subcommand on both the
//!   `memory` and `memoriesv2` command groups.
//! Dependencies: rusqlite (bundled SQLite with FTS5), std::fs, std::path, std::time, the
//!   crate-local args/json/runtime helpers.
//! Main Functions: `run_recall_command`, `sync_recall_index`, `query_recall_index`,
//!   `recall_database_path`, `default_search_roots`.
//! Side Effects: Creates and writes the SQLite index file at `<claude-home>/recall-index.sqlite3`,
//!   reads markdown files under `<claude-home>/memories`, `<claude-home>/memoriesv2`, and
//!   `<claude-home>/working-briefs`. No network. No global state.
//!
//! Invariants:
//!   * The on-disk schema is owned by this module. The `documents` virtual table is FTS5
//!     with `path UNINDEXED, modified_at UNINDEXED, size UNINDEXED, content` and the
//!     porter+unicode61+remove_diacritics tokenizer chain. A sibling `meta(key TEXT
//!     PRIMARY KEY, value TEXT NOT NULL)` table stores schema version and last-sync
//!     timestamps so this module can evolve without orphaning existing indexes.
//!   * Recall always reflects the current files on disk: every read-path call
//!     (`recall <query>` and `recall status`) runs `sync_recall_index` first. The
//!     mtime+size delta scan is sub-millisecond at single-user scale.
//!   * `recall reindex --force` drops and recreates the FTS5 table to recover from a
//!     corrupt or stale index without disturbing any other Claude Code home file.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension};

use crate::args::FlagSet;
use crate::json::{write_indented, Value};
use crate::runtime::{display_path, resolve_claude_home};

/// Schema version stamped into the `meta` table. Bump when the FTS5 column layout
/// or tokenizer chain changes so existing indexes get rebuilt automatically.
const SCHEMA_VERSION: &str = "1";

/// Top-level subdirectories under `<claude-home>` that recall indexes by default.
/// Listed explicitly so the indexer never wanders into binaries, hooks, or release
/// staging directories that happen to share the home root.
const DEFAULT_RECALL_ROOTS: &[&str] = &["memories", "memoriesv2", "working-briefs"];

/// Maximum number of FTS5 hits returned when `--limit` is not supplied.
const DEFAULT_RECALL_LIMIT: usize = 20;

/// Snippet window is short enough to fit into a terminal line on either side of
/// a match. Tuning here also affects the `snippet()` call below — keep in sync.
const SNIPPET_TOKENS: i64 = 24;

/// FTS5 snippet delimiters. We deliberately avoid `[` and `]` because Markdown
/// uses those for link syntax `[text](url)` and checkboxes `[x]`, which would
/// cause `locate_first_match_line` to attribute the wrong line. ASCII record
/// separators never appear in normal Markdown, so they make a clean signal.
/// We swap them for visible brackets right before rendering to the user so the
/// output is still human-readable.
const SNIPPET_OPEN_MARKER: char = '\u{0002}';
const SNIPPET_CLOSE_MARKER: char = '\u{0003}';

pub fn run_recall_command(
    command_group: &str,
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    if arguments.is_empty() {
        render_recall_help(command_group, standard_output);
        return 1;
    }
    match arguments[0].as_str() {
        "--help" | "-h" | "help" => {
            render_recall_help(command_group, standard_output);
            0
        }
        "reindex" => run_recall_reindex(
            command_group,
            &arguments[1..],
            standard_output,
            standard_error,
        ),
        "status" => run_recall_status(
            command_group,
            &arguments[1..],
            standard_output,
            standard_error,
        ),
        // Anything else is treated as the search query. We deliberately accept
        // queries that start with a flag (for example `--limit 5 -- "foo"`) by
        // letting `FlagSet::parse` consume the leading flags and treat the rest
        // as positionals.
        _ => run_recall_search(command_group, arguments, standard_output, standard_error),
    }
}

fn render_recall_help(command_group: &str, standard_output: &mut dyn Write) {
    let _ = writeln!(
        standard_output,
        "Usage: claude-skills {command_group} recall <query> [--limit N] [--json] [--claude-home PATH]"
    );
    let _ = writeln!(standard_output, "       claude-skills {command_group} recall reindex [--force] [--claude-home PATH]");
    let _ = writeln!(
        standard_output,
        "       claude-skills {command_group} recall status [--json] [--claude-home PATH]"
    );
    let _ = writeln!(standard_output);
    let _ = writeln!(
        standard_output,
        "Searches Markdown files under <claude-home>/{{memories,memoriesv2,working-briefs}} via SQLite FTS5."
    );
    let _ = writeln!(
        standard_output,
        "The index lives at <claude-home>/recall-index.sqlite3 and is refreshed automatically on every call."
    );
}

fn run_recall_search(
    command_group: &str,
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flag_set = FlagSet::new(format!("{command_group} recall"));
    flag_set.string_flag("limit", "");
    flag_set.string_flag("claude-home", "");
    flag_set.bool_flag("json", false);
    if let Err(parse_error) = flag_set.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 2;
    }
    let raw_query = flag_set.positional.join(" ");
    let trimmed_query = raw_query.trim();
    if trimmed_query.is_empty() {
        let _ = writeln!(
            standard_error,
            "{command_group} recall: missing query (try `claude-skills {command_group} recall --help`)"
        );
        return 1;
    }
    let limit = match parse_limit(flag_set.string_value("limit")) {
        Ok(parsed_limit) => parsed_limit,
        Err(error_message) => {
            let _ = writeln!(standard_error, "{command_group} recall: {error_message}");
            return 2;
        }
    };
    let claude_home = match resolve_claude_home(flag_set.string_value("claude-home")) {
        Ok(home) => home,
        Err(error_message) => {
            let _ = writeln!(standard_error, "{command_group} recall: {error_message}");
            return 1;
        }
    };

    let database_path = recall_database_path(&claude_home);
    let mut connection = match open_recall_connection(&database_path) {
        Ok(connection) => connection,
        Err(error_message) => {
            let _ = writeln!(
                standard_error,
                "{command_group} recall: open index {}: {error_message}",
                display_path(&database_path)
            );
            return 1;
        }
    };
    if let Err(error_message) = sync_recall_index(&mut connection, &claude_home, false) {
        let _ = writeln!(
            standard_error,
            "{command_group} recall: refresh index: {error_message}"
        );
        return 1;
    }

    let fts_query = match build_fts_query(trimmed_query) {
        Some(fts_query) => fts_query,
        None => {
            let _ = writeln!(
                standard_error,
                "{command_group} recall: query has no searchable terms"
            );
            return 1;
        }
    };
    let matches = match query_recall_index(&connection, &fts_query, limit) {
        Ok(matches) => matches,
        Err(error_message) => {
            let _ = writeln!(
                standard_error,
                "{command_group} recall: query index: {error_message}"
            );
            return 1;
        }
    };

    if flag_set.bool_value("json") {
        let payload = build_search_json(trimmed_query, &claude_home, &matches);
        if let Err(error) = write_indented(standard_output, &payload) {
            let _ = writeln!(standard_error, "{command_group} recall: {error}");
            return 1;
        }
        return 0;
    }

    let _ = writeln!(
        standard_output,
        "{command_group} recall: query={:?} matches={}",
        trimmed_query,
        matches.len()
    );
    if matches.is_empty() {
        let _ = writeln!(
            standard_output,
            "  (no Markdown documents under {} match)",
            display_path(&claude_home)
        );
        return 0;
    }
    for hit in &matches {
        let relative_path = relativize(&claude_home, &PathBuf::from(&hit.absolute_path));
        let line_label = if hit.line > 0 {
            format!(":{}", hit.line)
        } else {
            String::new()
        };
        let _ = writeln!(
            standard_output,
            "  {}{}  {}",
            relative_path, line_label, hit.snippet
        );
    }
    0
}

fn run_recall_reindex(
    command_group: &str,
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flag_set = FlagSet::new(format!("{command_group} recall reindex"));
    flag_set.bool_flag("force", false);
    flag_set.string_flag("claude-home", "");
    flag_set.bool_flag("json", false);
    if let Err(parse_error) = flag_set.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 2;
    }
    let claude_home = match resolve_claude_home(flag_set.string_value("claude-home")) {
        Ok(home) => home,
        Err(error_message) => {
            let _ = writeln!(
                standard_error,
                "{command_group} recall reindex: {error_message}"
            );
            return 1;
        }
    };
    let database_path = recall_database_path(&claude_home);
    if flag_set.bool_value("force") {
        if database_path.exists() {
            if let Err(io_error) = fs::remove_file(&database_path) {
                let _ = writeln!(
                    standard_error,
                    "{command_group} recall reindex: remove {}: {io_error}",
                    display_path(&database_path)
                );
                return 1;
            }
        }
    }
    let mut connection = match open_recall_connection(&database_path) {
        Ok(connection) => connection,
        Err(error_message) => {
            let _ = writeln!(
                standard_error,
                "{command_group} recall reindex: open index {}: {error_message}",
                display_path(&database_path)
            );
            return 1;
        }
    };
    let report = match sync_recall_index(&mut connection, &claude_home, true) {
        Ok(report) => report,
        Err(error_message) => {
            let _ = writeln!(
                standard_error,
                "{command_group} recall reindex: {error_message}"
            );
            return 1;
        }
    };
    if flag_set.bool_value("json") {
        let payload = Value::Object(vec![
            (
                "indexPath".into(),
                Value::String(display_path(&database_path)),
            ),
            (
                "claudeHome".into(),
                Value::String(display_path(&claude_home)),
            ),
            (
                "documentsIndexed".into(),
                Value::Number(report.indexed_total.to_string()),
            ),
            (
                "documentsUpdated".into(),
                Value::Number(report.updated.to_string()),
            ),
            (
                "documentsAdded".into(),
                Value::Number(report.added.to_string()),
            ),
            (
                "documentsRemoved".into(),
                Value::Number(report.removed.to_string()),
            ),
            (
                "lastIndexedAtMillis".into(),
                Value::Number(report.last_indexed_at_millis.to_string()),
            ),
        ]);
        if let Err(error) = write_indented(standard_output, &payload) {
            let _ = writeln!(standard_error, "{command_group} recall reindex: {error}");
            return 1;
        }
        return 0;
    }
    let _ = writeln!(
        standard_output,
        "{command_group} recall reindex: indexed={} added={} updated={} removed={} index={}",
        report.indexed_total,
        report.added,
        report.updated,
        report.removed,
        display_path(&database_path)
    );
    0
}

fn run_recall_status(
    command_group: &str,
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flag_set = FlagSet::new(format!("{command_group} recall status"));
    flag_set.string_flag("claude-home", "");
    flag_set.bool_flag("json", false);
    if let Err(parse_error) = flag_set.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 2;
    }
    let claude_home = match resolve_claude_home(flag_set.string_value("claude-home")) {
        Ok(home) => home,
        Err(error_message) => {
            let _ = writeln!(
                standard_error,
                "{command_group} recall status: {error_message}"
            );
            return 1;
        }
    };
    let database_path = recall_database_path(&claude_home);
    let mut connection = match open_recall_connection(&database_path) {
        Ok(connection) => connection,
        Err(error_message) => {
            let _ = writeln!(
                standard_error,
                "{command_group} recall status: open index {}: {error_message}",
                display_path(&database_path)
            );
            return 1;
        }
    };
    let report = match sync_recall_index(&mut connection, &claude_home, false) {
        Ok(report) => report,
        Err(error_message) => {
            let _ = writeln!(
                standard_error,
                "{command_group} recall status: {error_message}"
            );
            return 1;
        }
    };
    let document_count = match count_documents(&connection) {
        Ok(count) => count,
        Err(error_message) => {
            let _ = writeln!(
                standard_error,
                "{command_group} recall status: {error_message}"
            );
            return 1;
        }
    };
    if flag_set.bool_value("json") {
        let payload = Value::Object(vec![
            (
                "indexPath".into(),
                Value::String(display_path(&database_path)),
            ),
            (
                "claudeHome".into(),
                Value::String(display_path(&claude_home)),
            ),
            (
                "schemaVersion".into(),
                Value::String(SCHEMA_VERSION.to_string()),
            ),
            (
                "documents".into(),
                Value::Number(document_count.to_string()),
            ),
            (
                "lastIndexedAtMillis".into(),
                Value::Number(report.last_indexed_at_millis.to_string()),
            ),
            (
                "addedSinceLastSync".into(),
                Value::Number(report.added.to_string()),
            ),
            (
                "updatedSinceLastSync".into(),
                Value::Number(report.updated.to_string()),
            ),
            (
                "removedSinceLastSync".into(),
                Value::Number(report.removed.to_string()),
            ),
        ]);
        if let Err(error) = write_indented(standard_output, &payload) {
            let _ = writeln!(standard_error, "{command_group} recall status: {error}");
            return 1;
        }
        return 0;
    }
    let _ = writeln!(
        standard_output,
        "{command_group} recall status: documents={} index={} schema={} last_indexed_at_millis={}",
        document_count,
        display_path(&database_path),
        SCHEMA_VERSION,
        report.last_indexed_at_millis,
    );
    0
}

fn parse_limit(raw_value: &str) -> Result<usize, String> {
    let trimmed = raw_value.trim();
    if trimmed.is_empty() {
        return Ok(DEFAULT_RECALL_LIMIT);
    }
    match trimmed.parse::<usize>() {
        Ok(parsed) if parsed > 0 => Ok(parsed),
        Ok(_) => Err(format!("--limit must be a positive integer, got {trimmed:?}")),
        Err(_) => Err(format!("--limit must be a positive integer, got {trimmed:?}")),
    }
}

/// Quote each token in the user query for FTS5 and AND them together so the
/// default behaviour is "all words must appear, in any order, with prefix
/// match". This intentionally hides FTS5 syntax from the caller; advanced raw
/// queries can be added later if there's demand.
pub fn build_fts_query(raw_query: &str) -> Option<String> {
    let mut tokens: Vec<String> = Vec::new();
    for token in raw_query.split_whitespace() {
        let cleaned: String = token
            .chars()
            .filter(|character| {
                character.is_alphanumeric() || matches!(character, '-' | '_' | '.')
            })
            .collect();
        if !cleaned.is_empty() {
            tokens.push(format!("\"{cleaned}\"*"));
        }
    }
    if tokens.is_empty() {
        None
    } else {
        Some(tokens.join(" AND "))
    }
}

pub fn recall_database_path(claude_home: &Path) -> PathBuf {
    claude_home.join("recall-index.sqlite3")
}

pub fn default_search_roots(claude_home: &Path) -> Vec<PathBuf> {
    DEFAULT_RECALL_ROOTS
        .iter()
        .map(|name| claude_home.join(name))
        .collect()
}

fn open_recall_connection(database_path: &Path) -> Result<Connection, String> {
    if let Some(parent_directory) = database_path.parent() {
        fs::create_dir_all(parent_directory)
            .map_err(|io_error| format!("create {}: {io_error}", display_path(parent_directory)))?;
    }
    let connection = Connection::open(database_path)
        .map_err(|database_error| format!("open sqlite: {database_error}"))?;
    connection
        .pragma_update(None, "journal_mode", "WAL")
        .map_err(|database_error| format!("set journal_mode: {database_error}"))?;
    connection
        .pragma_update(None, "synchronous", "NORMAL")
        .map_err(|database_error| format!("set synchronous: {database_error}"))?;
    ensure_recall_schema(&connection)?;
    Ok(connection)
}

fn ensure_recall_schema(connection: &Connection) -> Result<(), String> {
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS meta (
                 key TEXT PRIMARY KEY,
                 value TEXT NOT NULL
             );
             CREATE VIRTUAL TABLE IF NOT EXISTS documents USING fts5(
                 path UNINDEXED,
                 modified_at UNINDEXED,
                 size UNINDEXED,
                 content,
                 tokenize = 'porter unicode61 remove_diacritics 2'
             );",
        )
        .map_err(|database_error| format!("ensure schema: {database_error}"))?;

    let stored_version: Option<String> = connection
        .query_row(
            "SELECT value FROM meta WHERE key = 'schema_version'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|database_error| format!("read schema_version: {database_error}"))?;

    match stored_version.as_deref() {
        Some(value) if value == SCHEMA_VERSION => {}
        Some(_) => {
            connection
                .execute_batch(
                    "DROP TABLE IF EXISTS documents;
                     CREATE VIRTUAL TABLE documents USING fts5(
                         path UNINDEXED,
                         modified_at UNINDEXED,
                         size UNINDEXED,
                         content,
                         tokenize = 'porter unicode61 remove_diacritics 2'
                     );",
                )
                .map_err(|database_error| format!("rebuild documents: {database_error}"))?;
            connection
                .execute(
                    "INSERT OR REPLACE INTO meta(key, value) VALUES ('schema_version', ?1)",
                    params![SCHEMA_VERSION],
                )
                .map_err(|database_error| format!("stamp schema_version: {database_error}"))?;
        }
        None => {
            connection
                .execute(
                    "INSERT OR REPLACE INTO meta(key, value) VALUES ('schema_version', ?1)",
                    params![SCHEMA_VERSION],
                )
                .map_err(|database_error| format!("stamp schema_version: {database_error}"))?;
        }
    }
    Ok(())
}

#[derive(Debug, Default, Clone)]
pub struct SyncReport {
    pub indexed_total: u64,
    pub added: u64,
    pub updated: u64,
    pub removed: u64,
    pub last_indexed_at_millis: u128,
}

/// Walk the default recall roots and bring the FTS5 table in line with what is
/// on disk. `force_full_rescan = true` re-reads every document and overwrites
/// the FTS row even if mtime/size match — used by the explicit `reindex`
/// subcommand. Returns counts so callers can render a status line.
pub fn sync_recall_index(
    connection: &mut Connection,
    claude_home: &Path,
    force_full_rescan: bool,
) -> Result<SyncReport, String> {
    let mut report = SyncReport::default();
    let mut existing_rows: std::collections::HashMap<String, (i64, i64)> =
        std::collections::HashMap::new();
    {
        let mut select_statement = connection
            .prepare("SELECT path, modified_at, size FROM documents")
            .map_err(|database_error| format!("prepare select: {database_error}"))?;
        let row_iterator = select_statement
            .query_map([], |row| {
                let path: String = row.get(0)?;
                let modified_at_text: String = row.get(1)?;
                let size_text: String = row.get(2)?;
                let modified_at = modified_at_text.parse::<i64>().unwrap_or(0);
                let size_bytes = size_text.parse::<i64>().unwrap_or(0);
                Ok((path, (modified_at, size_bytes)))
            })
            .map_err(|database_error| format!("query existing: {database_error}"))?;
        for row_result in row_iterator {
            let (path, metadata) =
                row_result.map_err(|database_error| format!("read row: {database_error}"))?;
            existing_rows.insert(path, metadata);
        }
    }

    let mut on_disk: Vec<DocumentRecord> = Vec::new();
    for root_directory in default_search_roots(claude_home) {
        if !root_directory.is_dir() {
            continue;
        }
        collect_markdown_files(&root_directory, &mut on_disk)?;
    }

    let mut on_disk_paths: std::collections::HashSet<String> = std::collections::HashSet::new();
    let transaction = connection
        .transaction()
        .map_err(|database_error| format!("begin transaction: {database_error}"))?;
    for document in &on_disk {
        on_disk_paths.insert(document.absolute_path.clone());
        let needs_write = match existing_rows.get(&document.absolute_path) {
            Some((stored_modified_at, stored_size)) => {
                force_full_rescan
                    || *stored_modified_at != document.modified_at_millis
                    || *stored_size != document.size_bytes
            }
            None => true,
        };
        if !needs_write {
            continue;
        }
        let content = match fs::read_to_string(&document.absolute_path) {
            Ok(text) => text,
            Err(_) => {
                // Skip files we can't read as UTF-8; they don't belong in a
                // Markdown text index. We deliberately do not fail the entire
                // sync over a single unreadable document.
                continue;
            }
        };
        transaction
            .execute(
                "DELETE FROM documents WHERE path = ?1",
                params![&document.absolute_path],
            )
            .map_err(|database_error| format!("delete stale row: {database_error}"))?;
        transaction
            .execute(
                "INSERT INTO documents(path, modified_at, size, content) VALUES (?1, ?2, ?3, ?4)",
                params![
                    &document.absolute_path,
                    document.modified_at_millis.to_string(),
                    document.size_bytes.to_string(),
                    content,
                ],
            )
            .map_err(|database_error| format!("insert document: {database_error}"))?;
        if existing_rows.contains_key(&document.absolute_path) {
            report.updated += 1;
        } else {
            report.added += 1;
        }
    }

    let mut paths_to_remove: Vec<String> = Vec::new();
    for path in existing_rows.keys() {
        if !on_disk_paths.contains(path) {
            paths_to_remove.push(path.clone());
        }
    }
    for path in &paths_to_remove {
        transaction
            .execute("DELETE FROM documents WHERE path = ?1", params![path])
            .map_err(|database_error| format!("delete vanished: {database_error}"))?;
        report.removed += 1;
    }

    let now_millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    transaction
        .execute(
            "INSERT OR REPLACE INTO meta(key, value) VALUES ('last_indexed_at_millis', ?1)",
            params![now_millis.to_string()],
        )
        .map_err(|database_error| format!("stamp last_indexed_at: {database_error}"))?;
    transaction
        .commit()
        .map_err(|database_error| format!("commit transaction: {database_error}"))?;

    report.indexed_total = on_disk.len() as u64;
    report.last_indexed_at_millis = now_millis;
    Ok(report)
}

#[derive(Debug, Clone)]
struct DocumentRecord {
    absolute_path: String,
    modified_at_millis: i64,
    size_bytes: i64,
}

fn collect_markdown_files(
    directory: &Path,
    out: &mut Vec<DocumentRecord>,
) -> Result<(), String> {
    let read_dir = match fs::read_dir(directory) {
        Ok(read_dir) => read_dir,
        Err(_) => return Ok(()),
    };
    for entry_result in read_dir {
        let entry = match entry_result {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let entry_path = entry.path();
        let metadata = match entry.metadata() {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };
        if metadata.is_dir() {
            collect_markdown_files(&entry_path, out)?;
            continue;
        }
        if !metadata.is_file() {
            continue;
        }
        let extension = entry_path
            .extension()
            .and_then(|os_str| os_str.to_str())
            .map(|extension| extension.to_ascii_lowercase());
        if extension.as_deref() != Some("md") {
            continue;
        }
        let modified_at_millis = metadata
            .modified()
            .ok()
            .and_then(|system_time| system_time.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_millis() as i64)
            .unwrap_or(0);
        let size_bytes = metadata.len() as i64;
        // Deliberately do NOT canonicalize: on Windows that yields a `\\?\` UNC
        // prefix that no longer shares a string-prefix with `claude_home`, which
        // would break `relativize` and force every hit to render with an
        // absolute path. `entry.path()` from a `read_dir` walk under a clean
        // root is already deterministic for our purposes.
        let absolute_path = entry_path.to_string_lossy().into_owned();
        out.push(DocumentRecord {
            absolute_path,
            modified_at_millis,
            size_bytes,
        });
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct RecallHit {
    pub absolute_path: String,
    pub score: f64,
    pub line: usize,
    pub snippet: String,
}

pub fn query_recall_index(
    connection: &Connection,
    fts_query: &str,
    limit: usize,
) -> Result<Vec<RecallHit>, String> {
    let mut prepared_statement = connection
        .prepare(
            "SELECT \
                 path, \
                 bm25(documents), \
                 snippet(documents, 3, ?1, ?2, '...', ?3), \
                 content \
             FROM documents \
             WHERE documents MATCH ?4 \
             ORDER BY bm25(documents) \
             LIMIT ?5",
        )
        .map_err(|database_error| format!("prepare query: {database_error}"))?;
    let open_marker = SNIPPET_OPEN_MARKER.to_string();
    let close_marker = SNIPPET_CLOSE_MARKER.to_string();
    let query_iterator = prepared_statement
        .query_map(
            params![open_marker, close_marker, SNIPPET_TOKENS, fts_query, limit as i64],
            |row| {
                let absolute_path: String = row.get(0)?;
                let score: f64 = row.get(1)?;
                let snippet_text: String = row.get(2)?;
                let content: String = row.get(3)?;
                Ok((absolute_path, score, snippet_text, content))
            },
        )
        .map_err(|database_error| format!("query: {database_error}"))?;
    let mut hits: Vec<RecallHit> = Vec::new();
    for row_result in query_iterator {
        let (absolute_path, score, snippet_text, content) = row_result
            .map_err(|database_error| format!("read result row: {database_error}"))?;
        let line = locate_first_match_line(&content, &snippet_text);
        let display_snippet = render_snippet_for_display(&snippet_text);
        hits.push(RecallHit {
            absolute_path,
            score,
            line,
            snippet: display_snippet,
        });
    }
    Ok(hits)
}

/// Returns the (1-indexed) line of `content` that contains the first FTS5
/// snippet match (the chunk between `SNIPPET_OPEN_MARKER` and
/// `SNIPPET_CLOSE_MARKER`). Falls back to 0 when the markers can't be found,
/// which keeps the renderer working without a line number rather than crashing.
///
/// We deliberately use ASCII control characters as markers instead of `[`/`]`
/// so that Markdown link syntax (`[text](url)`) and checkboxes (`[x]`) in the
/// snippet window do not get mistaken for the highlight delimiters.
fn locate_first_match_line(content: &str, snippet_text: &str) -> usize {
    let first_marker_offset = snippet_text.find(SNIPPET_OPEN_MARKER);
    let second_marker_offset = snippet_text.find(SNIPPET_CLOSE_MARKER);
    let (start_offset, end_offset) = match (first_marker_offset, second_marker_offset) {
        (Some(start), Some(end)) if end > start + SNIPPET_OPEN_MARKER.len_utf8() => {
            (start + SNIPPET_OPEN_MARKER.len_utf8(), end)
        }
        _ => return 0,
    };
    let highlighted_token = &snippet_text[start_offset..end_offset];
    if highlighted_token.is_empty() {
        return 0;
    }
    // FTS5 lower-cases for matching. Compare case-insensitively against the
    // source content so we still find the line on titles like `# Stripe`. Use
    // the Unicode-aware `to_lowercase` so non-ASCII tokens (accents, CJK)
    // still attribute the right line.
    let lower_content = content.to_lowercase();
    let lower_token = highlighted_token.to_lowercase();
    let byte_offset = match lower_content.find(&lower_token) {
        Some(offset) => offset,
        None => return 0,
    };
    1 + content[..byte_offset].matches('\n').count()
}

/// Replace the internal control-character markers with the visible `[`/`]`
/// delimiters the user expects, then collapse whitespace for a single-line
/// terminal-friendly excerpt.
fn render_snippet_for_display(snippet_text: &str) -> String {
    let with_visible_markers = snippet_text
        .replace(SNIPPET_OPEN_MARKER, "[")
        .replace(SNIPPET_CLOSE_MARKER, "]");
    collapse_whitespace(&with_visible_markers)
}

fn collapse_whitespace(text: &str) -> String {
    let mut collapsed_output = String::with_capacity(text.len());
    let mut previous_was_whitespace = false;
    for character in text.chars() {
        if character.is_whitespace() {
            if !previous_was_whitespace && !collapsed_output.is_empty() {
                collapsed_output.push(' ');
            }
            previous_was_whitespace = true;
        } else {
            collapsed_output.push(character);
            previous_was_whitespace = false;
        }
    }
    collapsed_output.trim().to_string()
}

fn count_documents(connection: &Connection) -> Result<u64, String> {
    let count: i64 = connection
        .query_row("SELECT COUNT(*) FROM documents", [], |row| row.get(0))
        .map_err(|database_error| format!("count documents: {database_error}"))?;
    Ok(count.max(0) as u64)
}

fn build_search_json(query: &str, claude_home: &Path, matches: &[RecallHit]) -> Value {
    let entries: Vec<Value> = matches
        .iter()
        .map(|hit| {
            Value::Object(vec![
                (
                    "path".into(),
                    Value::String(relativize(claude_home, &PathBuf::from(&hit.absolute_path))),
                ),
                (
                    "absolutePath".into(),
                    Value::String(hit.absolute_path.clone()),
                ),
                ("score".into(), Value::Number(format!("{:.4}", hit.score))),
                ("line".into(), Value::Number(hit.line.to_string())),
                ("snippet".into(), Value::String(hit.snippet.clone())),
            ])
        })
        .collect();
    Value::Object(vec![
        ("query".into(), Value::String(query.to_string())),
        (
            "claudeHome".into(),
            Value::String(display_path(claude_home)),
        ),
        ("count".into(), Value::Number(matches.len().to_string())),
        ("matches".into(), Value::Array(entries)),
    ])
}

fn relativize(claude_home: &Path, absolute_path: &Path) -> String {
    match absolute_path.strip_prefix(claude_home) {
        Ok(relative_path) => relative_path
            .to_string_lossy()
            .replace('\\', "/")
            .to_string(),
        Err(_) => display_path(absolute_path),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn tempdir_under(label: &str) -> PathBuf {
        let unique_suffix: u128 = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let candidate = std::env::temp_dir().join(format!("{label}-{unique_suffix}"));
        fs::create_dir_all(&candidate).expect("create tempdir");
        candidate
    }

    fn write_memory(claude_home: &Path, relative_path: &str, body: &str) {
        let absolute_path = claude_home.join(relative_path);
        if let Some(parent_directory) = absolute_path.parent() {
            fs::create_dir_all(parent_directory).expect("create parent directories");
        }
        fs::write(&absolute_path, body).expect("write fixture markdown");
    }

    fn run_with_home<F>(label: &str, body: F)
    where
        F: FnOnce(&Path),
    {
        let _guard = ENV_LOCK.lock().expect("lock environment override");
        let temporary_directory = tempdir_under(label);
        let claude_home = temporary_directory.join("claude-home");
        fs::create_dir_all(&claude_home).expect("create claude home");
        let previous_override = std::env::var("CLAUDE_TARGET_OVERRIDE").ok();
        std::env::set_var("CLAUDE_TARGET_OVERRIDE", &claude_home);
        body(&claude_home);
        if let Some(previous_value) = previous_override {
            std::env::set_var("CLAUDE_TARGET_OVERRIDE", previous_value);
        } else {
            std::env::remove_var("CLAUDE_TARGET_OVERRIDE");
        }
        let _ = fs::remove_dir_all(&temporary_directory);
    }

    #[test]
    fn build_fts_query_strips_punctuation_and_quotes_each_token() {
        let rendered = build_fts_query("OpenAPI diff! breaking-change?").expect("non-empty query");
        assert_eq!(rendered, "\"OpenAPI\"* AND \"diff\"* AND \"breaking-change\"*");
    }

    #[test]
    fn build_fts_query_returns_none_for_empty_input() {
        assert!(build_fts_query("   ?!  ").is_none());
    }

    #[test]
    fn collapse_whitespace_preserves_single_spaces() {
        assert_eq!(collapse_whitespace("foo\n\n  bar\tbaz"), "foo bar baz");
    }

    #[test]
    fn recall_indexes_markdown_under_memories_and_returns_hits() {
        run_with_home("claude-skills-recall-basic", |claude_home| {
            write_memory(
                claude_home,
                "memories/notes/openapi.md",
                "# OpenAPI breaking change checklist\n\nReview the diff before merging.\n",
            );
            write_memory(
                claude_home,
                "memoriesv2/team/stripe.md",
                "# Stripe payment intent\n\nVerify webhook signatures.\n",
            );
            write_memory(
                claude_home,
                "working-briefs/today.md",
                "# Working brief\n\nQuiet day, mostly OpenAPI cleanup.\n",
            );

            let mut stdout: Vec<u8> = Vec::new();
            let mut stderr: Vec<u8> = Vec::new();
            let exit_code = run_recall_command(
                "memory",
                &["openapi".to_string()],
                &mut stdout,
                &mut stderr,
            );
            assert_eq!(
                exit_code,
                0,
                "stderr: {}",
                String::from_utf8_lossy(&stderr)
            );
            let rendered = String::from_utf8_lossy(&stdout);
            assert!(
                rendered.contains("memory recall: query=\"openapi\""),
                "rendered: {rendered}"
            );
            assert!(rendered.contains("memories/notes/openapi.md"), "rendered: {rendered}");
            assert!(rendered.contains("working-briefs/today.md"), "rendered: {rendered}");
        });
    }

    #[test]
    fn recall_json_output_includes_score_and_line() {
        run_with_home("claude-skills-recall-json", |claude_home| {
            write_memory(
                claude_home,
                "memories/security/incident.md",
                "# Webhook signature incident\n\nReplay attack mitigation steps.\n",
            );
            let mut stdout: Vec<u8> = Vec::new();
            let mut stderr: Vec<u8> = Vec::new();
            let exit_code = run_recall_command(
                "memory",
                &["webhook".to_string(), "--json".to_string()],
                &mut stdout,
                &mut stderr,
            );
            assert_eq!(
                exit_code,
                0,
                "stderr: {}",
                String::from_utf8_lossy(&stderr)
            );
            let rendered = String::from_utf8_lossy(&stdout);
            assert!(rendered.contains("\"query\": \"webhook\""), "rendered: {rendered}");
            assert!(rendered.contains("\"count\":"), "rendered: {rendered}");
            assert!(rendered.contains("\"score\":"), "rendered: {rendered}");
            assert!(rendered.contains("memories/security/incident.md"), "rendered: {rendered}");
        });
    }

    #[test]
    fn recall_reflects_subsequent_edits_via_auto_sync() {
        run_with_home("claude-skills-recall-auto", |claude_home| {
            write_memory(
                claude_home,
                "memories/draft.md",
                "# Draft\n\nTalking about postgres migrations.\n",
            );
            let mut stdout: Vec<u8> = Vec::new();
            let mut stderr: Vec<u8> = Vec::new();
            let first_code = run_recall_command(
                "memory",
                &["postgres".to_string()],
                &mut stdout,
                &mut stderr,
            );
            assert_eq!(first_code, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
            assert!(String::from_utf8_lossy(&stdout).contains("memories/draft.md"));

            // Sleep just enough that mtime changes on filesystems with
            // 1-second resolution, then rewrite the file with new content.
            std::thread::sleep(std::time::Duration::from_millis(1100));
            write_memory(
                claude_home,
                "memories/draft.md",
                "# Draft\n\nNow we are talking about kubernetes.\n",
            );
            let mut stdout_after: Vec<u8> = Vec::new();
            let mut stderr_after: Vec<u8> = Vec::new();
            let second_code = run_recall_command(
                "memory",
                &["kubernetes".to_string()],
                &mut stdout_after,
                &mut stderr_after,
            );
            assert_eq!(
                second_code,
                0,
                "stderr: {}",
                String::from_utf8_lossy(&stderr_after)
            );
            let after_text = String::from_utf8_lossy(&stdout_after);
            assert!(
                after_text.contains("memories/draft.md"),
                "auto-sync did not reflect edit: {after_text}"
            );

            let mut stdout_post: Vec<u8> = Vec::new();
            let mut stderr_post: Vec<u8> = Vec::new();
            let post_code = run_recall_command(
                "memory",
                &["postgres".to_string()],
                &mut stdout_post,
                &mut stderr_post,
            );
            assert_eq!(
                post_code,
                0,
                "stderr: {}",
                String::from_utf8_lossy(&stderr_post)
            );
            let post_text = String::from_utf8_lossy(&stdout_post);
            assert!(
                post_text.contains("matches=0") || !post_text.contains("memories/draft.md"),
                "stale row not removed: {post_text}"
            );
        });
    }

    #[test]
    fn recall_reindex_force_rebuilds_from_scratch() {
        run_with_home("claude-skills-recall-reindex", |claude_home| {
            write_memory(
                claude_home,
                "memories/topic.md",
                "# Topic\n\nWebSocket presence channel.\n",
            );
            let mut stdout: Vec<u8> = Vec::new();
            let mut stderr: Vec<u8> = Vec::new();
            let exit_code = run_recall_command(
                "memory",
                &[
                    "reindex".to_string(),
                    "--force".to_string(),
                    "--json".to_string(),
                ],
                &mut stdout,
                &mut stderr,
            );
            assert_eq!(
                exit_code,
                0,
                "stderr: {}",
                String::from_utf8_lossy(&stderr)
            );
            let rendered = String::from_utf8_lossy(&stdout);
            assert!(rendered.contains("\"documentsAdded\":"), "rendered: {rendered}");
            assert!(rendered.contains("\"documentsIndexed\":"), "rendered: {rendered}");
        });
    }

    #[test]
    fn recall_status_reports_document_count() {
        run_with_home("claude-skills-recall-status", |claude_home| {
            write_memory(claude_home, "memories/a.md", "# A\nalpha alpha\n");
            write_memory(claude_home, "memories/b.md", "# B\nbeta\n");
            let mut stdout: Vec<u8> = Vec::new();
            let mut stderr: Vec<u8> = Vec::new();
            let exit_code = run_recall_command(
                "memory",
                &["status".to_string(), "--json".to_string()],
                &mut stdout,
                &mut stderr,
            );
            assert_eq!(
                exit_code,
                0,
                "stderr: {}",
                String::from_utf8_lossy(&stderr)
            );
            let rendered = String::from_utf8_lossy(&stdout);
            assert!(rendered.contains("\"documents\": 2"), "rendered: {rendered}");
            assert!(
                rendered.contains("\"schemaVersion\": \"1\""),
                "rendered: {rendered}"
            );
        });
    }

    #[test]
    fn recall_rejects_empty_query() {
        run_with_home("claude-skills-recall-empty", |claude_home| {
            write_memory(claude_home, "memories/x.md", "# placeholder\n");
            let mut stdout: Vec<u8> = Vec::new();
            let mut stderr: Vec<u8> = Vec::new();
            let exit_code =
                run_recall_command("memory", &["   ".to_string()], &mut stdout, &mut stderr);
            assert_eq!(exit_code, 1);
            assert!(String::from_utf8_lossy(&stderr).contains("missing query"));
        });
    }

    #[test]
    fn recall_returns_zero_matches_for_unknown_term() {
        run_with_home("claude-skills-recall-no-hits", |claude_home| {
            write_memory(
                claude_home,
                "memories/note.md",
                "# Note\nplain old text\n",
            );
            let mut stdout: Vec<u8> = Vec::new();
            let mut stderr: Vec<u8> = Vec::new();
            let exit_code = run_recall_command(
                "memory",
                &["nonexistentphrasezzz".to_string()],
                &mut stdout,
                &mut stderr,
            );
            assert_eq!(
                exit_code,
                0,
                "stderr: {}",
                String::from_utf8_lossy(&stderr)
            );
            let rendered = String::from_utf8_lossy(&stdout);
            assert!(rendered.contains("matches=0"), "rendered: {rendered}");
        });
    }

    #[test]
    fn locate_first_match_line_is_robust_to_markdown_link_brackets() {
        // Snippet contains a Markdown link before the highlighted match. With
        // the old `[`/`]` delimiters this would attribute the wrong line. The
        // control-character markers must isolate the real match.
        let content =
            "# Header\nSee [the spec](https://example.com) for details.\nThe match is webhook.\n";
        let snippet = format!(
            "See [the spec](https://example.com) for details. The match is {open}webhook{close}.",
            open = SNIPPET_OPEN_MARKER,
            close = SNIPPET_CLOSE_MARKER,
        );
        let line = locate_first_match_line(content, &snippet);
        assert_eq!(line, 3, "expected line 3 for `webhook`, got {line}");
    }

    #[test]
    fn render_snippet_for_display_swaps_markers_for_visible_brackets() {
        let snippet = format!(
            "before {open}match{close} after",
            open = SNIPPET_OPEN_MARKER,
            close = SNIPPET_CLOSE_MARKER,
        );
        let rendered = render_snippet_for_display(&snippet);
        assert_eq!(rendered, "before [match] after");
    }

    #[test]
    fn memoriesv2_command_group_uses_same_recall_index() {
        run_with_home("claude-skills-recall-memoriesv2", |claude_home| {
            write_memory(
                claude_home,
                "memoriesv2/library/postgres.md",
                "# Postgres\n\nLock timeout strategy for migrations.\n",
            );
            let mut stdout: Vec<u8> = Vec::new();
            let mut stderr: Vec<u8> = Vec::new();
            let exit_code = run_recall_command(
                "memoriesv2",
                &["postgres".to_string()],
                &mut stdout,
                &mut stderr,
            );
            assert_eq!(
                exit_code,
                0,
                "stderr: {}",
                String::from_utf8_lossy(&stderr)
            );
            let rendered = String::from_utf8_lossy(&stdout);
            assert!(rendered.starts_with("memoriesv2 recall:"), "rendered: {rendered}");
            assert!(
                rendered.contains("memoriesv2/library/postgres.md"),
                "rendered: {rendered}"
            );
        });
    }
}
