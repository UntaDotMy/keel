//! Hook lifecycle session_end responsibility split.

use super::*;

pub(super) fn prune_raw_output_store(standard_error: &mut dyn Write) {
    let retention_days = user_config_or_env_u64(
        PLUGIN_MEMORY_RETENTION_DAYS,
        "CLAUDE_SKILLS_RAW_RETENTION_DAYS",
        RAW_OUTPUT_DEFAULT_RETENTION_DAYS,
    );
    if retention_days == 0 {
        return;
    }
    if let Err(error) = RawStore::new().prune_older_than(retention_days) {
        let _ = writeln!(standard_error, "keel raw-output prune failed: {error}");
    }
}

/// Drop tool-timings JSONL rows older than the configured retention.
///
/// Mirrors `prune_raw_output_store` in shape: SessionEnd-only, env var
/// override, swallow errors so a telemetry housekeeping failure cannot
/// fail the hook. The store is per-day JSONL files under
/// `<claude_home>/state/tool-timings/`; the prune helper in
/// `tool_timings::prune_older_than` parses the date out of each filename
/// and removes files older than the cutoff.
///
/// `pub(crate)` so the env-var override and `retention=0` disable paths
/// are exercisable from the `tool_timings` module's existing isolated
/// `with_isolated_claude_home` test harness without duplicating the
/// `CLAUDE_TARGET_OVERRIDE` plumbing here.
pub(crate) fn prune_tool_timings_store(standard_error: &mut dyn Write) {
    let retention_days = user_config_or_env_u64(
        PLUGIN_MEMORY_RETENTION_DAYS,
        "CLAUDE_SKILLS_TIMINGS_RETENTION_DAYS",
        TIMINGS_DEFAULT_RETENTION_DAYS,
    );
    if retention_days == 0 {
        return;
    }
    if let Err(error) = tool_timings::prune_older_than(retention_days) {
        let _ = writeln!(standard_error, "keel tool-timings prune failed: {error}");
    }
}

/// Drop behavioral observation JSONL rows older than the configured retention.
/// SessionEnd-only, env-var override, errors swallowed — same housekeeping
/// contract as the timings and raw-output prunes.
pub(super) fn prune_observations_store(standard_error: &mut dyn Write) {
    let retention_days = user_config_or_env_u64(
        PLUGIN_MEMORY_RETENTION_DAYS,
        "CLAUDE_SKILLS_OBSERVATION_RETENTION_DAYS",
        OBSERVATION_DEFAULT_RETENTION_DAYS,
    );
    if retention_days == 0 {
        return;
    }
    if let Err(error) = observation::prune_older_than(retention_days) {
        let _ = writeln!(standard_error, "keel observation prune failed: {error}");
    }
}

/// SessionEnd housekeeping for the per-session gate/marker state under
/// `<claude_home>/state/`.
///
/// why: the one-file-per-session markers (iron-law satisfaction, per-gate block
/// counters, review-gate) were never pruned, so they grew unbounded
/// and a stale shared "default" iron-law marker could satisfy the gate across
/// id-less sessions forever. Bounding them by mtime caps both the growth and that
/// cross-session leak. Errors swallowed like the other prunes.
pub(super) fn prune_state_marker_stores(standard_error: &mut dyn Write) {
    let retention_days = user_config_or_env_u64(
        PLUGIN_MEMORY_RETENTION_DAYS,
        "CLAUDE_SKILLS_STATE_RETENTION_DAYS",
        TIMINGS_DEFAULT_RETENTION_DAYS,
    );
    if retention_days == 0 {
        return;
    }
    let Ok(claude_home) = resolve_claude_home("") else {
        return;
    };
    let cutoff_ms = now_ms().saturating_sub(retention_days.saturating_mul(86_400_000));
    let state = claude_home.join("state");
    let Ok(entries) = fs::read_dir(&state) else {
        return;
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        let is_marker_dir = matches!(
            name.as_str(),
            IRON_LAW_SATISFIED_DIR | IRON_LAW_LEGACY_GATE_DIR | "review-gate"
        ) || name.ends_with("-gate-blocks");
        if !is_marker_dir {
            continue;
        }
        if let Err(error) = prune_dir_files_older_than(&dir, cutoff_ms) {
            let _ = writeln!(
                standard_error,
                "keel state-marker prune failed for {}: {error}",
                dir.display()
            );
        }
    }
}

/// Remove regular files in `dir` whose mtime is older than `cutoff_ms`.
pub(super) fn prune_dir_files_older_than(dir: &Path, cutoff_ms: u64) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let stale = entry
            .metadata()
            .ok()
            .and_then(|meta| meta.modified().ok())
            .and_then(|mtime| mtime.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|dur| (dur.as_millis() as u64) < cutoff_ms)
            .unwrap_or(false);
        if stale {
            let _ = fs::remove_file(&path);
        }
    }
    Ok(())
}

/// Run the autonomous learning cycle at session end: distill the session's
/// observations into instincts and evolve trusted clusters into generated
/// skills. Fully automatic — no slash command. Set
/// `CLAUDE_SKILLS_LEARNING=off` to disable. Errors are swallowed so a learning
/// failure can never fail the SessionEnd hook.
pub(crate) fn run_session_end_learning(standard_error: &mut dyn Write) {
    if std::env::var("CLAUDE_SKILLS_LEARNING")
        .map(|value| value.trim().eq_ignore_ascii_case("off"))
        .unwrap_or(false)
    {
        return;
    }
    let claude_home = match resolve_claude_home("") {
        Ok(path) => path,
        Err(_) => return,
    };
    // synthesize: true so SessionStart can surface refinement briefs for any
    // template-state skills (growth without an LLM inside the binary).
    let options = learning::CycleOptions {
        synthesize: true,
        ..learning::CycleOptions::default()
    };
    let report = learning::run_learning_cycle(&claude_home, &options, standard_error);
    if report.skills_generated > 0
        || report.agents_generated > 0
        || report.instincts_recorded > 0
        || report.skills_rolled_back > 0
    {
        let _ = writeln!(
            standard_error,
            "keel learn: recorded {} instinct(s), generated {} skill(s), {} agent(s), rolled back {}",
            report.instincts_recorded,
            report.skills_generated,
            report.agents_generated,
            report.skills_rolled_back
        );
    }
}

/// Lifecycle events that should auto-refresh the workspace SYSTEM_MAP.
///
/// The agent has historically forgotten to refresh the map by hand, so we
/// fire it at the three natural transition points instead of trusting the
/// model to remember:
///
/// - `SessionStart` — first turn, the map needs to reflect today's layout
///   before the model reasons about the repo.
/// - `PreCompact` — context is about to be compacted; the post-compact
///   window resumes against a fresh map written *now*, so the layout the
///   model recovers matches reality even if files moved earlier in the
///   conversation.
/// - `SessionEnd` — the next session starts from the freshest possible map
///   without paying the cost on its first prompt.
///
/// Kept as a small slug-named function so the trigger set is testable in
/// isolation and the call site at `run_hook_lifecycle` reads as a single
/// predicate instead of a chain of equality checks.
pub(super) fn run_hook_session_end(
    standard_input: &mut dyn Read,
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    maybe_capture_session_summary(standard_input, standard_error);
    run_hook_lifecycle("session-end", standard_output, standard_error)
}

/// Bridge-compatible SessionEnd: runs summary capture then learning WITHOUT
/// stdin parsing (the bridge passes the session id directly). Calls
/// `maybe_capture_session_summary_with_id` so the capture can scope itself
/// without reading the hook JSON payload.
pub(crate) fn run_bridge_session_end(
    claude_home: &std::path::Path,
    session_id: &str,
    standard_error: &mut dyn Write,
) {
    maybe_capture_session_summary_with_id(claude_home, session_id, standard_error);
    run_session_end_learning(standard_error);
}

/// Bridge-compatible variant of [`maybe_capture_session_summary`]: takes the
/// session id directly instead of reading it from the hook stdin payload.
pub(super) fn maybe_capture_session_summary_with_id(
    _claude_home: &std::path::Path,
    session_id: &str,
    standard_error: &mut dyn Write,
) {
    if std::env::var(SESSION_CAPTURE_ENV_VAR)
        .map(|value| value.trim().eq_ignore_ascii_case("off"))
        .unwrap_or(false)
    {
        return;
    }
    if session_id.trim().is_empty() {
        return;
    }

    let Some(summary) = build_session_summary(session_id) else {
        return;
    };

    let mut capture_stderr = Vec::new();
    let code = crate::utility::memory_families::run_memory_family_command(
        "memory",
        "research-cache",
        &[
            "record".to_string(),
            "--question".to_string(),
            summary.question,
            "--answer".to_string(),
            summary.answer,
            "--source".to_string(),
            format!("auto-capture session {session_id}"),
        ],
        &mut std::io::sink(),
        &mut capture_stderr,
    );
    if code != 0 {
        let _ = writeln!(
            standard_error,
            "keel: session auto-capture skipped ({})",
            String::from_utf8_lossy(&capture_stderr).trim()
        );
    }
}

/// Auto-capture a one-line work summary to memory at SessionEnd.
///
/// This is the "after do, save to memory" half of the contract: the next
/// session starts informed without the model having to remember to write a
/// note. It reuses the behavioral observation log (the same edit/command
/// signatures the learning loop reads) to summarize what this session actually
/// did, then writes it through `memory research-cache record` — the path that
/// now syncs the recall index (s4), so the summary is immediately recallable.
///
/// Silent on pure no-op sessions (no edits, no failures, fewer than three
/// commands): research/question turns stay out of the memory store.
///
/// Best-effort by contract: every failure path returns without writing and
/// without changing the caller's exit code. The SessionEnd prunes and learning
/// cycle are the load-bearing work; this capture is additive.
pub(super) fn maybe_capture_session_summary(
    standard_input: &mut dyn Read,
    standard_error: &mut dyn Write,
) {
    if std::env::var(SESSION_CAPTURE_ENV_VAR)
        .map(|value| value.trim().eq_ignore_ascii_case("off"))
        .unwrap_or(false)
    {
        return;
    }

    // The session id arrives on stdin (the harness writes the hook payload then
    let stdin_payload = read_json_stdin_fail_open(standard_input);
    let session_id = stdin_payload
        .as_ref()
        .and_then(|payload| payload.get("session_id"))
        .and_then(JsonDocument::as_str)
        .unwrap_or_default();
    if session_id.trim().is_empty() {
        return;
    }

    let Some(summary) = build_session_summary(session_id) else {
        return; // No edit-class work this session — stay silent.
    };

    // Write through the family command so the capture inherits the s4 index
    let mut capture_stderr = Vec::new();
    let code = crate::utility::memory_families::run_memory_family_command(
        "memory",
        "research-cache",
        &[
            "record".to_string(),
            "--question".to_string(),
            summary.question,
            "--answer".to_string(),
            summary.answer,
            "--source".to_string(),
            format!("auto-capture session {session_id}"),
        ],
        &mut std::io::sink(),
        &mut capture_stderr,
    );
    if code != 0 {
        let _ = writeln!(
            standard_error,
            "keel: session auto-capture skipped ({})",
            String::from_utf8_lossy(&capture_stderr).trim()
        );
    }
}

/// A session work summary framed as a recallable question/answer pair.
pub(super) struct SessionSummary {
    question: String,
    answer: String,
}

/// Build a work summary for `session_id` from this session's behavioral
/// observations, or `None` when the session did no durable work.
///
/// Captures edits, successful commands, and failed command outcomes so recall
/// and the learning loop share the same episodic signal ("what happened today").
pub(super) fn build_session_summary(session_id: &str) -> Option<SessionSummary> {
    let rows = crate::runner::observation::iter_recent_rows(1).ok()?;
    let mut edit_count = 0usize;
    let mut command_count = 0usize;
    let mut failed_count = 0usize;
    let mut extensions: Vec<String> = Vec::new();
    let mut commands: Vec<String> = Vec::new();
    let mut failures: Vec<String> = Vec::new();
    let mut cwd = String::new();
    for row in rows {
        if row.session_id != session_id {
            continue;
        }
        if cwd.is_empty() && !row.cwd.is_empty() {
            cwd = row.cwd.clone();
        }
        if let Some(extension) = row.signature.strip_prefix("edit:") {
            edit_count += 1;
            let extension = extension.to_string();
            if !extensions.contains(&extension) {
                extensions.push(extension);
            }
        } else if row
            .signature
            .ends_with(crate::runner::observation::FAILURE_SIGNATURE_SUFFIX)
        {
            failed_count += 1;
            if !failures.contains(&row.signature) {
                failures.push(row.signature.clone());
            }
        } else {
            command_count += 1;
            if !commands.contains(&row.signature) {
                commands.push(row.signature.clone());
            }
        }
    }

    // Capture when the session edited code, recorded failures, or ran a
    // meaningful number of commands (tests/builds). Pure no-op sessions stay silent.
    if edit_count == 0 && command_count < 3 && failed_count == 0 {
        return None;
    }

    // Final path component only: low-cardinality anchor, no full-path leakage.
    let workspace = if cwd.is_empty() {
        "unknown workspace".to_string()
    } else {
        std::path::Path::new(&cwd)
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "unknown workspace".to_string())
    };
    let question = format!("What was done in {workspace} on {}?", today_date_string());
    let mut parts: Vec<String> = Vec::new();
    if edit_count > 0 {
        parts.push(format!(
            "Edited {edit_count} file(s) ({}).",
            if extensions.is_empty() {
                "no recorded extension".to_string()
            } else {
                extensions.join(", ")
            }
        ));
    }
    if command_count > 0 {
        parts.push(format!(
            "Ran {command_count} command(s): {}.",
            commands.join(", ")
        ));
    }
    if failed_count > 0 {
        parts.push(format!(
            "Recorded {failed_count} failed outcome(s): {}.",
            failures.join(", ")
        ));
    }
    let answer = parts.join(" ");
    Some(SessionSummary { question, answer })
}

/// Local calendar date as `YYYY-MM-DD`, matching the observation-log naming so a
/// captured summary's date lines up with the rows it was built from.
pub(super) fn today_date_string() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

pub(super) fn refresh_memory_scope_for_current_directory(
    standard_error: &mut dyn Write,
) -> Option<PathBuf> {
    let workspace_root = std::env::current_dir().ok()?;

    let mut stdout = Vec::new();

    let mut stderr = Vec::new();

    let arguments = vec![
        "scope".to_string(),
        "resolve".to_string(),
        "--workspace-root".to_string(),
        display_path(&workspace_root),
        "--create-missing".to_string(),
        "--refresh-system-map".to_string(),
        "--format".to_string(),
        "compact".to_string(),
    ];

    let code = utility::run_memory_command("memory", &arguments, &mut stdout, &mut stderr);

    if code != 0 {
        let _ = writeln!(
            standard_error,
            "keel lifecycle memory refresh failed: {}",
            String::from_utf8_lossy(&stderr).trim()
        );

        return None;
    }

    memory_system_map_path_for_workspace(&workspace_root)
}

pub(super) fn memory_scope_summary() -> String {
    match std::env::current_dir()

        .ok()

        .and_then(|workspace_root| memory_system_map_path_for_workspace(&workspace_root))

    {

        Some(path) => format!(

            "Workspace memory system map: {}. Read it before making repo-structure claims; refresh happens automatically at session start, pre-compact, and session end.",

            display_path(&path)

        ),

        None => "Workspace memory system map: unavailable; create it with keel memory scope resolve --create-missing --refresh-system-map before repo-structure claims.".to_string(),

    }
}

/// Byte budget for the whole pushed workspace digest. The SessionStart context
/// must stay under the ~9KB truncation ceiling (see the cap test); the compact
/// bootstrap + memory pointer already spend most of that, so the digest gets a
/// deliberately small slice. Sections are capped individually below this so the
/// digest degrades gracefully (drop the tail) rather than blowing the ceiling.
pub(super) const WORKSPACE_DIGEST_MAX_BYTES: usize = 1700;
/// Cap for learned-instinct lines appended at SessionStart. Without this, a
/// busy project's instinct store can grow SessionStart past the host
/// additionalContext truncation ceiling even when the compact bootstrap is fine.
pub(super) const INSTINCT_DIGEST_MAX_BYTES: usize = 400;
/// Cap for the learned-skill synthesis nudge appended at SessionStart.
pub(super) const SYNTHESIS_NUDGE_MAX_BYTES: usize = 200;
/// Per-section caps inside the digest. The system-map head is the most valuable
/// (it answers "what is this repo" without a tool call), so it gets the largest
/// share; the brief and memory note are one-liners pointing the model at detail
/// it can pull with `recall`/`brief_get` if needed.
pub(super) const DIGEST_MAP_HEAD_MAX_BYTES: usize = 1000;

pub(super) const DIGEST_BRIEF_MAX_BYTES: usize = 400;

pub(super) const DIGEST_MEMORY_MAX_BYTES: usize = 250;

/// Build a bounded digest of ACTUAL workspace memory content to PUSH into
/// context at session start and post-compact — so the agent does not have to
/// blind-search (call `system_map`/`recall`) before it knows the basics.
///
/// Three sections, each independently capped and any of which may be empty:
///   1. The head of the workspace SYSTEM_MAP.md (the structural map), so
///      "what is this project / where does X live" is answered up front.
///   2. The newest working brief for this workspace (request + first acceptance
///      criterion), so a resumed or fresh session sees the active intent.
///   3. The most recent durable memory note (research-cache record, which now
///      includes the s5 SessionEnd auto-capture), so "what was last done here"
///      is visible without a recall call.
///
/// Returns an empty string when nothing is available (first run in a fresh
/// workspace), so the caller appends nothing. The whole digest is truncated to
/// [`WORKSPACE_DIGEST_MAX_BYTES`] on a line boundary as a final guard. This is
/// the PUSH half of the contract; the model can still PULL the full artifacts
/// with `system_map`, `recall`, and `brief_get` when it needs more than the head.
/// Whether a brief's ISO-8601 `created_at` is within the digest staleness window.
/// A brief with an unparseable timestamp is treated as fresh (fail-open: never
/// hide a brief just because its date could not be parsed).
pub(super) fn brief_is_fresh(created_at: &str) -> bool {
    const DIGEST_BRIEF_STALE_DAYS: i64 = 30;
    match chrono::DateTime::parse_from_rfc3339(created_at.trim()) {
        Ok(created) => {
            chrono::Utc::now()
                .signed_duration_since(created.with_timezone(&chrono::Utc))
                .num_days()
                <= DIGEST_BRIEF_STALE_DAYS
        }
        Err(_) => true,
    }
}

pub(super) fn workspace_memory_digest() -> String {
    let Ok(claude_home) = resolve_claude_home("") else {
        return String::new();
    };
    let Ok(workspace_root) = std::env::current_dir() else {
        return String::new();
    };
    let workspace_display = display_path(&workspace_root);

    let mut sections: Vec<String> = Vec::new();

    // 1. System-map head.
    if let Some(map_path) = memory_system_map_path_for_workspace(&workspace_root) {
        if let Ok(map_body) = fs::read_to_string(&map_path) {
            let head = truncate_on_line_boundary(map_body.trim(), DIGEST_MAP_HEAD_MAX_BYTES);
            if !head.trim().is_empty() {
                sections.push(format!(
                    "## Workspace map (head; full map at {})\n{head}",
                    display_path(&map_path)
                ));
            }
        }
    }

    // Use workspace-matching briefs; legacy briefs are fallback-only.
    if let Ok(briefs) = crate::utility::working_brief::list_briefs(&claude_home) {
        let newest = briefs
            .iter()
            .rev()
            .find(|brief| brief.workspace == workspace_display)
            .or_else(|| {
                briefs
                    .iter()
                    .rev()
                    .find(|brief| brief.workspace.trim().is_empty())
            })
            .filter(|brief| brief_is_fresh(&brief.created_at));
        if let Some(brief) = newest {
            let mut line = format!("## Active working brief ({})\n{}", brief.id, brief.request);
            if let Some(first_criterion) = brief.acceptance_criteria.first() {
                line.push_str(&format!("\nAcceptance: {first_criterion}"));
            }
            sections.push(truncate_on_line_boundary(&line, DIGEST_BRIEF_MAX_BYTES));
        }
    }

    // 3. (work-graph digest removed with `keel work`; Anvil pieces/gates are the
    // single delivery state and are surfaced via `keel anvil` + recall.)

    // 4. Most recent durable memory note (includes s5 SessionEnd auto-capture).
    let store =
        crate::utility::record_store::RecordStore::new(&claude_home, "memory/research-cache");
    if let Ok(mut records) = store.list_records() {
        // Ids are time-ordered hex (rc-<millis:x>), so the last id is newest.
        records.sort_by(|left, right| left.0.cmp(&right.0));
        if let Some((_, fields)) = records.last() {
            let lookup = |key: &str| {
                fields
                    .iter()
                    .find(|(field_key, _)| field_key == key)
                    .map(|(_, value)| value.as_str())
                    .unwrap_or_default()
            };
            let question = lookup("question");
            let answer = lookup("answer");
            if !question.is_empty() || !answer.is_empty() {
                let line = format!("## Most recent memory note\nQ: {question}\nA: {answer}");
                sections.push(truncate_on_line_boundary(&line, DIGEST_MEMORY_MAX_BYTES));
            }
        }
    }

    if sections.is_empty() {
        return String::new();
    }

    let header = "# Workspace memory (pushed so you need not blind-search; pull more with system_map/recall/brief_get)";
    let body = format!("{header}\n\n{}", sections.join("\n\n"));
    truncate_on_line_boundary(&body, WORKSPACE_DIGEST_MAX_BYTES)
}

/// Truncate `text` to at most `max_bytes` on a line boundary for the workspace
/// digest, appending a short elision marker. A thin wrapper over the shared,
/// UTF-8-safe `skill_match::truncate_on_line_boundary` so the digest and the
/// per-prompt skill brief share one correct implementation (the earlier local
/// copy sliced `&str` by raw byte index and panicked on a multibyte char at the
/// boundary — map/brief/note text routinely contains em-dashes and smart quotes).
pub(super) fn truncate_on_line_boundary(text: &str, max_bytes: usize) -> String {
    crate::utility::skill_match::truncate_on_line_boundary(text, max_bytes, "\n…[truncated]")
}

pub(super) fn memory_system_map_path_for_workspace(workspace_root: &Path) -> Option<PathBuf> {
    let claude_home = resolve_claude_home("").ok()?;
    Some(
        crate::utility::memory::system_map_reference_directory(
            &claude_home,
            "memory",
            workspace_root,
        )
        .join("SYSTEM_MAP.md"),
    )
}

pub(crate) fn sanitize_memory_key(value: &str) -> String {
    let mut key = String::new();

    let mut previous_dash = false;

    for character in value.chars() {
        if character.is_ascii_alphanumeric() {
            key.push(character.to_ascii_lowercase());

            previous_dash = false;
        } else if !previous_dash {
            key.push('-');

            previous_dash = true;
        }
    }

    let trimmed = key.trim_matches('-').to_string();

    if trimmed.is_empty() {
        "workspace".to_string()
    } else {
        trimmed
    }
}
