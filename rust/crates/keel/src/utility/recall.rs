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
//!   * The on-disk schema is owned by this module. The `documents` virtual table
//!     stores section-aware memory chunks; `document_meta` stores line ranges,
//!     source family, workspace scope, branch marker, content fingerprint, lifecycle,
//!     and expiry metadata. The `meta` table stores schema version and sync timestamps.
//!   * Recall always reflects current files on disk: every read-path call
//!     (`recall <query>` and `recall status`) runs `sync_recall_index` first.
//!     Changed files are chunked and committed atomically; deleted files remove
//!     every chunk and metadata row.
//!   * `recall reindex --force` drops and recreates the FTS5 and metadata tables
//!     to recover from a corrupt or stale index without touching other files.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension};

use crate::args::FlagSet;
use crate::json::{write_indented, Value};
use crate::proxy::token_meter::TokenMeter;
use crate::runtime::{display_path, resolve_claude_home};
use crate::utility::hashing::{fnv1a64_hex, sha256_hex};
use crate::utility::record_store::{field, parse_object_of_strings, Record};
// The recall schema is shared by every build. FTS5 remains the deterministic
// source-of-truth index; structured workspace indexing lives in its own lane.
// The on-disk content hash is an integrity value, not a cache key: bump the
// schema with its algorithm so legacy FNV indexes rebuild instead of load.
const SCHEMA_VERSION: &str = "7";

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
const DEFAULT_RECALL_ROOTS: &[&str] = &["memory", "memories", "working-briefs", "anvil"];
// why: Anvil lock/prefix/report live under memories/workspaces/<slug>/anvil/
// so the memories root is what any CLI actually searches. The extra "anvil"
// root stays so a leftover top-level bank is still indexed.

/// Maximum number of FTS5 hits returned when `--limit` is not supplied.
pub const DEFAULT_RECALL_LIMIT: usize = 20;

/// Hard input bound for every recall owner, including programmatic callers.
/// Bytes are used instead of characters so malformed or high-density Unicode
/// cannot expand the FTS expression without passing the admission check.
pub const MAX_RECALL_QUERY_BYTES: usize = 4 * 1024;
/// Hard file size bound for indexed memory records (Markdown or JSON). Files
/// exceeding this bound are logs, traces, or runtime artifacts, not memory notes.
pub const MAX_INDEXABLE_FILE_BYTES: i64 = 256 * 1024;
/// Maximum chunks extracted from a single document to prevent unbounded FTS rows.
const MAX_CHUNKS_PER_FILE: usize = 64;
/// Number of documents written per SQLite transaction to ensure forward progress.
const RECALL_TRANSACTION_BATCH_SIZE: usize = 100;

/// Hard result-count bound shared by the CLI, memory-family retrieval, and MCP
/// callers. Callers may request less, but no caller can make the index return a
/// larger result set through this module.
pub const MAX_RECALL_LIMIT: usize = 100;

/// Hard model-visible memory-result bounds. The hit budget is measured from the
/// exact JSON representation of each hit with a small reserve for the enclosing
/// response object and array separators.
pub const MAX_RECALL_RESULT_BYTES: usize = 32 * 1024;
pub const MAX_RECALL_RESULT_TOKENS: usize = 900;

/// `search_recall_index` builds FTS expressions from a bounded raw query. This
/// separate bound also protects the public low-level query owner when an
/// embedder supplies an already-built expression directly.
const MAX_RECALL_FTS_QUERY_BYTES: usize = 16 * 1024;
const RECALL_RESULT_OVERHEAD_BYTES: usize = 1024;
const RECALL_RESULT_OVERHEAD_TOKENS: usize = 64;
/// Query text is echoed in the model-visible envelope and in the recovery
/// reference. Keep that echo bounded even when the admitted search query is
/// near its larger input limit; the full query is represented by its digest.
const MAX_RECALL_QUERY_PROJECTION_CHARS: usize = 256;
const MAX_RECALL_HOME_PROJECTION_CHARS: usize = 512;
const MAX_RECALL_EXCERPT_CHARS: usize = 600;
/// Direct recall/retrieve calls have their own total wall-clock budget. MCP
/// callers also have an outer deadline, but the low-level CLI owner must not
/// become an unbounded filesystem/SQLite scan when called programmatically.
const DEFAULT_RECALL_DEADLINE_MS: u64 = 5_000;
const MAX_RECALL_DEADLINE_MS: u64 = 30_000;

fn recall_deadline() -> Instant {
    let millis = std::env::var("KEEL_RECALL_DEADLINE_MS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_RECALL_DEADLINE_MS)
        .clamp(1, MAX_RECALL_DEADLINE_MS);
    Instant::now() + Duration::from_millis(millis)
}

fn check_recall_deadline(deadline: Option<Instant>) -> Result<(), String> {
    if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
        Err("recall retrieval deadline exceeded".to_string())
    } else {
        Ok(())
    }
}

fn remaining_recall_budget(deadline: Option<Instant>) -> Option<Duration> {
    deadline.map(|deadline| deadline.saturating_duration_since(Instant::now()))
}

/// Maximum age of a file-content verification before recall re-hashes an
/// otherwise unchanged file. Metadata changes are still detected immediately;
/// this bounded pass covers editors or external tools that preserve both mtime
/// and size without making every recall read the whole memory corpus.
const RECALL_DEFAULT_INTEGRITY_INTERVAL_SECS: u64 = 300;

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
        "Usage: keel {command_group} recall <query> [--limit N] [--json] [--claude-home PATH] [--workspace SCOPE] [--local-only]"
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
    let _ = writeln!(
        standard_output,
        "Retrieval is wall-clock bounded by KEEL_RECALL_DEADLINE_MS (default 5000ms, maximum 30000ms)."
    );
    let _ = writeln!(
        standard_output,
        "Structured stale, expired, quarantined, and superseded records are excluded from retrieval."
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
    let trimmed_query = match validate_recall_query(&raw_query) {
        Ok(query) => query,
        Err(error_message) => {
            let _ = writeln!(standard_error, "{command_group} recall: {error_message}");
            return 2;
        }
    };
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

    let local_only = flag_set.bool_value("local-only");
    let workspace_context =
        match recall_workspace_context(Some(flag_set.string_value("workspace")), local_only) {
            Ok(context) => context,
            Err(error_message) => {
                let _ = writeln!(standard_error, "{command_group} recall: {error_message}");
                return 1;
            }
        };
    let replay_workspace = if local_only {
        workspace_context.scope.as_deref()
    } else {
        let explicit = flag_set.string_value("workspace").trim();
        (!explicit.is_empty()).then_some(explicit)
    };

    let search = match search_recall_index_with_options(
        &claude_home,
        trimmed_query,
        limit,
        RecallQueryOptions {
            workspace_affinity: workspace_context.affinity.as_deref(),
            scope: workspace_context.scope.as_deref(),
            branch: workspace_context.branch.as_deref(),
        },
    ) {
        Ok(Some(search)) => search,
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
    let matches = search.hits;
    let stage = search.stage;

    if flag_set.bool_value("json") {
        let payload = build_search_json(
            trimmed_query,
            &claude_home,
            &matches,
            limit,
            stage,
            replay_workspace,
            local_only,
        );
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
    let snapshot = match recall_status_snapshot(&claude_home) {
        Ok(snapshot) => snapshot,
        Err(error_message) => {
            let _ = writeln!(
                standard_error,
                "{command_group} recall status: {error_message}"
            );
            return 1;
        }
    };
    let document_count = snapshot.document_count;
    let database_path = snapshot.index_path.clone();
    if flag_set.bool_value("json") {
        let fields = vec![
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
                Value::Number(snapshot.last_indexed_at_millis.to_string()),
            ),
            (
                "addedSinceLastSync".into(),
                Value::Number(snapshot.added_since_last_sync.to_string()),
            ),
            (
                "updatedSinceLastSync".into(),
                Value::Number(snapshot.updated_since_last_sync.to_string()),
            ),
            (
                "removedSinceLastSync".into(),
                Value::Number(snapshot.removed_since_last_sync.to_string()),
            ),
        ];
        let payload = Value::Object(fields);
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
        snapshot.last_indexed_at_millis,
    );
    0
}

/// Deliberate divergence vs `code_search::parse_limit`: recall is error-facing
/// (bad --limit fails loudly), while code-search silently caps its display limit
/// at 50 for search results. Valid recall limits are hard-capped so a caller
/// cannot turn a bounded retrieval into an unbounded table scan.
fn parse_limit(raw_value: &str) -> Result<usize, String> {
    let trimmed = raw_value.trim();
    if trimmed.is_empty() {
        return Ok(DEFAULT_RECALL_LIMIT);
    }
    match trimmed.parse::<usize>() {
        Ok(parsed) if parsed > 0 => Ok(parsed.min(MAX_RECALL_LIMIT)),
        Ok(_) => Err(format!(
            "--limit must be a positive integer, got {trimmed:?}"
        )),
        Err(_) => Err(format!(
            "--limit must be a positive integer, got {trimmed:?}"
        )),
    }
}

pub(crate) fn parse_recall_limit(raw_value: &str) -> Result<usize, String> {
    parse_limit(raw_value)
}

/// Validate raw recall input before opening or synchronizing the index. The
/// bound covers leading/trailing whitespace as well as searchable content so a
/// large whitespace-only request cannot make the read path do unbounded work.
pub(crate) fn validate_recall_query(raw_query: &str) -> Result<&str, String> {
    if raw_query.len() > MAX_RECALL_QUERY_BYTES {
        return Err(format!(
            "query is {} bytes, over the {}-byte limit",
            raw_query.len(),
            MAX_RECALL_QUERY_BYTES
        ));
    }
    Ok(raw_query.trim())
}

fn validate_fts_query(fts_query: &str) -> Result<(), String> {
    if fts_query.len() > MAX_RECALL_FTS_QUERY_BYTES {
        return Err(format!(
            "FTS query is {} bytes, over the {}-byte limit",
            fts_query.len(),
            MAX_RECALL_FTS_QUERY_BYTES
        ));
    }
    Ok(())
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
/// searchable; the "saved memory is not searchable" gap. Calling this at
/// the end of each write closes the window.
///
/// Best-effort by contract: a memory write must never fail because the index
/// could not be opened or synced. The durable file on disk is the source of
/// truth, and the next read-path sync reconciles it anyway, which is why a
/// caller can treat an `Err` here as advisory.
///
/// Invalidate only the known changed paths before the incremental scan, so a
/// same-size/same-mtime replacement cannot hide behind the integrity interval.
/// The built-in memory writers pass their returned path; a caller with no
/// retained path passes an empty slice.
pub fn reindex_after_write_paths(
    claude_home: &Path,
    changed_paths: &[&Path],
) -> Result<(), String> {
    let database_path = recall_database_path(claude_home);
    let mut connection = open_recall_connection(&database_path)?;
    invalidate_recall_paths(&mut connection, changed_paths)?;
    sync_recall_index(&mut connection, claude_home, false)?;
    Ok(())
}

fn invalidate_recall_paths(connection: &mut Connection, paths: &[&Path]) -> Result<(), String> {
    if paths.is_empty() {
        return Ok(());
    }
    let transaction = connection
        .transaction()
        .map_err(|database_error| format!("begin invalidation: {database_error}"))?;
    for path in paths {
        let path = path.to_string_lossy();
        transaction
            .execute(
                "DELETE FROM file_state WHERE path = ?1",
                params![path.as_ref()],
            )
            .map_err(|database_error| format!("invalidate {path}: {database_error}"))?;
    }
    transaction
        .commit()
        .map_err(|database_error| format!("commit invalidation: {database_error}"))?;
    Ok(())
}

/// Snapshot of recall-index health used by surfaces that just need to read
/// the current document count, last sync timestamp, and on-disk index path.
/// Read directly from the stored index with no filesystem sync; indexing is a
/// write-path concern owned by `reindex` and `reindex_after_write_paths`, so
/// callers see exactly the values an explicit `recall status` invocation
/// would print.
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
}

/// Result of a programmatic recall search: the canonicalized FTS expression
/// that was executed plus the matching hits. Callers already know the
/// `claude_home` and raw query they passed in, so this struct only carries
/// values they cannot trivially recompute (`fts_query` is produced by
/// `build_fts_query` against the trimmed input).
#[derive(Debug, Clone)]
pub struct RecallSearchResult {
    pub fts_query: String,
    /// Which deterministic retrieval stage produced these hits: `"exact"`,
    /// `"relaxed"`, `"fuzzy"`, or a fused structural stage.
    pub stage: &'static str,
    pub hits: Vec<RecallHit>,
}

/// Query constraints shared by CLI, MCP, and memory retrieval.
///
/// `workspace_affinity` only changes ranking. `scope` and `branch` are hard
/// eligibility filters applied in SQLite before candidate limits, so a
/// cross-workspace hit cannot consume a local result slot.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct RecallQueryOptions<'a> {
    pub(crate) workspace_affinity: Option<&'a str>,
    pub(crate) scope: Option<&'a str>,
    pub(crate) branch: Option<&'a str>,
}

/// Scope a retrieval result must be replayed with. Recovery references echo
/// `--workspace`/`--local-only` so a scoped search cannot be widened by
/// accident; the low-level recall projection, the memory family retrieval
/// command, and the MCP recall envelope all record the same pair.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct RecallReplay<'a> {
    /// Explicit or derived workspace scope, already trimmed.
    pub(crate) workspace: Option<&'a str>,
    /// Whether the search applied workspace/branch eligibility in SQLite.
    pub(crate) local_only: bool,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct RecallWorkspaceContext {
    pub(crate) affinity: Option<String>,
    pub(crate) scope: Option<String>,
    pub(crate) branch: Option<String>,
}

pub(crate) fn recall_workspace_context(
    explicit_workspace: Option<&str>,
    local_only: bool,
) -> Result<RecallWorkspaceContext, String> {
    let explicit_workspace = explicit_workspace
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let current_directory = std::env::current_dir().ok();
    let affinity = explicit_workspace
        .map(crate::utility::system_map::sanitize_key)
        .or_else(|| {
            current_directory
                .as_deref()
                .map(|path| crate::utility::system_map::sanitize_key(&path.to_string_lossy()))
        });
    let scope = if local_only {
        explicit_workspace
            .map(explicit_workspace_scope)
            .or_else(|| {
                current_directory
                    .as_deref()
                    .map(|path| crate::utility::system_map::workspace_key(&path.to_string_lossy()))
            })
    } else {
        None
    };
    if local_only && scope.is_none() {
        return Err(
            "local-only recall requires --workspace or an available current directory".to_string(),
        );
    }
    Ok(RecallWorkspaceContext {
        affinity,
        scope,
        branch: local_only.then(memory_branch),
    })
}

fn explicit_workspace_scope(value: &str) -> String {
    if value.contains('\\')
        || value.contains('/')
        || value.contains(':')
        || Path::new(value).is_dir()
    {
        crate::utility::system_map::workspace_key(value)
    } else {
        value.to_string()
    }
}

/// Run the same auto-sync + FTS5 query path as `recall <query>` without
/// touching stdout/stderr. Returns the prepared FTS expression alongside the
/// matching hits. Lifecycle filtering is always enabled; callers that have no
/// current-workspace context can still use the unscoped wrapper.
pub fn search_recall_index(
    claude_home: &Path,
    raw_query: &str,
    limit: usize,
    workspace_slug: Option<&str>,
) -> Result<Option<RecallSearchResult>, String> {
    search_recall_index_with_options(
        claude_home,
        raw_query,
        limit,
        RecallQueryOptions {
            workspace_affinity: workspace_slug,
            ..RecallQueryOptions::default()
        },
    )
}

pub(crate) fn search_recall_index_with_options(
    claude_home: &Path,
    raw_query: &str,
    limit: usize,
    options: RecallQueryOptions<'_>,
) -> Result<Option<RecallSearchResult>, String> {
    let trimmed_query = validate_recall_query(raw_query)?;
    if trimmed_query.is_empty() {
        return Ok(None);
    }
    let deadline = recall_deadline();
    check_recall_deadline(Some(deadline))?;
    let limit = limit.min(MAX_RECALL_LIMIT);
    let database_path = recall_database_path(claude_home);
    let mut connection = open_recall_connection_until(&database_path, Some(deadline))?;
    // Best-effort sync: another keel process may hold the write lock (a reindex
    // in progress). Searching the EXISTING index is always correct — it only
    // lags by one sync — so a lock timeout must degrade to a search, never to
    // a "database is locked" error that kills the MCP tool call.
    if let Err(sync_error) =
        sync_recall_index_until(&mut connection, claude_home, false, Some(deadline))
    {
        if !is_sync_degradable(&sync_error) {
            return Err(sync_error);
        }
    }
    check_recall_deadline(Some(deadline))?;
    match cascade_recall_query_until(&connection, trimmed_query, limit, options, Some(deadline))? {
        Some(cascade) => Ok(Some(RecallSearchResult {
            fts_query: cascade.query_expression,
            stage: cascade.stage,
            hits: cascade.hits,
        })),
        None => Ok(None),
    }
}

/// True when a recall open/sync error is transient (lock contention from another
/// keel process holding the WAL write lock, or a wall-clock deadline exceeded
/// mid-sync) rather than corruption. Callers treat this as "search the existing
/// index anyway"; anything else is a hard error.
fn is_sync_degradable(error_message: &str) -> bool {
    let lowered = error_message.to_ascii_lowercase();
    lowered.contains("locked") || lowered.contains("busy") || lowered.contains("deadline")
}

/// Open (and if necessary create) the recall index under `claude_home`, then
/// return a snapshot of the resulting health metrics read directly from the
/// stored index with no filesystem sync. Syncing is a write-path concern owned
/// by `reindex` and the write-time hook; status reads must stay millisecond-fast.
/// Used by the MCP `recall_status` tool and the `keel://recall/status`
/// resource so they share the same code path as `recall status` rather than
/// reaching into the schema directly.
pub fn recall_status_snapshot(claude_home: &Path) -> Result<RecallStatusSnapshot, String> {
    let database_path = recall_database_path(claude_home);
    let connection = open_recall_connection(&database_path)?;
    let document_count = count_documents(&connection)?;
    let last_indexed_at_millis: i64 = connection
        .query_row(
            "SELECT value FROM meta WHERE key = 'last_indexed_at_millis'",
            [],
            |row| row.get::<_, String>(0),
        )
        .ok()
        .and_then(|val| val.parse::<i64>().ok())
        .unwrap_or(0);
    Ok(RecallStatusSnapshot {
        claude_home: claude_home.to_path_buf(),
        index_path: database_path,
        schema_version: SCHEMA_VERSION.to_string(),
        document_count,
        last_indexed_at_millis: last_indexed_at_millis.max(0) as u128,
        added_since_last_sync: 0,
        updated_since_last_sync: 0,
        removed_since_last_sync: 0,
    })
}

fn default_search_roots(claude_home: &Path) -> Vec<PathBuf> {
    DEFAULT_RECALL_ROOTS
        .iter()
        .map(|name| claude_home.join(name))
        .collect()
}

fn recall_integrity_interval_millis() -> i64 {
    std::env::var("KEEL_RECALL_INTEGRITY_INTERVAL_SECS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(RECALL_DEFAULT_INTEGRITY_INTERVAL_SECS)
        .min(86_400)
        .saturating_mul(1_000) as i64
}

fn recall_now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

fn open_recall_connection(database_path: &Path) -> Result<Connection, String> {
    open_recall_connection_until(database_path, None)
}

fn open_recall_connection_until(
    database_path: &Path,
    deadline: Option<Instant>,
) -> Result<Connection, String> {
    check_recall_deadline(deadline)?;
    crate::utility::sqlite::create_parent_directory(database_path).map_err(|io_error| {
        format!(
            "create {}: {io_error}",
            display_path(database_path.parent().unwrap_or(database_path))
        )
    })?;
    let connection =
        crate::utility::sqlite::open_connection(database_path).map_err(|database_error| {
            recall_open_error_hint(database_path, &format!("open sqlite: {database_error}"))
        })?;
    check_recall_deadline(deadline)?;
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
    connection
        .pragma_update(None, "temp_store", "MEMORY")
        .map_err(|database_error| {
            recall_open_error_hint(database_path, &format!("set temp_store: {database_error}"))
        })?;
    connection
        .pragma_update(None, "mmap_size", 268_435_456i64)
        .map_err(|database_error| {
            recall_open_error_hint(database_path, &format!("set mmap_size: {database_error}"))
        })?;
    connection
        .pragma_update(None, "cache_size", -64_000i64)
        .map_err(|database_error| {
            recall_open_error_hint(database_path, &format!("set cache_size: {database_error}"))
        })?;
    // why: WAL lets one writer proceed alongside readers, so a short wait lets a
    // concurrent `keel mcp serve` finish its transaction instead of erroring. The
    // previous 750ms default surfaced as spurious "database is locked" failures —
    // and downstream `context_brief` timeouts — whenever two keel processes raced
    // the index. 5s absorbs normal contention; an orphaned writer still fails in
    // bounded time. Override `KEEL_RECALL_BUSY_TIMEOUT_MS`.
    let configured_busy_ms = std::env::var("KEEL_RECALL_BUSY_TIMEOUT_MS")
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .unwrap_or(5_000)
        .clamp(0, 30_000);
    let busy_ms = remaining_recall_budget(deadline)
        .map(|remaining| configured_busy_ms.min(remaining.as_millis().min(u64::MAX as u128) as u64))
        .unwrap_or(configured_busy_ms);
    connection
        .busy_timeout(std::time::Duration::from_millis(busy_ms))
        .map_err(|database_error| {
            recall_open_error_hint(
                database_path,
                &format!("set busy_timeout: {database_error}"),
            )
        })?;
    check_recall_deadline(deadline)?;
    ensure_recall_schema(&connection)
        .map_err(|schema_error| recall_open_error_hint(database_path, &schema_error))?;
    check_recall_deadline(deadline)?;
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
             );
             CREATE TABLE IF NOT EXISTS document_meta(
                 rowid INTEGER PRIMARY KEY,
                 path TEXT NOT NULL,
                 chunk_index INTEGER NOT NULL,
                 start_line INTEGER NOT NULL,
                 end_line INTEGER NOT NULL,
                 source_kind TEXT NOT NULL,
                 scope TEXT NOT NULL,
                 branch TEXT NOT NULL,
                 content_hash TEXT NOT NULL,
                 lifecycle TEXT NOT NULL,
                 expires_at INTEGER
             );
             CREATE TABLE IF NOT EXISTS file_state(
                  path TEXT PRIMARY KEY,
                  modified_at INTEGER NOT NULL,
                  size INTEGER NOT NULL,
                  content_hash TEXT NOT NULL,
                  last_verified_at INTEGER NOT NULL
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
                    "DROP TABLE IF EXISTS vec_items;
                      DROP TABLE IF EXISTS file_state;
                     DROP TABLE IF EXISTS document_meta;
                     DROP TABLE IF EXISTS documents;
                     CREATE VIRTUAL TABLE documents USING fts5(
                         path UNINDEXED,
                         modified_at UNINDEXED,
                         size UNINDEXED,
                         content,
                         tokenize = 'porter unicode61 remove_diacritics 2'
                     );
                     CREATE TABLE document_meta(
                         rowid INTEGER PRIMARY KEY,
                         path TEXT NOT NULL,
                         chunk_index INTEGER NOT NULL,
                         start_line INTEGER NOT NULL,
                         end_line INTEGER NOT NULL,
                         source_kind TEXT NOT NULL,
                         scope TEXT NOT NULL,
                         branch TEXT NOT NULL,
                         content_hash TEXT NOT NULL,
                         lifecycle TEXT NOT NULL,
                         expires_at INTEGER
                      );
                      CREATE TABLE file_state(
                          path TEXT PRIMARY KEY,
                          modified_at INTEGER NOT NULL,
                          size INTEGER NOT NULL,
                          content_hash TEXT NOT NULL,
                          last_verified_at INTEGER NOT NULL
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
    sync_recall_index_until(connection, claude_home, force_full_rescan, None)
}

fn sync_recall_index_until(
    connection: &mut Connection,
    claude_home: &Path,
    force_full_rescan: bool,
    deadline: Option<Instant>,
) -> Result<SyncReport, String> {
    check_recall_deadline(deadline)?;
    let mut report = SyncReport::default();
    let now_millis = recall_now_millis();
    let integrity_interval_millis = recall_integrity_interval_millis();
    let mut existing_rows: std::collections::HashMap<String, (i64, i64, String, i64)> =
        std::collections::HashMap::new();
    {
        let mut select_statement = connection
            .prepare(
                "SELECT path, modified_at, size, content_hash, last_verified_at \
                 FROM file_state",
            )
            .map_err(|database_error| format!("prepare select: {database_error}"))?;
        let row_iterator = select_statement
            .query_map([], |row| {
                let path: String = row.get(0)?;
                let modified_at: i64 = row.get(1)?;
                let size_bytes: i64 = row.get(2)?;
                let content_hash: String = row.get(3)?;
                let last_verified_at: i64 = row.get(4)?;
                Ok((
                    path,
                    (modified_at, size_bytes, content_hash, last_verified_at),
                ))
            })
            .map_err(|database_error| format!("query existing: {database_error}"))?;
        for row_result in row_iterator {
            check_recall_deadline(deadline)?;
            let (path, metadata) =
                row_result.map_err(|database_error| format!("read row: {database_error}"))?;
            existing_rows.insert(path, metadata);
        }
    }

    let mut on_disk: Vec<DocumentRecord> = Vec::new();
    for root_directory in default_search_roots(claude_home) {
        check_recall_deadline(deadline)?;
        if !root_directory.is_dir() {
            continue;
        }
        collect_indexable_files_until(&root_directory, &mut on_disk, deadline)?;
    }

    let mut on_disk_paths: std::collections::HashSet<String> = std::collections::HashSet::new();
    // Read changed documents before the short SQL-only transaction.
    struct PendingDocument {
        path: String,
        modified_at: String,
        size: String,
        content_hash: String,
        chunks: Vec<MemoryChunk>,
        source_kind: String,
        scope: String,
        branch: String,
        lifecycle: &'static str,
        expires_at_millis: Option<i64>,
        was_existing: bool,
    }
    let mut pending: Vec<PendingDocument> = Vec::new();
    let mut verified_paths: Vec<String> = Vec::new();
    for document in &on_disk {
        check_recall_deadline(deadline)?;
        on_disk_paths.insert(document.absolute_path.clone());
        let should_verify = match existing_rows.get(&document.absolute_path) {
            Some((stored_modified_at, stored_size, _stored_hash, last_verified_at)) => {
                force_full_rescan
                    || *stored_modified_at != document.modified_at_millis
                    || *stored_size != document.size_bytes
                    || now_millis.saturating_sub(*last_verified_at) >= integrity_interval_millis
            }
            None => true,
        };
        if !should_verify {
            continue;
        }
        let content = match fs::read_to_string(&document.absolute_path) {
            Ok(text) => text,
            Err(_) => {
                // Skip files we can't read as UTF-8; they don't belong in a
                // Markdown text index. We deliberately do not fail the entire
                // sync over a single unreadable document — but we count it so
                // a silently-excluded file is visible in the sync report
                // instead of vanishing from search with no signal.
                report.skipped += 1;
                continue;
            }
        };
        check_recall_deadline(deadline)?;
        let content_hash = stable_fingerprint(&content);
        let content_is_unchanged = existing_rows.get(&document.absolute_path).is_some_and(
            |(stored_modified_at, stored_size, stored_hash, _)| {
                !force_full_rescan
                    && *stored_modified_at == document.modified_at_millis
                    && *stored_size == document.size_bytes
                    && stored_hash == &content_hash
            },
        );
        if content_is_unchanged {
            verified_paths.push(document.absolute_path.clone());
            continue;
        }
        let source_kind = memory_source_kind(&document.absolute_path);
        let lifecycle_metadata =
            indexed_lifecycle(&source_kind, &document.absolute_path, &content, now_millis);
        pending.push(PendingDocument {
            path: document.absolute_path.clone(),
            modified_at: document.modified_at_millis.to_string(),
            size: document.size_bytes.to_string(),
            content_hash,
            chunks: split_memory_chunks(&content),
            source_kind,
            scope: memory_scope(&document.absolute_path),
            branch: memory_branch(),
            lifecycle: lifecycle_metadata.lifecycle,
            expires_at_millis: lifecycle_metadata.expires_at_millis,
            was_existing: existing_rows.contains_key(&document.absolute_path),
        });
    }
    if force_full_rescan {
        connection
            .execute(
                "INSERT INTO documents(documents, rank) VALUES('automerge', 0)",
                [],
            )
            .map_err(|database_error| format!("disable fts automerge: {database_error}"))?;
    }

    // Phase 2: batched SQL write transactions so partial progress is preserved.
    for document_batch in pending.chunks(RECALL_TRANSACTION_BATCH_SIZE) {
        check_recall_deadline(deadline)?;
        let transaction = connection
            .transaction()
            .map_err(|database_error| format!("begin transaction: {database_error}"))?;
        for document in document_batch {
            transaction
                .execute(
                    "DELETE FROM document_meta WHERE path = ?1",
                    params![&document.path],
                )
                .map_err(|database_error| format!("delete document metadata: {database_error}"))?;
            transaction
                .execute(
                    "DELETE FROM documents WHERE path = ?1",
                    params![&document.path],
                )
                .map_err(|database_error| format!("delete stale rows: {database_error}"))?;
            for (chunk_index, chunk) in document.chunks.iter().enumerate() {
                transaction
                    .execute(
                        "INSERT INTO documents(path, modified_at, size, content) VALUES (?1, ?2, ?3, ?4)",
                        params![&document.path, &document.modified_at, &document.size, &chunk.content],
                    )
                    .map_err(|database_error| format!("insert memory chunk: {database_error}"))?;
                let rowid = transaction.last_insert_rowid();
                transaction
                    .execute(
                        "INSERT INTO document_meta(rowid, path, chunk_index, start_line, end_line, source_kind, scope, branch, content_hash, lifecycle, expires_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                        params![
                            rowid,
                            &document.path,
                            chunk_index as i64,
                            chunk.start_line as i64,
                            chunk.end_line as i64,
                            &document.source_kind,
                            &document.scope,
                            &document.branch,
                            &document.content_hash,
                            document.lifecycle,
                            document.expires_at_millis,
                        ],
                    )
                    .map_err(|database_error| format!("insert memory metadata: {database_error}"))?;
            }
            transaction
                .execute(
                    "INSERT OR REPLACE INTO file_state(path, modified_at, size, content_hash, last_verified_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        &document.path,
                        document.modified_at.parse::<i64>().unwrap_or(0),
                        document.size.parse::<i64>().unwrap_or(0),
                        &document.content_hash,
                        now_millis,
                    ],
                )
                .map_err(|database_error| format!("insert file state: {database_error}"))?;
            if document.was_existing {
                report.updated += 1;
            } else {
                report.added += 1;
            }
        }
        transaction
            .commit()
            .map_err(|database_error| format!("commit batch: {database_error}"))?;
    }

    let transaction = connection
        .transaction()
        .map_err(|database_error| format!("begin final transaction: {database_error}"))?;
    for path in &verified_paths {
        transaction
            .execute(
                "UPDATE file_state SET last_verified_at = ?1 WHERE path = ?2",
                params![now_millis, path],
            )
            .map_err(|database_error| format!("update file verification: {database_error}"))?;
    }

    let mut paths_to_remove: Vec<String> = Vec::new();
    for path in existing_rows.keys() {
        if !on_disk_paths.contains(path) {
            paths_to_remove.push(path.clone());
        }
    }
    for path in &paths_to_remove {
        transaction
            .execute("DELETE FROM document_meta WHERE path = ?1", params![path])
            .map_err(|database_error| format!("delete document metadata: {database_error}"))?;
        transaction
            .execute("DELETE FROM documents WHERE path = ?1", params![path])
            .map_err(|database_error| format!("delete vanished rows: {database_error}"))?;
        transaction
            .execute("DELETE FROM file_state WHERE path = ?1", params![path])
            .map_err(|database_error| format!("delete vanished file state: {database_error}"))?;
        report.removed += 1;
    }

    transaction
        .execute(
            "INSERT OR REPLACE INTO meta(key, value) VALUES ('last_indexed_at_millis', ?1)",
            params![now_millis.to_string()],
        )
        .map_err(|database_error| format!("stamp last_indexed_at: {database_error}"))?;
    transaction
        .commit()
        .map_err(|database_error| format!("commit final transaction: {database_error}"))?;

    if force_full_rescan {
        connection
            .execute(
                "INSERT INTO documents(documents, rank) VALUES('optimize', 1)",
                [],
            )
            .map_err(|database_error| format!("optimize fts index: {database_error}"))?;
        connection
            .execute(
                "INSERT INTO documents(documents, rank) VALUES('automerge', 4)",
                [],
            )
            .map_err(|database_error| format!("restore fts automerge: {database_error}"))?;
    }
    connection
        .pragma_update(None, "wal_checkpoint", "PASSIVE")
        .map_err(|database_error| format!("checkpoint wal: {database_error}"))?;
    report.indexed_total = on_disk.len() as u64;
    report.last_indexed_at_millis = now_millis.max(0) as u128;
    Ok(report)
}

#[derive(Debug, Clone)]
struct DocumentRecord {
    absolute_path: String,
    modified_at_millis: i64,
    size_bytes: i64,
}
#[derive(Debug, Clone)]
struct MemoryChunk {
    start_line: usize,
    end_line: usize,
    content: String,
}

fn split_memory_chunks(content: &str) -> Vec<MemoryChunk> {
    const MAX_LINES: usize = 80;
    const MAX_BYTES: usize = 12_000;
    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() {
        return vec![MemoryChunk {
            start_line: 1,
            end_line: 1,
            content: String::new(),
        }];
    }
    let mut chunks = Vec::new();
    let mut start = 0usize;
    let mut bytes = 0usize;
    for index in 0..lines.len() {
        if chunks.len() >= MAX_CHUNKS_PER_FILE {
            break;
        }
        bytes = bytes.saturating_add(lines[index].len() + 1);
        let heading_boundary = index > start && lines[index].trim_start().starts_with('#');
        let size_boundary = index.saturating_sub(start) + 1 >= MAX_LINES || bytes >= MAX_BYTES;
        if heading_boundary || size_boundary {
            let content_text = lines[start..index].join("\n");
            chunks.push(MemoryChunk {
                start_line: start + 1,
                end_line: index.max(start + 1),
                content: content_text,
            });
            start = index;
            bytes = lines[index].len() + 1;
        }
    }
    if start < lines.len() && chunks.len() < MAX_CHUNKS_PER_FILE {
        chunks.push(MemoryChunk {
            start_line: start + 1,
            end_line: lines.len(),
            content: lines[start..].join("\n"),
        });
    }
    chunks
}

fn stable_fingerprint(content: &str) -> String {
    format!("sha256:{}", sha256_hex(content.as_bytes()))
}

fn memory_source_kind(path: &str) -> String {
    for kind in [
        "research-cache",
        "working-briefs",
        "completion-gates",
        "entities",
        "graph",
        "agent-packets",
        "instincts",
        "lessons",
        "anvil",
    ] {
        if path.contains(&format!("\\{kind}\\"))
            || path.contains(&format!("/{kind}/"))
            || path.contains(&format!("\\{kind}."))
            || path.contains(&format!("/{kind}."))
        {
            return kind.to_string();
        }
    }
    "memory".to_string()
}

fn memory_scope(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    let parts: Vec<&str> = normalized.split('/').collect();
    parts
        .windows(2)
        .find(|window| window[0] == "workspaces")
        .map(|window| window[1].to_string())
        .unwrap_or_else(|| "global".to_string())
}

fn memory_branch() -> String {
    std::env::var("KEEL_BRANCH")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}
const ACTIVE_LIFECYCLE: &str = "active";
const INACTIVE_LIFECYCLE: &str = "inactive";

#[derive(Debug, Clone, Copy)]
struct IndexedLifecycle {
    lifecycle: &'static str,
    expires_at_millis: Option<i64>,
}

fn indexed_lifecycle(
    source_kind: &str,
    path: &str,
    content: &str,
    now_millis: i64,
) -> IndexedLifecycle {
    let known_structured_kind = matches!(source_kind, "research-cache" | "lessons" | "entities");
    let is_json = Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("json"));
    if !known_structured_kind || !is_json {
        return IndexedLifecycle {
            lifecycle: ACTIVE_LIFECYCLE,
            expires_at_millis: None,
        };
    }

    let fields = match parse_object_of_strings(content) {
        Ok(fields) => fields,
        Err(_) => {
            // These families are machine-readable records. Indexing malformed
            // content would make an untrusted hand-edit look like live memory.
            return IndexedLifecycle {
                lifecycle: INACTIVE_LIFECYCLE,
                expires_at_millis: None,
            };
        }
    };

    match source_kind {
        "research-cache" => indexed_research_cache_lifecycle(&fields, now_millis),
        "lessons" => {
            let status = field(&fields, "status")
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase();
            IndexedLifecycle {
                lifecycle: if matches!(status.as_str(), "quarantined" | "superseded") {
                    INACTIVE_LIFECYCLE
                } else {
                    ACTIVE_LIFECYCLE
                },
                expires_at_millis: None,
            }
        }
        "entities" => IndexedLifecycle {
            lifecycle: if field(&fields, "supersededBy")
                .is_some_and(|value| !value.trim().is_empty())
            {
                INACTIVE_LIFECYCLE
            } else {
                ACTIVE_LIFECYCLE
            },
            expires_at_millis: None,
        },
        _ => unreachable!("known structured source kind is exhaustive"),
    }
}

fn indexed_research_cache_lifecycle(fields: &Record, now_millis: i64) -> IndexedLifecycle {
    let state = field(fields, "state")
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let expiry = match indexed_research_cache_expiry_millis(fields) {
        Ok(expiry) => expiry,
        Err(()) => {
            return IndexedLifecycle {
                lifecycle: INACTIVE_LIFECYCLE,
                expires_at_millis: None,
            }
        }
    };
    let expired = expiry.is_some_and(|expires_at| expires_at <= now_millis);
    IndexedLifecycle {
        lifecycle: if expired || matches!(state.as_str(), "stale" | "expired") {
            INACTIVE_LIFECYCLE
        } else {
            ACTIVE_LIFECYCLE
        },
        expires_at_millis: expiry,
    }
}

fn indexed_research_cache_expiry_millis(fields: &Record) -> Result<Option<i64>, ()> {
    if let Some(expires_at) = field(fields, "expiresAt")
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return parse_rfc3339_millis(expires_at).map(Some).ok_or(());
    }

    let Some(freshness) = field(fields, "freshness").map(str::trim) else {
        return Ok(None);
    };
    let Some(recorded_at) = field(fields, "recordedAt").map(str::trim) else {
        return Ok(None);
    };
    let Some(ttl_millis) = parse_freshness_ttl_millis(freshness) else {
        // Free-form freshness guidance is advisory and has no safe absolute
        // expiry. Keep it active, matching family lookup semantics.
        return Ok(None);
    };
    let recorded_at_millis = parse_rfc3339_millis(recorded_at).ok_or(())?;
    let expiry = u128::try_from(recorded_at_millis)
        .ok()
        .and_then(|recorded| recorded.checked_add(ttl_millis))
        .and_then(|millis| i64::try_from(millis).ok())
        .ok_or(())?;
    Ok(Some(expiry))
}

fn parse_rfc3339_millis(value: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(value.trim())
        .ok()
        .map(|timestamp| timestamp.timestamp_millis())
}

fn parse_freshness_ttl_millis(freshness: &str) -> Option<u128> {
    let normalized = freshness.trim().to_ascii_lowercase();
    let value = normalized
        .strip_prefix("ttl=")
        .or_else(|| normalized.strip_prefix("ttl:"))
        .unwrap_or(&normalized)
        .trim();
    if value.is_empty() {
        return None;
    }
    let (amount, unit) = if value.chars().all(|character| character.is_ascii_digit()) {
        return None;
    } else if value.split_whitespace().count() == 2 {
        let mut parts = value.split_whitespace();
        (parts.next()?, parts.next()?)
    } else {
        let split_at = value
            .find(|character: char| !character.is_ascii_digit())
            .unwrap_or(value.len());
        value.split_at(split_at)
    };
    let amount = amount.parse::<u128>().ok()?;
    let unit_millis = match unit.trim() {
        "s" | "sec" | "secs" | "second" | "seconds" => 1_000,
        "m" | "min" | "mins" | "minute" | "minutes" => 60_000,
        "h" | "hr" | "hrs" | "hour" | "hours" => 3_600_000,
        "d" | "day" | "days" => 86_400_000,
        "w" | "wk" | "wks" | "week" | "weeks" => 7 * 86_400_000,
        "mo" | "month" | "months" => 30 * 86_400_000,
        _ => return None,
    };
    amount.checked_mul(unit_millis)
}

fn collect_indexable_files_until(
    directory: &Path,
    out: &mut Vec<DocumentRecord>,
    deadline: Option<Instant>,
) -> Result<(), String> {
    check_recall_deadline(deadline)?;
    let read_dir = match fs::read_dir(directory) {
        Ok(read_dir) => read_dir,
        Err(_) => return Ok(()),
    };
    for entry_result in read_dir {
        check_recall_deadline(deadline)?;
        let entry = match entry_result {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let entry_path = entry.path();
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(_) => continue,
        };
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            let dir_name = entry_path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("");
            if matches!(
                dir_name,
                "plans" | "raw_store" | "target" | "node_modules" | ".git"
            ) {
                continue;
            }
            collect_indexable_files_until(&entry_path, out, deadline)?;
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        let metadata = match entry.metadata() {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };
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
        if metadata.len() > MAX_INDEXABLE_FILE_BYTES as u64 {
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

/// Render the fields that a retrieval caller can expose for one hit. Keeping
/// this measurement on the same deterministic JSON writer as the CLI avoids a
/// character-count estimate that could disagree with the bytes actually sent
/// to a host.
fn serialized_recall_hit(hit: &RecallHit) -> Vec<u8> {
    let value = Value::Object(vec![
        (
            "absolutePath".into(),
            Value::String(hit.absolute_path.clone()),
        ),
        ("score".into(), Value::Number(format!("{:.4}", hit.score))),
        ("line".into(), Value::Number(hit.line.to_string())),
        (
            "snippet".into(),
            Value::String(bounded_recall_excerpt(&hit.snippet)),
        ),
    ]);
    let mut rendered = Vec::new();
    // The in-memory Value is composed only of finite score text and UTF-8
    // strings, so this writer cannot fail for a Vec<u8> sink.
    write_indented(&mut rendered, &value).expect("render recall hit into memory");
    rendered
}

/// Enforce the count, byte, and exact-token budgets at the canonical low-level
/// retrieval owner. The reserve leaves room for the enclosing response object,
/// array delimiters, separators, and a compact truncation/provenance envelope
/// added by higher-level callers. Oversized individual hits are skipped rather
/// than returned unbounded; later ranked hits may still fit the budget.
fn bound_recall_hits(hits: Vec<RecallHit>, requested_limit: usize) -> Vec<RecallHit> {
    let count_limit = requested_limit.min(MAX_RECALL_LIMIT);
    let byte_budget = MAX_RECALL_RESULT_BYTES.saturating_sub(RECALL_RESULT_OVERHEAD_BYTES);
    let token_budget = MAX_RECALL_RESULT_TOKENS.saturating_sub(RECALL_RESULT_OVERHEAD_TOKENS);
    let mut selected = Vec::new();
    let mut used_bytes = 0usize;
    let mut used_tokens = 0usize;

    for hit in hits {
        if selected.len() >= count_limit {
            break;
        }
        let rendered = serialized_recall_hit(&hit);
        let hit_bytes = rendered.len();
        let hit_tokens = TokenMeter::count_bytes(&rendered);
        if hit_bytes > byte_budget.saturating_sub(used_bytes)
            || hit_tokens > token_budget.saturating_sub(used_tokens)
        {
            continue;
        }
        used_bytes = used_bytes.saturating_add(hit_bytes);
        used_tokens = used_tokens.saturating_add(hit_tokens);
        selected.push(hit);
    }
    selected
}

fn bounded_recall_excerpt(text: &str) -> String {
    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= MAX_RECALL_EXCERPT_CHARS {
        return compact;
    }
    let mut excerpt = compact
        .chars()
        .take(MAX_RECALL_EXCERPT_CHARS)
        .collect::<String>();
    excerpt.push_str("… [truncated]");
    excerpt
}

pub fn query_recall_index(
    connection: &Connection,
    fts_query: &str,
    limit: usize,
    workspace_slug: Option<&str>,
) -> Result<Vec<RecallHit>, String> {
    query_recall_index_until(
        connection,
        fts_query,
        limit,
        RecallQueryOptions {
            workspace_affinity: workspace_slug,
            ..RecallQueryOptions::default()
        },
        None,
    )
}

fn query_recall_index_until(
    connection: &Connection,
    fts_query: &str,
    limit: usize,
    options: RecallQueryOptions<'_>,
    deadline: Option<Instant>,
) -> Result<Vec<RecallHit>, String> {
    check_recall_deadline(deadline)?;
    validate_fts_query(fts_query)?;
    let limit = limit.min(MAX_RECALL_LIMIT);
    let raw_query_terms = fts_terms(fts_query);
    // Over-fetch BM25 candidates so the relevance re-rank has room to promote a
    // high-coverage match that BM25 alone ranked below a term-frequency match.
    let candidate_limit = limit
        .saturating_mul(RERANK_CANDIDATE_MULTIPLIER)
        .min(RERANK_CANDIDATE_CAP)
        .max(limit);
    let now_millis = recall_now_millis();
    let scope = options.scope.map(str::to_string);
    let branch = options.branch.map(str::to_string);
    let mut prepared_statement = connection
        .prepare(
            "SELECT \
                 documents.path, \
                 bm25(documents), \
                 snippet(documents, 3, ?1, ?2, '...', ?3), \
                 documents.content, \
                 COALESCE(document_meta.start_line, 1) \
             FROM documents \
             INNER JOIN document_meta ON document_meta.rowid = documents.rowid \
             WHERE documents MATCH ?4 \
               AND document_meta.lifecycle = 'active' \
               AND (document_meta.expires_at IS NULL OR document_meta.expires_at > ?5) \
               AND (?6 IS NULL OR document_meta.scope = ?6) \
               AND (?7 IS NULL OR document_meta.branch = ?7 OR document_meta.branch = 'unknown') \
             ORDER BY bm25(documents) \
             LIMIT ?8",
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
                now_millis,
                scope,
                branch,
                candidate_limit as i64
            ],
            |row| {
                let absolute_path: String = row.get(0)?;
                // bm25() is read only to force SQLite to apply the ranking;
                // the deterministic reranker uses the BM25 position as a tie-break.
                let _bm25: f64 = row.get(1)?;
                let snippet_text: String = row.get(2)?;
                let content: String = row.get(3)?;
                let start_line: usize = row.get::<_, i64>(4)?.max(1) as usize;
                Ok((absolute_path, snippet_text, content, start_line))
            },
        )
        .map_err(|database_error| format!("query: {database_error}"))?;
    let mut candidates: Vec<RerankCandidate> = Vec::new();
    for (rank, row_result) in query_iterator.enumerate() {
        check_recall_deadline(deadline)?;
        let (absolute_path, snippet_text, content, start_line) =
            row_result.map_err(|database_error| format!("read result row: {database_error}"))?;
        let local_line = locate_first_match_line(&content, &snippet_text);
        let line = start_line.saturating_add(local_line.saturating_sub(1));
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
    check_recall_deadline(deadline)?;
    Ok(bound_recall_hits(
        rerank_by_relevance(
            candidates,
            &raw_query_terms,
            limit,
            options.workspace_affinity,
        ),
        limit,
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
///     document; a doc matching all the query's words is topically on-point.
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
    let limit = limit.min(MAX_RECALL_LIMIT);
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
/// `"fused"`), and the hits. The label lets callers tell the user how a result
/// was found: exact, typo-tolerant, or structurally expanded.
#[derive(Debug, Clone)]
pub struct CascadeResult {
    pub query_expression: String,
    pub stage: &'static str,
    pub hits: Vec<RecallHit>,
}

/// Run the deterministic recall cascade: exact (AND prefix) → relaxed (OR
/// prefix) → fuzzy (trigram similarity). Each stage only runs when the
/// previous stage returned no usable results.
#[cfg(test)]
fn cascade_recall_query(
    connection: &Connection,
    raw_query: &str,
    limit: usize,
    workspace_slug: Option<&str>,
) -> Result<Option<CascadeResult>, String> {
    cascade_recall_query_until(
        connection,
        raw_query,
        limit,
        RecallQueryOptions {
            workspace_affinity: workspace_slug,
            ..RecallQueryOptions::default()
        },
        None,
    )
}

fn cascade_recall_query_until(
    connection: &Connection,
    raw_query: &str,
    limit: usize,
    options: RecallQueryOptions<'_>,
    deadline: Option<Instant>,
) -> Result<Option<CascadeResult>, String> {
    check_recall_deadline(deadline)?;
    let limit = limit.min(MAX_RECALL_LIMIT);
    let exact = match build_fts_query(raw_query) {
        Some(query) => query,
        None => return Ok(None),
    };

    let exact_hits = query_recall_index_until(connection, &exact, limit, options, deadline)?;
    if !exact_hits.is_empty() {
        return Ok(Some(CascadeResult {
            query_expression: exact,
            stage: "exact",
            hits: exact_hits,
        }));
    }

    // Stage 2 — relaxed OR: only meaningful for multi-term queries (a single
    // token's OR and AND expressions are identical, so build_relaxed returns
    // None and we skip straight to fuzzy).
    if let Some(relaxed) = build_relaxed_fts_query(raw_query) {
        let relaxed_hits =
            query_recall_index_until(connection, &relaxed, limit, options, deadline)?;
        if !relaxed_hits.is_empty() {
            return Ok(Some(CascadeResult {
                query_expression: relaxed,
                stage: "relaxed",
                hits: relaxed_hits,
            }));
        }
    }

    // Stage 3 — fuzzy trigram scan: recovers single-word typos that prefix
    // matching cannot reach (e.g. "webhok" -> "webhook").
    let tokens = clean_query_tokens(raw_query);
    let fuzzy_hits = bound_recall_hits(
        query_recall_index_fuzzy_until(connection, &tokens, limit, options, deadline)?,
        limit,
    );
    if !fuzzy_hits.is_empty() {
        return Ok(Some(CascadeResult {
            query_expression: format!("fuzzy({})", tokens.join(" ")),
            stage: "fuzzy",
            hits: fuzzy_hits,
        }));
    }
    Ok(Some(CascadeResult {
        query_expression: exact,
        stage: "exact",
        hits: Vec::new(),
    }))
}

/// Minimum Sørensen–Dice trigram similarity for the fuzzy stage to accept a
/// word as a match. 0.45 catches single-character typos in words of moderate
/// length without admitting unrelated short words.
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
fn query_recall_index_fuzzy_until(
    connection: &Connection,
    query_tokens: &[String],
    limit: usize,
    options: RecallQueryOptions<'_>,
    deadline: Option<Instant>,
) -> Result<Vec<RecallHit>, String> {
    check_recall_deadline(deadline)?;
    let limit = limit.min(MAX_RECALL_LIMIT);
    if query_tokens.is_empty() {
        return Ok(Vec::new());
    }
    let now_millis = recall_now_millis();
    let scope = options.scope.map(str::to_string);
    let branch = options.branch.map(str::to_string);
    let mut statement = connection
        .prepare(
            "SELECT documents.path, documents.content \
             FROM documents \
             INNER JOIN document_meta ON document_meta.rowid = documents.rowid \
             WHERE document_meta.lifecycle = 'active' \
               AND (document_meta.expires_at IS NULL OR document_meta.expires_at > ?1) \
               AND (?2 IS NULL OR document_meta.scope = ?2) \
               AND (?3 IS NULL OR document_meta.branch = ?3 OR document_meta.branch = 'unknown')",
        )
        .map_err(|database_error| format!("prepare fuzzy scan: {database_error}"))?;
    let row_iterator = statement
        .query_map(params![now_millis, scope, branch], |row| {
            let path: String = row.get(0)?;
            let content: String = row.get(1)?;
            Ok((path, content))
        })
        .map_err(|database_error| format!("fuzzy scan: {database_error}"))?;

    let mut scored: Vec<(f64, RecallHit)> = Vec::new();
    for row_result in row_iterator {
        check_recall_deadline(deadline)?;
        let (absolute_path, content) =
            row_result.map_err(|database_error| format!("read fuzzy row: {database_error}"))?;
        let mut best_similarity = 0.0f64;
        let mut best_line = 0usize;
        let mut best_word = String::new();
        for (line_index, line) in content.lines().enumerate() {
            check_recall_deadline(deadline)?;
            for word in split_words(line) {
                check_recall_deadline(deadline)?;
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
    check_recall_deadline(deadline)?;
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
        .query_row("SELECT COUNT(DISTINCT path) FROM documents", [], |row| {
            row.get(0)
        })
        .map_err(|database_error| format!("count documents: {database_error}"))?;
    Ok(count.max(0) as u64)
}

fn build_search_json(
    query: &str,
    claude_home: &Path,
    matches: &[RecallHit],
    limit: usize,
    stage: &str,
    replay_workspace: Option<&str>,
    local_only: bool,
) -> Value {
    let (projected_query, query_truncated) =
        bounded_projection_text(query, MAX_RECALL_QUERY_PROJECTION_CHARS);
    let query_digest = sha256_hex(query.as_bytes());
    let projected_home =
        bounded_projection_text(&display_path(claude_home), MAX_RECALL_HOME_PROJECTION_CHARS).0;
    let projected_replay_workspace = replay_workspace
        .map(|value| bounded_projection_text(value, MAX_RECALL_HOME_PROJECTION_CHARS).0);
    let projection = RecallSearchProjection {
        query: &projected_query,
        query_truncated,
        query_digest: &query_digest,
        projected_home: &projected_home,
        claude_home,
        stage,
        limit,
        replay_workspace: projected_replay_workspace.as_deref(),
        local_only,
    };

    let mut selected: Vec<RecallHit> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut dropped = false;
    for hit in matches {
        let relative = relativize(claude_home, &PathBuf::from(&hit.absolute_path));
        let excerpt = bounded_recall_excerpt(&hit.snippet);
        let dedupe_material = format!(
            "{}\0{}",
            relative.to_ascii_lowercase(),
            excerpt.to_ascii_lowercase()
        );
        if !seen.insert(fnv1a64_hex(&dedupe_material)) {
            dropped = true;
            continue;
        }
        let mut candidate = selected.clone();
        candidate.push(hit.clone());
        let candidate_payload = recall_search_payload(
            &projection,
            &candidate,
            dropped || candidate.len() < matches.len(),
        );
        if recall_projection_within_budget(&candidate_payload) {
            selected.push(hit.clone());
        } else {
            dropped = true;
        }
    }
    let payload = recall_search_payload(
        &projection,
        &selected,
        dropped || selected.len() < matches.len(),
    );
    if recall_projection_within_budget(&payload) {
        return payload;
    }

    // Keep a second, stricter fallback so a long home path or unusual Unicode
    // cannot turn a budget failure into an unbounded/raw response.
    let minimal_query = bounded_projection_text(&projected_query, 64).0;
    let minimal_home = bounded_projection_text(&projected_home, 128).0;
    let fallback_projection = RecallSearchProjection {
        query: &minimal_query,
        query_truncated: true,
        query_digest: &query_digest,
        projected_home: &minimal_home,
        claude_home,
        stage,
        limit,
        replay_workspace: projection.replay_workspace,
        local_only: projection.local_only,
    };

    recall_search_payload(&fallback_projection, &[], true)
}

fn recall_search_payload(
    projection: &RecallSearchProjection<'_>,
    matches: &[RecallHit],
    truncated: bool,
) -> Value {
    let entries = matches
        .iter()
        .map(|hit| {
            recall_hit_value(
                projection.query,
                projection.limit,
                hit,
                projection.claude_home,
                projection.replay_workspace,
                projection.local_only,
            )
        })
        .collect();
    Value::Object(vec![
        ("query".into(), Value::String(projection.query.to_string())),
        (
            "queryDigest".into(),
            Value::String(format!("sha256:{}", projection.query_digest)),
        ),
        (
            "queryTruncated".into(),
            Value::Bool(projection.query_truncated),
        ),
        ("stage".into(), Value::String(projection.stage.to_string())),
        (
            "claudeHome".into(),
            Value::String(projection.projected_home.to_string()),
        ),
        (
            "limit".into(),
            Value::Number(projection.limit.min(MAX_RECALL_LIMIT).to_string()),
        ),
        ("count".into(), Value::Number(matches.len().to_string())),
        ("truncated".into(), Value::Bool(truncated)),
        ("matches".into(), Value::Array(entries)),
    ])
}

struct RecallSearchProjection<'a> {
    query: &'a str,
    query_truncated: bool,
    query_digest: &'a str,
    projected_home: &'a str,
    claude_home: &'a Path,
    stage: &'a str,
    limit: usize,
    replay_workspace: Option<&'a str>,
    local_only: bool,
}

fn recall_hit_value(
    query: &str,
    limit: usize,
    hit: &RecallHit,
    claude_home: &Path,
    replay_workspace: Option<&str>,
    local_only: bool,
) -> Value {
    let memory_id = sha256_hex(
        format!(
            "recall-memory\0{}\0{}\0{}",
            hit.absolute_path, hit.line, hit.snippet
        )
        .as_bytes(),
    );
    let provenance_id = sha256_hex(
        format!(
            "recall-provenance\0{}\0{}\0{}",
            query, hit.absolute_path, hit.line
        )
        .as_bytes(),
    );
    let relative = relativize(claude_home, &PathBuf::from(&hit.absolute_path));
    let retrieval_query = query.to_string();
    let retrieval_limit = limit.clamp(1, MAX_RECALL_LIMIT);
    let retrieval_ref = recall_retrieval_ref(
        &retrieval_query,
        retrieval_limit,
        replay_workspace,
        local_only,
    );
    Value::Object(vec![
        ("path".into(), Value::String(relative)),
        (
            "absolutePath".into(),
            Value::String(hit.absolute_path.clone()),
        ),
        ("score".into(), Value::Number(format!("{:.4}", hit.score))),
        ("line".into(), Value::Number(hit.line.to_string())),
        (
            "snippet".into(),
            Value::String(bounded_recall_excerpt(&hit.snippet)),
        ),
        (
            "memoryId".into(),
            Value::String(format!("memory-{memory_id}")),
        ),
        (
            "provenanceId".into(),
            Value::String(format!("prov-sha256:{provenance_id}")),
        ),
        ("retrievalRef".into(), Value::String(retrieval_ref)),
    ])
}

fn recall_retrieval_ref(
    query: &str,
    limit: usize,
    replay_workspace: Option<&str>,
    local_only: bool,
) -> String {
    let mut retrieval_ref = format!("keel memory recall {:?} --limit {limit}", query);
    if let Some(workspace) = replay_workspace {
        retrieval_ref.push_str(&format!(" --workspace {workspace:?}"));
    }
    if local_only {
        retrieval_ref.push_str(" --local-only");
    }
    retrieval_ref
}

fn bounded_projection_text(text: &str, max_chars: usize) -> (String, bool) {
    let mut characters = text.chars();
    let mut value = characters.by_ref().take(max_chars).collect::<String>();
    let truncated = characters.next().is_some();
    if truncated {
        value.push('…');
    }
    (value, truncated)
}

fn serialized_value(value: &Value) -> Vec<u8> {
    let mut rendered = Vec::new();
    write_indented(&mut rendered, value).expect("render recall projection into memory");
    rendered
}

fn recall_projection_within_budget(value: &Value) -> bool {
    let rendered = serialized_value(value);
    rendered.len() <= MAX_RECALL_RESULT_BYTES
        && TokenMeter::count_bytes(&rendered) <= MAX_RECALL_RESULT_TOKENS
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
        let _home_precedence = crate::test_support::HomePrecedenceGuard::clear_keel_home();
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
    fn recall_rejects_queries_over_the_hard_byte_bound() {
        let oversized = "x".repeat(MAX_RECALL_QUERY_BYTES + 1);
        let error = validate_recall_query(&oversized).expect_err("oversized query must fail");
        assert!(error.contains("over the"), "error: {error}");

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let exit = run_recall_command("memory", &[oversized], &mut stdout, &mut stderr);
        assert_eq!(exit, 2, "oversized CLI query must be rejected");
        assert!(
            String::from_utf8_lossy(&stderr).contains("byte"),
            "stderr: {}",
            String::from_utf8_lossy(&stderr)
        );
    }

    #[test]
    fn recall_limit_is_hard_capped_but_malformed_values_still_fail() {
        assert_eq!(
            parse_limit(&(MAX_RECALL_LIMIT + 1).to_string()),
            Ok(MAX_RECALL_LIMIT)
        );
        assert!(parse_limit("0").is_err());
        assert!(parse_limit("not-a-number").is_err());
    }

    #[test]
    fn recall_deadline_fails_closed_when_expired() {
        let expired = Instant::now() - Duration::from_millis(1);
        let error = check_recall_deadline(Some(expired)).expect_err("expired deadline");
        assert!(error.contains("deadline"), "error: {error}");
    }

    #[test]
    fn recall_hits_respect_count_byte_and_token_bounds() {
        let hits = (0..MAX_RECALL_LIMIT + 10)
            .map(|index| RecallHit {
                absolute_path: format!("C:/memory/note-{index}.md"),
                score: 0.5,
                line: index + 1,
                snippet: format!("[match] memory result {index}"),
            })
            .collect();
        let bounded = bound_recall_hits(hits, usize::MAX);
        assert!(bounded.len() <= MAX_RECALL_LIMIT);

        let total_bytes: usize = bounded
            .iter()
            .map(|hit| serialized_recall_hit(hit).len())
            .sum();
        let total_tokens: usize = bounded
            .iter()
            .map(|hit| TokenMeter::count_bytes(&serialized_recall_hit(hit)))
            .sum();
        assert!(
            total_bytes <= MAX_RECALL_RESULT_BYTES.saturating_sub(RECALL_RESULT_OVERHEAD_BYTES),
            "result bytes exceeded bound: {total_bytes}"
        );
        assert!(
            total_tokens <= MAX_RECALL_RESULT_TOKENS.saturating_sub(RECALL_RESULT_OVERHEAD_TOKENS),
            "result tokens exceeded bound: {total_tokens}"
        );
    }

    #[test]
    fn recall_json_projection_recounts_complete_envelope_and_keeps_recovery() {
        let query = "query ".repeat(MAX_RECALL_QUERY_BYTES / 6);
        let hits = (0..MAX_RECALL_LIMIT)
            .map(|index| RecallHit {
                absolute_path: format!("C:/memory/note-{index}.md"),
                score: 0.5,
                line: index + 1,
                snippet: format!("[match] {}", "memory result ".repeat(80)),
            })
            .collect::<Vec<_>>();
        let payload = build_search_json(
            &query,
            Path::new("C:/memory"),
            &hits,
            MAX_RECALL_LIMIT,
            "exact",
            None,
            false,
        );

        let rendered = serialized_value(&payload);
        assert!(
            rendered.len() <= MAX_RECALL_RESULT_BYTES,
            "complete recall envelope exceeded byte bound: {}",
            rendered.len()
        );
        assert!(
            TokenMeter::count_bytes(&rendered) <= MAX_RECALL_RESULT_TOKENS,
            "complete recall envelope exceeded token bound"
        );
        let text = String::from_utf8(rendered).expect("json utf8");
        assert!(text.contains("provenanceId"), "provenance missing: {text}");
        assert!(text.contains("retrievalRef"), "recovery missing: {text}");
        assert!(
            text.contains("queryTruncated"),
            "query bound missing: {text}"
        );
        assert!(serde_json::from_str::<serde_json::Value>(&text).is_ok());
    }

    #[test]
    fn recall_recovery_reference_preserves_scope_filters() {
        let payload = build_search_json(
            "webhook",
            Path::new("C:/memory"),
            &[RecallHit {
                absolute_path: "C:/memory/notes.md".into(),
                score: 0.5,
                line: 1,
                snippet: "webhook signature".into(),
            }],
            1,
            "exact",
            Some("project"),
            true,
        );
        let text = String::from_utf8(serialized_value(&payload)).expect("json utf8");
        let parsed: serde_json::Value = serde_json::from_str(&text).expect("valid recall json");
        let reference = parsed["matches"][0]["retrievalRef"]
            .as_str()
            .expect("retrievalRef is a string");
        assert!(
            reference.contains("--workspace \"project\"") && reference.contains("--local-only"),
            "recovery reference must preserve retrieval scope: {reference}"
        );
    }

    #[test]
    fn fuzzy_stage_respects_the_same_result_bounds() {
        run_with_home("keel-recall-fuzzy-bounds", |claude_home| {
            for index in 0..MAX_RECALL_LIMIT + 10 {
                write_memory(
                    claude_home,
                    &format!("memories/fuzzy-{index}.md"),
                    "# Webhook incident\nThe webhook signature is verified.\n",
                );
            }
            let database_path = recall_database_path(claude_home);
            let mut connection = open_recall_connection(&database_path).expect("open recall index");
            sync_recall_index(&mut connection, claude_home, true).expect("sync recall index");
            let result = cascade_recall_query(&connection, "webhok", usize::MAX, None)
                .expect("fuzzy cascade")
                .expect("fuzzy result");
            assert_eq!(result.stage, "fuzzy");
            assert!(result.hits.len() <= MAX_RECALL_LIMIT);
            let total_bytes: usize = result
                .hits
                .iter()
                .map(|hit| serialized_recall_hit(hit).len())
                .sum();
            let total_tokens: usize = result
                .hits
                .iter()
                .map(|hit| TokenMeter::count_bytes(&serialized_recall_hit(hit)))
                .sum();
            assert!(
                total_bytes <= MAX_RECALL_RESULT_BYTES.saturating_sub(RECALL_RESULT_OVERHEAD_BYTES),
                "fuzzy result bytes exceeded bound: {total_bytes}"
            );
            assert!(
                total_tokens
                    <= MAX_RECALL_RESULT_TOKENS.saturating_sub(RECALL_RESULT_OVERHEAD_TOKENS),
                "fuzzy result tokens exceeded bound: {total_tokens}"
            );
        });
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

            // The write-time sync the production handlers now call. This caller
            // retains no path, so it passes the empty slice.
            reindex_after_write_paths(claude_home, &[]).expect("reindex_after_write succeeds");

            // Query the index directly — NO search_recall_index (no read-path sync).
            let database_path = recall_database_path(claude_home);
            let connection =
                open_recall_connection(&database_path).expect("open recall index read-only");
            let fts_query = build_fts_query("blind tool search").expect("non-empty query");
            let hits = query_recall_index(&connection, &fts_query, 20, None).expect("query index");

            assert!(
                hits.iter()
                    .any(|hit| hit.absolute_path.contains("rc-42.json")),
                "reindex_after_write_paths must index the new record so it is found with no read-path sync; hits: {:?}",
                hits.iter().map(|h| &h.absolute_path).collect::<Vec<_>>()
            );
        });
    }

    #[test]
    fn targeted_reindex_invalidates_only_the_written_file() {
        run_with_home("keel-reindex-targeted", |claude_home| {
            write_memory(
                claude_home,
                "memory/changed.json",
                "{\"note\":\"before\"}\n",
            );
            write_memory(
                claude_home,
                "memory/unchanged.json",
                "{\"note\":\"stable\"}\n",
            );
            let database_path = recall_database_path(claude_home);
            let mut connection = open_recall_connection(&database_path).expect("open index");
            sync_recall_index(&mut connection, claude_home, true).expect("initial sync");

            let changed_path = claude_home.join("memory/changed.json");
            fs::write(&changed_path, "{\"note\":\"after\"}\n").expect("replace content");
            let metadata = fs::metadata(&changed_path).expect("changed metadata");
            let modified_at = metadata
                .modified()
                .expect("changed mtime")
                .duration_since(UNIX_EPOCH)
                .expect("changed mtime epoch")
                .as_millis() as i64;
            let future = recall_now_millis().saturating_add(600_000);
            let changed_path_text = changed_path.to_string_lossy().to_string();
            connection
                .execute(
                    "UPDATE file_state SET modified_at = ?1, size = ?2, last_verified_at = ?3 WHERE path = ?4",
                    params![modified_at, metadata.len() as i64, future, changed_path_text],
                )
                .expect("seed changed file metadata");
            connection
                .execute(
                    "UPDATE file_state SET last_verified_at = ?1 WHERE path LIKE ?2",
                    params![future, "%unchanged.json"],
                )
                .expect("seed unchanged verification timestamp");
            drop(connection);

            reindex_after_write_paths(claude_home, &[changed_path.as_path()])
                .expect("targeted reindex");

            let connection = open_recall_connection(&database_path).expect("reopen index");
            let fts_query = build_fts_query("after").expect("changed query");
            let hits = query_recall_index(&connection, &fts_query, 20, None).expect("query index");
            assert!(
                hits.iter()
                    .any(|hit| hit.absolute_path.ends_with("changed.json")),
                "targeted reindex must refresh the changed file"
            );
            let unchanged_verified: i64 = connection
                .query_row(
                    "SELECT last_verified_at FROM file_state WHERE path LIKE ?1",
                    params!["%unchanged.json"],
                    |row| row.get(0),
                )
                .expect("unchanged file state");
            assert_eq!(
                unchanged_verified, future,
                "unchanged files must not be reread by a targeted write sync"
            );
        });
    }

    #[cfg(unix)]
    #[test]
    fn recall_skips_symlinked_memory_files() {
        run_with_home("keel-recall-symlink", |claude_home| {
            let outside = claude_home
                .parent()
                .expect("temporary parent")
                .join("outside.md");
            fs::write(&outside, "# outside secret\n").expect("outside file");
            let link = claude_home.join("memories").join("linked.md");
            fs::create_dir_all(link.parent().expect("memory link parent"))
                .expect("create memory link parent");
            std::os::unix::fs::symlink(&outside, &link).expect("symlink");

            let database_path = recall_database_path(claude_home);
            let mut connection = open_recall_connection(&database_path).expect("open recall index");
            sync_recall_index(&mut connection, claude_home, true).expect("sync recall");
            let query = build_fts_query("outside secret").expect("query");
            let hits = query_recall_index(&connection, &query, 20, None).expect("search");
            assert!(
                hits.iter()
                    .all(|hit| !hit.absolute_path.contains("linked.md")),
                "symlinked memory file must not be indexed: {hits:?}"
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
                !post_text.contains("memories/draft.md"),
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
            // Status reads the stored index without syncing: index first.
            let reindex_code =
                run_recall_command("memory", &["reindex".to_string()], &mut stdout, &mut stderr);
            assert_eq!(
                reindex_code,
                0,
                "stderr: {}",
                String::from_utf8_lossy(&stderr)
            );
            stdout.clear();
            stderr.clear();
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
                rendered.contains("matches=0"),
                "unknown query should return 0 matches: {rendered}"
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
    #[test]
    fn scoped_recall_filters_scope_before_exact_and_fuzzy_limits() {
        run_with_home("keel-recall-scoped-limits", |claude_home| {
            for index in 0..8 {
                write_memory(
                    claude_home,
                    &format!("memories/workspaces/other/foreign-{index}.md"),
                    "# Webhook\nforeign workspace webhook notes\n",
                );
            }
            write_memory(
                claude_home,
                "memories/workspaces/project/local.md",
                "# Webhook\ncurrent workspace webhook notes\n",
            );
            let options = RecallQueryOptions {
                scope: Some("project"),
                ..RecallQueryOptions::default()
            };

            let exact = search_recall_index_with_options(claude_home, "webhook", 1, options)
                .expect("scoped exact search")
                .expect("exact query");
            assert_eq!(exact.hits.len(), 1);
            assert!(
                exact.hits[0]
                    .absolute_path
                    .ends_with("workspaces\\project\\local.md")
                    || exact.hits[0]
                        .absolute_path
                        .ends_with("workspaces/project/local.md"),
                "scope filter must run before LIMIT: {:?}",
                exact.hits
            );

            let fuzzy = search_recall_index_with_options(claude_home, "webhok", 1, options)
                .expect("scoped fuzzy search")
                .expect("fuzzy query");
            assert_eq!(fuzzy.stage, "fuzzy");
            assert_eq!(fuzzy.hits.len(), 1);
            assert!(
                fuzzy.hits[0]
                    .absolute_path
                    .ends_with("workspaces\\project\\local.md")
                    || fuzzy.hits[0]
                        .absolute_path
                        .ends_with("workspaces/project/local.md"),
                "fuzzy scope filter must run before LIMIT: {:?}",
                fuzzy.hits
            );

            let home_argument = claude_home.to_string_lossy().to_string();
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            let exit_code = run_recall_command(
                "memory",
                &[
                    "webhook".to_string(),
                    "--limit".to_string(),
                    "1".to_string(),
                    "--workspace".to_string(),
                    "project".to_string(),
                    "--local-only".to_string(),
                    "--claude-home".to_string(),
                    home_argument,
                ],
                &mut stdout,
                &mut stderr,
            );
            assert_eq!(exit_code, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
            let rendered = String::from_utf8_lossy(&stdout);
            assert!(rendered.contains("workspaces/project/local.md"));
            assert!(!rendered.contains("workspaces/other/foreign-"));
        });
    }

    #[test]
    fn recall_excludes_inactive_structured_memory_but_preserves_files() {
        run_with_home("keel-recall-lifecycle", |claude_home| {
            write_memory(
                claude_home,
                "memory/research-cache/stale.json",
                "{\n  \"id\": \"stale\",\n  \"question\": \"lifecycle-boundary\",\n  \"answer\": \"stale answer\",\n  \"state\": \"stale\"\n}\n",
            );
            write_memory(
                claude_home,
                "memory/research-cache/expired.json",
                "{\n  \"id\": \"expired\",\n  \"question\": \"lifecycle-boundary\",\n  \"answer\": \"expired answer\",\n  \"state\": \"fresh\",\n  \"expiresAt\": \"1970-01-01T00:00:00Z\"\n}\n",
            );
            write_memory(
                claude_home,
                "memory/lessons/quarantined.json",
                "{\n  \"id\": \"quarantined\",\n  \"pattern\": \"lifecycle-boundary\",\n  \"evidence\": \"regressed\",\n  \"response\": \"do not reuse\",\n  \"status\": \"quarantined\"\n}\n",
            );
            write_memory(
                claude_home,
                "memory/entities/superseded.json",
                "{\n  \"id\": \"old\",\n  \"name\": \"lifecycle-boundary\",\n  \"summary\": \"old decision\",\n  \"supersededBy\": \"new\"\n}\n",
            );
            write_memory(
                claude_home,
                "memory/lessons/active.json",
                "{\n  \"id\": \"active\",\n  \"pattern\": \"lifecycle-boundary\",\n  \"evidence\": \"verified\",\n  \"response\": \"reuse\",\n  \"status\": \"active\"\n}\n",
            );

            let database_path = recall_database_path(claude_home);
            let mut connection = open_recall_connection(&database_path).expect("open recall index");
            sync_recall_index(&mut connection, claude_home, true).expect("sync recall index");
            let query = build_fts_query("lifecycle-boundary").expect("query");
            let hits = query_recall_index(&connection, &query, 20, None).expect("query index");
            let paths: Vec<&str> = hits.iter().map(|hit| hit.absolute_path.as_str()).collect();
            assert!(
                paths.iter().any(|path| path.ends_with("active.json")),
                "active lesson should remain recallable: {paths:?}"
            );
            for blocked in [
                "stale.json",
                "expired.json",
                "quarantined.json",
                "superseded.json",
            ] {
                assert!(
                    paths.iter().all(|path| !path.ends_with(blocked)),
                    "inactive record leaked into recall: {blocked}; {paths:?}"
                );
            }

            let expiry: Option<i64> = connection
                .query_row(
                    "SELECT expires_at FROM document_meta WHERE path LIKE ?1 LIMIT 1",
                    params!["%expired.json"],
                    |row| row.get(0),
                )
                .expect("expired metadata");
            assert!(expiry.is_some(), "absolute expiry must be indexed");
            assert!(
                claude_home
                    .join("memory/research-cache/expired.json")
                    .is_file(),
                "lifecycle filtering must not delete source records"
            );
        });
    }
}

#[cfg(test)]
mod recall_edge_tests {
    use super::*;

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

    #[test]
    fn memory_chunks_preserve_section_line_ranges() {
        let chunks = split_memory_chunks("# First\nalpha\n# Second\nbeta\n");
        assert_eq!(chunks.len(), 2);
        assert_eq!((chunks[0].start_line, chunks[0].end_line), (1, 2));
        assert_eq!((chunks[1].start_line, chunks[1].end_line), (3, 4));
        assert_eq!(
            memory_source_kind("C:/home/memory/research-cache/r.json"),
            "research-cache"
        );
        assert_eq!(
            memory_scope("C:/home/memories/workspaces/project/r.json"),
            "project"
        );
    }
}
