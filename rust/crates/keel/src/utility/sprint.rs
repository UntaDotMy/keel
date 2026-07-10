//! Purpose: Scrum-style sprint loop ledger, backing `keel sprint`.
//!   A sprint is a set of story records (the confirmed user-story backlog); each
//!   story carries a Definition-of-Done state. `review` is the fail-closed loop
//!   gate: the sprint is not done while any story is not Done, so the agent loops
//!   back instead of presenting half-built work as complete.
//! Caller: commands.rs `sprint` dispatch arm.
//! Dependencies: std::io, crate::args::FlagSet, crate::runtime path helpers,
//!   crate::utility::record_store (durable per-story JSON records), serde_json.
//! Main Functions: run_sprint_command (plan|status|advance|review|list).
//! Side Effects: Reads/writes story records under
//!   `<claude_home>/sprint/<workspace-slug>/`. One active sprint per workspace.
//!
//! Why a ledger and not just chat state: the loop must survive compaction and a
//! fresh session — "which stories are still not Done" is the one fact the agent
//! cannot afford to lose mid-sprint. Storing it as records (the same RecordStore
//! the memory families use) makes the loop resumable and the `review` gate
//! deterministic rather than a recollection.

use std::io::Write;
use std::path::Path;

use crate::args::FlagSet;
use crate::runtime::{display_path, resolve_claude_home, resolve_repository_root};
use crate::utility::record_store::{allocate_unique_record_id, field, Record, RecordStore};

/// Valid Definition-of-Done states for a story, in board order. `done` is the only
/// state that satisfies the review gate; `blocked` is explicitly NOT done so a
/// blocked story keeps the sprint open (a blocker is surfaced, never silently
/// counted as complete).
const STATES: &[&str] = &["todo", "in-progress", "blocked", "done"];
const STATE_DONE: &str = "done";

/// An open (not-Done) story in an active sprint: its id, narrative, and current
/// state. Returned by [`open_stories_for_workspace`] for the honest-closeout gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenStory {
    pub id: String,
    pub story: String,
    pub state: String,
}

/// Closeout view of a workspace's sprint, for the PostToolBatch honest-closeout
/// gate. Returns:
/// - `None` when there is **no active sprint** for this workspace (no story
///   records at all) — the gate stays silent, so ordinary/question turns and any
///   project that never started a sprint are unaffected.
/// - `Some(vec![])` when a sprint exists and every story is Done — the gate is
///   satisfied (the sprint is complete).
/// - `Some(open)` listing the still-open/blocked stories when a sprint exists but
///   is not complete — the gate reports these as gaps.
///
/// This is the single source of truth the gate shares with `sprint review`: a
/// story is open unless its state is exactly `done`, so a `blocked` story counts
/// as a gap (never silently complete). Pure read over the per-workspace store;
/// any IO error surfaces as `Err` so the caller can fail open.
pub fn open_stories_for_workspace(
    claude_home: &Path,
    workspace_root: &str,
) -> Result<Option<Vec<OpenStory>>, String> {
    let store = RecordStore::new(claude_home, &sprint_group_for_workspace(workspace_root));
    let stories = store.list_records()?;
    if stories.is_empty() {
        return Ok(None);
    }
    let open = stories
        .iter()
        .filter(|(_, record)| field(record, "state") != Some(STATE_DONE))
        .map(|(id, record)| OpenStory {
            id: id.clone(),
            story: field(record, "story").unwrap_or("").to_string(),
            state: field(record, "state").unwrap_or("todo").to_string(),
        })
        .collect();
    Ok(Some(open))
}

/// Resolve the per-workspace sprint store group path (`sprint/<slug>`) for a
/// workspace path string, normalizing the path the SAME way the `sprint` CLI does
/// (`display_path` of the constructed `PathBuf`) so the honest-closeout gate reads
/// the exact directory `sprint plan` wrote — separator/case differences between
/// the timing-row cwd and the CLI `--workspace-root` cannot split them.
fn sprint_group_for_workspace(workspace_root: &str) -> String {
    let normalized = display_path(&std::path::PathBuf::from(workspace_root));
    format!("sprint/{}", workspace_slug(&normalized))
}

/// Test-only accessor for the workspace slug used by the sprint store, so the
/// hook-lifecycle closeout-gate tests can seed records under the same directory
/// the gate resolves. Mirrors the normalization in [`sprint_group_for_workspace`].
#[cfg(test)]
pub fn workspace_slug_for_test(workspace_root: &str) -> String {
    let normalized = display_path(&std::path::PathBuf::from(workspace_root));
    workspace_slug(&normalized)
}

/// CLI: `keel sprint <plan|status|advance|review|list> [flags]`.
pub fn run_sprint_command(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let action = arguments.first().map(String::as_str).unwrap_or("");
    if action.is_empty() || matches!(action, "help" | "--help" | "-h") {
        let _ = writeln!(
            standard_output,
            "Usage: keel sprint <plan|status|advance|review|list> [flags]\n\
             \n\
             plan     Add a confirmed user story to the sprint backlog.\n\
             \x20          --story \"As a ..., I want ..., so that ...\" [--id <id>]\n\
             status   Show every story and its Definition-of-Done state.\n\
             advance  Update a story's state.\n\
             \x20          --id <id> --state <todo|in-progress|blocked|done> [--note <text>]\n\
             review   Loop gate: report whether ALL stories are Done.\n\
             \x20          Exit 0 only when the sprint is complete; non-zero while any\n\
             \x20          story is not Done (so the loop continues).\n\
             list     Alias for status.\n\
             \n\
             Common flags: --workspace-root <path>  --claude-home <path>  --json"
        );
        return if action.is_empty() { 1 } else { 0 };
    }
    match action {
        "plan" => run_plan(&arguments[1..], standard_output, standard_error),
        "status" | "list" => run_status(&arguments[1..], standard_output, standard_error),
        "advance" => run_advance(&arguments[1..], standard_output, standard_error),
        "review" => run_review(&arguments[1..], standard_output, standard_error),
        other => {
            let _ = writeln!(standard_error, "sprint: unknown subcommand: {other}");
            1
        }
    }
}

/// Resolve the per-workspace sprint store. The group path is keyed by a slug of
/// the workspace root so two projects never share a backlog. `claude_home` is
/// the resolved value of the `--claude-home` flag (empty = default home), so the
/// sprint store can be isolated for tests and redirected by callers just like
/// every other stateful family (`memory`, `workflow`, `orchestration`).
fn resolve_store(
    workspace_root: &str,
    claude_home: &str,
    label: &str,
    standard_error: &mut dyn Write,
) -> Option<RecordStore> {
    let claude_home = match resolve_claude_home(claude_home) {
        Ok(path) => path,
        Err(error) => {
            let _ = writeln!(standard_error, "{label}: {error}");
            return None;
        }
    };
    let root = if workspace_root.is_empty() {
        resolve_repository_root("").unwrap_or_else(|_| std::path::PathBuf::from("."))
    } else {
        std::path::PathBuf::from(workspace_root)
    };
    Some(RecordStore::new(
        &claude_home,
        &sprint_group_for_workspace(&display_path(&root)),
    ))
}

/// Lowercase alphanumeric slug of a workspace path, matching the learning loop's
/// project-slug shape so the same project resolves to a stable directory.
fn workspace_slug(path: &str) -> String {
    let mut slug = String::new();
    let mut previous_dash = false;
    for character in path.chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character.to_ascii_lowercase());
            previous_dash = false;
        } else if !previous_dash {
            slug.push('-');
            previous_dash = true;
        }
    }
    let trimmed = slug.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "workspace".to_string()
    } else {
        trimmed
    }
}

fn run_plan(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flags = FlagSet::new("sprint plan");
    flags.string_flag("story", "");
    flags.string_flag("id", "");
    flags.string_flag("workspace-root", "");
    flags.string_flag("claude-home", "");
    flags.bool_flag("json", false);
    if let Err(error) = flags.parse(arguments) {
        let _ = writeln!(standard_error, "{}", error.message);
        return 1;
    }
    let story = flags.string_value("story").trim().to_string();
    if story.is_empty() {
        let _ = writeln!(
            standard_error,
            "sprint plan: --story required (the confirmed user-story narrative)"
        );
        return 1;
    }
    let Some(store) = resolve_store(
        flags.string_value("workspace-root"),
        flags.string_value("claude-home"),
        "sprint plan",
        standard_error,
    ) else {
        return 1;
    };

    let requested_id = flags.string_value("id").trim().to_string();
    let base_id = if requested_id.is_empty() {
        // Stable ordinal id: count existing stories and use the next slot.
        let count = store
            .list_records()
            .map(|records| records.len())
            .unwrap_or(0);
        format!("s{}", count + 1)
    } else {
        requested_id
    };
    let id = allocate_unique_record_id(&store, &base_id);

    let record: Record = vec![
        ("id".into(), id.clone()),
        ("story".into(), story.clone()),
        ("state".into(), "todo".into()),
        ("note".into(), String::new()),
    ];
    if let Err(error) = store.write_record(&id, &record) {
        let _ = writeln!(standard_error, "sprint plan: {error}");
        return 1;
    }

    if flags.bool_value("json") {
        emit_json(
            standard_output,
            standard_error,
            &serde_json::json!({"planned": true, "id": id, "story": story, "state": "todo"}),
        )
    } else {
        let _ = writeln!(standard_output, "sprint plan: added {id} [todo] {story}");
        0
    }
}

fn run_advance(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flags = FlagSet::new("sprint advance");
    flags.string_flag("id", "");
    flags.string_flag("state", "");
    flags.string_flag("note", "");
    flags.string_flag("workspace-root", "");
    flags.string_flag("claude-home", "");
    flags.bool_flag("json", false);
    if let Err(error) = flags.parse(arguments) {
        let _ = writeln!(standard_error, "{}", error.message);
        return 1;
    }
    let id = flags.string_value("id").trim().to_string();
    let state = flags.string_value("state").trim().to_string();
    if id.is_empty() || state.is_empty() {
        let _ = writeln!(
            standard_error,
            "sprint advance: --id and --state required (state: {})",
            STATES.join("|")
        );
        return 1;
    }
    if !STATES.contains(&state.as_str()) {
        let _ = writeln!(
            standard_error,
            "sprint advance: invalid state {state:?}; expected one of {}",
            STATES.join("|")
        );
        return 1;
    }
    let Some(store) = resolve_store(
        flags.string_value("workspace-root"),
        flags.string_value("claude-home"),
        "sprint advance",
        standard_error,
    ) else {
        return 1;
    };
    let mut record = match store.read_record(&id) {
        Ok(Some(record)) => record,
        Ok(None) => {
            let _ = writeln!(standard_error, "sprint advance: no story with id {id}");
            return 1;
        }
        Err(error) => {
            let _ = writeln!(standard_error, "sprint advance: {error}");
            return 1;
        }
    };
    set_field(&mut record, "state", &state);
    let note = flags.string_value("note").trim().to_string();
    if !note.is_empty() {
        set_field(&mut record, "note", &note);
    }
    if let Err(error) = store.write_record(&id, &record) {
        let _ = writeln!(standard_error, "sprint advance: {error}");
        return 1;
    }
    if flags.bool_value("json") {
        emit_json(
            standard_output,
            standard_error,
            &serde_json::json!({"advanced": true, "id": id, "state": state}),
        )
    } else {
        let _ = writeln!(standard_output, "sprint advance: {id} -> {state}");
        0
    }
}

fn run_status(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flags = FlagSet::new("sprint status");
    flags.string_flag("workspace-root", "");
    flags.string_flag("claude-home", "");
    flags.bool_flag("json", false);
    if let Err(error) = flags.parse(arguments) {
        let _ = writeln!(standard_error, "{}", error.message);
        return 1;
    }
    let Some(store) = resolve_store(
        flags.string_value("workspace-root"),
        flags.string_value("claude-home"),
        "sprint status",
        standard_error,
    ) else {
        return 1;
    };
    let stories = match store.list_records() {
        Ok(records) => records,
        Err(error) => {
            let _ = writeln!(standard_error, "sprint status: {error}");
            return 1;
        }
    };
    let done = count_done(&stories);
    let total = stories.len();

    if flags.bool_value("json") {
        let items: Vec<serde_json::Value> = stories
            .iter()
            .map(|(id, record)| {
                serde_json::json!({
                    "id": id,
                    "state": field(record, "state").unwrap_or("todo"),
                    "story": field(record, "story").unwrap_or(""),
                    "note": field(record, "note").unwrap_or(""),
                })
            })
            .collect();
        emit_json(
            standard_output,
            standard_error,
            &serde_json::json!({"total": total, "done": done, "complete": total > 0 && done == total, "stories": items}),
        )
    } else {
        if total == 0 {
            let _ = writeln!(
                standard_output,
                "sprint status: no stories planned yet (use `sprint plan --story ...`)"
            );
            return 0;
        }
        let _ = writeln!(
            standard_output,
            "sprint status: {done}/{total} stories Done"
        );
        for (id, record) in &stories {
            let _ = writeln!(
                standard_output,
                "  [{}] {} :: {}",
                field(record, "state").unwrap_or("todo"),
                id,
                field(record, "story").unwrap_or("")
            );
        }
        0
    }
}

/// The loop gate. Exit 0 only when there is at least one story and every story is
/// Done. While any story is not Done it exits non-zero and names the open stories,
/// so the caller (or a CI step) treats "sprint not complete" as a hard stop —
/// this is the fail-closed "loop until all okay" contract.
fn run_review(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flags = FlagSet::new("sprint review");
    flags.string_flag("workspace-root", "");
    flags.string_flag("claude-home", "");
    flags.bool_flag("json", false);
    if let Err(error) = flags.parse(arguments) {
        let _ = writeln!(standard_error, "{}", error.message);
        return 1;
    }
    let Some(store) = resolve_store(
        flags.string_value("workspace-root"),
        flags.string_value("claude-home"),
        "sprint review",
        standard_error,
    ) else {
        return 1;
    };
    let stories = match store.list_records() {
        Ok(records) => records,
        Err(error) => {
            let _ = writeln!(standard_error, "sprint review: {error}");
            return 1;
        }
    };
    let total = stories.len();
    let open: Vec<&(String, Record)> = stories
        .iter()
        .filter(|(_, record)| field(record, "state") != Some(STATE_DONE))
        .collect();
    let complete = total > 0 && open.is_empty();

    if flags.bool_value("json") {
        let open_ids: Vec<&str> = open.iter().map(|(id, _)| id.as_str()).collect();
        let code = if complete { 0 } else { 1 };
        let _ = emit_json(
            standard_output,
            standard_error,
            &serde_json::json!({
                "complete": complete,
                "total": total,
                "done": total - open.len(),
                "openStories": open_ids,
            }),
        );
        return code;
    }

    if total == 0 {
        let _ = writeln!(
            standard_error,
            "sprint review: no stories planned — nothing to review (the sprint is empty, not complete)"
        );
        return 1;
    }
    if complete {
        let _ = writeln!(
            standard_output,
            "sprint review: COMPLETE — all {total} stories meet Definition of Done."
        );
        let _ = writeln!(
            standard_output,
            "  Next: demo the increment and capture a retro (keel memory ...)."
        );
        0
    } else {
        let _ = writeln!(
            standard_output,
            "sprint review: NOT COMPLETE — {}/{} stories still open. Loop back:",
            open.len(),
            total
        );
        for (id, record) in &open {
            let _ = writeln!(
                standard_output,
                "  [{}] {} :: {}",
                field(record, "state").unwrap_or("todo"),
                id,
                field(record, "story").unwrap_or("")
            );
        }
        1
    }
}

fn count_done(stories: &[(String, Record)]) -> usize {
    stories
        .iter()
        .filter(|(_, record)| field(record, "state") == Some(STATE_DONE))
        .count()
}

fn set_field(record: &mut Record, key: &str, value: &str) {
    if let Some(slot) = record.iter_mut().find(|(field_key, _)| field_key == key) {
        slot.1 = value.to_string();
    } else {
        record.push((key.to_string(), value.to_string()));
    }
}

fn emit_json(
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
    payload: &serde_json::Value,
) -> u8 {
    match serde_json::to_string_pretty(payload) {
        Ok(text) => {
            let _ = writeln!(standard_output, "{text}");
            0
        }
        Err(error) => {
            let _ = writeln!(standard_error, "sprint: render json: {error}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::ENV_LOCK;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// Run `body` with CLAUDE_TARGET_OVERRIDE pointed at an isolated home and a
    /// fixed workspace root, so the sprint store is deterministic and per-test.
    fn isolated<F: FnOnce(&str)>(label: &str, body: F) {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let root: PathBuf = std::env::temp_dir().join(format!(
            "keel-sprint-{}-{nanos}-{label}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("create test home");
        let previous = std::env::var("CLAUDE_TARGET_OVERRIDE").ok();
        std::env::set_var("CLAUDE_TARGET_OVERRIDE", &root);
        // A stable workspace root string for the slug.
        body("/work/sprintproj");
        match previous {
            Some(value) => std::env::set_var("CLAUDE_TARGET_OVERRIDE", value),
            None => std::env::remove_var("CLAUDE_TARGET_OVERRIDE"),
        }
        let _ = fs::remove_dir_all(&root);
    }

    fn run(args: &[&str]) -> (u8, String, String) {
        let owned: Vec<String> = args.iter().map(|a| a.to_string()).collect();
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run_sprint_command(&owned, &mut out, &mut err);
        (
            code,
            String::from_utf8_lossy(&out).to_string(),
            String::from_utf8_lossy(&err).to_string(),
        )
    }

    #[test]
    fn plan_then_status_lists_story_as_todo() {
        isolated("plan", |ws| {
            let (code, _, _) = run(&[
                "plan",
                "--workspace-root",
                ws,
                "--story",
                "As a dev, I want X, so that Y.",
            ]);
            assert_eq!(code, 0);
            let (code, out, _) = run(&["status", "--workspace-root", ws]);
            assert_eq!(code, 0);
            assert!(out.contains("0/1 stories Done"), "status: {out}");
            assert!(out.contains("[todo]"), "story starts todo: {out}");
        });
    }

    #[test]
    fn claude_home_flag_isolates_the_sprint_store() {
        // Regression: sprint subcommands previously defined no --claude-home flag
        // and resolve_store hardcoded resolve_claude_home(""), so sprint state
        // could only ever live in the real ~/.claude (untestable, unredirectable).
        // This pins the fix: --claude-home routes the store to the given home,
        // and an env-resolved "real" home (CLAUDE_TARGET_OVERRIDE) stays empty.
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let base =
            std::env::temp_dir().join(format!("keel-sprint-home-{}-{nanos}", std::process::id()));
        let explicit_home = base.join("explicit");
        let sentinel_home = base.join("sentinel");
        fs::create_dir_all(&explicit_home).expect("create explicit home");
        fs::create_dir_all(&sentinel_home).expect("create sentinel home");

        let previous = std::env::var("CLAUDE_TARGET_OVERRIDE").ok();
        std::env::set_var("CLAUDE_TARGET_OVERRIDE", &sentinel_home);

        let explicit = explicit_home.to_string_lossy().to_string();
        let (code, _, err) = run(&[
            "plan",
            "--workspace-root",
            "/work/homeflag",
            "--claude-home",
            &explicit,
            "--story",
            "As a dev, I want isolation, so that tests are safe.",
        ]);

        match previous {
            Some(value) => std::env::set_var("CLAUDE_TARGET_OVERRIDE", value),
            None => std::env::remove_var("CLAUDE_TARGET_OVERRIDE"),
        }

        assert_eq!(code, 0, "stderr: {err}");
        // The sprint store must exist under the explicit home...
        let explicit_sprint = explicit_home.join("sprint");
        assert!(
            explicit_sprint.is_dir(),
            "sprint store must be created under --claude-home"
        );
        // ...and the env-resolved sentinel home must be untouched.
        let sentinel_sprint = sentinel_home.join("sprint");
        assert!(
            !sentinel_sprint.exists(),
            "sprint must NOT write the env-resolved home when --claude-home is given"
        );

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn review_is_fail_closed_until_all_done() {
        isolated("review", |ws| {
            run(&["plan", "--workspace-root", ws, "--story", "story one"]);
            run(&["plan", "--workspace-root", ws, "--story", "story two"]);

            // Not complete -> non-zero exit (the loop must continue).
            let (code, out, _) = run(&["review", "--workspace-root", ws]);
            assert_eq!(code, 1, "review fails while stories are open: {out}");
            assert!(out.contains("NOT COMPLETE"));

            // Finish one -> still not complete.
            run(&[
                "advance",
                "--workspace-root",
                ws,
                "--id",
                "s1",
                "--state",
                "done",
            ]);
            let (code, _, _) = run(&["review", "--workspace-root", ws]);
            assert_eq!(code, 1, "one of two done is still not complete");

            // Finish the second -> complete, exit 0.
            run(&[
                "advance",
                "--workspace-root",
                ws,
                "--id",
                "s2",
                "--state",
                "done",
            ]);
            let (code, out, _) = run(&["review", "--workspace-root", ws]);
            assert_eq!(code, 0, "all done -> complete: {out}");
            assert!(out.contains("COMPLETE"));
        });
    }

    #[test]
    fn review_on_empty_sprint_is_not_complete() {
        isolated("empty", |ws| {
            let (code, _, err) = run(&["review", "--workspace-root", ws]);
            assert_eq!(code, 1, "an empty sprint is not 'complete'");
            assert!(err.contains("no stories"), "err: {err}");
        });
    }

    #[test]
    fn blocked_story_keeps_sprint_open() {
        isolated("blocked", |ws| {
            run(&["plan", "--workspace-root", ws, "--story", "blocked one"]);
            run(&[
                "advance",
                "--workspace-root",
                ws,
                "--id",
                "s1",
                "--state",
                "blocked",
            ]);
            let (code, _, _) = run(&["review", "--workspace-root", ws]);
            assert_eq!(code, 1, "blocked is explicitly not done");
        });
    }

    #[test]
    fn advance_rejects_unknown_state() {
        isolated("badstate", |ws| {
            run(&["plan", "--workspace-root", ws, "--story", "s"]);
            let (code, _, err) = run(&[
                "advance",
                "--workspace-root",
                ws,
                "--id",
                "s1",
                "--state",
                "shipped",
            ]);
            assert_eq!(code, 1);
            assert!(err.contains("invalid state"), "err: {err}");
        });
    }

    #[test]
    fn advance_unknown_id_fails() {
        isolated("badid", |ws| {
            let (code, _, err) = run(&[
                "advance",
                "--workspace-root",
                ws,
                "--id",
                "s99",
                "--state",
                "done",
            ]);
            assert_eq!(code, 1);
            assert!(err.contains("no story with id"), "err: {err}");
        });
    }

    #[test]
    fn review_json_reports_open_stories() {
        isolated("json", |ws| {
            run(&["plan", "--workspace-root", ws, "--story", "one"]);
            let (code, out, _) = run(&["review", "--workspace-root", ws, "--json"]);
            assert_eq!(code, 1);
            assert!(out.contains("\"complete\": false"), "json: {out}");
            assert!(out.contains("s1"), "names the open story: {out}");
        });
    }

    #[test]
    fn open_stories_none_when_no_active_sprint() {
        isolated("closeout-none", |ws| {
            let home = resolve_claude_home("").expect("home");
            let result = open_stories_for_workspace(&home, ws).expect("ok");
            assert!(
                result.is_none(),
                "no sprint planned -> None so the closeout gate stays silent"
            );
        });
    }

    #[test]
    fn open_stories_lists_open_and_blocked_but_not_done() {
        isolated("closeout-open", |ws| {
            run(&["plan", "--workspace-root", ws, "--story", "alpha"]);
            run(&["plan", "--workspace-root", ws, "--story", "beta"]);
            run(&["plan", "--workspace-root", ws, "--story", "gamma"]);
            run(&[
                "advance",
                "--workspace-root",
                ws,
                "--id",
                "s1",
                "--state",
                "done",
            ]);
            run(&[
                "advance",
                "--workspace-root",
                ws,
                "--id",
                "s2",
                "--state",
                "blocked",
            ]);
            // s3 stays todo.
            let home = resolve_claude_home("").expect("home");
            let open = open_stories_for_workspace(&home, ws)
                .expect("ok")
                .expect("active sprint");
            let ids: Vec<&str> = open.iter().map(|story| story.id.as_str()).collect();
            assert_eq!(ids, vec!["s2", "s3"], "done excluded, blocked + todo open");
        });
    }

    #[test]
    fn open_stories_empty_vec_when_all_done() {
        isolated("closeout-done", |ws| {
            run(&["plan", "--workspace-root", ws, "--story", "only"]);
            run(&[
                "advance",
                "--workspace-root",
                ws,
                "--id",
                "s1",
                "--state",
                "done",
            ]);
            let home = resolve_claude_home("").expect("home");
            let open = open_stories_for_workspace(&home, ws)
                .expect("ok")
                .expect("active sprint");
            assert!(open.is_empty(), "all done -> sprint complete, no gaps");
        });
    }
}
