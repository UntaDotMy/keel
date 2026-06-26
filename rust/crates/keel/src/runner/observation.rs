//! Purpose: Append-only per-day JSONL log of behavioral observations that feeds
//!   the autonomous learning loop (observe -> instinct -> generated skill).
//! Caller: runner::hook_lifecycle PostToolUse; utility::learning reads it back.
//! Dependencies: serde_json, runtime::resolve_claude_home, std::fs.
//! Main Functions: record_observation, iter_recent_rows, prune_older_than.
//! Side Effects: Creates `<claude_home>/state/observations/<YYYY-MM-DD>.jsonl`
//!   and appends one line per qualifying tool call.
//!
//! Design note: this is a deliberately separate concern from `tool_timings`.
//! tool-timings answers "how long did each tool take" (a perf log with a frozen
//! schema). Observations answer "what does the user repeatedly *do*" — the
//! signal the learning loop distills into instincts and, eventually, generated
//! skills. Keeping them in two modules keeps each single-responsibility rather
//! than overloading the timing row with behavioral fields it was never meant to
//! carry. Every write is fail-open: a learning-capture error must never fail the
//! PostToolUse hook, mirroring `tool_timings::record_tool_timing`.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value as JsonDocument};

use crate::runtime::resolve_claude_home;

/// Maximum characters retained for the human-readable `detail` field. A command
/// or path far longer than this carries no extra learning signal and would only
/// bloat the log, so we truncate. Mirrors ECC's 5000-char cap intent at a size
/// tuned for signatures rather than full tool payloads.
const DETAIL_MAX_CHARS: usize = 240;

/// One parsed observation ready to serialize. Public so the learning engine can
/// reconstruct rows it reads back without re-parsing raw JSON in two places.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observation {
    pub recorded_at_ms: u64,
    pub session_id: String,
    pub cwd: String,
    pub tool_name: String,
    /// Stable, low-cardinality key for the action (e.g. `git commit`,
    /// `cargo test`, `edit:rs`). This is what the learning loop counts.
    pub signature: String,
    /// Short, secret-scrubbed, truncated human detail for the digest body.
    pub detail: String,
}

/// Append one observation derived from a PostToolUse hook `input` document.
///
/// `input` is the already-parsed stdin JSON the caller also handed to
/// `record_tool_timing`, so no second stdin read is needed. Returns `Ok(false)`
/// when the call carries no learnable signature (an unknown tool with no input)
/// so the caller can stay silent, and `Ok(true)` when a row was appended.
///
/// This is the SUCCESS path. Failed tool calls go through
/// [`record_failure_observation`] so the learning loop can distinguish "what the
/// user does" from "what reliably goes wrong here".
pub fn record_observation(input: &JsonDocument) -> std::io::Result<bool> {
    record_observation_with_outcome(input, false)
}

/// Append one observation from flat parts (no the harness hook JSON required).
///
/// This is the host-neutral adapter: callers outside the harness hook path
/// (bridge, OpenCode plugin) pass the individual fields directly instead of
/// synthesizing a fake hook-JSON document. Reuses the same signature-derivation
/// logic as [`record_observation_with_outcome`] by constructing a minimal
/// JSON-like lookup keyed on `tool_input`.
pub fn record_observation_from_parts(
    claude_home: &std::path::Path,
    tool_name: &str,
    tool_input_json: &str,
    cwd: &str,
    session_id: &str,
    failed: bool,
) -> std::io::Result<bool> {
    let tool_name = tool_name.trim();
    if tool_name.is_empty() {
        return Ok(false);
    }
    let tool_input: JsonDocument = if tool_input_json.trim().is_empty() {
        JsonDocument::Null
    } else {
        match serde_json::from_str(tool_input_json) {
            Ok(value) => value,
            Err(_) => JsonDocument::Null,
        }
    };

    // Wrap the raw tool input in a { tool_input: ... } envelope so
    // derive_signature can resolve it the same way the native Claude hook
    // path does (where the full hook input already contains tool_input).
    let envelope = serde_json::json!({ "tool_input": tool_input });
    let Some((mut signature, detail)) = derive_signature(tool_name, &envelope) else {
        return Ok(false);
    };
    if failed {
        signature.push_str(FAILURE_SIGNATURE_SUFFIX);
    }

    let observations_path = claude_home.join("state").join("observations");
    std::fs::create_dir_all(&observations_path)?;
    let date = chrono::Local::now().format("%Y-%m-%d").to_string();
    let path = observations_path.join(format!("{date}.jsonl"));

    let line = serde_json::json!({
        "recorded_at_ms": now_ms(),
        "session_id": session_id,
        "cwd": cwd,
        "tool_name": tool_name,
        "signature": signature,
        "detail": detail,
    });

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    writeln!(file, "{line}")?;
    Ok(true)
}

/// Append a FAILURE observation derived from a PostToolUseFailure hook `input`.
///
/// Identical capture to [`record_observation`] except the derived signature is
/// suffixed with [`FAILURE_SIGNATURE_SUFFIX`] so a failing `cargo test` clusters
/// separately from a passing one. A failure pattern that recurs at the trust bar
/// becomes its own instinct and surfaces in the SessionStart digest — the
/// Reflexion-style "learn from what goes wrong" signal, built on the same
/// observe→instinct pipeline as success capture. Returns `Ok(false)` when the
/// failed call carries no learnable signature.
pub fn record_failure_observation(input: &JsonDocument) -> std::io::Result<bool> {
    record_observation_with_outcome(input, true)
}

/// Marker appended to a signature when the observed tool call FAILED. Kept as a
/// human-readable suffix (not a separate JSON field) so the existing
/// signature-keyed clustering, instinct ids, and digest phrasing pick it up with
/// no schema change — a failed `cargo test` simply has signature
/// `cargo test (failed)` and clusters on its own.
pub const FAILURE_SIGNATURE_SUFFIX: &str = " (failed)";

fn record_observation_with_outcome(input: &JsonDocument, failed: bool) -> std::io::Result<bool> {
    let tool_name = input
        .get("tool_name")
        .and_then(JsonDocument::as_str)
        .unwrap_or_default();
    if tool_name.is_empty() {
        return Ok(false);
    }

    let Some((mut signature, detail)) = derive_signature(tool_name, input) else {
        return Ok(false);
    };
    if failed {
        signature.push_str(FAILURE_SIGNATURE_SUFFIX);
    }

    let Some(path) = observations_path_for_today() else {
        return Ok(false);
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let line = json!({
        "recorded_at_ms": now_ms(),
        "session_id": input.get("session_id").and_then(JsonDocument::as_str).unwrap_or_default(),
        "cwd": input.get("cwd").and_then(JsonDocument::as_str).unwrap_or_default(),
        "tool_name": tool_name,
        "signature": signature,
        "detail": detail,
    });

    let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
    writeln!(file, "{line}")?;
    Ok(true)
}

/// Derive a stable signature and a short detail string for a tool call.
///
/// Returns `None` when the call has no useful learning signal. The signature is
/// the unit the learning loop counts, so it must be low-cardinality and stable
/// across runs: a full command line would never repeat, but its leading
/// program+subcommand (`git commit`, `cargo test`) does.
fn derive_signature(tool_name: &str, input: &JsonDocument) -> Option<(String, String)> {
    let tool_input = input.get("tool_input");
    let tool_name_lower = tool_name.to_ascii_lowercase();
    match tool_name_lower.as_str() {
        "bash" => {
            let command = tool_input
                .and_then(|value| value.get("command"))
                .and_then(JsonDocument::as_str)
                .unwrap_or_default();
            let scrubbed = scrub_secrets(command);
            let signature = command_signature(&scrubbed)?;
            Some((signature, truncate_detail(&scrubbed)))
        }
        "edit" | "write" | "multiedit" | "notebookedit" => {
            let path = tool_input
                .and_then(|value| value.get("file_path"))
                .and_then(JsonDocument::as_str)
                .unwrap_or_default();
            let extension = file_extension(path);
            let signature = format!("edit:{extension}");
            Some((signature, truncate_detail(path)))
        }
        // Read/Grep/Glob/etc. are navigation noise for behavioral learning:
        // they recur constantly without expressing an intent worth distilling.
        // Skipping them keeps the signal-to-noise high and the log small.
        _ => None,
    }
}

/// Extract `program` or `program subcommand` from a shell command line.
///
/// Stops at the first shell operator (`|`, `&&`, `;`, redirects) so a pipeline's
/// signature is its first stage. Returns `None` for an empty command.
fn command_signature(command: &str) -> Option<String> {
    let head = command
        .split(['|', '&', ';', '>', '<', '\n'])
        .next()
        .unwrap_or("")
        .trim();
    let mut tokens = head.split_whitespace();
    let program = tokens.next()?;
    // Strip a leading path so `/usr/bin/git` and `git` share one signature.
    let program = program.rsplit(['/', '\\']).next().unwrap_or(program);
    if program.is_empty() {
        return None;
    }
    // A subcommand is the next bare word (not a flag, not an assignment) for the
    // small set of multiplexer tools where the subcommand is the real verb.
    let take_subcommand = matches!(
        program,
        "git"
            | "cargo"
            | "npm"
            | "pnpm"
            | "yarn"
            | "docker"
            | "kubectl"
            | "go"
            | "python"
            | "python3"
            | "pip"
            | "make"
            | "gh"
            | "terraform"
            | "dotnet"
    );
    if take_subcommand {
        if let Some(subcommand) = tokens.find(|token| {
            !token.starts_with('-') && !token.contains('=') && token.chars().all(is_signature_char)
        }) {
            return Some(format!("{program} {subcommand}"));
        }
    }
    Some(program.to_string())
}

fn is_signature_char(character: char) -> bool {
    character.is_ascii_alphanumeric() || character == '-' || character == '_' || character == ':'
}

fn file_extension(path: &str) -> String {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() && !extension.is_empty() => {
            extension.to_ascii_lowercase()
        }
        _ => "none".to_string(),
    }
}

/// Redact obvious secret-bearing tokens before anything reaches disk. This is
/// the same defensive posture ECC's observe hook takes: a learning log lives
/// under the user's home and must never become a credential leak.
fn scrub_secrets(text: &str) -> String {
    text.split_whitespace()
        .map(|token| {
            let lowered = token.to_ascii_lowercase();
            let looks_secret = lowered.contains("token")
                || lowered.contains("secret")
                || lowered.contains("password")
                || lowered.contains("passwd")
                || lowered.contains("apikey")
                || lowered.contains("api_key")
                || lowered.contains("authorization")
                || lowered.contains("bearer")
                || lowered.contains("credential");
            if looks_secret {
                "[REDACTED]".to_string()
            } else {
                token.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn truncate_detail(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= DETAIL_MAX_CHARS {
        return trimmed.to_string();
    }
    let prefix: String = trimmed.chars().take(DETAIL_MAX_CHARS).collect();
    format!("{prefix}…")
}

/// Read every observation row recorded within the last `days` days, oldest
/// first. Unparseable lines are skipped (fail-open) so one corrupt row never
/// poisons a learning cycle. Returns an empty vec when nothing has been
/// recorded yet so the engine's cold path needs no special-casing.
pub fn iter_recent_rows(days: u64) -> std::io::Result<Vec<Observation>> {
    if days == 0 {
        return Ok(Vec::new());
    }
    let Some(directory) = observations_directory() else {
        return Ok(Vec::new());
    };
    if !directory.exists() {
        return Ok(Vec::new());
    }

    let today = chrono::Local::now().date_naive();
    let mut rows = Vec::new();
    let mut dates: Vec<chrono::NaiveDate> = Vec::new();
    for offset in 0..days {
        let Some(date) = today.checked_sub_days(chrono::Days::new(offset)) else {
            break;
        };
        dates.push(date);
    }
    dates.sort();
    for date in dates {
        let path = directory.join(format!("{}.jsonl", date.format("%Y-%m-%d")));
        let Ok(body) = fs::read_to_string(&path) else {
            continue;
        };
        for line in body.lines() {
            if let Some(observation) = parse_row(line) {
                rows.push(observation);
            }
        }
    }
    Ok(rows)
}

fn parse_row(line: &str) -> Option<Observation> {
    let value: JsonDocument = serde_json::from_str(line).ok()?;
    let signature = value.get("signature").and_then(JsonDocument::as_str)?;
    if signature.is_empty() {
        return None;
    }
    Some(Observation {
        recorded_at_ms: value
            .get("recorded_at_ms")
            .and_then(JsonDocument::as_u64)
            .unwrap_or(0),
        session_id: value
            .get("session_id")
            .and_then(JsonDocument::as_str)
            .unwrap_or_default()
            .to_string(),
        cwd: value
            .get("cwd")
            .and_then(JsonDocument::as_str)
            .unwrap_or_default()
            .to_string(),
        tool_name: value
            .get("tool_name")
            .and_then(JsonDocument::as_str)
            .unwrap_or_default()
            .to_string(),
        signature: signature.to_string(),
        detail: value
            .get("detail")
            .and_then(JsonDocument::as_str)
            .unwrap_or_default()
            .to_string(),
    })
}

/// Delete per-day observation files older than `days` ago, by filename date.
/// Same contract as `tool_timings::prune_older_than`: filename is the
/// structural truth, foreign files are left untouched, missing dir => Ok(0).
pub fn prune_older_than(days: u64) -> std::io::Result<usize> {
    let Some(directory) = observations_directory() else {
        return Ok(0);
    };
    if !directory.exists() {
        return Ok(0);
    }
    let today = chrono::Local::now().date_naive();
    let cutoff = match today.checked_sub_days(chrono::Days::new(days)) {
        Some(date) => date,
        None => return Ok(0),
    };
    let mut removed = 0usize;
    for entry in fs::read_dir(&directory)? {
        let entry = entry?;
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else {
            continue;
        };
        let Some(stem) = name.strip_suffix(".jsonl") else {
            continue;
        };
        let Ok(file_date) = chrono::NaiveDate::parse_from_str(stem, "%Y-%m-%d") else {
            continue;
        };
        if file_date < cutoff {
            fs::remove_file(entry.path())?;
            removed += 1;
        }
    }
    Ok(removed)
}

fn observations_path_for_today() -> Option<PathBuf> {
    let directory = observations_directory()?;
    let date = chrono::Local::now().format("%Y-%m-%d").to_string();
    Some(directory.join(format!("{date}.jsonl")))
}

fn observations_directory() -> Option<PathBuf> {
    let claude_home = resolve_claude_home("").ok()?;
    Some(claude_home.join("state").join("observations"))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|delta| delta.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::ENV_LOCK;
    use serde_json::json;

    fn with_isolated_claude_home<F: FnOnce(&PathBuf) -> R, R>(suffix: &str, run: F) -> R {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let root = std::env::temp_dir().join(format!(
            "keel-observations-{}-{nanos}-{suffix}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("create test claude home");
        let previous_home = std::env::var("CLAUDE_TARGET_OVERRIDE").ok();
        std::env::set_var("CLAUDE_TARGET_OVERRIDE", &root);
        let result = run(&root);
        match previous_home {
            Some(value) => std::env::set_var("CLAUDE_TARGET_OVERRIDE", value),
            None => std::env::remove_var("CLAUDE_TARGET_OVERRIDE"),
        }
        let _ = fs::remove_dir_all(&root);
        result
    }

    #[test]
    fn command_signature_extracts_program_and_subcommand() {
        assert_eq!(
            command_signature("git commit -m 'msg'").as_deref(),
            Some("git commit")
        );
        assert_eq!(
            command_signature("cargo test --workspace").as_deref(),
            Some("cargo test")
        );
        assert_eq!(command_signature("ls -la").as_deref(), Some("ls"));
        assert_eq!(
            command_signature("/usr/bin/git status").as_deref(),
            Some("git status")
        );
        assert_eq!(command_signature("   ").as_deref(), None);
    }

    #[test]
    fn command_signature_uses_first_pipeline_stage() {
        assert_eq!(
            command_signature("cargo build 2>&1 | tail -5").as_deref(),
            Some("cargo build")
        );
        assert_eq!(
            command_signature("git log && git status").as_deref(),
            Some("git log")
        );
    }

    #[test]
    fn file_extension_lowercases_and_defaults() {
        assert_eq!(file_extension("/src/main.RS"), "rs");
        assert_eq!(file_extension("Makefile"), "none");
        assert_eq!(file_extension("a/b/c.test.ts"), "ts");
    }

    #[test]
    fn scrub_secrets_redacts_token_bearing_tokens() {
        let scrubbed = scrub_secrets("curl -H Authorization=Bearer-abc --token=xyz123 site");
        assert!(scrubbed.contains("[REDACTED]"));
        assert!(!scrubbed.contains("xyz123"));
        assert!(scrubbed.contains("curl"));
        assert!(scrubbed.contains("site"));
    }

    #[test]
    fn record_observation_writes_bash_row() {
        with_isolated_claude_home("bash", |root| {
            let input = json!({
                "tool_name": "Bash",
                "session_id": "s1",
                "cwd": "/repo",
                "tool_input": { "command": "cargo test --workspace" },
            });
            assert!(record_observation(&input).expect("record"));
            let rows = iter_recent_rows(1).expect("iter");
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].signature, "cargo test");
            assert_eq!(rows[0].tool_name, "Bash");
            assert_eq!(rows[0].cwd, "/repo");
            let _ = root;
        });
    }

    #[test]
    fn record_observation_writes_edit_row_by_extension() {
        with_isolated_claude_home("edit", |_root| {
            let input = json!({
                "tool_name": "Edit",
                "tool_input": { "file_path": "/repo/src/lib.rs" },
            });
            assert!(record_observation(&input).expect("record"));
            let rows = iter_recent_rows(1).expect("iter");
            assert_eq!(rows[0].signature, "edit:rs");
        });
    }

    #[test]
    fn record_failure_observation_suffixes_signature() {
        with_isolated_claude_home("failure", |_root| {
            let input = json!({
                "tool_name": "Bash",
                "session_id": "s1",
                "cwd": "/repo",
                "tool_input": { "command": "cargo test --workspace" },
            });
            assert!(record_failure_observation(&input).expect("record"));
            let rows = iter_recent_rows(1).expect("iter");
            assert_eq!(rows.len(), 1);
            assert_eq!(
                rows[0].signature,
                format!("cargo test{FAILURE_SIGNATURE_SUFFIX}"),
                "a failed call must cluster under a distinct failure signature"
            );
        });
    }

    #[test]
    fn success_and_failure_of_same_command_cluster_separately() {
        with_isolated_claude_home("split", |_root| {
            let input = json!({
                "tool_name": "Bash",
                "session_id": "s1",
                "cwd": "/repo",
                "tool_input": { "command": "cargo test" },
            });
            assert!(record_observation(&input).expect("ok"));
            assert!(record_failure_observation(&input).expect("fail"));
            let rows = iter_recent_rows(1).expect("iter");
            let signatures: std::collections::BTreeSet<&str> =
                rows.iter().map(|r| r.signature.as_str()).collect();
            assert!(
                signatures.contains("cargo test"),
                "success signature present"
            );
            assert!(
                signatures.contains(&*format!("cargo test{FAILURE_SIGNATURE_SUFFIX}")),
                "failure signature present and distinct"
            );
        });
    }

    #[test]
    fn record_observation_skips_navigation_tools() {
        with_isolated_claude_home("skip", |root| {
            let input = json!({
                "tool_name": "Read",
                "tool_input": { "file_path": "/repo/src/lib.rs" },
            });
            assert!(!record_observation(&input).expect("record"));
            assert!(!root.join("state").join("observations").exists());
        });
    }

    #[test]
    fn iter_recent_rows_empty_when_directory_missing() {
        with_isolated_claude_home("cold", |_root| {
            assert!(iter_recent_rows(7).expect("iter").is_empty());
        });
    }

    #[test]
    fn prune_older_than_removes_only_stale_files() {
        with_isolated_claude_home("prune", |root| {
            let directory = root.join("state").join("observations");
            fs::create_dir_all(&directory).expect("mkdir");
            let today = chrono::Local::now().date_naive();
            let stale = today.checked_sub_days(chrono::Days::new(30)).unwrap();
            for date in [today, stale] {
                fs::write(
                    directory.join(format!("{}.jsonl", date.format("%Y-%m-%d"))),
                    "{}\n",
                )
                .expect("write");
            }
            let removed = prune_older_than(14).expect("prune");
            assert_eq!(removed, 1);
            assert!(directory
                .join(format!("{}.jsonl", today.format("%Y-%m-%d")))
                .exists());
        });
    }
}
