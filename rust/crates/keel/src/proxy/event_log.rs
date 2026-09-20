//! Purpose: Append token-saving measurement events for the gain/discover surfaces.
//! Caller: proxy::run after raw output is saved and compact output is rendered.
//! Dependencies: RunMeta, CompactResult, harness home resolution, and JSONL file storage.
//! Main Functions: record_compaction_event, rotate_event_log_if_needed.
//! Side Effects: Appends one JSON object per proxied command to the command compaction event log, rotates when size exceeds 5MB.

use crate::proxy::adapter::CompactResult;
use crate::proxy::injection_guard::InjectionFinding;
use crate::proxy::raw_store::RunMeta;
use crate::runtime::{
    display_path, resolve_claude_home, write_text, COMMAND_COMPACTION_EVENTS_FILE_NAME,
};
use crate::utility::file_lock::{is_lock_contention, LOCK_RETRY_INTERVAL, LOCK_TIMEOUT};
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};

/// Maximum event log size in bytes before rotation trims old entries (5 MB).
const MAX_EVENT_LOG_BYTES: u64 = 5 * 1024 * 1024;
/// Number of most-recent lines to keep after rotation.
const EVENT_LOG_KEEP_LINES: usize = 10_000;

const SECRET_VALUE_FLAGS: &[&str] = &[
    "--access-token",
    "--api-key",
    "--api_key",
    "--apikey",
    "--auth-token",
    "--authorization",
    "--bearer-token",
    "--client-secret",
    "--connection-string",
    "--credential",
    "--credentials",
    "--database-url",
    "--passphrase",
    "--passwd",
    "--password",
    "--private-key",
    "--redis-url",
    "--secret",
    "--secret-access-key",
    "--secret-key",
    "--token",
];

fn sensitive_assignment_key(key: &str) -> bool {
    let key = key
        .trim_matches(|character: char| character == '$' || character == '\'' || character == '"')
        .to_ascii_lowercase()
        .replace('-', "_");
    [
        "access_key",
        "api_key",
        "auth_token",
        "client_secret",
        "credential",
        "database_url",
        "password",
        "passwd",
        "private_key",
        "redis_url",
        "secret",
        "token",
    ]
    .iter()
    .any(|needle| key.contains(needle))
}

fn argument_has_secret_marker(argument: &str) -> bool {
    let trimmed = argument.trim_matches(|character: char| character == '\'' || character == '"');
    let lower = trimmed.to_ascii_lowercase();
    if SECRET_VALUE_FLAGS.iter().any(|flag| {
        lower == *flag
            || lower
                .strip_prefix(flag)
                .is_some_and(|suffix| suffix.starts_with('='))
    }) {
        return true;
    }
    if let Some((key, value)) = trimmed.split_once('=') {
        if !value.is_empty() && sensitive_assignment_key(key) {
            return true;
        }
    }
    crate::adapters::common::redact_possible_secret(trimmed) != trimmed
}

fn redacted_event_command(meta: &RunMeta) -> String {
    let command_was_redacted =
        crate::adapters::common::redact_possible_secret(&meta.command) != meta.command;
    let argv_has_secret = meta
        .args
        .iter()
        .any(|argument| argument_has_secret_marker(argument));
    let embedded_shell_secret = meta
        .command
        .split_whitespace()
        .any(argument_has_secret_marker);
    if !command_was_redacted && !argv_has_secret && !embedded_shell_secret {
        return meta.command.clone();
    }

    let program = meta.program.trim();
    let safe_program = if program.is_empty()
        || crate::adapters::common::redact_possible_secret(program) != program
    {
        "command"
    } else {
        program
    };
    format!("{safe_program} [redacted arguments]")
}

/// Reserve capacity before appending. The caller holds the sidecar file lock
/// across rotation and append; the lock inode is never replaced or removed.
fn rotate_event_log_if_needed(event_path: &std::path::Path, incoming_bytes: u64) {
    rotate_event_log(
        event_path,
        MAX_EVENT_LOG_BYTES - incoming_bytes,
        EVENT_LOG_KEEP_LINES,
    );
}

fn rotate_event_log(event_path: &std::path::Path, max_bytes: u64, keep_lines: usize) {
    // Versions before the unique atomic writer used this fixed sibling name.
    // It is Keel-owned and safe to reclaim after an interrupted rotation.
    let legacy_temp = event_path.with_extension("log.rotate-tmp");
    let _ = fs::remove_file(legacy_temp);

    let Ok(mut file) = fs::File::open(event_path) else {
        return;
    };
    let Ok(metadata) = file.metadata() else {
        return;
    };
    let size = metadata.len();
    if size <= max_bytes {
        return;
    }
    // Include the preceding byte so a record exactly at the tail boundary survives.
    let read_limit = max_bytes.saturating_add(1);
    let offset = size.saturating_sub(read_limit);
    if file.seek(SeekFrom::Start(offset)).is_err() {
        return;
    }
    let mut tail = Vec::new();
    if file.take(read_limit).read_to_end(&mut tail).is_err() {
        return;
    }
    let start = if offset == 0 {
        0
    } else {
        tail.iter()
            .position(|&byte| byte == b'\n')
            .map_or(tail.len(), |index| index + 1)
    };
    // Never retain a partial JSONL record, including a concurrently appended tail.
    let end = tail
        .iter()
        .rposition(|&byte| byte == b'\n')
        .map_or(0, |index| index + 1);
    let complete = &tail[start.min(end)..end];
    let mut retained = 0usize;
    for line in complete
        .split_inclusive(|&byte| byte == b'\n')
        .rev()
        .take(keep_lines)
    {
        if (retained as u64).saturating_add(line.len() as u64) > max_bytes {
            break;
        }
        retained += line.len();
    }
    let Ok(trimmed) = std::str::from_utf8(&complete[complete.len() - retained..]) else {
        return;
    };
    // The shared writer uses collision-free sibling temps, reclaims temps
    // whose owner died, and handles replace semantics on Windows.
    let _ = write_text(event_path, trimmed);
}

pub fn record_compaction_event(
    meta: &RunMeta,
    compact: &CompactResult,
    findings: &[InjectionFinding],
) {
    let Ok(claude_home) = resolve_claude_home("") else {
        return;
    };
    if fs::create_dir_all(&claude_home).is_err() {
        return;
    }
    let event_path = claude_home.join(COMMAND_COMPACTION_EVENTS_FILE_NAME);
    let injection_patterns: Vec<&str> = findings.iter().map(|f| f.pattern).collect();
    let redacted_command = redacted_event_command(meta);
    let (tokens_after, tokens_saved, savings_pct) = if meta.compacted {
        (
            meta.estimated_tokens_after,
            meta.estimated_tokens_saved.max(0) as usize,
            meta.savings_pct,
        )
    } else {
        (meta.estimated_tokens_before, 0, 0.0)
    };
    let payload = serde_json::json!({
        "timestamp": meta.started_at.to_string(),
        "command": &redacted_command,
        "exit_code": meta.exit_code,
        "exitCode": meta.exit_code,
        "compacted": meta.compacted,
        "adapter_name": &meta.adapter_name,
        "adapterName": &meta.adapter_name,
        "tokens_before": meta.estimated_tokens_before,
        "reducer": &compact.adapter_name,
        "tokens_after": tokens_after,
        "commandFamily": &compact.adapter_name,
        "tokens_saved": tokens_saved,
        "exact_tokens_before": meta.estimated_tokens_before,
        "exact_tokens_after": tokens_after,
        "exact_tokens_saved": tokens_saved,
        "tokenizer": "o200k_base",
        "token_counting": "exact",
        "summary": &compact.summary,
        "savings_pct": savings_pct,
        "stdoutBytes": meta.stdout_bytes,
        "raw_path": display_path(&meta.raw_path),
        "stderrBytes": meta.stderr_bytes,
        "rawBytes": meta.stdout_bytes + meta.stderr_bytes,
        "compact_path": display_path(&meta.compact_path),
        "renderedBytes": meta.compact_stdout_bytes + meta.compact_stderr_bytes,
        "savedBytes": (meta.stdout_bytes + meta.stderr_bytes)
            .saturating_sub(meta.compact_stdout_bytes + meta.compact_stderr_bytes),
        "tokensBefore": meta.estimated_tokens_before,
        "tokensAfter": tokens_after,
        "tokensSaved": tokens_saved,
        "savingsPct": savings_pct,
        "rawPath": display_path(&meta.raw_path),
        "rawOutputPath": display_path(&meta.raw_path),
        "compactPath": display_path(&meta.compact_path),
        "agent": &meta.agent,
        "workspace": display_path(&meta.workspace),
        "injection_neutralized": !findings.is_empty(),
        "injection_findings": findings.len(),
        "injection_patterns": injection_patterns,
    });
    let Ok(rendered) = serde_json::to_string(&payload) else {
        return;
    };
    // why: telemetry failure must not change the wrapped command's exit status.
    let _ = record_compaction_event_impl(&event_path, &rendered);
}

/// Serialize rotation/append/reset across processes. Bounded wait: a stalled
/// or suspended lock holder must never delay the wrapped command indefinitely,
/// so contention returns TimedOut and the caller fails open (telemetry only).
fn lock_event_log(event_path: &std::path::Path) -> std::io::Result<fs::File> {
    let mut lock_path = event_path.as_os_str().to_owned();
    lock_path.push(".lock");
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(std::path::Path::new(&lock_path))?;
    let deadline = std::time::Instant::now() + LOCK_TIMEOUT;
    loop {
        // Windows reports lock conflicts as OS error 33 (Uncategorized), not
        // WouldBlock; treat both as contention and retry until the deadline.
        match fs2::FileExt::try_lock_exclusive(&file) {
            Ok(()) => return Ok(file),
            Err(error) if is_lock_contention(&error) => {
                if std::time::Instant::now() >= deadline {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "event log lock remained held",
                    ));
                }
                std::thread::sleep(LOCK_RETRY_INTERVAL);
            }
            Err(error) => return Err(error),
        }
    }
}

/// Open the event log for append, retrying transient Windows sharing
/// violations from a concurrent rotation rename. Bounded so a persistent
/// conflict surfaces as an error instead of hanging the wrapped command.
fn open_event_log_for_append(event_path: &std::path::Path) -> std::io::Result<fs::File> {
    let deadline = std::time::Instant::now() + LOCK_TIMEOUT;
    loop {
        match fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(event_path)
        {
            Ok(file) => return Ok(file),
            Err(error) if is_lock_contention(&error) => {
                if std::time::Instant::now() >= deadline {
                    return Err(error);
                }
                std::thread::sleep(LOCK_RETRY_INTERVAL);
            }
            Err(error) => return Err(error),
        }
    }
}

pub(crate) fn reset_event_log(event_path: &std::path::Path) -> std::io::Result<()> {
    let _lock = lock_event_log(event_path)?;
    fs::remove_file(event_path)
}

fn record_compaction_event_impl(
    event_path: &std::path::Path,
    rendered: &str,
) -> std::io::Result<()> {
    // Include the JSONL newline; reject before rotation can discard existing events.
    if rendered.len() as u64 >= MAX_EVENT_LOG_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "compaction event exceeds log byte limit",
        ));
    }
    let _lock = lock_event_log(event_path)?;
    let incoming_bytes = rendered.len() as u64 + 1;
    rotate_event_log_if_needed(event_path, incoming_bytes);
    let mut file = open_event_log_for_append(event_path)?;
    if file.metadata()?.len() > MAX_EVENT_LOG_BYTES - incoming_bytes {
        return Err(std::io::Error::other(
            "compaction log rotation did not free enough space",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = file.metadata()?.permissions();
        perms.set_mode(0o600);
        fs::set_permissions(event_path, perms)?;
    }
    writeln!(file, "{rendered}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "subprocess fixture"]
    fn event_log_process_fixture() {
        let path = std::path::PathBuf::from(
            std::env::var_os("KEEL_EVENT_LOG_FIXTURE").expect("fixture path"),
        );
        if std::env::var_os("KEEL_EVENT_LOCK_HOLDER").is_some() {
            let _lock = lock_event_log(&path).expect("acquire child lock");
            fs::write(path.with_extension("ready"), "ready").expect("publish readiness");
            std::thread::sleep(std::time::Duration::from_secs(60));
            return;
        }
        for sequence in 0..16 {
            let event = serde_json::json!({"pid": std::process::id(), "sequence": sequence, "payload": "x".repeat(32768)});
            record_compaction_event_impl(&path, &event.to_string()).expect("append child event");
            assert!(fs::metadata(&path).unwrap().len() <= MAX_EVENT_LOG_BYTES);
        }
    }

    fn event_log_child(path: &std::path::Path) -> std::process::Command {
        let mut command =
            std::process::Command::new(std::env::current_exe().expect("test executable"));
        command
            .args([
                "--exact",
                "proxy::event_log::tests::event_log_process_fixture",
                "--ignored",
                "--nocapture",
            ])
            .env("KEEL_EVENT_LOG_FIXTURE", path)
            .env_remove("KEEL_EVENT_LOCK_HOLDER")
            .stdout(std::process::Stdio::null());
        command
    }

    #[test]
    fn concurrent_process_appends_preserve_events_after_rotation() {
        let (_root, path) = rotation_fixture("keel-event-processes");
        let old = format!("\"{}\"\n", "x".repeat(MAX_EVENT_LOG_BYTES as usize - 3));
        fs::write(&path, old).unwrap();
        let mut children: Vec<_> = (0..4)
            .map(|_| event_log_child(&path).spawn().unwrap())
            .collect();
        for child in &mut children {
            assert!(child.wait().unwrap().success());
        }
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.len() as u64 <= MAX_EVENT_LOG_BYTES);
        let identities: std::collections::HashSet<_> = text
            .lines()
            .map(|line| {
                let row: serde_json::Value = serde_json::from_str(line).unwrap();
                (
                    row["pid"].as_u64().unwrap(),
                    row["sequence"].as_u64().unwrap(),
                )
            })
            .collect();
        assert_eq!(identities.len(), 64);
        assert_eq!(text.lines().count(), 64);
    }

    #[test]
    fn killed_process_releases_event_log_lock() {
        let (_root, path) = rotation_fixture("keel-event-killed-lock");
        let mut child = event_log_child(&path)
            .env("KEEL_EVENT_LOCK_HOLDER", "1")
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !path.with_extension("ready").exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let ready = path.with_extension("ready").exists();
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(ready, "child did not acquire the lock");
        record_compaction_event_impl(&path, "{}").expect("append after terminated owner");
        reset_event_log(&path).expect("reset after terminated owner");
        record_compaction_event_impl(&path, "{\"after_reset\":true}").unwrap();
        assert_eq!(
            fs::read_to_string(path).unwrap(),
            "{\"after_reset\":true}\n"
        );
    }

    #[test]
    fn event_log_lock_wait_is_bounded() {
        let (_root, path) = rotation_fixture("keel-event-lock-timeout");
        let holder = lock_event_log(&path).expect("acquire in-process lock");
        let started = std::time::Instant::now();
        let error = record_compaction_event_impl(&path, "{}").unwrap_err();
        let elapsed = started.elapsed();
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
        assert!(elapsed >= LOCK_TIMEOUT, "elapsed {elapsed:?}");
        assert!(elapsed < LOCK_TIMEOUT + std::time::Duration::from_secs(2));
        drop(holder);
        record_compaction_event_impl(&path, "{}").expect("append after release");
        assert_eq!(fs::read_to_string(&path).unwrap(), "{}\n");
    }

    fn sample_meta(command: &str, program: &str, args: &[&str]) -> RunMeta {
        RunMeta {
            raw_id: "20260911-000000-deadbeef".to_string(),
            command: command.to_string(),
            program: program.to_string(),
            args: args.iter().map(|arg| (*arg).to_string()).collect(),
            cwd: std::path::PathBuf::from("."),
            started_at: 1,
            duration_ms: 1,
            exit_code: 0,
            adapter_name: "generic".to_string(),
            raw_path: std::path::PathBuf::from("raw"),
            compact_path: std::path::PathBuf::from("compact"),
            agent: "test".to_string(),
            workspace: std::path::PathBuf::from("."),
            stdout_bytes: 0,
            stderr_bytes: 0,
            compact_stdout_bytes: 0,
            compact_stderr_bytes: 0,
            estimated_tokens_before: 0,
            estimated_tokens_after: 0,
            estimated_tokens_saved: 0,
            savings_pct: 0.0,
            compacted: false,
        }
    }

    fn rotation_fixture(dir_name: &str) -> (crate::test_support::TestTempDir, std::path::PathBuf) {
        let root = crate::test_support::unique_temp_dir(dir_name);
        let event_path = root.as_path().join("events.jsonl");
        // why: TestTempDir removes the tree on drop, including during unwinding
        (root, event_path)
    }

    #[test]
    fn oversized_compaction_event_preserves_existing_log() {
        let (_root, event_path) = rotation_fixture("keel-oversized-event");
        let existing = format!("{}\n", "x".repeat(MAX_EVENT_LOG_BYTES as usize));
        fs::write(&event_path, &existing).expect("seed log requiring rotation");
        let oversized = "x".repeat(MAX_EVENT_LOG_BYTES as usize);

        assert_eq!(
            record_compaction_event_impl(&event_path, &oversized)
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::InvalidInput
        );

        assert_eq!(
            fs::read_to_string(&event_path).expect("read preserved log"),
            existing
        );
        fs::remove_file(&event_path).expect("remove fixture log");
        assert!(record_compaction_event_impl(&event_path, &oversized).is_err());
        assert!(!event_path.exists(), "rejected event must not create a log");
        record_compaction_event_impl(&event_path, "{}").expect("append valid event");
        assert_eq!(
            fs::read_to_string(&event_path).expect("read accepted event"),
            "{}\n"
        );
    }

    #[test]
    fn compaction_event_append_reserves_space_within_cap() {
        let (_root, event_path) = rotation_fixture("keel-event-append-cap");
        let record = format!("\"{}\"", "x".repeat(MAX_EVENT_LOG_BYTES as usize / 2));
        for _ in 0..3 {
            record_compaction_event_impl(&event_path, &record).expect("append bounded event");
            assert!(fs::metadata(&event_path).expect("log metadata").len() <= MAX_EVENT_LOG_BYTES);
        }
        assert_eq!(
            fs::read_to_string(event_path).expect("read retained event"),
            format!("{record}\n")
        );
    }

    #[test]
    fn rotation_replaces_existing_log_and_keeps_newest_lines() {
        let (_root, event_path) = rotation_fixture("keel-event-log-rotate");
        fs::write(&event_path, "one\ntwo\nthree\n").unwrap();

        rotate_event_log(&event_path, 10, 2);

        assert_eq!(fs::read_to_string(event_path).unwrap(), "two\nthree\n");
    }

    #[test]
    fn rotation_bounds_bytes_and_discards_oversized_records() {
        let (_root, event_path) = rotation_fixture("keel-event-log-byte-bound");
        fs::write(&event_path, format!("{}\nnewest\n", "x".repeat(4096))).unwrap();

        rotate_event_log(&event_path, 32, 10_000);

        assert_eq!(fs::read_to_string(&event_path).unwrap(), "newest\n");
    }

    #[test]
    fn rotation_preserves_utf8_record_exactly_at_byte_limit() {
        let (_root, event_path) = rotation_fixture("keel-event-log-utf8-boundary");
        fs::write(&event_path, "old\néé\n").unwrap();

        rotate_event_log(&event_path, 5, 10);

        assert_eq!(fs::read_to_string(&event_path).unwrap(), "éé\n");
    }

    #[test]
    fn rotation_discards_unterminated_record_without_leaving_fragment() {
        let (_root, event_path) = rotation_fixture("keel-event-log-partial-tail");
        fs::write(&event_path, "old-old-old\nkeep\npartial").unwrap();

        rotate_event_log(&event_path, 16, 10);

        assert_eq!(fs::read_to_string(&event_path).unwrap(), "keep\n");
        fs::write(&event_path, "oversized-unterminated-record").unwrap();
        rotate_event_log(&event_path, 4, 10);
        assert_eq!(fs::read(&event_path).unwrap(), b"");
    }

    #[test]
    fn rotation_reclaims_legacy_temp_even_when_log_is_small() {
        let root = crate::test_support::unique_temp_dir("keel-event-log-stale");
        let event_path = root.join("events.jsonl");
        let legacy_temp = event_path.with_extension("log.rotate-tmp");
        fs::write(&event_path, "one\n").unwrap();
        fs::write(&legacy_temp, "stale").unwrap();

        rotate_event_log(&event_path, 1024, 2);

        assert!(!legacy_temp.exists());
        assert_eq!(fs::read_to_string(event_path).unwrap(), "one\n");
    }

    #[test]
    fn event_command_redacts_separate_secret_flag_values() {
        let meta = sample_meta(
            // The display command can omit or normalize argv, so the decision
            // must inspect the authoritative argument vector as well.
            "curl https://example.invalid",
            "curl",
            &["--password", "hunter2", "https://example.invalid"],
        );

        let redacted = redacted_event_command(&meta);
        assert_eq!(redacted, "curl [redacted arguments]");
        assert!(!redacted.contains("hunter2"));
    }

    #[test]
    fn event_command_redacts_secret_assignments_in_argv() {
        for argument in [
            "--api-key=sk-example",
            "DATABASE_URL=postgres://user:secret@example.invalid/db",
            "$AUTH_TOKEN=opaque-value",
        ] {
            let meta = sample_meta("deploy", "deploy", &[argument]);
            assert_eq!(
                redacted_event_command(&meta),
                "deploy [redacted arguments]",
                "expected argv assignment to be treated as sensitive: {argument}"
            );
        }
    }

    #[test]
    fn event_command_preserves_non_secret_command_shape() {
        let meta = sample_meta(
            "cargo test --workspace --tokenize",
            "cargo",
            &["test", "--workspace", "--tokenize"],
        );
        assert_eq!(
            redacted_event_command(&meta),
            "cargo test --workspace --tokenize"
        );
    }
}
