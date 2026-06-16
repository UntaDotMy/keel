//! Purpose: Parallel worker dispatcher — a durable worker ledger, real git
//!   worktree isolation, and a fail-closed merge coordinator. Backs
//!   `claude-skills dispatch`. This is the buildable substrate the
//!   `dispatching-parallel-agents` and `using-git-worktrees` skills drive: the
//!   CLI owns the worktree lifecycle and the merge gate; the skill (the main
//!   Claude thread) drives the agents that fill each worktree. Mirrors how
//!   `sprint` owns the loop ledger + fail-closed `review` gate while the
//!   `running-a-sprint` skill drives the loop.
//! Caller: commands.rs `dispatch` dispatch arm.
//! Dependencies: std::io, crate::args::FlagSet, crate::runtime path/exec helpers,
//!   crate::utility::record_store (durable per-worker JSON records), serde_json.
//! Main Functions: run_dispatch_command (plan|start|status|complete|merge|abandon|list).
//! Side Effects: Reads/writes worker records under
//!   `<claude_home>/dispatch/<workspace-slug>/`; `start` creates a git worktree,
//!   `merge` mutates the current branch (guarded by --confirm + a clean-merge
//!   gate), `abandon` removes a worktree (guarded by --confirm).
//!
//! Why a ledger + real worktrees and not just advice: the competitive audit
//! flagged that delegation here is sequential and a subagent cannot spawn
//! subagents, so parallel fan-out had no durable coordination substrate. A
//! worker's isolation must be a real `git worktree` (two workers editing the
//! same files cannot corrupt each other), and the merge back must be
//! fail-closed: a worker is merged only when it is `complete` AND its branch
//! applies without conflict; on conflict the merge is aborted and the tree is
//! left clean, never half-merged. That gate is the one fact the orchestrator
//! cannot afford to get wrong, so it lives in deterministic code, not a prompt.

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::args::FlagSet;
use crate::runtime::{display_path, resolve_claude_home, resolve_repository_root, run_command};
use crate::utility::record_store::{allocate_unique_record_id, field, Record, RecordStore};

/// Worker lifecycle states, in board order. `complete` is the only state from
/// which a worker may merge; `merged` is terminal-success; `abandoned` is
/// terminal-discard. A worker that is not exactly `complete` can never pass the
/// merge gate — that is the fail-closed state guard. The verbs validate against
/// the individual `STATE_*` constants rather than scanning this slice, so it is
/// documentation + a test anchor (membership is asserted by a test); dead in the
/// shipped binary, live under `cfg(test)`.
#[cfg_attr(not(test), allow(dead_code))]
const STATES: &[&str] = &["pending", "running", "complete", "merged", "abandoned"];
const STATE_PENDING: &str = "pending";
const STATE_RUNNING: &str = "running";
const STATE_COMPLETE: &str = "complete";
const STATE_MERGED: &str = "merged";
const STATE_ABANDONED: &str = "abandoned";

/// Resolved per-invocation context shared by every verb: the durable worker
/// store, the git repository root the worktrees branch from and merge into, and
/// the base directory worktrees are created under.
struct DispatchContext {
    store: RecordStore,
    repo_root: PathBuf,
    worktree_base: PathBuf,
}

/// CLI: `claude-skills dispatch <plan|start|status|complete|merge|abandon|list> [flags]`.
pub fn run_dispatch_command(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let action = arguments.first().map(String::as_str).unwrap_or("");
    if action.is_empty() || matches!(action, "help" | "--help" | "-h") {
        render_help(standard_output);
        return if action.is_empty() { 1 } else { 0 };
    }
    match action {
        "plan" => run_plan(&arguments[1..], standard_output, standard_error),
        "start" => run_start(&arguments[1..], standard_output, standard_error),
        "status" | "list" => run_status(&arguments[1..], standard_output, standard_error),
        "complete" => run_complete(&arguments[1..], standard_output, standard_error),
        "merge" => run_merge(&arguments[1..], standard_output, standard_error),
        "abandon" => run_abandon(&arguments[1..], standard_output, standard_error),
        other => {
            let _ = writeln!(standard_error, "dispatch: unknown subcommand: {other}");
            1
        }
    }
}

fn render_help(standard_output: &mut dyn Write) {
    let _ = writeln!(
        standard_output,
        "Usage: claude-skills dispatch <plan|start|status|complete|merge|abandon|list> [flags]\n\
         \n\
         plan      Register a parallel worker task.\n\
         \x20           --task \"<description>\" [--id <id>]\n\
         start     Create the worker's isolated git worktree (real `git worktree add`).\n\
         \x20           --id <id> [--start-point <commit-ish>]\n\
         status    Show every worker and its state, plus a coordinator summary.\n\
         complete  Mark a worker's branch as ready to merge.\n\
         \x20           --id <id> [--note <text>]\n\
         merge     Fail-closed merge of a completed worker's branch into the current branch.\n\
         \x20           --id <id> --confirm [--message <text>]\n\
         \x20           Refuses unless the worker is `complete`; on conflict, aborts the\n\
         \x20           merge and leaves the tree clean (never half-merged).\n\
         abandon   Remove a worker's worktree and delete its branch.\n\
         \x20           --id <id> --confirm\n\
         list      Alias for status.\n\
         \n\
         Common flags: --workspace-root <path>  --claude-home <path>\n\
         \x20             --worktree-root <path>  --json"
    );
}

/// Lowercase alphanumeric slug of a workspace path. Duplicated from `sprint` (a
/// small, self-contained helper) rather than coupling the two command modules;
/// the shape matches the learning loop's project-slug so the same project
/// resolves to a stable per-workspace directory.
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

/// Resolve the store, repo root, and worktree base for this invocation. The
/// store is keyed by a slug of the repo root so two projects never share a
/// worker board; `--claude-home` redirects the store (tests, isolation) exactly
/// like every other stateful family. Worktrees default to
/// `<claude_home>/worktrees/<slug>/` — entirely outside the repo tree so a
/// worktree checkout never shows up as untracked noise in the parent — and
/// `--worktree-root` overrides that base.
fn resolve_context(
    workspace_root: &str,
    claude_home: &str,
    worktree_root: &str,
    label: &str,
    standard_error: &mut dyn Write,
) -> Option<DispatchContext> {
    let home = match resolve_claude_home(claude_home) {
        Ok(path) => path,
        Err(error) => {
            let _ = writeln!(standard_error, "{label}: {error}");
            return None;
        }
    };
    let repo_root = match resolve_repository_root(workspace_root) {
        Ok(path) => path,
        Err(error) => {
            let _ = writeln!(standard_error, "{label}: {error}");
            return None;
        }
    };
    let slug = workspace_slug(&display_path(&repo_root));
    let store = RecordStore::new(&home, &format!("dispatch/{slug}"));
    let worktree_base = if worktree_root.trim().is_empty() {
        home.join("worktrees").join(&slug)
    } else {
        PathBuf::from(worktree_root.trim())
    };
    Some(DispatchContext {
        store,
        repo_root,
        worktree_base,
    })
}

/// Run a git subcommand that must succeed; non-zero exit is treated as failure
/// and reported. Returns trimmed stdout on success. Mirrors `checkpoint::git`.
/// For `merge` we deliberately bypass this helper and inspect the exit code
/// directly, because a non-zero merge (a conflict) is an expected, handled path.
fn git(
    repo_root: &Path,
    args: &[&str],
    label: &str,
    standard_error: &mut dyn Write,
) -> Option<String> {
    let owned: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
    match run_command("git", &owned, Some(repo_root)) {
        Ok(result) if result.code == 0 => {
            Some(String::from_utf8_lossy(&result.stdout).trim().to_string())
        }
        Ok(result) => {
            let _ = writeln!(
                standard_error,
                "{label}: git {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&result.stderr).trim()
            );
            None
        }
        Err(error) => {
            let _ = writeln!(standard_error, "{label}: {error}");
            None
        }
    }
}

fn run_plan(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flags = FlagSet::new("dispatch plan");
    flags.string_flag("task", "");
    flags.string_flag("id", "");
    flags.string_flag("workspace-root", "");
    flags.string_flag("claude-home", "");
    flags.string_flag("worktree-root", "");
    flags.bool_flag("json", false);
    if let Err(error) = flags.parse(arguments) {
        let _ = writeln!(standard_error, "dispatch plan: {}", error.message);
        return 1;
    }
    let task = flags.string_value("task").trim().to_string();
    if task.is_empty() {
        let _ = writeln!(
            standard_error,
            "dispatch plan: --task required (the worker's task description)"
        );
        return 1;
    }
    let Some(context) = resolve_context(
        flags.string_value("workspace-root"),
        flags.string_value("claude-home"),
        flags.string_value("worktree-root"),
        "dispatch plan",
        standard_error,
    ) else {
        return 1;
    };

    let requested_id = flags.string_value("id").trim().to_string();
    let base_id = if requested_id.is_empty() {
        let count = context
            .store
            .list_records()
            .map(|records| records.len())
            .unwrap_or(0);
        format!("w{}", count + 1)
    } else {
        requested_id
    };
    let id = allocate_unique_record_id(&context.store, &base_id);
    let branch = format!("claude/worker-{id}");

    let record: Record = vec![
        ("id".into(), id.clone()),
        ("task".into(), task.clone()),
        ("branch".into(), branch.clone()),
        ("worktree".into(), String::new()),
        ("state".into(), STATE_PENDING.into()),
        ("note".into(), String::new()),
    ];
    if let Err(error) = context.store.write_record(&id, &record) {
        let _ = writeln!(standard_error, "dispatch plan: {error}");
        return 1;
    }

    if flags.bool_value("json") {
        emit_json(
            standard_output,
            standard_error,
            &serde_json::json!({
                "planned": true,
                "id": id,
                "task": task,
                "branch": branch,
                "state": STATE_PENDING,
            }),
        )
    } else {
        let _ = writeln!(
            standard_output,
            "dispatch plan: registered {id} [{STATE_PENDING}] on branch {branch} :: {task}"
        );
        let _ = writeln!(
            standard_output,
            "  next: claude-skills dispatch start --id {id}"
        );
        0
    }
}

fn run_start(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flags = FlagSet::new("dispatch start");
    flags.string_flag("id", "");
    flags.string_flag("start-point", "HEAD");
    flags.string_flag("workspace-root", "");
    flags.string_flag("claude-home", "");
    flags.string_flag("worktree-root", "");
    flags.bool_flag("json", false);
    if let Err(error) = flags.parse(arguments) {
        let _ = writeln!(standard_error, "dispatch start: {}", error.message);
        return 1;
    }
    let id = flags.string_value("id").trim().to_string();
    if id.is_empty() {
        let _ = writeln!(standard_error, "dispatch start: --id required");
        return 1;
    }
    let Some(context) = resolve_context(
        flags.string_value("workspace-root"),
        flags.string_value("claude-home"),
        flags.string_value("worktree-root"),
        "dispatch start",
        standard_error,
    ) else {
        return 1;
    };
    let mut record = match read_worker(&context.store, &id, "dispatch start", standard_error) {
        Some(record) => record,
        None => return 1,
    };
    let state = field(&record, "state").unwrap_or(STATE_PENDING);
    if state != STATE_PENDING {
        let _ = writeln!(
            standard_error,
            "dispatch start: worker {id} is `{state}`, not `{STATE_PENDING}` — its worktree was already created"
        );
        return 1;
    }
    let branch = field(&record, "branch").unwrap_or("").to_string();
    if branch.is_empty() {
        let _ = writeln!(standard_error, "dispatch start: worker {id} has no branch");
        return 1;
    }
    let worktree_path = context.worktree_base.join(&id);
    if let Err(error) = std::fs::create_dir_all(&context.worktree_base) {
        let _ = writeln!(
            standard_error,
            "dispatch start: create {}: {error}",
            display_path(&context.worktree_base)
        );
        return 1;
    }
    let worktree_arg = display_path(&worktree_path);
    let start_point = flags.string_value("start-point").trim().to_string();
    // Real isolation: a new worktree on a fresh branch off the chosen start point.
    if git(
        &context.repo_root,
        &[
            "worktree",
            "add",
            &worktree_arg,
            "-b",
            &branch,
            &start_point,
        ],
        "dispatch start",
        standard_error,
    )
    .is_none()
    {
        return 1;
    }

    set_field(&mut record, "state", STATE_RUNNING);
    set_field(&mut record, "worktree", &worktree_arg);
    if let Err(error) = context.store.write_record(&id, &record) {
        let _ = writeln!(standard_error, "dispatch start: {error}");
        return 1;
    }

    if flags.bool_value("json") {
        emit_json(
            standard_output,
            standard_error,
            &serde_json::json!({
                "started": true,
                "id": id,
                "branch": branch,
                "worktree": worktree_arg,
                "state": STATE_RUNNING,
            }),
        )
    } else {
        let _ = writeln!(
            standard_output,
            "dispatch start: worker {id} running in worktree {worktree_arg} (branch {branch})"
        );
        let _ = writeln!(
            standard_output,
            "  work + commit in that worktree, then: claude-skills dispatch complete --id {id}"
        );
        0
    }
}

fn run_complete(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flags = FlagSet::new("dispatch complete");
    flags.string_flag("id", "");
    flags.string_flag("note", "");
    flags.string_flag("workspace-root", "");
    flags.string_flag("claude-home", "");
    flags.string_flag("worktree-root", "");
    flags.bool_flag("json", false);
    if let Err(error) = flags.parse(arguments) {
        let _ = writeln!(standard_error, "dispatch complete: {}", error.message);
        return 1;
    }
    let id = flags.string_value("id").trim().to_string();
    if id.is_empty() {
        let _ = writeln!(standard_error, "dispatch complete: --id required");
        return 1;
    }
    let Some(context) = resolve_context(
        flags.string_value("workspace-root"),
        flags.string_value("claude-home"),
        flags.string_value("worktree-root"),
        "dispatch complete",
        standard_error,
    ) else {
        return 1;
    };
    let mut record = match read_worker(&context.store, &id, "dispatch complete", standard_error) {
        Some(record) => record,
        None => return 1,
    };
    let state = field(&record, "state").unwrap_or(STATE_PENDING);
    if state != STATE_RUNNING {
        let _ = writeln!(
            standard_error,
            "dispatch complete: worker {id} is `{state}`, not `{STATE_RUNNING}` — only a running worker (one with a worktree) can be completed"
        );
        return 1;
    }
    set_field(&mut record, "state", STATE_COMPLETE);
    let note = flags.string_value("note").trim().to_string();
    if !note.is_empty() {
        set_field(&mut record, "note", &note);
    }
    if let Err(error) = context.store.write_record(&id, &record) {
        let _ = writeln!(standard_error, "dispatch complete: {error}");
        return 1;
    }
    if flags.bool_value("json") {
        emit_json(
            standard_output,
            standard_error,
            &serde_json::json!({"completed": true, "id": id, "state": STATE_COMPLETE}),
        )
    } else {
        let _ = writeln!(
            standard_output,
            "dispatch complete: worker {id} ready to merge"
        );
        let _ = writeln!(
            standard_output,
            "  merge with: claude-skills dispatch merge --id {id} --confirm"
        );
        0
    }
}

/// The fail-closed merge coordinator. A worker merges only when it is exactly
/// `complete` (state gate) and `--confirm` is given (this mutates the current
/// branch). The merge runs `git merge --no-ff`; on a non-zero exit (conflict or
/// any failure) the merge is aborted with `git merge --abort` so the tree is
/// left clean, the worker stays `complete`, and the command exits non-zero. A
/// conflicting worker is never silently half-merged — that is the fail-closed
/// contract the orchestrator relies on.
fn run_merge(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flags = FlagSet::new("dispatch merge");
    flags.string_flag("id", "");
    flags.string_flag("message", "");
    flags.string_flag("workspace-root", "");
    flags.string_flag("claude-home", "");
    flags.string_flag("worktree-root", "");
    flags.bool_flag("confirm", false);
    flags.bool_flag("json", false);
    if let Err(error) = flags.parse(arguments) {
        let _ = writeln!(standard_error, "dispatch merge: {}", error.message);
        return 1;
    }
    let id = flags.string_value("id").trim().to_string();
    if id.is_empty() {
        let _ = writeln!(standard_error, "dispatch merge: --id required");
        return 1;
    }
    let Some(context) = resolve_context(
        flags.string_value("workspace-root"),
        flags.string_value("claude-home"),
        flags.string_value("worktree-root"),
        "dispatch merge",
        standard_error,
    ) else {
        return 1;
    };
    let mut record = match read_worker(&context.store, &id, "dispatch merge", standard_error) {
        Some(record) => record,
        None => return 1,
    };
    let state = field(&record, "state").unwrap_or(STATE_PENDING);
    // Fail-closed state gate: only a `complete` worker may merge.
    if state != STATE_COMPLETE {
        let _ = writeln!(
            standard_error,
            "dispatch merge: worker {id} is `{state}`, not `{STATE_COMPLETE}` — refusing to merge (complete it first)"
        );
        return 1;
    }
    // This mutates the current branch; require explicit confirmation.
    if !flags.bool_value("confirm") {
        let _ = writeln!(
            standard_error,
            "dispatch merge: this merges worker {id}'s branch into the current branch. Re-run with --confirm.\n  On conflict the merge is aborted automatically and the tree left clean."
        );
        return 1;
    }
    let branch = field(&record, "branch").unwrap_or("").to_string();
    if branch.is_empty() {
        let _ = writeln!(standard_error, "dispatch merge: worker {id} has no branch");
        return 1;
    }
    let task = field(&record, "task").unwrap_or("").to_string();
    let message = {
        let raw = flags.string_value("message").trim().to_string();
        if raw.is_empty() {
            format!("Merge worker {id}: {task}")
        } else {
            raw
        }
    };

    let merge_args: Vec<String> = vec![
        "merge".into(),
        "--no-ff".into(),
        "-m".into(),
        message,
        branch.clone(),
    ];
    let merge_result = match run_command("git", &merge_args, Some(&context.repo_root)) {
        Ok(result) => result,
        Err(error) => {
            let _ = writeln!(standard_error, "dispatch merge: {error}");
            return 1;
        }
    };

    if merge_result.code != 0 {
        // Conflict or any other failure: abort so the tree is left clean. This is
        // best-effort — if there is nothing to abort, git reports it and we still
        // surface the original merge failure as the reason.
        let abort = run_command(
            "git",
            &["merge".to_string(), "--abort".to_string()],
            Some(&context.repo_root),
        );
        let conflict_detail = String::from_utf8_lossy(&merge_result.stdout)
            .lines()
            .chain(String::from_utf8_lossy(&merge_result.stderr).lines())
            .find(|line| line.to_lowercase().contains("conflict"))
            .unwrap_or("merge failed")
            .trim()
            .to_string();
        let aborted = matches!(abort, Ok(ref result) if result.code == 0);
        let _ = writeln!(
            standard_error,
            "dispatch merge: worker {id} did NOT merge ({conflict_detail}). Merge {}; worker left `{STATE_COMPLETE}`.",
            if aborted {
                "aborted, working tree restored clean"
            } else {
                "abort attempted"
            }
        );
        if flags.bool_value("json") {
            let _ = emit_json(
                standard_output,
                standard_error,
                &serde_json::json!({
                    "merged": false,
                    "id": id,
                    "state": STATE_COMPLETE,
                    "aborted": aborted,
                    "reason": conflict_detail,
                }),
            );
        }
        return 1;
    }

    set_field(&mut record, "state", STATE_MERGED);
    if let Err(error) = context.store.write_record(&id, &record) {
        let _ = writeln!(standard_error, "dispatch merge: {error}");
        return 1;
    }
    if flags.bool_value("json") {
        emit_json(
            standard_output,
            standard_error,
            &serde_json::json!({"merged": true, "id": id, "branch": branch, "state": STATE_MERGED}),
        )
    } else {
        let _ = writeln!(
            standard_output,
            "dispatch merge: worker {id} merged (branch {branch})"
        );
        let _ = writeln!(
            standard_output,
            "  reclaim its worktree with: claude-skills dispatch abandon --id {id} --confirm"
        );
        0
    }
}

fn run_abandon(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flags = FlagSet::new("dispatch abandon");
    flags.string_flag("id", "");
    flags.string_flag("workspace-root", "");
    flags.string_flag("claude-home", "");
    flags.string_flag("worktree-root", "");
    flags.bool_flag("confirm", false);
    flags.bool_flag("json", false);
    if let Err(error) = flags.parse(arguments) {
        let _ = writeln!(standard_error, "dispatch abandon: {}", error.message);
        return 1;
    }
    let id = flags.string_value("id").trim().to_string();
    if id.is_empty() {
        let _ = writeln!(standard_error, "dispatch abandon: --id required");
        return 1;
    }
    let Some(context) = resolve_context(
        flags.string_value("workspace-root"),
        flags.string_value("claude-home"),
        flags.string_value("worktree-root"),
        "dispatch abandon",
        standard_error,
    ) else {
        return 1;
    };
    let mut record = match read_worker(&context.store, &id, "dispatch abandon", standard_error) {
        Some(record) => record,
        None => return 1,
    };
    // Removing a worktree may discard uncommitted work in it; require --confirm.
    if !flags.bool_value("confirm") {
        let _ = writeln!(
            standard_error,
            "dispatch abandon: this removes worker {id}'s worktree and deletes its branch (uncommitted work in the worktree is lost). Re-run with --confirm."
        );
        return 1;
    }
    let worktree = field(&record, "worktree").unwrap_or("").to_string();
    let branch = field(&record, "branch").unwrap_or("").to_string();
    let state = field(&record, "state").unwrap_or(STATE_PENDING).to_string();

    // Best-effort cleanup: report failures but still mark the worker abandoned so
    // the board does not wedge on a half-removed worktree.
    if !worktree.is_empty() {
        if let Ok(result) = run_command(
            "git",
            &[
                "worktree".to_string(),
                "remove".to_string(),
                "--force".to_string(),
                worktree.clone(),
            ],
            Some(&context.repo_root),
        ) {
            if result.code != 0 {
                let _ = writeln!(
                    standard_error,
                    "dispatch abandon: warning: `git worktree remove` failed: {}",
                    String::from_utf8_lossy(&result.stderr).trim()
                );
            }
        }
    }
    // Delete the branch unless it was merged (a merged branch may have been kept
    // intentionally; -D is force, so only attempt for non-merged discards).
    if !branch.is_empty() && state != STATE_MERGED {
        let _ = run_command(
            "git",
            &["branch".to_string(), "-D".to_string(), branch.clone()],
            Some(&context.repo_root),
        );
    }

    set_field(&mut record, "state", STATE_ABANDONED);
    if let Err(error) = context.store.write_record(&id, &record) {
        let _ = writeln!(standard_error, "dispatch abandon: {error}");
        return 1;
    }
    if flags.bool_value("json") {
        emit_json(
            standard_output,
            standard_error,
            &serde_json::json!({"abandoned": true, "id": id, "state": STATE_ABANDONED}),
        )
    } else {
        let _ = writeln!(
            standard_output,
            "dispatch abandon: worker {id} abandoned (worktree removed)"
        );
        0
    }
}

fn run_status(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flags = FlagSet::new("dispatch status");
    flags.string_flag("workspace-root", "");
    flags.string_flag("claude-home", "");
    flags.string_flag("worktree-root", "");
    flags.bool_flag("json", false);
    if let Err(error) = flags.parse(arguments) {
        let _ = writeln!(standard_error, "dispatch status: {}", error.message);
        return 1;
    }
    let Some(context) = resolve_context(
        flags.string_value("workspace-root"),
        flags.string_value("claude-home"),
        flags.string_value("worktree-root"),
        "dispatch status",
        standard_error,
    ) else {
        return 1;
    };
    let workers = match context.store.list_records() {
        Ok(records) => records,
        Err(error) => {
            let _ = writeln!(standard_error, "dispatch status: {error}");
            return 1;
        }
    };
    let total = workers.len();
    let count_in = |wanted: &str| -> usize {
        workers
            .iter()
            .filter(|(_, record)| field(record, "state") == Some(wanted))
            .count()
    };
    let merged = count_in(STATE_MERGED);

    if flags.bool_value("json") {
        let items: Vec<serde_json::Value> = workers
            .iter()
            .map(|(id, record)| {
                serde_json::json!({
                    "id": id,
                    "state": field(record, "state").unwrap_or(STATE_PENDING),
                    "task": field(record, "task").unwrap_or(""),
                    "branch": field(record, "branch").unwrap_or(""),
                    "worktree": field(record, "worktree").unwrap_or(""),
                    "note": field(record, "note").unwrap_or(""),
                })
            })
            .collect();
        return emit_json(
            standard_output,
            standard_error,
            &serde_json::json!({
                "total": total,
                "merged": merged,
                "pending": count_in(STATE_PENDING),
                "running": count_in(STATE_RUNNING),
                "complete": count_in(STATE_COMPLETE),
                "abandoned": count_in(STATE_ABANDONED),
                "allMerged": total > 0 && merged + count_in(STATE_ABANDONED) == total,
                "workers": items,
            }),
        );
    }

    if total == 0 {
        let _ = writeln!(
            standard_output,
            "dispatch status: no workers (register one with `dispatch plan --task ...`)"
        );
        return 0;
    }
    let _ = writeln!(
        standard_output,
        "dispatch status: {total} worker(s) — {} pending, {} running, {} complete, {merged} merged, {} abandoned",
        count_in(STATE_PENDING),
        count_in(STATE_RUNNING),
        count_in(STATE_COMPLETE),
        count_in(STATE_ABANDONED),
    );
    for (id, record) in &workers {
        let _ = writeln!(
            standard_output,
            "  [{}] {} :: {}",
            field(record, "state").unwrap_or(STATE_PENDING),
            id,
            field(record, "task").unwrap_or(""),
        );
    }
    0
}

/// Read a worker record by id, reporting a clear error when it is missing or the
/// store read fails. Returns `None` (and writes to `standard_error`) on either.
fn read_worker(
    store: &RecordStore,
    id: &str,
    label: &str,
    standard_error: &mut dyn Write,
) -> Option<Record> {
    match store.read_record(id) {
        Ok(Some(record)) => Some(record),
        Ok(None) => {
            let _ = writeln!(standard_error, "{label}: no worker with id {id}");
            None
        }
        Err(error) => {
            let _ = writeln!(standard_error, "{label}: {error}");
            None
        }
    }
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
            let _ = writeln!(standard_error, "dispatch: render json: {error}");
            1
        }
    }
}

/// `STATES` documents the full lifecycle for readers and keeps the constants
/// referenced; the verbs validate against the individual `STATE_*` constants
/// rather than scanning this slice, so assert the membership invariant here.
#[cfg(test)]
fn state_is_known(state: &str) -> bool {
    STATES.contains(&state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// A test fixture: an isolated git repo (the workspace), an isolated claude
    /// home (the store), and an isolated worktree root — so dispatch operations
    /// are fully deterministic and leave nothing behind.
    struct Fixture {
        repo: PathBuf,
        home: PathBuf,
        worktrees: PathBuf,
    }

    impl Fixture {
        fn repo_arg(&self) -> String {
            self.repo.to_string_lossy().to_string()
        }
        fn home_arg(&self) -> String {
            self.home.to_string_lossy().to_string()
        }
        fn worktrees_arg(&self) -> String {
            self.worktrees.to_string_lossy().to_string()
        }
        /// Common flags every verb needs to be isolated to this fixture.
        fn common(&self) -> Vec<String> {
            vec![
                "--workspace-root".into(),
                self.repo_arg(),
                "--claude-home".into(),
                self.home_arg(),
                "--worktree-root".into(),
                self.worktrees_arg(),
            ]
        }
    }

    fn unique(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!(
            "claude-skills-dispatch-{label}-{}-{nanos}",
            std::process::id()
        ))
    }

    fn git_in(dir: &Path, args: &[&str]) -> (i32, String) {
        let owned: Vec<String> = args.iter().map(|a| a.to_string()).collect();
        let result = run_command("git", &owned, Some(dir)).expect("git runs");
        (
            result.code,
            String::from_utf8_lossy(&result.stdout).to_string(),
        )
    }

    /// Initialize a repo with one committed tracked file so worktrees branch off
    /// a real commit. Returns the fixture.
    fn fixture(label: &str) -> Fixture {
        let base = unique(label);
        let repo = base.join("repo");
        let home = base.join("home");
        let worktrees = base.join("wt");
        fs::create_dir_all(&repo).expect("create repo");
        fs::create_dir_all(&home).expect("create home");
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec!["config", "user.email", "t@example.com"],
            vec!["config", "user.name", "t"],
        ] {
            git_in(&repo, &args);
        }
        fs::write(repo.join("file.txt"), "base\n").expect("seed file");
        git_in(&repo, &["add", "file.txt"]);
        git_in(&repo, &["commit", "-q", "-m", "base"]);
        Fixture {
            repo,
            home,
            worktrees,
        }
    }

    fn run(fixture_flags: &[String], verb: &[&str]) -> (u8, String, String) {
        let mut args: Vec<String> = verb.iter().map(|a| a.to_string()).collect();
        args.extend(fixture_flags.iter().cloned());
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run_dispatch_command(&args, &mut out, &mut err);
        (
            code,
            String::from_utf8_lossy(&out).to_string(),
            String::from_utf8_lossy(&err).to_string(),
        )
    }

    /// Commit a change to `file.txt` inside worker `id`'s worktree, simulating the
    /// agent doing work in its isolated checkout.
    fn commit_in_worktree(fixture: &Fixture, id: &str, contents: &str, message: &str) {
        let worktree = fixture.worktrees.join(id);
        fs::write(worktree.join("file.txt"), contents).expect("write in worktree");
        git_in(&worktree, &["add", "file.txt"]);
        git_in(&worktree, &["commit", "-q", "-m", message]);
    }

    fn cleanup(fixture: &Fixture) {
        // Both repo and home live under the same unique base dir.
        if let Some(base) = fixture.repo.parent() {
            let _ = fs::remove_dir_all(base);
        }
    }

    #[test]
    fn all_state_constants_are_known() {
        for state in [
            STATE_PENDING,
            STATE_RUNNING,
            STATE_COMPLETE,
            STATE_MERGED,
            STATE_ABANDONED,
        ] {
            assert!(state_is_known(state), "{state} must be in STATES");
        }
        assert!(!state_is_known("bogus"));
    }

    #[test]
    fn plan_then_status_lists_worker_as_pending() {
        let fixture = fixture("plan");
        let (code, out, err) = run(&fixture.common(), &["plan", "--task", "do a thing"]);
        assert_eq!(code, 0, "plan stderr: {err}");
        assert!(out.contains("[pending]"), "plan output: {out}");

        let (code, out, _) = run(&fixture.common(), &["status"]);
        assert_eq!(code, 0);
        assert!(out.contains("1 worker(s)"), "status: {out}");
        assert!(out.contains("[pending]"), "status: {out}");
        cleanup(&fixture);
    }

    #[test]
    fn start_creates_a_real_worktree_and_marks_running() {
        let fixture = fixture("start");
        run(&fixture.common(), &["plan", "--task", "isolated work"]);
        let (code, out, err) = run(&fixture.common(), &["start", "--id", "w1"]);
        assert_eq!(code, 0, "start stderr: {err}");
        assert!(out.contains("running"), "start output: {out}");

        // The worktree is a real directory with a checkout of the tracked file.
        let worktree = fixture.worktrees.join("w1");
        assert!(
            worktree.join("file.txt").is_file(),
            "worktree checkout exists"
        );
        // And git knows about it as a registered worktree.
        let (_, list) = git_in(&fixture.repo, &["worktree", "list"]);
        assert!(
            list.contains("w1") || list.replace('\\', "/").contains("wt/w1"),
            "git worktree list should include the new worktree: {list}"
        );
        cleanup(&fixture);
    }

    #[test]
    fn merge_refuses_a_worker_that_is_not_complete() {
        let fixture = fixture("notcomplete");
        run(&fixture.common(), &["plan", "--task", "x"]);
        run(&fixture.common(), &["start", "--id", "w1"]);
        // Worker is `running`, not `complete` — the state gate must refuse.
        let (code, _, err) = run(&fixture.common(), &["merge", "--id", "w1", "--confirm"]);
        assert_eq!(code, 1, "merge must refuse a non-complete worker");
        assert!(err.contains("not `complete`"), "err: {err}");
        cleanup(&fixture);
    }

    #[test]
    fn merge_without_confirm_refuses() {
        let fixture = fixture("noconfirm");
        run(&fixture.common(), &["plan", "--task", "x"]);
        run(&fixture.common(), &["start", "--id", "w1"]);
        commit_in_worktree(&fixture, "w1", "w1\n", "w1 work");
        run(&fixture.common(), &["complete", "--id", "w1"]);
        // Complete, but no --confirm: the mutation gate must refuse.
        let (code, _, err) = run(&fixture.common(), &["merge", "--id", "w1"]);
        assert_eq!(code, 1);
        assert!(err.contains("--confirm"), "err: {err}");
        cleanup(&fixture);
    }

    #[test]
    fn merge_applies_a_clean_worker_branch_and_marks_merged() {
        let fixture = fixture("clean");
        run(&fixture.common(), &["plan", "--task", "clean change"]);
        run(&fixture.common(), &["start", "--id", "w1"]);
        commit_in_worktree(&fixture, "w1", "from worker one\n", "w1 work");
        run(&fixture.common(), &["complete", "--id", "w1"]);

        let (code, out, err) = run(&fixture.common(), &["merge", "--id", "w1", "--confirm"]);
        assert_eq!(code, 0, "clean merge should succeed; stderr: {err}");
        assert!(out.contains("merged"), "merge output: {out}");

        // The change is now in the main repo's working tree.
        let merged = fs::read_to_string(fixture.repo.join("file.txt")).expect("read merged");
        assert_eq!(merged.trim_end(), "from worker one");

        // And the worker's recorded state is `merged`.
        let (_, status, _) = run(&fixture.common(), &["status", "--json"]);
        assert!(
            status.contains("\"state\": \"merged\""),
            "status json: {status}"
        );
        cleanup(&fixture);
    }

    #[test]
    fn merge_aborts_on_conflict_and_leaves_the_tree_clean() {
        // The crown-jewel fail-closed test: two workers edit the same line off the
        // same base. The first merges cleanly; the second conflicts and MUST be
        // aborted, leaving the tree clean and the worker still `complete`.
        let fixture = fixture("conflict");
        run(&fixture.common(), &["plan", "--task", "alpha"]); // w1
        run(&fixture.common(), &["plan", "--task", "beta"]); // w2
        run(&fixture.common(), &["start", "--id", "w1"]);
        run(&fixture.common(), &["start", "--id", "w2"]);
        // Both branch off the same base commit, both rewrite the same single line.
        commit_in_worktree(&fixture, "w1", "ALPHA\n", "w1 edits the line");
        commit_in_worktree(&fixture, "w2", "BETA\n", "w2 edits the same line");
        run(&fixture.common(), &["complete", "--id", "w1"]);
        run(&fixture.common(), &["complete", "--id", "w2"]);

        // First merge is clean.
        let (code, _, err) = run(&fixture.common(), &["merge", "--id", "w1", "--confirm"]);
        assert_eq!(code, 0, "first merge should be clean; stderr: {err}");
        assert_eq!(
            fs::read_to_string(fixture.repo.join("file.txt"))
                .unwrap()
                .trim_end(),
            "ALPHA"
        );

        // Second merge conflicts -> must fail-closed: abort + clean tree.
        let (code, _out, err) = run(&fixture.common(), &["merge", "--id", "w2", "--confirm"]);
        assert_eq!(code, 1, "conflicting merge must fail");
        assert!(
            err.to_lowercase().contains("conflict") || err.contains("did NOT merge"),
            "merge should report the conflict: {err}"
        );

        // The working tree is clean: no conflict markers, no merge in progress.
        let contents = fs::read_to_string(fixture.repo.join("file.txt")).unwrap();
        assert!(
            !contents.contains("<<<<<<<") && !contents.contains(">>>>>>>"),
            "no conflict markers should remain: {contents:?}"
        );
        assert_eq!(
            contents.trim_end(),
            "ALPHA",
            "tree restored to the first (clean) merge result"
        );
        let (_, porcelain) = git_in(&fixture.repo, &["status", "--porcelain"]);
        assert!(
            porcelain.trim().is_empty(),
            "working tree must be clean after abort: {porcelain:?}"
        );
        // MERGE_HEAD must be gone (no merge in progress).
        assert!(
            !fixture.repo.join(".git").join("MERGE_HEAD").exists(),
            "no merge should be in progress after abort"
        );

        // The conflicting worker stays `complete` (not silently merged).
        let (_, status, _) = run(&fixture.common(), &["status", "--json"]);
        assert!(
            status.contains("\"id\": \"w2\""),
            "w2 present in status: {status}"
        );
        cleanup(&fixture);
    }

    #[test]
    fn abandon_removes_the_worktree() {
        let fixture = fixture("abandon");
        run(&fixture.common(), &["plan", "--task", "x"]);
        run(&fixture.common(), &["start", "--id", "w1"]);
        let worktree = fixture.worktrees.join("w1");
        assert!(worktree.exists(), "worktree exists before abandon");

        let (code, out, err) = run(&fixture.common(), &["abandon", "--id", "w1", "--confirm"]);
        assert_eq!(code, 0, "abandon stderr: {err}");
        assert!(out.contains("abandoned"), "abandon output: {out}");
        assert!(!worktree.exists(), "worktree removed after abandon");
        cleanup(&fixture);
    }

    #[test]
    fn abandon_without_confirm_refuses() {
        let fixture = fixture("abandon-noconfirm");
        run(&fixture.common(), &["plan", "--task", "x"]);
        run(&fixture.common(), &["start", "--id", "w1"]);
        let (code, _, err) = run(&fixture.common(), &["abandon", "--id", "w1"]);
        assert_eq!(code, 1);
        assert!(err.contains("--confirm"), "err: {err}");
        // Worktree must still be there since we refused.
        assert!(fixture.worktrees.join("w1").exists());
        cleanup(&fixture);
    }

    #[test]
    fn unknown_id_fails_clearly() {
        let fixture = fixture("badid");
        let (code, _, err) = run(&fixture.common(), &["complete", "--id", "nope"]);
        assert_eq!(code, 1);
        assert!(err.contains("no worker with id"), "err: {err}");
        cleanup(&fixture);
    }

    #[test]
    fn unknown_subcommand_errors() {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run_dispatch_command(&["frobnicate".to_string()], &mut out, &mut err);
        assert_eq!(code, 1);
        assert!(
            String::from_utf8_lossy(&err).contains("unknown subcommand"),
            "stderr: {}",
            String::from_utf8_lossy(&err)
        );
    }
}
