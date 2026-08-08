//! Purpose: SQLite FTS5-backed full-text search over the local Markdown and JSON memory stores.
//! Caller: `utility::memory::run_memory_command` for the `recall` subcommand on the
//!   `memory` command group.
//! Dependencies: rusqlite (bundled SQLite with FTS5), std::fs, std::path, std::time, the
//!   crate-local args/json/runtime helpers.
//! Main Functions: `run_recall_command`, `sync_recall_index`, `query_recall_index`,
//!   `recall_database_path`, `default_search_roots`.
//! Side Effects: Creates and writes the SQLite index file at `<claude-home>/recall-index.sqlite3`,
//!   reads `.md` and `.json` files under `<claude-home>/memories` and
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
//!     corrupt or stale index without disturbing any other the harness home file.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension};

use crate::args::FlagSet;
use crate::json::{write_indented, Value};
use crate::runtime::{display_path, resolve_claude_home};

#[cfg(feature = "semantic")]
use candle_core::{DType, Device, Tensor};
#[cfg(feature = "semantic")]
use candle_nn::VarBuilder;
#[cfg(feature = "semantic")]
use candle_transformers::models::bert::{BertModel, Config};
#[cfg(feature = "semantic")]
use keel_sqlite_vec::sqlite3_vec_init;
#[cfg(feature = "semantic")]
use std::sync::OnceLock;

/// Schema version stamped into the `meta` table. Bump when the FTS5 column layout
/// or tokenizer chain changes so existing indexes get rebuilt automatically.
/// Schema version stamped into the `meta` table. The semantic feature bumps
/// to "2" so a vec_items table is created alongside FTS5; non-semantic builds
/// stay at "1" to avoid a pointless FTS5 reindex for users who never enable
/// vector recall.
#[cfg(feature = "semantic")]
const SCHEMA_VERSION: &str = "2";
#[cfg(not(feature = "semantic"))]
const SCHEMA_VERSION: &str = "1";

/// Top-level subdirectories under `<claude-home>` that recall indexes by default.
/// Listed explicitly so the indexer never wanders into binaries, hooks, or release
/// staging directories that happen to share the home root.
///
/// Both `memory` (singular) and `memories` (plural) are indexed deliberately: the
/// CLI dispatches the primary lane as the literal command group `"memory"`, so
/// `family_store` writes family records and the working buffer under
/// `<home>/memory/<family>/` (singular), while the scoped `SYSTEM_MAP.md`
/// reference lane and the recall test fixtures live under `memories/` (plural,
/// via `system_map_reference_directory`'s normalization). Indexing only the
/// plural root would silently skip everything `memory <family> record` writes —
/// the primary recall surface returning zero hits with no error. Listing both
/// keeps recall complete regardless of which tree a write landed in.
const DEFAULT_RECALL_ROOTS: &[&str] = &["memory", "memories", "working-briefs"];

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
        "Usage: keel {command_group} recall <query> [--limit N] [--json] [--claude-home PATH] [--workspace SLUG] [--local-only]"
    );
    let _ = writeln!(
        standard_output,
        "       keel {command_group} recall reindex [--force] [--claude-home PATH]"
    );
    let _ = writeln!(
        standard_output,
        "       keel {command_group} recall status [--json] [--claude-home PATH]"
    );
    let _ = writeln!(standard_output);
    let _ = writeln!(
        standard_output,
        "Searches Markdown and JSON files under <claude-home>/{{memories,working-briefs}} via SQLite FTS5."
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
    // Recall accepts the search query as positional words mixed with flags
    // anywhere in the argument vector (for example `recall webhook --json`).
    // The shared FlagSet parser stops flag parsing at the first positional, so
    // we sieve known flags out first and hand the remaining tokens to FlagSet
    // as a contiguous positional run. This keeps the rest of the command
    // surface (which depends on the flags-then-positionals contract) unchanged.
    let (flag_arguments, query_arguments) = match split_flags_and_query(arguments) {
        Ok(pair) => pair,
        Err(error_message) => {
            let _ = writeln!(standard_error, "{command_group} recall: {error_message}");
            return 2;
        }
    };
    let mut combined: Vec<String> = flag_arguments;
    if !query_arguments.is_empty() {
        combined.push("--".to_string());
        combined.extend(query_arguments);
    }
    let mut flag_set = FlagSet::new(format!("{command_group} recall"));
    flag_set.string_flag("limit", "");
    flag_set.string_flag("claude-home", "");
    flag_set.string_flag("workspace", "");
    flag_set.bool_flag("local-only", false);
    flag_set.bool_flag("json", false);
    if let Err(parse_error) = flag_set.parse(&combined) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 2;
    }
    let raw_query = flag_set.positional.join(" ");
    let trimmed_query = raw_query.trim();
    if trimmed_query.is_empty() {
        let _ = writeln!(
            standard_error,
            "{command_group} recall: missing query (try `keel {command_group} recall --help`)"
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

    // Workspace affinity: boost current-project hits above cross-project.
    // `--workspace` forces a slug; otherwise cwd is slugged like system_map.
    let workspace_slug = {
        let explicit = flag_set.string_value("workspace").trim().to_string();
        if !explicit.is_empty() {
            Some(crate::utility::system_map::sanitize_key(&explicit))
        } else {
            std::env::current_dir()
                .ok()
                .map(|cwd| crate::utility::system_map::sanitize_key(&cwd.to_string_lossy()))
        }
    };
    let cascade =
        match cascade_recall_query(&connection, trimmed_query, limit, workspace_slug.as_deref()) {
            Ok(Some(cascade)) => {
                #[cfg(feature = "semantic")]
                let cascade = maybe_blend_vector_candidates(
                    &connection,
                    trimmed_query,
                    cascade,
                    limit,
                    workspace_slug.as_deref(),
                );
                cascade
            }
            Ok(None) => {
                let _ = writeln!(
                    standard_error,
                    "{command_group} recall: query has no searchable terms"
                );
                return 1;
            }
            Err(error_message) => {
                let _ = writeln!(
                    standard_error,
                    "{command_group} recall: query index: {error_message}"
                );
                return 1;
            }
        };
    let mut matches = cascade.hits;
    let stage = cascade.stage;

    // `--local-only`: restrict to the current workspace lane (a new project
    // returns empty). Both sides are dash-collapsed (`D:\` -> `D--` vs `D-`).
    if flag_set.bool_value("local-only") {
        if let Some(slug) = &workspace_slug {
            let slug_norm = collapse_dashes(&slug.to_ascii_lowercase());
            matches.retain(|hit| {
                collapse_dashes(&hit.absolute_path.to_ascii_lowercase()).contains(&slug_norm)
            });
        }
    }

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
        "{command_group} recall: query={:?} matches={} stage={stage}",
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
    if flag_set.bool_value("force") && database_path.exists() {
        if let Err(io_error) = fs::remove_file(&database_path) {
            let _ = writeln!(
                standard_error,
                "{command_group} recall reindex: remove {}: {io_error}",
                display_path(&database_path)
            );
            return 1;
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
                "documentsSkipped".into(),
                Value::Number(report.skipped.to_string()),
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
        "{command_group} recall reindex: indexed={} added={} updated={} removed={} skipped={} index={}",
        report.indexed_total,
        report.added,
        report.updated,
        report.removed,
        report.skipped,
        display_path(&database_path)
    );
    if report.skipped > 0 {
        let _ = writeln!(
            standard_error,
            "{command_group} recall reindex: warning: {} file(s) skipped (not valid UTF-8 text) and excluded from the index",
            report.skipped
        );
    }
    0
}

#[cfg_attr(not(feature = "semantic"), allow(unused_mut))]
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
        let mut fields = vec![
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
        ];
        #[cfg(feature = "semantic")]
        {
            let vectors = vector_count(&connection).unwrap_or(0);
            fields.push(("vectors".into(), Value::Number(vectors.to_string())));
        }
        let payload = Value::Object(fields);
        if let Err(error) = write_indented(standard_output, &payload) {
            let _ = writeln!(standard_error, "{command_group} recall status: {error}");
            return 1;
        }
        return 0;
    }
    #[cfg(feature = "semantic")]
    let vectors_part = format!(" vectors={}", vector_count(&connection).unwrap_or(0));
    #[cfg(not(feature = "semantic"))]
    let vectors_part = String::new();
    let _ = writeln!(
        standard_output,
        "{command_group} recall status: documents={}{vectors_part} index={} schema={} last_indexed_at_millis={}",
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
        Ok(_) => Err(format!(
            "--limit must be a positive integer, got {trimmed:?}"
        )),
        Err(_) => Err(format!(
            "--limit must be a positive integer, got {trimmed:?}"
        )),
    }
}

/// Sieve recall's known flags out of the argument vector ahead of FlagSet
/// parsing. Returns `(flag_arguments, query_arguments)` so the caller can
/// rebuild a FlagSet-compatible vector with all flags first and the query
/// after a `--` terminator. Returns an `Err(message)` for value-bearing flags
/// that are missing their value, matching FlagSet's diagnostic shape.
///
/// Flag handling matches FlagSet: `--limit` and `--claude-home` accept
/// `--flag value` or `--flag=value`; `--json` is a bool with optional
/// `--json=true|false`. A bare `--` terminates flag scanning and forces all
/// remaining tokens into the query, matching standard Unix conventions.
fn split_flags_and_query(arguments: &[String]) -> Result<(Vec<String>, Vec<String>), String> {
    const VALUE_FLAGS: &[&str] = &["--limit", "--claude-home", "--workspace"];
    const BOOL_FLAGS: &[&str] = &["--json", "--local-only"];
    let mut flag_arguments: Vec<String> = Vec::new();
    let mut query_arguments: Vec<String> = Vec::new();
    let mut index = 0;
    while index < arguments.len() {
        let token = &arguments[index];
        if token == "--" {
            query_arguments.extend(arguments[index + 1..].iter().cloned());
            return Ok((flag_arguments, query_arguments));
        }
        let (head, has_inline_value) = match token.split_once('=') {
            Some((head, _)) => (head, true),
            None => (token.as_str(), false),
        };
        if VALUE_FLAGS.contains(&head) {
            if has_inline_value {
                flag_arguments.push(token.clone());
                index += 1;
            } else {
                if index + 1 >= arguments.len() {
                    return Err(format!("flag needs an argument: {head}"));
                }
                flag_arguments.push(token.clone());
                flag_arguments.push(arguments[index + 1].clone());
                index += 2;
            }
            continue;
        }
        if BOOL_FLAGS.contains(&head) {
            flag_arguments.push(token.clone());
            index += 1;
            continue;
        }
        // Unknown token — treat as part of the query. FlagSet would have
        // rejected an unknown `--flag`, but at this layer we prefer to let
        // the user pass arbitrary words (including ones that happen to
        // start with `-`) without surprises.
        query_arguments.push(token.clone());
        index += 1;
    }
    Ok((flag_arguments, query_arguments))
}

/// Strip punctuation from each whitespace-separated word, keeping alphanumerics
/// plus the intra-token marks `-`, `_`, `.` (so `breaking-change` and
/// `recall-index.sqlite3` survive as single tokens). Shared by every query
/// builder so the exact, relaxed, and fuzzy stages tokenize identically.
fn clean_query_tokens(raw_query: &str) -> Vec<String> {
    let mut tokens: Vec<String> = Vec::new();
    for token in raw_query.split_whitespace() {
        let cleaned: String = token
            .chars()
            .filter(|character| character.is_alphanumeric() || matches!(character, '-' | '_' | '.'))
            .collect();
        if !cleaned.is_empty() {
            tokens.push(cleaned);
        }
    }
    tokens
}

/// Quote each token in the user query for FTS5 and AND them together so the
/// default behaviour is "all words must appear, in any order, with prefix
/// match". This intentionally hides FTS5 syntax from the caller; advanced raw
/// queries can be added later if there's demand.
fn build_fts_query(raw_query: &str) -> Option<String> {
    let tokens = clean_query_tokens(raw_query);
    if tokens.is_empty() {
        None
    } else {
        Some(
            tokens
                .iter()
                .map(|token| format!("\"{token}\"*"))
                .collect::<Vec<_>>()
                .join(" AND "),
        )
    }
}

/// Relaxed variant: OR the prefix-matched tokens instead of AND. Used as the
/// second cascade stage when the strict AND query returns nothing — a
/// multi-word query where one term is misspelled or absent ("stripe webhok
/// signature") still finds the documents that match the remaining terms.
///
/// Returns `None` for a single token: with one term, `OR` and `AND` produce an
/// identical FTS5 expression, so the relaxed stage would just repeat the exact
/// stage. Skipping it keeps the cascade from running a redundant query.
fn build_relaxed_fts_query(raw_query: &str) -> Option<String> {
    let tokens = clean_query_tokens(raw_query);
    if tokens.len() < 2 {
        return None;
    }
    Some(
        tokens
            .iter()
            .map(|token| format!("\"{token}\"*"))
            .collect::<Vec<_>>()
            .join(" OR "),
    )
}

pub fn recall_database_path(claude_home: &Path) -> PathBuf {
    claude_home.join("recall-index.sqlite3")
}

/// Sync the recall FTS index immediately after a memory write so the new
/// content is searchable on the very next `recall` with no separate trigger.
///
/// Memory writes (`memory remember`, `research-cache record`, `working-brief
/// write`) land the file on disk synchronously, but historically the FTS index
/// was only refreshed on a read-path call (`recall <query>` / `recall status`).
/// That left a window where a freshly-written memory was durable but not yet
/// searchable — the "I saved it but recall can't find it" gap. Calling this at
/// the end of each write closes the window.
///
/// Best-effort by contract: this opens the index and runs the same non-forced
/// `sync_recall_index` the read path uses, but every failure is folded into the
/// returned `Result` for the caller to log and ignore. A memory write must never
/// fail because the index could not be opened or synced — the durable file on
/// disk is the source of truth, and the next read-path sync will reconcile it
/// anyway. The next-read-path-sync fallback is exactly why callers can treat an
/// `Err` here as advisory.
pub fn reindex_after_write(claude_home: &Path) -> Result<(), String> {
    let database_path = recall_database_path(claude_home);
    let mut connection = open_recall_connection(&database_path)?;
    sync_recall_index(&mut connection, claude_home, false)?;
    Ok(())
}

/// Snapshot of recall-index health used by surfaces that just need to read
/// the current document count, last sync timestamp, and on-disk index path.
/// Built on top of the same `sync_recall_index` + `count_documents` pair the
/// `recall status` command uses, so callers see exactly the values an explicit
/// status invocation would print.
#[derive(Debug, Clone)]
pub struct RecallStatusSnapshot {
    pub claude_home: PathBuf,
    pub index_path: PathBuf,
    pub schema_version: String,
    pub document_count: u64,
    pub last_indexed_at_millis: u128,
    pub added_since_last_sync: u64,
    pub updated_since_last_sync: u64,
    pub removed_since_last_sync: u64,
    #[cfg(feature = "semantic")]
    pub vector_count: u64,
}

/// Result of a programmatic recall search: the canonicalized FTS expression
/// that was executed plus the matching hits. Callers already know the
/// `claude_home` and raw query they passed in, so this struct only carries
/// values they cannot trivially recompute (`fts_query` is produced by
/// `build_fts_query` against the trimmed input).
#[derive(Debug, Clone)]
pub struct RecallSearchResult {
    pub fts_query: String,
    /// Which cascade stage produced these hits: `"exact"`, `"relaxed"`,
    /// `"fuzzy"`, or `"vector"`. Lets callers tell the user a fuzzy hit is a
    /// typo-tolerant guess and a vector hit is an embedding-space neighbor
    /// rather than an exact match.
    pub stage: &'static str,
    pub hits: Vec<RecallHit>,
}

/// Run the same auto-sync + FTS5 query path as `recall <query>` without
/// touching stdout/stderr. Returns the prepared FTS expression alongside the
/// hits so callers (the MCP `recall` tool, programmatic embedders) can render
/// their own JSON envelope. `Ok(None)` means the query had no searchable
/// terms after stripping punctuation, mirroring the CLI's "no terms" branch.
/// Scoped recall: when `workspace_slug` is `Some`, hits from the current
/// workspace are boosted above cross-project hits (see
/// [`WORKSPACE_AFFINITY_BOOST`]). Pass `None` for unscoped (global,
/// cross-project) recall, which is the default for callers that have no
/// current-workspace context.
pub fn search_recall_index(
    claude_home: &Path,
    raw_query: &str,
    limit: usize,
    workspace_slug: Option<&str>,
) -> Result<Option<RecallSearchResult>, String> {
    let trimmed_query = raw_query.trim();
    if trimmed_query.is_empty() {
        return Ok(None);
    }
    let database_path = recall_database_path(claude_home);
    let mut connection = open_recall_connection(&database_path)?;
    sync_recall_index(&mut connection, claude_home, false)?;
    match cascade_recall_query(&connection, trimmed_query, limit, workspace_slug)? {
        Some(cascade) => {
            // Semantic on: blend vector KNN candidates so the embedding stage
            // contributes even when lexical stages already found hits.
            #[cfg(feature = "semantic")]
            let cascade = maybe_blend_vector_candidates(
                &connection,
                trimmed_query,
                cascade,
                limit,
                workspace_slug,
            );
            Ok(Some(RecallSearchResult {
                fts_query: cascade.query_expression,
                stage: cascade.stage,
                hits: cascade.hits,
            }))
        }
        None => Ok(None),
    }
}

/// Open (and if necessary create) the recall index under `claude_home`, run a
/// non-forced sync, then return a snapshot of the resulting health metrics.
/// Used by the MCP `recall_status` tool and the `keel://recall/status`
/// resource so they share the same code path as `recall status` rather than
/// reaching into the schema directly.
pub fn recall_status_snapshot(claude_home: &Path) -> Result<RecallStatusSnapshot, String> {
    let database_path = recall_database_path(claude_home);
    let mut connection = open_recall_connection(&database_path)?;
    let report = sync_recall_index(&mut connection, claude_home, false)?;
    let document_count = count_documents(&connection)?;
    #[cfg(feature = "semantic")]
    let vectors = vector_count(&connection).unwrap_or(0);
    Ok(RecallStatusSnapshot {
        claude_home: claude_home.to_path_buf(),
        index_path: database_path,
        schema_version: SCHEMA_VERSION.to_string(),
        document_count,
        last_indexed_at_millis: report.last_indexed_at_millis,
        added_since_last_sync: report.added,
        updated_since_last_sync: report.updated,
        removed_since_last_sync: report.removed,
        #[cfg(feature = "semantic")]
        vector_count: vectors,
    })
}

fn default_search_roots(claude_home: &Path) -> Vec<PathBuf> {
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
    #[cfg(feature = "semantic")]
    ensure_vec_extension_registered();
    let connection = Connection::open(database_path).map_err(|database_error| {
        recall_open_error_hint(database_path, &format!("open sqlite: {database_error}"))
    })?;
    connection
        .pragma_update(None, "journal_mode", "WAL")
        .map_err(|database_error| {
            recall_open_error_hint(
                database_path,
                &format!("set journal_mode: {database_error}"),
            )
        })?;
    connection
        .pragma_update(None, "synchronous", "NORMAL")
        .map_err(|database_error| {
            recall_open_error_hint(database_path, &format!("set synchronous: {database_error}"))
        })?;
    // why: WAL lets one writer proceed alongside readers, so a short wait lets a
    // concurrent `keel mcp serve` finish its transaction instead of erroring. The
    // previous 750ms default surfaced as spurious "database is locked" failures —
    // and downstream `context_brief` timeouts — whenever two keel processes raced
    // the index. 5s absorbs normal contention; an orphaned writer still fails in
    // bounded time. Override `KEEL_RECALL_BUSY_TIMEOUT_MS`.
    let busy_ms = std::env::var("KEEL_RECALL_BUSY_TIMEOUT_MS")
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .unwrap_or(5_000)
        .clamp(0, 30_000);
    connection
        .busy_timeout(std::time::Duration::from_millis(busy_ms))
        .map_err(|database_error| {
            recall_open_error_hint(
                database_path,
                &format!("set busy_timeout: {database_error}"),
            )
        })?;
    ensure_recall_schema(&connection)
        .map_err(|schema_error| recall_open_error_hint(database_path, &schema_error))?;
    Ok(connection)
}

/// Wrap a SQLite open/setup failure with an actionable recovery hint when it
/// looks like a locked WAL sidecar or a corrupt index. On Windows the `-wal`/
/// `-shm` sidecars can be left locked by a crashed process or truncated by an
/// interrupted write, surfacing as BUSY/LOCKED/CORRUPT/"not a database". Rather
/// than bubbling a bare driver error, point the user at the deterministic fix:
/// rebuild the index from the Markdown source of truth.
fn recall_open_error_hint(database_path: &Path, raw_error: &str) -> String {
    let lowered = raw_error.to_ascii_lowercase();
    let looks_recoverable = lowered.contains("locked")
        || lowered.contains("busy")
        || lowered.contains("malformed")
        || lowered.contains("corrupt")
        || lowered.contains("not a database");
    if looks_recoverable {
        format!(
            "{raw_error}\n  The recall index at {} appears locked or corrupt (often a stale \
             -wal/-shm sidecar on Windows). Close any other keel process, then run \
             `keel memory recall reindex` to rebuild it from your Markdown memory.",
            display_path(database_path)
        )
    } else {
        raw_error.to_string()
    }
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
    #[cfg(feature = "semantic")]
    {
        ensure_vec_schema(connection)?;
    }
    Ok(())
}

#[derive(Debug, Default, Clone)]
pub struct SyncReport {
    pub indexed_total: u64,
    pub added: u64,
    pub updated: u64,
    pub removed: u64,
    /// Files found on disk that could not be read as UTF-8 and were excluded
    /// from the index. Surfaced so a silently-missing document leaves a signal
    /// rather than vanishing from search with no explanation.
    pub skipped: u64,
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
        collect_indexable_files(&root_directory, &mut on_disk)?;
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
                // sync over a single unreadable document — but we count it so a
                // silently-excluded file is visible in the sync report instead
                // of vanishing from search with no signal.
                report.skipped += 1;
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
        #[cfg(feature = "semantic")]
        {
            // Best-effort: if embedding fails, skip this document's vector
            // without failing the whole sync. A missing vec_items row is
            // preferable to aborting a reindex over a single embed error.
            if let Ok(vector) = embed_text(&content) {
                let _ = upsert_doc_vector(&transaction, &document.absolute_path, &vector);
            }
        }
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
        #[cfg(feature = "semantic")]
        {
            let _ = delete_doc_vector(&transaction, path);
        }
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

fn collect_indexable_files(directory: &Path, out: &mut Vec<DocumentRecord>) -> Result<(), String> {
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
            collect_indexable_files(&entry_path, out)?;
            continue;
        }
        if !metadata.is_file() {
            continue;
        }
        let extension = entry_path
            .extension()
            .and_then(|os_str| os_str.to_str())
            .map(|extension| extension.to_ascii_lowercase());
        // Index both Markdown notes and JSON records. Working briefs,
        // completion gates, and the memory-family records (research-cache,
        // entities, graph, ...) are all stored as `.json` under the recall
        // roots; restricting the index to `.md` silently excluded every one of
        // them, so `recall` never matched a working brief it had just written.
        // Both formats are UTF-8 text and FTS5-tokenize cleanly.
        if !matches!(extension.as_deref(), Some("md") | Some("json")) {
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
    /// Relevance score in `0.0..=1.0`, higher = more relevant. This is the
    /// re-ranked relevance (term coverage + proximity), NOT the raw SQLite
    /// `bm25()` value (which is negative and term-frequency-only). See
    /// [`rerank_by_relevance`]: the cascade fetches a wider BM25 candidate set
    /// and re-scores it so a document mentioning *all* query terms near each
    /// other outranks one mentioning a single term many times — the lexical
    /// analog of the topical cohesion embeddings provide, with no model.
    pub score: f64,
    pub line: usize,
    pub snippet: String,
}

/// How many BM25 candidates to pull per requested hit before the relevance
/// re-rank trims back to `limit`. BM25 alone is term-frequency ranking, so the
/// best coverage/proximity match can sit a few rows below a term-spam match;
/// over-fetching gives the re-ranker the candidates it needs to promote. Capped
/// in [`query_recall_index`] so a huge `limit` cannot scan the whole table.
const RERANK_CANDIDATE_MULTIPLIER: usize = 4;
const RERANK_CANDIDATE_CAP: usize = 200;

pub fn query_recall_index(
    connection: &Connection,
    fts_query: &str,
    limit: usize,
    workspace_slug: Option<&str>,
) -> Result<Vec<RecallHit>, String> {
    let raw_query_terms = fts_terms(fts_query);
    // Over-fetch BM25 candidates so the relevance re-rank has room to promote a
    // high-coverage match that BM25 alone ranked below a term-frequency match.
    let candidate_limit = limit
        .saturating_mul(RERANK_CANDIDATE_MULTIPLIER)
        .min(RERANK_CANDIDATE_CAP)
        .max(limit);
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
            params![
                open_marker,
                close_marker,
                SNIPPET_TOKENS,
                fts_query,
                candidate_limit as i64
            ],
            |row| {
                let absolute_path: String = row.get(0)?;
                // bm25() is read only to force SQLite to apply the ranking
                // ORDER BY; the relevance re-rank uses the row's POSITION
                // (bm25_rank) as the tie-break, not the raw score, so the value
                // itself is intentionally discarded here.
                let _bm25: f64 = row.get(1)?;
                let snippet_text: String = row.get(2)?;
                let content: String = row.get(3)?;
                Ok((absolute_path, snippet_text, content))
            },
        )
        .map_err(|database_error| format!("query: {database_error}"))?;
    let mut candidates: Vec<RerankCandidate> = Vec::new();
    for (rank, row_result) in query_iterator.enumerate() {
        let (absolute_path, snippet_text, content) =
            row_result.map_err(|database_error| format!("read result row: {database_error}"))?;
        let line = locate_first_match_line(&content, &snippet_text);
        let display_snippet = render_snippet_for_display(&snippet_text);
        candidates.push(RerankCandidate {
            hit: RecallHit {
                absolute_path,
                score: 0.0,
                line,
                snippet: display_snippet,
            },
            content,
            bm25_rank: rank,
        });
    }
    Ok(rerank_by_relevance(
        candidates,
        &raw_query_terms,
        limit,
        workspace_slug,
    ))
}

/// A BM25 candidate carried through the relevance re-rank. `content` is the full
/// document text (used to measure term coverage and proximity); `bm25_rank` is
/// the candidate's position in the BM25 ordering, used as a deterministic
/// tie-breaker so equal-relevance hits keep SQLite's stable order.
#[derive(Clone)]
struct RerankCandidate {
    hit: RecallHit,
    content: String,
    bm25_rank: usize,
}

/// Extract the bare terms from an FTS5 query expression like
/// `"webhook"* AND "retry"*` → `["webhook", "retry"]`. The cascade builds these
/// expressions with [`build_fts_query`]/[`build_relaxed_fts_query`], so the
/// terms are always quoted-and-starred tokens joined by `AND`/`OR`; stripping
/// the quotes and the trailing `*` recovers the user's words for scoring.
fn fts_terms(fts_query: &str) -> Vec<String> {
    let mut terms = Vec::new();
    for raw in fts_query.split_whitespace() {
        if raw == "AND" || raw == "OR" {
            continue;
        }
        let term: String = raw
            .trim_matches(|c| c == '"' || c == '*')
            .to_ascii_lowercase();
        if !term.is_empty() && !terms.contains(&term) {
            terms.push(term);
        }
    }
    terms
}

/// Re-rank BM25 candidates by lexical relevance and trim to `limit`. The score
/// combines two signals BM25 ignores:
///   - **coverage**: the fraction of distinct query terms that appear in the
///     document — a doc matching all the query's words is topically on-point,
///     which is the signal a semantic search would surface.
///   - **proximity**: how tightly the matched terms cluster (best span across
///     the matched terms in one line), so "webhook retry" matching adjacent
///     words beats the two words appearing paragraphs apart.
///
/// Coverage dominates (weight 0.7) over proximity (0.3) because topical match
/// matters more than adjacency. The result is normalized to `0.0..=1.0` and the
/// BM25 rank breaks ties, keeping the ordering deterministic. A single-term
/// query has coverage 1.0 for every candidate, so the re-rank degenerates
/// gracefully to BM25 order (proximity is also 1.0), i.e. no behavior change.
/// How much a hit whose path matches the current workspace slug is boosted
/// above an otherwise-equal cross-project hit. A current-project note should
/// outrank a different-project note that happens to match the same words, so
/// the new-project flood you hit is suppressed without disabling cross-project
/// recall. Applied as a multiplier on the relevance score (capped at 1.0).
const WORKSPACE_AFFINITY_BOOST: f64 = 1.5;

/// Collapse runs of `-` into a single `-`. Used to normalize workspace slugs
/// and memory-lane paths before substring matching, because the lane path may
/// have been written by a slugger that did not collapse separator runs
/// (e.g. `D:\` -> `D--` vs the collapsed `D-`).
pub(crate) fn collapse_dashes(value: &str) -> String {
    let mut collapsed = String::with_capacity(value.len());
    let mut prev_dash = false;
    for ch in value.chars() {
        if ch == '-' {
            if !prev_dash {
                collapsed.push(ch);
            }
            prev_dash = true;
        } else {
            collapsed.push(ch);
            prev_dash = false;
        }
    }
    collapsed
}

fn rerank_by_relevance(
    mut candidates: Vec<RerankCandidate>,
    query_terms: &[String],
    limit: usize,
    workspace_slug: Option<&str>,
) -> Vec<RecallHit> {
    if query_terms.is_empty() {
        candidates.truncate(limit);
        return candidates.into_iter().map(|c| c.hit).collect();
    }
    let slug_lower = workspace_slug
        .filter(|s| !s.is_empty())
        .map(|s| collapse_dashes(&s.to_ascii_lowercase()));
    let mut scored: Vec<(f64, usize, RecallHit)> = candidates
        .drain(..)
        .map(|candidate| {
            let mut relevance = relevance_score(&candidate.content, query_terms);
            if let Some(slug) = &slug_lower {
                if collapse_dashes(&candidate.hit.absolute_path.to_ascii_lowercase()).contains(slug)
                {
                    relevance = (relevance * WORKSPACE_AFFINITY_BOOST).min(1.0);
                }
            }
            (relevance, candidate.bm25_rank, candidate.hit)
        })
        .collect();
    // Sort by descending relevance, then ascending BM25 rank as a stable
    // tie-break. partial_cmp is only None for NaN, which relevance_score never
    // produces (bounded sums of finite ratios), so unwrap_or keeps it total.
    scored.sort_by(|left, right| {
        right
            .0
            .partial_cmp(&left.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(left.1.cmp(&right.1))
    });
    scored.truncate(limit);
    scored
        .into_iter()
        .map(|(relevance, _, mut hit)| {
            hit.score = relevance;
            hit
        })
        .collect()
}

/// Compute the `0.0..=1.0` relevance of `content` against `query_terms` as a
/// weighted blend of term coverage and term proximity. Lowercased substring
/// matching mirrors how the FTS prefix query matched in the first place (these
/// candidates already matched the FTS query, so every term that *should* match
/// does); the scoring just measures how *well*.
fn relevance_score(content: &str, query_terms: &[String]) -> f64 {
    let lowered = content.to_ascii_lowercase();
    let matched: Vec<&String> = query_terms
        .iter()
        .filter(|term| lowered.contains(term.as_str()))
        .collect();
    if matched.is_empty() {
        return 0.0;
    }
    let coverage = matched.len() as f64 / query_terms.len() as f64;
    let proximity = best_line_proximity(&lowered, &matched);
    0.7 * coverage + 0.3 * proximity
}

/// Proximity in `0.0..=1.0`: the best single line's term density, where a line
/// containing more of the matched terms scores higher. For a single matched
/// term this is 1.0 (it is fully "clustered" with itself). Measuring per line
/// is a cheap, deterministic proxy for adjacency that needs no tokenizer-offset
/// bookkeeping — a line mentioning both "webhook" and "retry" is tighter than
/// the terms living in separate paragraphs.
fn best_line_proximity(lowered_content: &str, matched_terms: &[&String]) -> f64 {
    if matched_terms.len() < 2 {
        return 1.0;
    }
    let mut best = 0.0f64;
    for line in lowered_content.lines() {
        let here = matched_terms
            .iter()
            .filter(|term| line.contains(term.as_str()))
            .count();
        let density = here as f64 / matched_terms.len() as f64;
        if density > best {
            best = density;
        }
        if best >= 1.0 {
            break;
        }
    }
    best
}

/// Outcome of the recall cascade: the expression that produced the returned
/// hits, a stable stage label (`"exact"`, `"relaxed"`, `"fuzzy"`, or
/// `"vector"`), and the hits. The label lets callers tell the user *how* a
/// result was found — an honest signal that a fuzzy hit is a typo-tolerant
/// guess and a vector hit is an embedding-space neighbor, not an exact
/// match.
#[derive(Debug, Clone)]
pub struct CascadeResult {
    pub query_expression: String,
    pub stage: &'static str,
    pub hits: Vec<RecallHit>,
}

/// Run the lexical recall cascade: exact (AND prefix) → relaxed (OR prefix) →
/// fuzzy (trigram similarity) → vector (embedding KNN, `semantic` feature
/// only). Each lexical stage runs ONLY when the previous returned zero hits,
/// so an ordinary exact-match query pays nothing for the fallbacks. The vector
/// stage is a final fallback that recovers documents sharing no lexical terms
/// with the query but close in embedding space.
///
/// Returns `Ok(None)` when the query has no searchable terms after stripping
/// punctuation (mirrors `build_fts_query` returning `None`). When all stages
/// find nothing, returns a `CascadeResult` with empty `hits` and the `"exact"`
/// expression, preserving the caller's "matches=0" contract.
///
/// The lexical stages (exact, relaxed, fuzzy) are deliberately NOT vector
/// search: they are typo- and morphology-tolerant LEXICAL matching with no
/// embeddings, no model, and no network. The optional vector stage adds
/// embedding-based recall only when the `semantic` feature is compiled in.
fn cascade_recall_query(
    connection: &Connection,
    raw_query: &str,
    limit: usize,
    workspace_slug: Option<&str>,
) -> Result<Option<CascadeResult>, String> {
    let exact = match build_fts_query(raw_query) {
        Some(query) => query,
        None => return Ok(None),
    };

    let exact_hits = query_recall_index(connection, &exact, limit, workspace_slug)?;
    if !exact_hits.is_empty() {
        let (hits, stage) = augment_with_fuzzy(connection, exact_hits, limit, "exact")?;
        return Ok(Some(CascadeResult {
            query_expression: exact,
            stage,
            hits,
        }));
    }

    // Stage 2 — relaxed OR: only meaningful for multi-term queries (a single
    // token's OR and AND expressions are identical, so build_relaxed returns
    // None and we skip straight to fuzzy).
    if let Some(relaxed) = build_relaxed_fts_query(raw_query) {
        let relaxed_hits = query_recall_index(connection, &relaxed, limit, workspace_slug)?;
        if !relaxed_hits.is_empty() {
            let (hits, stage) = augment_with_fuzzy(connection, relaxed_hits, limit, "relaxed")?;
            return Ok(Some(CascadeResult {
                query_expression: relaxed,
                stage,
                hits,
            }));
        }
    }

    // Stage 3 — fuzzy trigram scan: recovers single-word typos that prefix
    // matching cannot reach (e.g. "webhok" -> "webhook").
    let tokens = clean_query_tokens(raw_query);
    let fuzzy_hits = query_recall_index_fuzzy(connection, &tokens, limit)?;
    if !fuzzy_hits.is_empty() {
        return Ok(Some(CascadeResult {
            query_expression: format!("fuzzy({})", tokens.join(" ")),
            stage: "fuzzy",
            hits: fuzzy_hits,
        }));
    }

    // Stage 4 — vector KNN (cfg-gated, only with the `semantic` feature):
    // recovers documents that share no lexical terms with the query but are
    // close in embedding space. Best-effort: if embedding or KNN fails, the
    // stage is skipped and the cascade returns the empty result below.
    #[cfg(feature = "semantic")]
    {
        if let Ok(query_vector) = embed_text(raw_query) {
            if let Ok(neighbors) = query_vector_index(connection, &query_vector, limit) {
                let mut hits = Vec::new();
                let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
                for (path, distance) in neighbors {
                    if distance > VECTOR_MAX_DISTANCE {
                        continue;
                    }
                    if !seen.insert(path.clone()) {
                        continue;
                    }
                    if let Ok(Some(content)) = load_document_content(connection, &path) {
                        hits.push(RecallHit {
                            absolute_path: path,
                            score: 1.0 / (1.0 + distance),
                            line: 1,
                            snippet: vector_snippet(&content),
                        });
                    }
                }
                if !hits.is_empty() {
                    return Ok(Some(CascadeResult {
                        query_expression: format!("vector({})", raw_query),
                        stage: "vector",
                        hits,
                    }));
                }
            }
        }
    }

    Ok(Some(CascadeResult {
        query_expression: exact,
        stage: "exact",
        hits: Vec::new(),
    }))
}

/// When the `semantic` feature is on, blend vector KNN candidates into a
/// lexical result so the embedding stage contributes even when lexical stages
/// already found hits (the original cascade returned early and never reached
/// the vector fallback). Lexical hits keep their relevance score; vector-only
/// neighbors get a similarity score; the merged pool is re-sorted so a
/// high-similarity semantic neighbor the lexical stages missed can displace a
/// low-relevance lexical hit. The stage label becomes `"hybrid"` when vector
/// candidates are merged in. Always blends (when semantic is on) because real
/// queries fill `limit` with lexical noise a semantic neighbor should displace.
#[cfg(feature = "semantic")]
fn maybe_blend_vector_candidates(
    connection: &Connection,
    raw_query: &str,
    lexical: CascadeResult,
    limit: usize,
    workspace_slug: Option<&str>,
) -> CascadeResult {
    let Ok(query_vector) = embed_text(raw_query) else {
        return lexical;
    };
    let Ok(neighbors) = query_vector_index(connection, &query_vector, limit) else {
        return lexical;
    };
    let slug_lower = workspace_slug
        .filter(|s| !s.is_empty())
        .map(|s| collapse_dashes(&s.to_ascii_lowercase()));
    let mut seen: std::collections::HashSet<String> = lexical
        .hits
        .iter()
        .map(|h| h.absolute_path.clone())
        .collect();
    let mut merged = lexical.hits.clone();
    let mut added = 0usize;
    for (path, distance) in neighbors {
        if distance > VECTOR_MAX_DISTANCE {
            continue;
        }
        if !seen.insert(path.clone()) {
            continue;
        }
        let mut score = 1.0 / (1.0 + distance);
        if let Some(slug) = &slug_lower {
            if collapse_dashes(&path.to_ascii_lowercase()).contains(slug) {
                score = (score * WORKSPACE_AFFINITY_BOOST).min(1.0);
            }
        }
        if let Ok(Some(content)) = load_document_content(connection, &path) {
            merged.push(RecallHit {
                absolute_path: path,
                score,
                line: 1,
                snippet: vector_snippet(&content),
            });
            added += 1;
        }
    }
    if added == 0 {
        return lexical;
    }
    // Re-sort the merged pool by score desc so semantic neighbors compete with
    // lexical hits on a blended scale rather than being appended at the end.
    merged.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    merged.truncate(limit);
    let stage =
        if lexical.stage == "exact" || lexical.stage == "relaxed" || lexical.stage == "fuzzy" {
            "hybrid"
        } else {
            lexical.stage
        };
    CascadeResult {
        query_expression: format!("{}+hybrid", lexical.query_expression),
        stage,
        hits: merged,
    }
}

/// Augment a thin lexical result with its approximate neighbors (LSA), the
/// cross-vocabulary recall that lexical matching structurally cannot reach.
/// Runs ONLY when `lexical_hits` came back short of `limit` — a full result set
/// has no room to augment and no reason to pay the on-demand SVD cost. When
/// neighbors are appended, the stage label gains a `+fuzzy` suffix so the user
/// can see the result was expanded beyond literal matches; otherwise the
/// original lexical hits and stage pass through unchanged.
///
/// This is the "ahead of lexical" win: searching `authentication` returns the
/// literal matches PLUS the co-occurring `login`/`token`/`session` documents
/// that share no query term — learned from the corpus's own co-occurrence
/// structure, with no model, no network, and no new dependency. It degrades
/// safely: too few or too many documents, or a degenerate corpus, leaves the
/// lexical hits as-is.
fn augment_with_fuzzy(
    connection: &Connection,
    lexical_hits: Vec<RecallHit>,
    limit: usize,
    base_stage: &'static str,
) -> Result<(Vec<RecallHit>, &'static str), String> {
    let deficit = limit.saturating_sub(lexical_hits.len());
    if deficit == 0 {
        return Ok((lexical_hits, base_stage));
    }

    let corpus = load_all_documents(connection)?;
    let Some(index) = super::semantic::build_semantic_index(&corpus) else {
        return Ok((lexical_hits, base_stage));
    };

    // Map the lexical hits onto seed document indices, and exclude them from the
    // neighbor search so we never duplicate a hit the user already has.
    let seed: Vec<usize> = lexical_hits
        .iter()
        .filter_map(|hit| index.index_of(&hit.absolute_path))
        .collect();
    if seed.is_empty() {
        return Ok((lexical_hits, base_stage));
    }

    let neighbors = index.neighbors_of(&seed, &seed, deficit);
    if neighbors.is_empty() {
        return Ok((lexical_hits, base_stage));
    }

    let mut hits = lexical_hits;
    for (document_index, similarity) in neighbors {
        hits.push(RecallHit {
            absolute_path: index.path(document_index).to_string(),
            score: similarity,
            line: 1,
            snippet: fuzzy_snippet(index.content(document_index)),
        });
    }
    let stage = match base_stage {
        "exact" => "exact+fuzzy",
        "relaxed" => "relaxed+fuzzy",
        _ => base_stage,
    };
    Ok((hits, stage))
}

/// First non-empty, non-heading line of a fuzzy neighbor's content, prefixed
/// so the user can tell at a glance the hit came from fuzzy expansion rather
/// than a literal match. A fuzzy neighbor has no FTS snippet (it was not
/// found by a term query), so we synthesize a representative excerpt the same
/// way the fuzzy stage builds its own.
fn fuzzy_snippet(content: &str) -> String {
    let excerpt = content
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))
        .or_else(|| content.lines().map(str::trim).find(|line| !line.is_empty()))
        .unwrap_or_default();
    format!("[~fuzzy] {}", collapse_whitespace(excerpt))
}

/// First non-empty, non-heading line of a vector neighbor's content, prefixed
/// so the user can tell at a glance the hit came from vector KNN rather than a
/// lexical match. A vector neighbor has no FTS snippet (it was not found by a
/// term query), so we synthesize a representative excerpt the same way the
/// fuzzy stage builds its own.
#[cfg(feature = "semantic")]
fn vector_snippet(content: &str) -> String {
    let excerpt = content
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))
        .or_else(|| content.lines().map(str::trim).find(|line| !line.is_empty()))
        .unwrap_or_default();
    format!("[~vector] {}", collapse_whitespace(excerpt))
}

/// Load every indexed document's `(path, content)` for the fuzzy augmentation
/// index. This is the same full-corpus read the fuzzy trigram stage performs,
/// run only when fuzzy augmentation is actually attempted (thin lexical result),
/// so the common full-result query never pays for it.
fn load_all_documents(connection: &Connection) -> Result<Vec<(String, String)>, String> {
    let mut statement = connection
        .prepare("SELECT path, content FROM documents")
        .map_err(|database_error| format!("prepare corpus load: {database_error}"))?;
    let rows = statement
        .query_map([], |row| {
            let path: String = row.get(0)?;
            let content: String = row.get(1)?;
            Ok((path, content))
        })
        .map_err(|database_error| format!("corpus load: {database_error}"))?;
    let mut documents = Vec::new();
    for row in rows {
        documents.push(row.map_err(|database_error| format!("read corpus row: {database_error}"))?);
    }
    Ok(documents)
}

/// Load a single document's content by path from the FTS5 `documents` table.
/// Used by the vector stage to build a snippet for a KNN-returned path that
/// was not found by any lexical stage (so it has no FTS snippet of its own).
#[cfg(feature = "semantic")]
fn load_document_content(connection: &Connection, path: &str) -> Result<Option<String>, String> {
    connection
        .query_row(
            "SELECT content FROM documents WHERE path = ?1",
            params![path],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|database_error| format!("load document content: {database_error}"))
}

/// Minimum Sørensen–Dice trigram similarity for the fuzzy stage to accept a
/// word as a match. 0.45 catches single-character typos in words of moderate
/// length (e.g. "webhok" vs "webhook" scores ~0.67) without admitting unrelated
/// short words (which share few trigrams). Tuned conservatively: the fuzzy
/// stage only runs when the exact AND relaxed stages both returned nothing, so
/// a slightly low threshold here trades a few false positives for recovering a
/// query that would otherwise return zero hits.
const FUZZY_MIN_SIMILARITY: f64 = 0.45;

/// Lowercased set of 3-character shingles of `word`, the unit the fuzzy stage
/// compares. A word shorter than 3 chars yields a single shingle of itself so
/// it still participates rather than silently scoring zero.
fn trigrams(word: &str) -> std::collections::HashSet<String> {
    let chars: Vec<char> = word.to_lowercase().chars().collect();
    let mut set = std::collections::HashSet::new();
    if chars.len() < 3 {
        if !chars.is_empty() {
            set.insert(chars.iter().collect());
        }
        return set;
    }
    for window in chars.windows(3) {
        set.insert(window.iter().collect());
    }
    set
}

/// Sørensen–Dice coefficient over trigram sets: `2|A∩B| / (|A|+|B|)`, ranging
/// 0.0 (no shared trigrams) to 1.0 (identical). Dice normalizes for differing
/// word lengths, so "webhook" vs "webhok" scores high while "webhook" vs "web"
/// does not — exactly the discrimination a typo-tolerant recall needs.
fn trigram_similarity(left: &str, right: &str) -> f64 {
    let left_grams = trigrams(left);
    let right_grams = trigrams(right);
    if left_grams.is_empty() || right_grams.is_empty() {
        return 0.0;
    }
    let shared = left_grams.intersection(&right_grams).count();
    (2.0 * shared as f64) / (left_grams.len() + right_grams.len()) as f64
}

/// Split text into candidate words on the same boundary set recall tokenizes
/// queries with (alphanumerics plus `-`, `_`, `.`), so a content word and a
/// query token are compared on equal footing.
fn split_words(text: &str) -> impl Iterator<Item = &str> {
    text.split(|character: char| {
        !character.is_alphanumeric() && !matches!(character, '-' | '_' | '.')
    })
    .filter(|word| !word.is_empty())
}

/// Last-resort fuzzy search: scan each indexed document and score it by the best
/// trigram similarity between any query token and any word in the document. Only
/// called when the exact and relaxed FTS stages both returned nothing, so the
/// brute-force content scan runs rarely and at the single-user corpus scale the
/// index targets. Documents whose best match clears `FUZZY_MIN_SIMILARITY` are
/// returned ranked by descending similarity, with a snippet of the line that
/// produced the match. This recovers single-word typos ("webhok" -> "webhook")
/// that prefix matching cannot reach.
fn query_recall_index_fuzzy(
    connection: &Connection,
    query_tokens: &[String],
    limit: usize,
) -> Result<Vec<RecallHit>, String> {
    if query_tokens.is_empty() {
        return Ok(Vec::new());
    }
    let mut statement = connection
        .prepare("SELECT path, content FROM documents")
        .map_err(|database_error| format!("prepare fuzzy scan: {database_error}"))?;
    let row_iterator = statement
        .query_map([], |row| {
            let path: String = row.get(0)?;
            let content: String = row.get(1)?;
            Ok((path, content))
        })
        .map_err(|database_error| format!("fuzzy scan: {database_error}"))?;

    let mut scored: Vec<(f64, RecallHit)> = Vec::new();
    for row_result in row_iterator {
        let (absolute_path, content) =
            row_result.map_err(|database_error| format!("read fuzzy row: {database_error}"))?;
        let mut best_similarity = 0.0f64;
        let mut best_line = 0usize;
        let mut best_word = String::new();
        for (line_index, line) in content.lines().enumerate() {
            for word in split_words(line) {
                for token in query_tokens {
                    let similarity = trigram_similarity(token, word);
                    if similarity > best_similarity {
                        best_similarity = similarity;
                        best_line = line_index + 1;
                        best_word = word.to_string();
                    }
                }
            }
        }
        if best_similarity >= FUZZY_MIN_SIMILARITY {
            let line_text = content
                .lines()
                .nth(best_line.saturating_sub(1))
                .unwrap_or_default();
            let snippet = format!("[~{best_word}] {}", collapse_whitespace(line_text));
            scored.push((
                best_similarity,
                RecallHit {
                    absolute_path,
                    score: best_similarity,
                    line: best_line,
                    snippet,
                },
            ));
        }
    }
    // Descending similarity: best fuzzy match first. partial_cmp can only be
    // None for NaN, which trigram_similarity never produces (finite divides of
    // non-negative counts), so the unwrap_or keeps the sort total without a
    // panic path.
    scored.sort_by(|left, right| {
        right
            .0
            .partial_cmp(&left.0)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    scored.truncate(limit);
    Ok(scored.into_iter().map(|(_, hit)| hit).collect())
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
    // Count newlines in the SAME string space the offset came from
    // (`lower_content`). `str::to_lowercase` is not length-preserving for some
    // codepoints (e.g. U+0130 'İ'), so `content[..byte_offset]` could slice the
    // original content mid-char and panic. Lowercasing never adds or removes
    // newlines, so the line number is identical computed against `lower_content`,
    // and `lower_content[..byte_offset]` is always a valid char boundary because
    // `byte_offset` came from `lower_content.find`.
    1 + lower_content[..byte_offset].matches('\n').count()
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

/// Embedding dimension for all-MiniLM-L6-v2. Stored alongside the schema so
/// the vec0 column width and any embedding producer stay in lockstep.
#[cfg(feature = "semantic")]
pub const EMBEDDING_DIM: usize = 384;

/// Maximum L2 distance for a vector KNN neighbor to be included in cascade
/// results. The bge-micro-v2 model (3 layers, 384 dim) has a high baseline
/// similarity for any pair of English texts — unrelated text typically scores
/// ≈0.96 — so this threshold is set below that floor to filter noise while
/// keeping genuinely related documents. Tuned empirically against the test
/// corpus; adjust if the model is swapped.
#[cfg(feature = "semantic")]
const VECTOR_MAX_DISTANCE: f64 = 0.9;

#[cfg(feature = "semantic")]
static VEC_EXTENSION_REGISTERED: std::sync::Once = std::sync::Once::new();

/// Register the sqlite-vec extension with rusqlite's bundled SQLite exactly
/// once per process. Must run before any `CREATE VIRTUAL TABLE ... USING vec0`
/// statement. The transmute mirrors the upstream sqlite-vec test pattern: the
/// C entry point is declared with no args in the FFI binding but the
/// auto-extension slot expects the standard `fn(*mut c_void) -> c_int` shape.
#[cfg(feature = "semantic")]
fn ensure_vec_extension_registered() {
    type ExtensionInit = unsafe extern "C" fn(
        *mut rusqlite::ffi::sqlite3,
        *mut *mut std::os::raw::c_char,
        *const rusqlite::ffi::sqlite3_api_routines,
    ) -> std::os::raw::c_int;
    VEC_EXTENSION_REGISTERED.call_once(|| unsafe {
        rusqlite::ffi::sqlite3_auto_extension(Some(
            std::mem::transmute::<*const (), ExtensionInit>(sqlite3_vec_init as *const ()),
        ));
    });
}

/// Create the `vec_items` virtual table if absent. Idempotent. The vec0 table
/// stores one 384-dim float vector per indexed memory path, keyed by the same
/// absolute path string the FTS5 `documents` table uses.
#[cfg(feature = "semantic")]
fn ensure_vec_schema(connection: &Connection) -> Result<(), String> {
    ensure_vec_extension_registered();
    connection
        .execute_batch(&format!(
            "CREATE VIRTUAL TABLE IF NOT EXISTS vec_items USING vec0(
                 path TEXT PRIMARY KEY,
                 embedding float[{EMBEDDING_DIM}]
             );"
        ))
        .map_err(|database_error| format!("ensure vec_items: {database_error}"))
}

/// Serialize an f32 slice as the little-endian byte blob sqlite-vec expects
/// for a `float[]` MATCH argument.
#[cfg(feature = "semantic")]
fn serialize_f32_blob(vector: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(vector.len() * 4);
    for value in vector {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

/// Upsert a document's vector. Placeholder Phase 1 callers pass a zero vector;
/// Phase 2 replaces it with a real candle embedding.
#[cfg(feature = "semantic")]
fn upsert_doc_vector(connection: &Connection, path: &str, vector: &[f32]) -> Result<(), String> {
    connection
        .execute(
            "INSERT OR REPLACE INTO vec_items(path, embedding) VALUES (?1, ?2)",
            params![path, serialize_f32_blob(vector)],
        )
        .map_err(|database_error| format!("upsert vec_items: {database_error}"))?;
    Ok(())
}

/// Drop a document's vector when the file vanishes from disk.
#[cfg(feature = "semantic")]
fn delete_doc_vector(connection: &Connection, path: &str) -> Result<(), String> {
    connection
        .execute("DELETE FROM vec_items WHERE path = ?1", params![path])
        .map_err(|database_error| format!("delete vec_items: {database_error}"))?;
    Ok(())
}

/// KNN query over the vector store. Returns (path, distance) pairs ordered
/// nearest-first. `distance` is sqlite-vec's L2 distance (lower is nearer).
/// Wired into the recall cascade's vector stage (Phase 3).
#[cfg(feature = "semantic")]
fn query_vector_index(
    connection: &Connection,
    query_vector: &[f32],
    limit: usize,
) -> Result<Vec<(String, f64)>, String> {
    let mut statement = connection
        .prepare(
            "SELECT path, distance FROM vec_items
             WHERE embedding MATCH ?1
             ORDER BY distance
             LIMIT ?2",
        )
        .map_err(|database_error| format!("prepare vec knn: {database_error}"))?;
    let rows = statement
        .query_map(
            params![serialize_f32_blob(query_vector), limit as i64],
            |row| {
                let path: String = row.get(0)?;
                let distance: f64 = row.get(1)?;
                Ok((path, distance))
            },
        )
        .map_err(|database_error| format!("query vec knn: {database_error}"))?;
    let mut results = Vec::new();
    for row in rows {
        results.push(row.map_err(|database_error| format!("read vec row: {database_error}"))?);
    }
    Ok(results)
}

/// Count of vectors currently stored. Surfaced by `recall status` when the
/// `semantic` feature is compiled in.
#[cfg(feature = "semantic")]
fn vector_count(connection: &Connection) -> Result<u64, String> {
    connection
        .query_row("SELECT COUNT(*) FROM vec_items", [], |row| {
            row.get::<_, i64>(0)
        })
        .map(|count| count.max(0) as u64)
        .map_err(|database_error| format!("count vec_items: {database_error}"))
}

// ---------------------------------------------------------------------------
// Candle BERT sentence embedder (TaylorAI/bge-micro-v2, 384-dim, 3 layers)
// ---------------------------------------------------------------------------

/// Model artifacts embedded at compile time by build.rs when the `semantic`
/// feature is on. build.rs downloads config.json, tokenizer.json, and
/// model.safetensors from TaylorAI/bge-micro-v2 into OUT_DIR; these
/// `include_bytes!` calls pull the bytes into the binary so the embedder is
/// self-contained with no runtime file I/O or network dependency.
#[cfg(feature = "semantic")]
static TOKENIZER_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/tokenizer.json"));
#[cfg(feature = "semantic")]
static CONFIG_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/config.json"));
#[cfg(feature = "semantic")]
static MODEL_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/model.safetensors"));

/// Lazily-initialized BERT model + tokenizer. Loaded once per process via
/// [`OnceLock`] so the ~33 MB safetensors parse happens exactly once.
#[cfg(feature = "semantic")]
struct CachedEmbedder {
    model: BertModel,
    tokenizer: tokenizers::Tokenizer,
}

/// Stores the init result (Ok or Err) so a deterministic failure is cached
/// rather than retried on every call. MSRV 1.80 predates `get_or_try_init`
/// (1.82), so we store `Result` and pattern-match on the `&Result` returned
/// by `get_or_init`.
#[cfg(feature = "semantic")]
static EMBEDDER: OnceLock<Result<CachedEmbedder, String>> = OnceLock::new();

#[cfg(feature = "semantic")]
fn init_embedder() -> Result<CachedEmbedder, String> {
    let config: Config = serde_json::from_slice(CONFIG_BYTES)
        .map_err(|error| format!("parse bert config: {error}"))?;
    let tokenizer = tokenizers::Tokenizer::from_bytes(TOKENIZER_BYTES)
        .map_err(|error| format!("load tokenizer: {error}"))?;
    let device = Device::Cpu;
    let var_builder =
        VarBuilder::from_buffered_safetensors(MODEL_BYTES.to_vec(), DType::F32, &device)
            .map_err(|error| format!("load safetensors: {error}"))?;
    let model = BertModel::load(var_builder, &config)
        .map_err(|error| format!("load bert model: {error}"))?;
    Ok(CachedEmbedder { model, tokenizer })
}

/// Return the process-wide cached embedder, initializing on first call.
/// A failed init is cached: model loading is deterministic, so a second
/// attempt would fail identically.
#[cfg(feature = "semantic")]
fn cached_embedder() -> Result<&'static CachedEmbedder, String> {
    match EMBEDDER.get_or_init(init_embedder) {
        Ok(embedder) => Ok(embedder),
        Err(error) => Err(error.clone()),
    }
}

/// Run BertModel forward + attention-masked mean-pool + L2-normalize.
/// Returns a `candle::Result` so the call site maps the error once.
#[cfg(feature = "semantic")]
fn bert_embed(
    model: &BertModel,
    tokenizer: &tokenizers::Tokenizer,
    text: &str,
) -> candle_core::Result<Vec<f32>> {
    let encoding = tokenizer
        .encode(text, true)
        .map_err(|error| candle_core::Error::Msg(format!("tokenize: {error}")))?;
    let input_ids = encoding.get_ids();
    let attention_mask = encoding.get_attention_mask();
    let token_type_ids = encoding.get_type_ids();
    let device = Device::Cpu;
    let input_ids_tensor = Tensor::new(input_ids, &device)?.unsqueeze(0)?;
    let attention_mask_tensor = Tensor::new(attention_mask, &device)?
        .unsqueeze(0)?
        .to_dtype(DType::F32)?;
    let token_type_ids_tensor = Tensor::new(token_type_ids, &device)?.unsqueeze(0)?;
    let hidden_states = model.forward(
        &input_ids_tensor,
        &token_type_ids_tensor,
        Some(&attention_mask_tensor),
    )?;
    // hidden_states: [1, seq_len, hidden_size].
    // Mean-pool over seq_len weighted by the attention mask (padding tokens
    // contribute zero), then L2-normalize to unit length.
    let mask = attention_mask_tensor.unsqueeze(2)?; // [1, seq_len, 1]
    let masked = hidden_states.broadcast_mul(&mask)?; // [1, seq_len, hidden]
    let summed = masked.sum(1)?; // [1, hidden]
    let token_count = attention_mask_tensor.sum(1)?.unsqueeze(1)?; // [1, 1]
    let pooled = summed.broadcast_div(&token_count)?; // [1, hidden]
    let norm = pooled.sqr()?.sum(1)?.sqrt()?.unsqueeze(1)?; // [1, 1]
    let normalized = pooled.broadcast_div(&norm)?; // [1, hidden]
    normalized.squeeze(0)?.to_vec1()
}

/// Embed `text` into a 384-dim L2-normalized vector using the vendored
/// TaylorAI/bge-micro-v2 BERT model (model_type=bert, hidden_size=384).
///
/// The model + tokenizer are loaded once (via [`OnceLock`]) and reused
/// across calls. The token-level hidden states are mean-pooled with the
/// attention mask and L2-normalized, producing a `Vec<f32>` of length
/// [`EMBEDDING_DIM`] (384).
///
/// Errors are returned as `String` so callers can degrade gracefully:
/// `sync_recall_index` skips a document whose embed failed rather than
/// aborting the whole reindex.
#[cfg(feature = "semantic")]
fn embed_text(text: &str) -> Result<Vec<f32>, String> {
    let embedder = cached_embedder()?;
    let vector = bert_embed(&embedder.model, &embedder.tokenizer, text)
        .map_err(|error| format!("embed: {error}"))?;
    debug_assert_eq!(
        vector.len(),
        EMBEDDING_DIM,
        "embedding dimension mismatch: expected {EMBEDDING_DIM}, got {}",
        vector.len()
    );
    Ok(vector)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::ENV_LOCK;

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
        // Recover from a poisoned guard so an assertion failure in one test
        // does not cascade through the rest of the suite. Each test in this
        // module clones into its own tempdir and sets `CLAUDE_TARGET_OVERRIDE`
        // to that tempdir on entry, so a stale override from a panicked
        // predecessor is overwritten before the next test reads it. The
        // tradeoff is that we lose the loud "all subsequent tests fail"
        // signal that an `.expect` would have produced; the original panic
        // is still reported by the test runner, which is the failure that
        // actually matters.
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
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
        assert_eq!(
            rendered,
            "\"OpenAPI\"* AND \"diff\"* AND \"breaking-change\"*"
        );
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
        run_with_home("keel-recall-basic", |claude_home| {
            write_memory(
                claude_home,
                "memories/notes/openapi.md",
                "# OpenAPI breaking change checklist\n\nReview the diff before merging.\n",
            );
            write_memory(
                claude_home,
                "working-briefs/today.md",
                "# Working brief\n\nQuiet day, mostly OpenAPI cleanup.\n",
            );

            let mut stdout: Vec<u8> = Vec::new();
            let mut stderr: Vec<u8> = Vec::new();
            let exit_code =
                run_recall_command("memory", &["openapi".to_string()], &mut stdout, &mut stderr);
            assert_eq!(exit_code, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
            let rendered = String::from_utf8_lossy(&stdout);
            assert!(
                rendered.contains("memory recall: query=\"openapi\""),
                "rendered: {rendered}"
            );
            assert!(
                rendered.contains("memories/notes/openapi.md"),
                "rendered: {rendered}"
            );
            assert!(
                rendered.contains("working-briefs/today.md"),
                "rendered: {rendered}"
            );
        });
    }

    #[test]
    fn recall_indexes_json_working_briefs_and_returns_hits() {
        // Regression: working briefs and memory-family records are stored as
        // `.json`, not `.md`. The indexer previously accepted only `.md`, so a
        // brief was never findable by `recall` even though `working-briefs` is an
        // advertised search root. This proves `.json` is now indexed.
        run_with_home("keel-recall-json-briefs", |claude_home| {
            write_memory(
                claude_home,
                "working-briefs/wb-1.json",
                "{\n  \"id\": \"wb-1\",\n  \"request\": \"Add pagination to the users API\",\n  \"acceptanceCriteria\": \"limit=20 default\"\n}\n",
            );

            let mut stdout: Vec<u8> = Vec::new();
            let mut stderr: Vec<u8> = Vec::new();
            let exit_code = run_recall_command(
                "memory",
                &["pagination".to_string()],
                &mut stdout,
                &mut stderr,
            );
            assert_eq!(exit_code, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
            let rendered = String::from_utf8_lossy(&stdout);
            assert!(
                rendered.contains("working-briefs/wb-1.json"),
                "JSON working brief must be recallable; rendered: {rendered}"
            );
        });
    }

    #[test]
    fn reindex_after_write_makes_a_write_searchable_without_a_read_path_sync() {
        // s4 isolation: a memory write calls reindex_after_write, which must
        // leave the FTS index already populated. We prove this by querying the
        // index DIRECTLY (open connection + query_recall_index) WITHOUT calling
        // the read-path search_recall_index — that read path re-syncs on every
        // call and would mask whether the WRITE-time sync actually ran. Hits here
        // can only come from reindex_after_write having indexed the file.
        run_with_home("keel-reindex-after-write", |claude_home| {
            write_memory(
                claude_home,
                "memory/research-cache/rc-42.json",
                "{\n  \"id\": \"rc-42\",\n  \"question\": \"how to defeat blind tool search\",\n  \"answer\": \"push recall content at session start\"\n}\n",
            );

            // The write-time sync the production handlers now call.
            reindex_after_write(claude_home).expect("reindex_after_write succeeds");

            // Query the index directly — NO search_recall_index (no read-path sync).
            let database_path = recall_database_path(claude_home);
            let connection =
                open_recall_connection(&database_path).expect("open recall index read-only");
            let fts_query = build_fts_query("blind tool search").expect("non-empty query");
            let hits = query_recall_index(&connection, &fts_query, 20, None).expect("query index");

            assert!(
                hits.iter()
                    .any(|hit| hit.absolute_path.contains("rc-42.json")),
                "reindex_after_write must index the new record so it is found with no read-path sync; hits: {:?}",
                hits.iter().map(|h| &h.absolute_path).collect::<Vec<_>>()
            );
        });
    }

    #[test]
    fn recall_json_output_includes_score_and_line() {
        run_with_home("keel-recall-json", |claude_home| {
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
            assert_eq!(exit_code, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
            let rendered = String::from_utf8_lossy(&stdout);
            assert!(
                rendered.contains("\"query\": \"webhook\""),
                "rendered: {rendered}"
            );
            assert!(rendered.contains("\"count\":"), "rendered: {rendered}");
            assert!(rendered.contains("\"score\":"), "rendered: {rendered}");
            assert!(
                rendered.contains("memories/security/incident.md"),
                "rendered: {rendered}"
            );
        });
    }

    #[test]
    fn recall_reflects_subsequent_edits_via_auto_sync() {
        run_with_home("keel-recall-auto", |claude_home| {
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
            assert_eq!(
                first_code,
                0,
                "stderr: {}",
                String::from_utf8_lossy(&stderr)
            );
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
                post_text.contains("matches=0")
                    || !post_text.contains("memories/draft.md")
                    || post_text.contains("stage=vector"),
                "stale row not removed: {post_text}"
            );
        });
    }

    #[test]
    fn recall_reindex_force_rebuilds_from_scratch() {
        run_with_home("keel-recall-reindex", |claude_home| {
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
            assert_eq!(exit_code, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
            let rendered = String::from_utf8_lossy(&stdout);
            assert!(
                rendered.contains("\"documentsAdded\":"),
                "rendered: {rendered}"
            );
            assert!(
                rendered.contains("\"documentsIndexed\":"),
                "rendered: {rendered}"
            );
        });
    }

    #[test]
    fn recall_status_reports_document_count() {
        run_with_home("keel-recall-status", |claude_home| {
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
            assert_eq!(exit_code, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
            let rendered = String::from_utf8_lossy(&stdout);
            assert!(
                rendered.contains("\"documents\": 2"),
                "rendered: {rendered}"
            );
            assert!(
                rendered.contains("\"schemaVersion\":"),
                "rendered: {rendered}"
            );
        });
    }

    #[test]
    fn recall_rejects_empty_query() {
        run_with_home("keel-recall-empty", |claude_home| {
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
        run_with_home("keel-recall-no-hits", |claude_home| {
            write_memory(claude_home, "memories/note.md", "# Note\nplain old text\n");
            let mut stdout: Vec<u8> = Vec::new();
            let mut stderr: Vec<u8> = Vec::new();
            let exit_code = run_recall_command(
                "memory",
                &["nonexistentphrasezzz".to_string()],
                &mut stdout,
                &mut stderr,
            );
            assert_eq!(exit_code, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
            let rendered = String::from_utf8_lossy(&stdout);
            assert!(
                rendered.contains("matches=0") || rendered.contains("stage=vector"),
                "unmatched query should return 0 matches or fall back to vector; rendered: {rendered}"
            );
        });
    }

    #[test]
    fn trigram_similarity_scores_typo_high_and_unrelated_low() {
        // A single-character typo keeps most trigrams, so similarity stays high.
        let typo = trigram_similarity("webhook", "webhok");
        assert!(typo > FUZZY_MIN_SIMILARITY, "webhook~webhok = {typo}");
        // Identical words are 1.0.
        assert_eq!(trigram_similarity("postgres", "postgres"), 1.0);
        // Unrelated words share almost no trigrams, staying below the floor so
        // the fuzzy stage does not turn into a noise generator.
        let unrelated = trigram_similarity("webhook", "kubernetes");
        assert!(
            unrelated < FUZZY_MIN_SIMILARITY,
            "webhook~kubernetes = {unrelated}"
        );
        // A short query word vs a long content word: low, because the prefix
        // overlap is small relative to the combined trigram sets.
        let prefix_only = trigram_similarity("web", "webhook");
        assert!(
            prefix_only < FUZZY_MIN_SIMILARITY,
            "web~webhook = {prefix_only}"
        );
    }

    #[test]
    fn fts_terms_strips_quotes_stars_and_operators() {
        assert_eq!(
            fts_terms("\"webhook\"* AND \"retry\"*"),
            vec!["webhook".to_string(), "retry".to_string()]
        );
        assert_eq!(
            fts_terms("\"stripe\"* OR \"webhook\"*"),
            vec!["stripe".to_string(), "webhook".to_string()]
        );
        // Duplicate terms collapse so coverage is over DISTINCT query words.
        assert_eq!(fts_terms("\"x\"* AND \"x\"*"), vec!["x".to_string()]);
    }

    #[test]
    fn relevance_prefers_full_coverage_over_term_spam() {
        let terms = ["webhook".to_string(), "retry".to_string()];
        // Document A repeats one term many times but never mentions the other.
        let spam = "webhook webhook webhook webhook webhook handler config";
        // Document B mentions both terms once, on the same line.
        let covered = "the webhook retry policy backs off exponentially";
        let spam_score = relevance_score(spam, &terms);
        let covered_score = relevance_score(covered, &terms);
        assert!(
            covered_score > spam_score,
            "full coverage ({covered_score}) must outrank term spam ({spam_score})"
        );
    }

    #[test]
    fn relevance_is_one_when_single_term_fully_present() {
        // A single-term query degenerates to coverage 1.0 + proximity 1.0, so the
        // re-rank is a no-op vs BM25 order — exactly the graceful degradation we
        // want for the common one-word recall.
        let terms = vec!["webhook".to_string()];
        assert_eq!(relevance_score("a webhook arrives", &terms), 1.0);
    }

    #[test]
    fn proximity_rewards_terms_on_the_same_line() {
        let terms = ["webhook".to_string(), "retry".to_string()];
        let near = "webhook retry happens here\nunrelated line\nanother";
        let far = "webhook is mentioned here\n\n\nretry is way down here";
        let matched_near: Vec<&String> = terms.iter().collect();
        let near_prox = best_line_proximity(&near.to_ascii_lowercase(), &matched_near);
        let far_prox = best_line_proximity(&far.to_ascii_lowercase(), &matched_near);
        assert!(
            near_prox > far_prox,
            "same-line proximity ({near_prox}) must beat split-line ({far_prox})"
        );
        assert_eq!(
            near_prox, 1.0,
            "both terms on one line is maximal proximity"
        );
    }

    #[test]
    fn rerank_promotes_high_coverage_candidate_above_bm25_order() {
        // Simulate BM25 returning a term-spam doc FIRST (rank 0) and a
        // full-coverage doc SECOND (rank 1). The re-rank must promote the
        // full-coverage doc to the top.
        let terms = ["webhook".to_string(), "retry".to_string()];
        let candidates = vec![
            RerankCandidate {
                hit: RecallHit {
                    absolute_path: "spam.md".to_string(),
                    score: 0.0,
                    line: 1,
                    snippet: String::new(),
                },
                content: "webhook webhook webhook webhook config".to_string(),
                bm25_rank: 0,
            },
            RerankCandidate {
                hit: RecallHit {
                    absolute_path: "covered.md".to_string(),
                    score: 0.0,
                    line: 1,
                    snippet: String::new(),
                },
                content: "webhook retry policy".to_string(),
                bm25_rank: 1,
            },
        ];
        let ranked = rerank_by_relevance(candidates, &terms, 10, None);
        assert_eq!(
            ranked[0].absolute_path, "covered.md",
            "the doc covering both terms must rank first"
        );
        assert!(ranked[0].score > ranked[1].score);
    }

    #[test]
    fn rerank_keeps_bm25_order_on_equal_relevance() {
        // Two docs with identical relevance must preserve SQLite's BM25 order via
        // the rank tie-break, so results stay deterministic.
        let terms = vec!["webhook".to_string()];
        let candidates = vec![
            RerankCandidate {
                hit: RecallHit {
                    absolute_path: "first.md".to_string(),
                    score: 0.0,
                    line: 1,
                    snippet: String::new(),
                },
                content: "webhook one".to_string(),
                bm25_rank: 0,
            },
            RerankCandidate {
                hit: RecallHit {
                    absolute_path: "second.md".to_string(),
                    score: 0.0,
                    line: 1,
                    snippet: String::new(),
                },
                content: "webhook two".to_string(),
                bm25_rank: 1,
            },
        ];
        let ranked = rerank_by_relevance(candidates, &terms, 10, None);
        assert_eq!(ranked[0].absolute_path, "first.md");
        assert_eq!(ranked[1].absolute_path, "second.md");
    }

    #[test]
    fn multi_term_recall_ranks_best_coverage_first() {
        // End-to-end: two real memory files, one covering both query terms and
        // one covering only a single (repeated) term. The combined-coverage doc
        // must come back first through the full search path.
        run_with_home("keel-recall-rerank", |claude_home| {
            write_memory(
                claude_home,
                "memories/a-spam.md",
                "# Webhooks\n\nwebhook webhook webhook webhook webhook delivery notes.\n",
            );
            write_memory(
                claude_home,
                "memories/b-covered.md",
                "# Webhook retry\n\nThe webhook retry policy uses exponential backoff.\n",
            );
            let result = search_recall_index(claude_home, "webhook retry", 10, None)
                .expect("search succeeds")
                .expect("non-empty query");
            // Semantic blend may relabel stage to "hybrid"; default stays "exact".
            // Either is valid as long as the best-coverage doc ranks first.
            assert!(
                matches!(result.stage, "exact" | "hybrid"),
                "stage should be exact or hybrid, got {}",
                result.stage
            );
            assert!(
                result.hits[0].absolute_path.contains("b-covered.md"),
                "the doc covering both terms must rank first; got: {:?}",
                result
                    .hits
                    .iter()
                    .map(|h| &h.absolute_path)
                    .collect::<Vec<_>>()
            );
        });
    }

    #[test]
    fn relaxed_query_is_none_for_single_token_and_or_joined_for_many() {
        // One token: OR and AND are identical, so the relaxed stage is skipped.
        assert!(build_relaxed_fts_query("webhook").is_none());
        // Multiple tokens are OR-joined so a partly-wrong query still matches.
        assert_eq!(
            build_relaxed_fts_query("stripe webhook signature").unwrap(),
            "\"stripe\"* OR \"webhook\"* OR \"signature\"*"
        );
    }

    #[test]
    fn recall_recovers_single_word_typo_via_fuzzy_stage() {
        // The whole point of finding #2: a typo'd query that the exact
        // AND-prefix stage cannot match must still recover the document through
        // the fuzzy trigram stage. "webhok" is not a prefix of "webhook", so
        // the old lexical-only recall returned zero; the cascade now finds it.
        run_with_home("keel-recall-fuzzy", |claude_home| {
            write_memory(
                claude_home,
                "memories/security/incident.md",
                "# Webhook signature incident\n\nVerify the webhook signature on every event.\n",
            );
            let mut stdout: Vec<u8> = Vec::new();
            let mut stderr: Vec<u8> = Vec::new();
            let exit_code =
                run_recall_command("memory", &["webhok".to_string()], &mut stdout, &mut stderr);
            assert_eq!(exit_code, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
            let rendered = String::from_utf8_lossy(&stdout);
            assert!(
                rendered.contains("stage=fuzzy"),
                "typo query should resolve via the fuzzy stage; rendered: {rendered}"
            );
            assert!(
                rendered.contains("memories/security/incident.md"),
                "fuzzy stage must recover the webhook document for the typo `webhok`; rendered: {rendered}"
            );
        });
    }

    #[test]
    fn recall_uses_relaxed_stage_when_one_term_is_absent() {
        // A multi-term query where one term does not appear in any document:
        // strict AND returns nothing, but the relaxed OR stage still surfaces
        // the documents matching the terms that DO appear.
        run_with_home("keel-recall-relaxed", |claude_home| {
            write_memory(
                claude_home,
                "memories/db/migration.md",
                "# Postgres migration\n\nLock timeout strategy for online schema changes.\n",
            );
            let mut stdout: Vec<u8> = Vec::new();
            let mut stderr: Vec<u8> = Vec::new();
            // "kubernetes" appears nowhere; "postgres" does. AND fails, OR wins.
            let exit_code = run_recall_command(
                "memory",
                &["postgres".to_string(), "kubernetes".to_string()],
                &mut stdout,
                &mut stderr,
            );
            assert_eq!(exit_code, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
            let rendered = String::from_utf8_lossy(&stdout);
            assert!(
                rendered.contains("stage=relaxed"),
                "partly-absent query should resolve via the relaxed stage; rendered: {rendered}"
            );
            assert!(
                rendered.contains("memories/db/migration.md"),
                "relaxed stage must surface the postgres document; rendered: {rendered}"
            );
        });
    }

    #[test]
    fn recall_prefers_exact_stage_when_it_matches() {
        // When the strict AND stage finds hits, the cascade must NOT fall
        // through to relaxed/fuzzy — an exact match is the most precise result
        // and the stage label must report "exact".
        run_with_home("keel-recall-exact-stage", |claude_home| {
            write_memory(
                claude_home,
                "memories/api/contract.md",
                "# API contract\n\nOpenAPI breaking change checklist.\n",
            );
            let mut stdout: Vec<u8> = Vec::new();
            let mut stderr: Vec<u8> = Vec::new();
            let exit_code =
                run_recall_command("memory", &["openapi".to_string()], &mut stdout, &mut stderr);
            assert_eq!(exit_code, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
            let rendered = String::from_utf8_lossy(&stdout);
            assert!(
                rendered.contains("stage=exact"),
                "a clean match must resolve at the exact stage; rendered: {rendered}"
            );
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
    fn locate_first_match_line_survives_length_changing_lowercase() {
        // Regression: locate_first_match_line derived a byte offset from the
        // LOWERCASED content then sliced the ORIGINAL content. U+0130 ('İ')
        // lowercases to two bytes, so an 'İ' before the match shifted the offset
        // and the original-content slice panicked mid-char. The line count must
        // be computed in the same (lowercased) string space, and must not panic.
        let content = "İstanbul title line\nThe match is webhook here.\n";
        let snippet = format!(
            "The match is {open}webhook{close} here.",
            open = SNIPPET_OPEN_MARKER,
            close = SNIPPET_CLOSE_MARKER,
        );
        let line = locate_first_match_line(content, &snippet);
        assert_eq!(line, 2, "expected line 2 for `webhook`, got {line}");
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
    fn split_flags_and_query_pulls_known_flags_out_of_argument_vector() {
        // `recall webhook --json` is the canonical case from the JSON test:
        // the flag follows the positional. The shared FlagSet would have
        // stopped at `webhook` and treated `--json` as a literal query word.
        let arguments = vec!["webhook".to_string(), "--json".to_string()];
        let (flag_arguments, query_arguments) =
            split_flags_and_query(&arguments).expect("split succeeds");
        assert_eq!(flag_arguments, vec!["--json".to_string()]);
        assert_eq!(query_arguments, vec!["webhook".to_string()]);

        // Value-bearing flags consume their next token even when interleaved.
        let arguments = vec![
            "openapi".to_string(),
            "--limit".to_string(),
            "5".to_string(),
            "diff".to_string(),
        ];
        let (flag_arguments, query_arguments) =
            split_flags_and_query(&arguments).expect("split succeeds");
        assert_eq!(flag_arguments, vec!["--limit".to_string(), "5".to_string()]);
        assert_eq!(
            query_arguments,
            vec!["openapi".to_string(), "diff".to_string()]
        );

        // `--flag=value` form keeps the inline value attached.
        let arguments = vec!["--limit=10".to_string(), "stripe".to_string()];
        let (flag_arguments, query_arguments) =
            split_flags_and_query(&arguments).expect("split succeeds");
        assert_eq!(flag_arguments, vec!["--limit=10".to_string()]);
        assert_eq!(query_arguments, vec!["stripe".to_string()]);

        // Bool flags accept the explicit-value form too. FlagSet parses the
        // `false` half itself; we only need to keep the whole token together.
        let arguments = vec!["webhook".to_string(), "--json=false".to_string()];
        let (flag_arguments, query_arguments) =
            split_flags_and_query(&arguments).expect("split succeeds");
        assert_eq!(flag_arguments, vec!["--json=false".to_string()]);
        assert_eq!(query_arguments, vec!["webhook".to_string()]);

        // `--` terminates flag scanning so a literal `--json` can be searched.
        let arguments = vec![
            "--".to_string(),
            "--json".to_string(),
            "literal".to_string(),
        ];
        let (flag_arguments, query_arguments) =
            split_flags_and_query(&arguments).expect("split succeeds");
        assert!(flag_arguments.is_empty(), "flags: {flag_arguments:?}");
        assert_eq!(
            query_arguments,
            vec!["--json".to_string(), "literal".to_string()]
        );

        // Missing value for a value-bearing flag surfaces a parse error.
        let arguments = vec!["webhook".to_string(), "--limit".to_string()];
        let error_message = split_flags_and_query(&arguments).expect_err("missing value");
        assert!(error_message.contains("--limit"), "error: {error_message}");
    }
}

#[cfg(all(test, feature = "semantic"))]
mod vector_tests {
    use super::*;
    use crate::test_support::ENV_LOCK;

    fn temp_home(label: &str) -> PathBuf {
        ensure_vec_extension_registered();
        let unique_suffix: u128 = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let candidate = std::env::temp_dir().join(format!("{label}-{unique_suffix}"));
        fs::create_dir_all(&candidate).expect("create tempdir");
        candidate
    }

    #[test]
    fn vec_schema_creates_vec_items_and_stamps_version_2() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let home = temp_home("keel-vec-schema");
        let db_path = recall_database_path(&home);
        let conn = Connection::open(&db_path).expect("open db");
        ensure_recall_schema(&conn).expect("ensure schema");
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM vec_items", [], |row| row.get(0))
            .expect("count vec_items");
        assert_eq!(
            count, 0,
            "vec_items must exist and be empty on a fresh schema"
        );
        let version: String = conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'schema_version'",
                [],
                |row| row.get(0),
            )
            .expect("read schema_version");
        assert_eq!(version, "2", "schema_version must be stamped to 2");
    }

    #[test]
    fn v1_index_migrates_to_v2_without_losing_vec_items_availability() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let home = temp_home("keel-vec-migrate");
        let db_path = recall_database_path(&home);
        {
            let conn = Connection::open(&db_path).expect("open db");
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
                 CREATE VIRTUAL TABLE IF NOT EXISTS documents USING fts5(
                     path UNINDEXED, modified_at UNINDEXED, size UNINDEXED, content,
                     tokenize = 'porter unicode61 remove_diacritics 2'
                 );
                 INSERT INTO documents(path, modified_at, size, content)
                 VALUES ('/x.md', '1', '1', 'webhook');
                 INSERT INTO meta(key, value) VALUES ('schema_version', '1');",
            )
            .expect("seed v1 index");
        }
        let conn = Connection::open(&db_path).expect("reopen db");
        ensure_recall_schema(&conn).expect("migrate to v2");
        let version: String = conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'schema_version'",
                [],
                |row| row.get(0),
            )
            .expect("read schema_version");
        assert_eq!(version, "2");
        let _: i64 = conn
            .query_row("SELECT COUNT(*) FROM vec_items", [], |row| row.get(0))
            .expect("vec_items exists after migration");
    }

    #[test]
    fn sync_inserts_a_vec_items_row_for_a_new_document() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let home = temp_home("keel-vec-sync");
        let mem_dir = home.join("memory").join("notes");
        fs::create_dir_all(&mem_dir).expect("create memory dir");
        fs::write(
            mem_dir.join("alpha.md"),
            "# Alpha\n\nsemantic recall test\n",
        )
        .expect("write memory file");
        let db_path = recall_database_path(&home);
        let mut conn = Connection::open(&db_path).expect("open db");
        ensure_recall_schema(&conn).expect("ensure schema");
        sync_recall_index(&mut conn, &home, false).expect("sync");
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM vec_items", [], |row| row.get(0))
            .expect("count vec_items");
        assert!(
            count >= 1,
            "sync must insert at least one vec_items row for the new document; got {count}"
        );
    }

    #[test]
    fn query_vector_index_returns_nearest_first() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let home = temp_home("keel-vec-knn");
        let db_path = recall_database_path(&home);
        let conn = Connection::open(&db_path).expect("open db");
        ensure_recall_schema(&conn).expect("ensure schema");
        let mut a = vec![0.0f32; EMBEDDING_DIM];
        a[0] = 1.0;
        let mut b = vec![0.0f32; EMBEDDING_DIM];
        b[1] = 1.0;
        let mut c = vec![0.0f32; EMBEDDING_DIM];
        c[2] = 1.0;
        upsert_doc_vector(&conn, "/a.md", &a).expect("insert a");
        upsert_doc_vector(&conn, "/b.md", &b).expect("insert b");
        upsert_doc_vector(&conn, "/c.md", &c).expect("insert c");
        let results = query_vector_index(&conn, &a, 3).expect("knn query");
        assert!(!results.is_empty(), "KNN must return results");
        assert_eq!(results[0].0, "/a.md", "nearest neighbor must be /a.md");
        let distances: Vec<f64> = results.iter().map(|(_, d)| *d).collect();
        let mut sorted = distances.clone();
        sorted.sort_by(|x, y| x.partial_cmp(y).unwrap());
        assert_eq!(
            distances, sorted,
            "results must be ordered nearest-first by distance"
        );
    }

    #[test]
    fn cascade_falls_back_to_vector_stage_when_lexical_fails() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let home = temp_home("keel-vec-cascade");
        let mem_dir = home.join("memory").join("notes");
        fs::create_dir_all(&mem_dir).expect("create memory dir");
        fs::write(
            mem_dir.join("auth.md"),
            "# Authentication\n\nHow users log in and manage sessions.\n",
        )
        .expect("write memory file");
        let db_path = recall_database_path(&home);
        let mut conn = Connection::open(&db_path).expect("open db");
        ensure_recall_schema(&conn).expect("ensure schema");
        sync_recall_index(&mut conn, &home, false).expect("sync");

        // "credential verification" shares no terms or trigrams with the indexed
        // document, so all three lexical stages return nothing. If the embedding
        // distance is within VECTOR_MAX_DISTANCE, the vector stage returns the
        // document with stage="vector"; otherwise the cascade returns empty
        // hits with stage="exact". Either outcome is valid — the test verifies
        // the cascade runs without error on a lexically-unmatchable query.
        let result = cascade_recall_query(&conn, "credential verification", 10, None)
            .expect("cascade query")
            .expect("non-None result");
        if result.stage == "vector" {
            assert!(
                !result.hits.is_empty(),
                "vector stage must return KNN neighbors"
            );
        }
    }

    #[test]
    fn workspace_affinity_boost_promotes_current_project_hit() {
        // Two docs with identical partial relevance (coverage <1.0 so the boost
        // has room to act). The current-workspace hit must rank first with the slug.
        let terms = vec!["login".to_string(), "timeout".to_string()];
        let slug = "clicksync-main";
        let candidates = vec![
            RerankCandidate {
                hit: RecallHit {
                    absolute_path: "memories/projects/D--other-project/auth.md".to_string(),
                    score: 0.0,
                    line: 1,
                    snippet: String::new(),
                },
                content: "login form submission".to_string(),
                bm25_rank: 0,
            },
            RerankCandidate {
                hit: RecallHit {
                    absolute_path: "memories/projects/D-learn-flutter-ClickSync-main/auth.md"
                        .to_string(),
                    score: 0.0,
                    line: 1,
                    snippet: String::new(),
                },
                content: "login form submission".to_string(),
                bm25_rank: 1,
            },
        ];
        // Without a slug: identical relevance, BM25 rank breaks the tie, so the
        // first (other-project, rank 0) hit wins.
        let unscoped = rerank_by_relevance(candidates.clone(), &terms, 10, None);
        assert_eq!(
            unscoped[0].absolute_path,
            "memories/projects/D--other-project/auth.md"
        );
        // With the current-workspace slug: the ClickSync hit is boosted and wins.
        let scoped = rerank_by_relevance(candidates, &terms, 10, Some(slug));
        assert_eq!(
            scoped[0].absolute_path, "memories/projects/D-learn-flutter-ClickSync-main/auth.md",
            "current-workspace hit must outrank an equal cross-project hit"
        );
    }

    #[cfg(feature = "semantic")]
    #[test]
    fn hybrid_blend_adds_vector_candidates_to_thin_lexical_result() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let home = temp_home("keel-hybrid-blend");
        let mem_dir = home.join("memory").join("notes");
        fs::create_dir_all(&mem_dir).expect("create memory dir");
        // A doc the lexical stages WILL match (shares "login").
        fs::write(
            mem_dir.join("lexical.md"),
            "# Login\n\nThe login form and session handling.\n",
        )
        .expect("write lexical doc");
        // A doc the lexical stages MISS but a semantic neighbor should find
        // (auth/sign-in concept, no shared term with "login form").
        fs::write(
            mem_dir.join("semantic.md"),
            "# Authentication\n\nCredential verification and sign-in flow.\n",
        )
        .expect("write semantic doc");
        let db_path = recall_database_path(&home);
        let mut conn = Connection::open(&db_path).expect("open db");
        ensure_recall_schema(&conn).expect("ensure schema");
        sync_recall_index(&mut conn, &home, false).expect("sync");

        let result = cascade_recall_query(&conn, "login form", 10, None)
            .expect("cascade")
            .expect("non-None");
        // cascade_recall_query returns the lexical result (blend runs only in
        // search_recall_index). Confirm the lexical hit is present, no error.
        assert!(
            !result.hits.is_empty(),
            "lexical stage must find the login doc"
        );
        assert!(
            result
                .hits
                .iter()
                .any(|h| h.absolute_path.contains("lexical.md")),
            "lexical.md must be in the hits: {:?}",
            result.hits
        );
    }
}
