//! Purpose: Rust-native review, hosted-review artifact, and git-workflow text helpers.
//! Caller: commands.rs for `review` and `git-workflow` command groups.
//! Dependencies: args, json, runtime helpers, std::fs, std::io, and std::path.
//! Main Functions: run_review_command, run_git_workflow_command.
//! Side Effects: Reads git diffs, writes optional hosted-review payload/body artifacts, and writes rendered text to stdout/stderr.

use std::fs;
use std::io::Write;
use std::path::Path;
use std::thread::sleep;
use std::time::{Duration, Instant};

use crate::args::FlagSet;
use crate::json::{write_indented, Value};
use crate::runtime::{resolve_claude_home, resolve_repository_root, run_command, write_text};
use crate::utility::record_store::RecordStore;

/// Whether a review invocation counts as a reviewer pass that clears the
/// PostToolBatch review gate.
///
/// why: only a PASSING `gates` / `pre-pr` / `pre-commit` is a real reviewer
/// pass. A non-zero exit (blocking findings) must not satisfy the gate, or
/// run-and-ignore clears it; and the informational `diff` / `init` surfaces
/// review nothing, so they never clear it.
fn review_pass_clears_gate(surface: &str, code: u8) -> bool {
    code == 0 && matches!(surface, "gates" | "pre-pr" | "pre-commit")
}

pub fn run_review_command(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    if arguments.is_empty() || is_help_argument(&arguments[0]) {
        render_review_help(standard_output);
        return if arguments.is_empty() { 1 } else { 0 };
    }
    match arguments[0].as_str() {
        "gates" => {
            let code = run_review_gates_command(&arguments[1..], standard_output, standard_error);
            if review_pass_clears_gate("gates", code) {
                crate::runner::hook_lifecycle::record_review_gate_clear();
            }
            code
        }
        "hosted" => run_review_hosted_command(&arguments[1..], standard_output, standard_error),
        "pre-pr" | "pre-commit" | "diff" | "init" => {
            let code = run_review_surface_command(
                arguments[0].as_str(),
                &arguments[1..],
                standard_output,
                standard_error,
            );
            if review_pass_clears_gate(arguments[0].as_str(), code) {
                crate::runner::hook_lifecycle::record_review_gate_clear();
            }
            code
        }
        "policy" => run_review_policy_command(&arguments[1..], standard_output, standard_error),
        "comments" => run_review_comments_command(&arguments[1..], standard_output, standard_error),
        other => {
            let _ = writeln!(standard_error, "Unknown review command: {other}");
            render_review_help(standard_output);
            1
        }
    }
}

pub fn run_git_workflow_command(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    if arguments.is_empty() || is_help_argument(&arguments[0]) {
        render_git_workflow_help(standard_output);
        return if arguments.is_empty() { 1 } else { 0 };
    }
    match arguments[0].as_str() {
        "commit-message" => {
            render_generated_message("commit", &arguments[1..], standard_output, standard_error)
        }
        "pr-body" => {
            render_generated_message("pr", &arguments[1..], standard_output, standard_error)
        }
        "lint-message" => lint_message(&arguments[1..], standard_output, standard_error),
        "preflight" => run_git_workflow_preflight(&arguments[1..], standard_output, standard_error),
        "await-ci" => run_git_workflow_await_ci(&arguments[1..], standard_output, standard_error),
        "configure" => run_git_workflow_configure(&arguments[1..], standard_output, standard_error),
        "show" => run_git_workflow_show(&arguments[1..], standard_output, standard_error),
        other => {
            let _ = writeln!(standard_error, "Unknown git-workflow command: {other}");
            render_git_workflow_help(standard_output);
            1
        }
    }
}

/// Preferred work-branch prefix going forward (`task/<task>` and
/// `task/<task>/<subtask>`). Integration stays `feat` (a bare branch name);
/// work must NOT use `feat/` because Git cannot store both `refs/heads/feat`
/// and `refs/heads/feat/...` at once.
const PREFERRED_BRANCH_PREFIXES: &[&str] = &["task/"];

/// Legacy work-branch prefixes still accepted so in-flight branches keep
/// working. Preflight warns (does not block) and asks for `task/` going forward.
const LEGACY_BRANCH_PREFIXES: &[&str] = &[
    "add/",
    "config/",
    "refactor/",
    "wip/",
    "fix/",
    "docs/",
    "feature/",
];

/// All prefixes preflight will allow (preferred + legacy). Unknown prefixes block.
const SANCTIONED_BRANCH_PREFIXES: &[&str] = &[
    "task/",
    "add/",
    "config/",
    "refactor/",
    "wip/",
    "fix/",
    "docs/",
    "feature/",
];

/// Default hierarchy text for configure/show.
const DEFAULT_BRANCH_TIERS: &str = "main <- dev <- feat <- task/<task> [<- task/<task>/<subtask>]";

/// Conventional commit-subject prefixes the preflight expects; a subject that
/// matches none of these (and fails the keel colon form) earns a non-blocking
/// drift warning. Includes lowercase category tokens used by both the new
/// `Add : FEATURE : info` form and legacy `add: FEATURE: info`.
const SANCTIONED_COMMIT_PREFIXES: &[&str] = &[
    "feat", "fix", "docs", "chore", "refactor", "test", "perf", "build", "ci", "style", "revert",
    "improve", "add", "config", "wip",
];

/// Run the native Git workflow preflight described in WORKFLOW.md: block on
/// branch naming, dirty worktrees, empty diffs, and missing committed history
/// against the target base ref; warn on commit-subject prefix drift. Returns 0
/// when no blocking check fails, 1 otherwise. Warnings never change the exit
/// code. A non-git directory or unreadable git state is a blocking failure —
/// preflight cannot vouch for what it cannot inspect.
fn run_git_workflow_preflight(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flag_set = FlagSet::new("git-workflow preflight");
    flag_set.string_flag("repo-root", "");
    flag_set.string_flag("base-ref", "");
    flag_set.string_flag("format", "compact");
    if let Err(parse_error) = flag_set.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 1;
    }
    let repository_root = match resolve_repository_root(flag_set.string_value("repo-root")) {
        Ok(path) => path,
        Err(error) => {
            let _ = writeln!(standard_error, "git-workflow preflight: {error}");
            return 1;
        }
    };
    let repo = Some(repository_root.as_path());
    let base_ref_raw = flag_set.string_value("base-ref").trim().to_string();
    let base_ref = if base_ref_raw.is_empty() {
        "origin/main".to_string()
    } else {
        base_ref_raw
    };

    let mut blocking: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();

    // 0. Confirm we are inside a work tree at all — otherwise every check below
    //    is meaningless and a green result would be a lie.
    match git_text(repo, &["rev-parse", "--is-inside-work-tree"]) {
        Some(value) if value.trim() == "true" => {}
        _ => {
            let _ = writeln!(
                standard_error,
                "git-workflow preflight: {} is not a git work tree",
                repository_root.display()
            );
            return 1;
        }
    }

    // 1. Branch naming. Hierarchy: main ← dev ← feat ← task/<task>
    //    [← task/<task>/<subtask>]. Hands-on work lives under task/ (preferred)
    //    or a legacy prefix (warn). main/master never receive direct work.
    //    Bare `feat`/`dev` are promotion tiers only.
    let branch = git_text(repo, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap_or_default();
    let branch = branch.trim().to_string();
    if branch.is_empty() || branch == "HEAD" {
        blocking.push("detached HEAD — check out a work branch before preflight".to_string());
    } else if matches!(branch.as_str(), "main" | "master") {
        blocking.push(format!(
            "on final-stable branch '{branch}' — never push from it directly; create a task/<task> work branch (e.g. task/rgb-sync)"
        ));
    } else if matches!(branch.as_str(), "dev" | "feat") {
        // Integration tiers: valid to stand on only when promoting upward
        // (feat→dev, dev→main). Allowed through, but flagged so an accidental
        // direct commit to a tier is visible rather than silent.
        warnings.push(format!(
            "on integration tier '{branch}' — only valid when promoting upward (feat→dev→main), not for hands-on work"
        ));
    } else if PREFERRED_BRANCH_PREFIXES
        .iter()
        .any(|prefix| branch.starts_with(prefix))
    {
        // Preferred: task/<task> or task/<task>/<subtask>.
    } else if LEGACY_BRANCH_PREFIXES
        .iter()
        .any(|prefix| branch.starts_with(prefix))
    {
        warnings.push(format!(
            "branch '{branch}' uses a legacy work-branch prefix — keep working; new branches should use task/<task> (or task/<task>/<subtask>)"
        ));
    } else if !SANCTIONED_BRANCH_PREFIXES
        .iter()
        .any(|prefix| branch.starts_with(prefix))
    {
        blocking.push(format!(
            "branch '{branch}' does not use a sanctioned work-branch prefix (prefer task/; legacy still allowed: {})",
            LEGACY_BRANCH_PREFIXES.join(", ")
        ));
    }

    // 2. Dirty worktree. Uncommitted changes must not leak into a push/MR.
    match git_text(repo, &["status", "--porcelain"]) {
        Some(status) if !status.trim().is_empty() => {
            let dirty = status
                .lines()
                .filter(|line| !line.trim().is_empty())
                .count();
            blocking.push(format!(
                "{dirty} uncommitted change(s) in the worktree — commit or stash before preflight"
            ));
        }
        Some(_) => {}
        None => blocking.push("could not read `git status`".to_string()),
    }

    // 3 & 4. Committed history against the base ref, and a non-empty diff. Both
    //    require the base ref to exist locally; if it does not, that itself is a
    //    blocking condition (cannot prove the branch diverges from the base).
    if git_text(repo, &["rev-parse", "--verify", "--quiet", &base_ref]).is_none() {
        blocking.push(format!(
            "base ref '{base_ref}' not found — fetch it (e.g. `git fetch origin`) or pass --base-ref"
        ));
    } else {
        let range = format!("{base_ref}..HEAD");
        let commit_count = git_text(repo, &["rev-list", "--count", &range])
            .and_then(|value| value.trim().parse::<u64>().ok())
            .unwrap_or(0);
        if commit_count == 0 {
            blocking.push(format!(
                "no commits on HEAD ahead of {base_ref} — nothing to push or review"
            ));
        }
        // `git diff --quiet <range>` exits 0 when the trees are identical and 1
        // when they differ. Commits that produce no net diff (merge-only or
        // fully reverted) are worth a warning, not a block.
        if commit_count > 0 && git_exit_code(repo, &["diff", "--quiet", &range]) == Some(0) {
            warnings.push(format!(
                "commits exist but `git diff {range}` is empty (merge-only or reverted changes)"
            ));
        }

        // Warn on commit-subject prefix drift across the range.
        if let Some(subjects) = git_lines(repo, &["log", "--format=%s", &range]) {
            for subject in subjects {
                if !commit_subject_has_sanctioned_prefix(&subject) {
                    warnings.push(format!(
                        "commit subject does not start with a conventional prefix: \"{}\"",
                        truncate_subject(&subject)
                    ));
                }
            }
        }
    }

    render_preflight_result(
        &branch,
        &base_ref,
        &blocking,
        &warnings,
        flag_set.string_value("format"),
        standard_output,
    )
}

/// Run a git command in `repo`, returning raw stdout (callers trim as needed)
/// on exit code 0, or `None` on non-zero exit or spawn failure.
fn git_text(repo: Option<&Path>, args: &[&str]) -> Option<String> {
    let owned: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
    let result = run_command("git", &owned, repo).ok()?;
    if result.code != 0 {
        return None;
    }
    Some(String::from_utf8_lossy(&result.stdout).to_string())
}

/// Run a git command and return its exit code (or None on spawn failure).
fn git_exit_code(repo: Option<&Path>, args: &[&str]) -> Option<i32> {
    let owned: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
    run_command("git", &owned, repo)
        .ok()
        .map(|result| result.code)
}

/// Run a git command and split stdout into non-empty trimmed lines.
fn git_lines(repo: Option<&Path>, args: &[&str]) -> Option<Vec<String>> {
    git_text(repo, args).map(|text| {
        text.lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect()
    })
}

fn commit_subject_has_sanctioned_prefix(subject: &str) -> bool {
    // Prefer the keel colon form (new or legacy spacing/casing).
    if validate_commit_subject(subject).is_ok() {
        return true;
    }
    let lowered = subject.trim_start().to_ascii_lowercase();
    SANCTIONED_COMMIT_PREFIXES.iter().any(|prefix| {
        // Match "feat:", "feat(scope):", "feat!:", and spaced "add :" shapes.
        lowered.starts_with(&format!("{prefix}:"))
            || lowered.starts_with(&format!("{prefix} :"))
            || lowered.starts_with(&format!("{prefix}("))
            || lowered.starts_with(&format!("{prefix}!"))
    })
}

fn truncate_subject(subject: &str) -> String {
    const MAX: usize = 60;
    if subject.chars().count() <= MAX {
        subject.to_string()
    } else {
        let truncated: String = subject.chars().take(MAX).collect();
        format!("{truncated}…")
    }
}

// why: the "do not merge blind" rule. Wait for the head commit's CI checks to go
// green before merging; block while any check is red or pending. No CI passes.

/// Per-check status surfaced to the merge gate.
#[derive(Debug, Clone, PartialEq, Eq)]
enum CheckState {
    Pending,
    Green,
    Red,
}

#[derive(Debug, Clone)]
struct CiCheck {
    name: String,
    state: CheckState,
}

/// Which CI provider CLI was detected for this repo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CiProvider {
    Glab,
    Gh,
}

impl CiProvider {
    fn label(self) -> &'static str {
        match self {
            CiProvider::Glab => "glab",
            CiProvider::Gh => "gh",
        }
    }
}

/// Detect the CI provider by remote URL first (authoritative), then by CLI
/// availability. A GitLab remote uses `glab`; a GitHub remote uses `gh`. When the
/// remote gives no signal, fall back to whichever CLI is installed (glab first).
/// Returns `None` when no usable provider exists; the gate then reports no-CI.
fn detect_ci_provider(repo: Option<&Path>) -> Option<CiProvider> {
    let remote = git_text(repo, &["config", "--get", "remote.origin.url"])
        .unwrap_or_default()
        .to_ascii_lowercase();
    let wants_gitlab = remote.contains("gitlab");
    let wants_github = remote.contains("github");
    if wants_gitlab && cli_available("glab") {
        return Some(CiProvider::Glab);
    }
    if wants_github && cli_available("gh") {
        return Some(CiProvider::Gh);
    }
    // Remote gave no usable signal (or its CLI is missing): fall back to
    // whichever CLI is installed, glab first per the auto-detect order.
    if cli_available("glab") {
        return Some(CiProvider::Glab);
    }
    if cli_available("gh") {
        return Some(CiProvider::Gh);
    }
    None
}

/// Whether a CLI is invocable at all (spawn succeeds). `--version` is cheap and
/// offline-safe for both `gh` and `glab`.
fn cli_available(program: &str) -> bool {
    run_command(program, &["--version".to_string()], None)
        .map(|result| result.code == 0)
        .unwrap_or(false)
}

/// Map a free-form CI status/conclusion string onto the tri-state. Anything
/// that is not an explicit success or an explicit still-running state is
/// treated as red so an unknown conclusion fails closed (never merges blind).
fn classify_check_state(raw: &str) -> CheckState {
    let value = raw.trim().to_ascii_lowercase();
    match value.as_str() {
        "success" | "passed" | "pass" | "ok" => CheckState::Green,
        "pending" | "running" | "queued" | "in_progress" | "waiting" | "requested" | "created"
        | "" => CheckState::Pending,
        _ => CheckState::Red,
    }
}

/// Tri-state result of querying a CI provider. `Error` (provider CLI failed,
/// so no signal) must block, while `Checks` with an empty vec (provider
/// reachable, genuinely zero checks) is the only honest "no CI" case.
#[derive(Debug, Clone)]
enum CiQuery {
    /// Provider ran; vec is the parsed checks (empty = genuinely no checks).
    Checks(Vec<CiCheck>),
    /// Provider CLI errored, was unavailable, or output was unparseable.
    Error,
}

/// Query the current head's checks via `glab ci status`. Output is line-based
/// (`name: status`); it is parsed loosely and requires at least one real check so
/// an empty or parse failure reads as "no CI" rather than "green".
fn query_checks_glab(repo: Option<&Path>) -> CiQuery {
    let result = match run_command(
        "glab",
        &["ci".to_string(), "status".to_string(), "--live".to_string()],
        repo,
    ) {
        Ok(result) => result,
        Err(_) => return CiQuery::Error,
    };
    if result.code != 0 {
        return CiQuery::Error;
    }
    CiQuery::Checks(parse_glab_status(&String::from_utf8_lossy(&result.stdout)).unwrap_or_default())
}

fn parse_glab_status(stdout: &str) -> Option<Vec<CiCheck>> {
    let mut checks = Vec::new();
    for line in stdout.lines() {
        let line = line.trim();
        // Typical shapes: "job-name: success" or "✓ job-name  success".
        let cleaned = line
            .trim_start_matches(|c: char| !c.is_alphanumeric() && c != '_')
            .trim();
        if cleaned.is_empty() {
            continue;
        }
        if let Some((name, status)) = cleaned.rsplit_once(':') {
            let name = name.trim();
            if name.is_empty() {
                continue;
            }
            checks.push(CiCheck {
                name: name.to_string(),
                state: classify_check_state(status),
            });
        }
    }
    if checks.is_empty() {
        None
    } else {
        Some(checks)
    }
}

/// Query the current head's checks via `gh pr checks` (PR-scoped), parsing the
/// tabular output so this gate owns the polling loop and timeout.
///
/// A non-zero exit means either "no PR for this branch" (a legitimate no-CI
/// case that may pass) or a real error (auth, network) that yields no signal
/// and must block. The two are told apart by the error text.
fn query_checks_gh(repo: Option<&Path>) -> CiQuery {
    let result = match run_command("gh", &["pr".to_string(), "checks".to_string()], repo) {
        Ok(result) => result,
        Err(_) => return CiQuery::Error, // gh failed to launch (absent/unexecutable).
    };
    if result.code != 0 {
        let detail = format!(
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        )
        .to_ascii_lowercase();
        // The honest no-CI case: gh reached the provider and found no PR.
        // Anything else (auth, network, not-a-repo) is an error, so fail closed.
        if detail.contains("no pull requests found")
            || detail.contains("no open pull request")
            || detail.contains("no pr found")
            || detail.contains("no pull request associated")
        {
            return CiQuery::Checks(Vec::new());
        }
        return CiQuery::Error;
    }
    CiQuery::Checks(parse_gh_checks(&String::from_utf8_lossy(&result.stdout)).unwrap_or_default())
}

fn parse_gh_checks(stdout: &str) -> Option<Vec<CiCheck>> {
    let mut checks = Vec::new();
    for line in stdout.lines() {
        let line = line.trim();
        if cleaned_header(line) {
            continue;
        }
        // gh pr checks columns: NAME  STATUS  ... (whitespace/tab separated).
        let mut columns = line.split_whitespace();
        let name = match columns.next() {
            Some(value) if !value.is_empty() => value,
            _ => continue,
        };
        let status = columns.next().unwrap_or("");
        checks.push(CiCheck {
            name: name.to_string(),
            state: classify_check_state(status),
        });
    }
    if checks.is_empty() {
        None
    } else {
        Some(checks)
    }
}

/// Header / blank / separator detection for `gh pr checks` tabular output.
fn cleaned_header(line: &str) -> bool {
    if line.is_empty() {
        return true;
    }
    let upper = line.to_ascii_uppercase();
    upper.starts_with("NAME")
        || upper.starts_with("CHECK")
        || line
            .chars()
            .all(|c| c == '-' || c == '+' || c.is_whitespace())
}

/// Poll the head commit's checks until green, red, or timeout. Returns the
/// process exit code. `--watch` keeps polling; without it the gate evaluates once
/// and reports. On any red check the gate blocks (exit 1) and tells the caller to
/// fix first; merge must not proceed.
fn run_git_workflow_await_ci(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flag_set = FlagSet::new("git-workflow await-ci");
    flag_set.string_flag("repo-root", "");
    flag_set.string_flag("provider", "auto");
    flag_set.string_flag("timeout-secs", "600");
    flag_set.string_flag("interval-secs", "15");
    flag_set.bool_flag("watch", false);
    flag_set.string_flag("format", "compact");
    if let Err(parse_error) = flag_set.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 1;
    }
    let repository_root = match resolve_repository_root(flag_set.string_value("repo-root")) {
        Ok(path) => path,
        Err(error) => {
            let _ = writeln!(standard_error, "git-workflow await-ci: {error}");
            return 1;
        }
    };
    let repo = Some(repository_root.as_path());
    if git_text(repo, &["rev-parse", "--is-inside-work-tree"])
        .map(|value| value.trim() != "true")
        .unwrap_or(true)
    {
        let _ = writeln!(
            standard_error,
            "git-workflow await-ci: {} is not a git work tree",
            repository_root.display()
        );
        return 1;
    }

    let provider = match resolve_provider(flag_set.string_value("provider"), repo) {
        ProviderResolution::Found(provider) => provider,
        ProviderResolution::NoneDetected => {
            // Auto-detect found no CI provider: the repo has no CI configured.
            // Not a failure. Report and pass so CI-less repos are never blocked.
            return render_await_ci_result(
                standard_output,
                flag_set.string_value("format"),
                AwaitCiOutcome::NoCi,
                "auto",
                &[],
                0,
            );
        }
        ProviderResolution::ExplicitUnavailable(requested) => {
            // The caller named a provider that is not installed. That is a
            // misconfiguration, not "no CI", so fail closed.
            return render_await_ci_result(
                standard_output,
                flag_set.string_value("format"),
                AwaitCiOutcome::Error,
                &requested,
                &[],
                0,
            );
        }
    };

    let timeout_secs = flag_set
        .string_value("timeout-secs")
        .trim()
        .parse::<u64>()
        .unwrap_or(600);
    let interval_secs = flag_set
        .string_value("interval-secs")
        .trim()
        .parse::<u64>()
        .unwrap_or(15)
        .max(2);
    let watch = flag_set.bool_value("watch");

    let started = Instant::now();
    let deadline = Duration::from_secs(timeout_secs);
    let interval = Duration::from_secs(interval_secs);
    let mut attempts = 0usize;

    loop {
        attempts += 1;
        match query_provider_checks(provider, repo) {
            CiQuery::Error => {
                // The provider CLI errored or was unavailable, so there is no
                // signal. Fail closed rather than merge blind.
                return render_await_ci_result(
                    standard_output,
                    flag_set.string_value("format"),
                    AwaitCiOutcome::Error,
                    provider.label(),
                    &[],
                    attempts,
                );
            }
            CiQuery::Checks(checks) => match evaluate_checks(&checks) {
                CiVerdict::Green => {
                    return render_await_ci_result(
                        standard_output,
                        flag_set.string_value("format"),
                        AwaitCiOutcome::Green,
                        provider.label(),
                        &checks,
                        attempts,
                    );
                }
                CiVerdict::Red => {
                    // Block: do NOT continue to merge on a red pipeline.
                    return render_await_ci_result(
                        standard_output,
                        flag_set.string_value("format"),
                        AwaitCiOutcome::Red,
                        provider.label(),
                        &checks,
                        attempts,
                    );
                }
                CiVerdict::NoChecks => {
                    // Provider reachable but genuinely reporting no checks is the
                    // only no-CI pass path; errors never reach it.
                    return render_await_ci_result(
                        standard_output,
                        flag_set.string_value("format"),
                        AwaitCiOutcome::NoCi,
                        provider.label(),
                        &[],
                        attempts,
                    );
                }
                CiVerdict::Pending => {
                    if !watch || started.elapsed() >= deadline {
                        let outcome = if watch {
                            AwaitCiOutcome::Timeout
                        } else {
                            AwaitCiOutcome::Pending
                        };
                        return render_await_ci_result(
                            standard_output,
                            flag_set.string_value("format"),
                            outcome,
                            provider.label(),
                            &checks,
                            attempts,
                        );
                    }
                    sleep(interval);
                }
            },
        }
    }
}

enum CiVerdict {
    Green,
    Red,
    Pending,
    NoChecks,
}

/// Why provider resolution failed. An explicit provider that is missing fails
/// closed; auto-detect finding no provider means the repo has no CI and may pass.
#[derive(Debug)]
enum ProviderResolution {
    Found(CiProvider),
    /// Auto-detect found no provider -> repo has no CI configured.
    NoneDetected,
    /// Caller named a provider explicitly but it is unavailable/unknown.
    ExplicitUnavailable(String),
}

fn resolve_provider(requested: &str, repo: Option<&Path>) -> ProviderResolution {
    let normalized = requested.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "auto" | "" => match detect_ci_provider(repo) {
            Some(provider) => ProviderResolution::Found(provider),
            None => ProviderResolution::NoneDetected,
        },
        "glab" if cli_available("glab") => ProviderResolution::Found(CiProvider::Glab),
        "gh" if cli_available("gh") => ProviderResolution::Found(CiProvider::Gh),
        _ => ProviderResolution::ExplicitUnavailable(normalized),
    }
}

fn query_provider_checks(provider: CiProvider, repo: Option<&Path>) -> CiQuery {
    match provider {
        CiProvider::Glab => query_checks_glab(repo),
        CiProvider::Gh => query_checks_gh(repo),
    }
}

fn evaluate_checks(checks: &[CiCheck]) -> CiVerdict {
    if checks.is_empty() {
        return CiVerdict::NoChecks;
    }
    if checks.iter().any(|check| check.state == CheckState::Red) {
        return CiVerdict::Red;
    }
    if checks
        .iter()
        .any(|check| check.state == CheckState::Pending)
    {
        return CiVerdict::Pending;
    }
    CiVerdict::Green
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AwaitCiOutcome {
    Green,
    Red,
    Pending,
    Timeout,
    NoCi,
    /// The CI provider could not be queried (CLI error / unavailable): no
    /// signal. Always blocks, never merge blind.
    Error,
}

impl AwaitCiOutcome {
    /// Exit code: only a fully green pipeline (or a repo with genuinely no CI)
    /// may proceed to merge. Red, pending, timeout, and a provider error all
    /// block; an errored/absent provider yields no signal, so it fails closed.
    fn exit_code(self) -> u8 {
        match self {
            AwaitCiOutcome::Green | AwaitCiOutcome::NoCi => 0,
            AwaitCiOutcome::Red
            | AwaitCiOutcome::Pending
            | AwaitCiOutcome::Timeout
            | AwaitCiOutcome::Error => 1,
        }
    }

    fn label(self) -> &'static str {
        match self {
            AwaitCiOutcome::Green => "GREEN — all checks passed, safe to merge",
            AwaitCiOutcome::Red => {
                "RED — fix the failing checks before merging; do NOT merge blind"
            }
            AwaitCiOutcome::Pending => "PENDING — checks still running; wait before merging",
            AwaitCiOutcome::Timeout => {
                "TIMEOUT — checks did not go green in time; do NOT merge blind"
            }
            AwaitCiOutcome::NoCi => {
                "NO CI — no CI/CD checks detected for this branch; nothing to await"
            }
            AwaitCiOutcome::Error => {
                "ERROR — could not query the CI provider (auth/network/CLI); no signal, do NOT merge blind"
            }
        }
    }
}

fn render_await_ci_result(
    standard_output: &mut dyn Write,
    output_format: &str,
    outcome: AwaitCiOutcome,
    provider: &str,
    checks: &[CiCheck],
    attempts: usize,
) -> u8 {
    if output_format == "json" {
        let payload = Value::Object(vec![
            (
                "command".into(),
                Value::String("git-workflow await-ci".into()),
            ),
            ("passed".into(), Value::Bool(outcome.exit_code() == 0)),
            ("outcome".into(), Value::String(format!("{outcome:?}"))),
            ("provider".into(), Value::String(provider.into())),
            ("attempts".into(), Value::Number(attempts.to_string())),
            (
                "checks".into(),
                Value::Array(
                    checks
                        .iter()
                        .map(|check| {
                            Value::Object(vec![
                                ("name".into(), Value::String(check.name.clone())),
                                ("state".into(), Value::String(format!("{:?}", check.state))),
                            ])
                        })
                        .collect(),
                ),
            ),
        ]);
        let _ = write_indented(standard_output, &payload);
        return outcome.exit_code();
    }
    let _ = writeln!(
        standard_output,
        "git-workflow await-ci [{}]: {}",
        provider,
        outcome.label()
    );
    for check in checks {
        let marker = match check.state {
            CheckState::Green => "ok",
            CheckState::Pending => "…",
            CheckState::Red => "FAIL",
        };
        let _ = writeln!(standard_output, "  [{marker:>4}] {}", check.name);
    }
    if outcome == AwaitCiOutcome::Red {
        let _ = writeln!(
            standard_output,
            "  fix the red checks, push, and re-run `keel git-workflow await-ci --watch` before merging"
        );
    }
    outcome.exit_code()
}

// why: persist the chosen branch+commit workflow to the global per-workspace
// memory lane so it survives sessions; this records the model, not new formats.

/// The four-tier model is the supported default; `configure` records the user's
/// choice (and notes) so `show` and later sessions recall it.
const WORKFLOW_PREF_RECORD_ID: &str = "active";

fn workflow_pref_store(repository_root: &Path, claude_home: &Path) -> RecordStore {
    let slug = workflow_slug(&repository_root.to_string_lossy());
    RecordStore::new(
        claude_home,
        &format!("memories/workspaces/{slug}/git-workflow"),
    )
}

/// Slug a workspace path into a safe directory segment (mirrors the SYSTEM_MAP
/// per-workspace lane naming).
fn workflow_slug(raw: &str) -> String {
    let mut slug = String::with_capacity(raw.len());
    let mut last_dash = false;
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            slug.push('-');
            last_dash = true;
        }
    }
    let bounded: String = slug.trim_matches('-').chars().take(64).collect();
    if bounded.is_empty() {
        "workspace".to_string()
    } else {
        bounded
    }
}

fn run_git_workflow_configure(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flag_set = FlagSet::new("git-workflow configure");
    flag_set.string_flag("repo-root", "");
    flag_set.string_flag("claude-home", "");
    flag_set.string_flag("model", "four-tier");
    flag_set.string_flag("note", "");
    flag_set.string_flag("format", "compact");
    if let Err(parse_error) = flag_set.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 1;
    }
    let repository_root = match resolve_repository_root(flag_set.string_value("repo-root")) {
        Ok(path) => path,
        Err(error) => {
            let _ = writeln!(standard_error, "git-workflow configure: {error}");
            return 1;
        }
    };
    let claude_home = match resolve_claude_home(flag_set.string_value("claude-home")) {
        Ok(path) => path,
        Err(error) => {
            let _ = writeln!(standard_error, "git-workflow configure: {error}");
            return 1;
        }
    };
    let model = flag_set.string_value("model").trim().to_string();
    if model.is_empty() {
        let _ = writeln!(
            standard_error,
            "git-workflow configure: --model must not be empty"
        );
        return 1;
    }
    let note = flag_set.string_value("note").trim().to_string();
    let store = workflow_pref_store(&repository_root, &claude_home);
    let record = vec![
        ("model".to_string(), model.clone()),
        ("note".to_string(), note.clone()),
        (
            "repoRoot".to_string(),
            repository_root.to_string_lossy().to_string(),
        ),
        ("branchTiers".to_string(), DEFAULT_BRANCH_TIERS.to_string()),
        (
            "workBranchPrefixes".to_string(),
            format!(
                "preferred: {}; legacy: {}",
                PREFERRED_BRANCH_PREFIXES.join(" "),
                LEGACY_BRANCH_PREFIXES.join(" ")
            ),
        ),
        (
            "commitPrefixes".to_string(),
            "Add|Config|Refactor|Wip|Fix|Docs : FEATURE : short info".to_string(),
        ),
    ];
    let path = match store.write_record(WORKFLOW_PREF_RECORD_ID, &record) {
        Ok(path) => path,
        Err(error) => {
            let _ = writeln!(standard_error, "git-workflow configure: {error}");
            return 1;
        }
    };
    if flag_set.string_value("format") == "json" {
        let payload = Value::Object(vec![
            (
                "command".into(),
                Value::String("git-workflow configure".into()),
            ),
            ("saved".into(), Value::Bool(true)),
            ("model".into(), Value::String(model)),
            ("note".into(), Value::String(note)),
            (
                "path".into(),
                Value::String(path.to_string_lossy().to_string()),
            ),
        ]);
        let _ = write_indented(standard_output, &payload);
    } else {
        let _ = writeln!(
            standard_output,
            "git-workflow configure: saved workflow preference (model={model})"
        );
        let _ = writeln!(standard_output, "  stored at {}", path.display());
        let _ = writeln!(
            standard_output,
            "  recall later with `keel git-workflow show`"
        );
    }
    0
}

fn run_git_workflow_show(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flag_set = FlagSet::new("git-workflow show");
    flag_set.string_flag("repo-root", "");
    flag_set.string_flag("claude-home", "");
    flag_set.string_flag("format", "compact");
    if let Err(parse_error) = flag_set.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 1;
    }
    let repository_root = match resolve_repository_root(flag_set.string_value("repo-root")) {
        Ok(path) => path,
        Err(error) => {
            let _ = writeln!(standard_error, "git-workflow show: {error}");
            return 1;
        }
    };
    let claude_home = match resolve_claude_home(flag_set.string_value("claude-home")) {
        Ok(path) => path,
        Err(error) => {
            let _ = writeln!(standard_error, "git-workflow show: {error}");
            return 1;
        }
    };
    let store = workflow_pref_store(&repository_root, &claude_home);
    let record = match store.read_record(WORKFLOW_PREF_RECORD_ID) {
        Ok(Some(record)) => record,
        Ok(None) => {
            let _ = writeln!(
                standard_output,
                "git-workflow show: no saved preference for this workspace (default model=four-tier). Run `keel git-workflow configure` to set one."
            );
            let _ = writeln!(standard_output, "  default tiers: {DEFAULT_BRANCH_TIERS}");
            let _ = writeln!(
                standard_output,
                "  work-branch prefixes: preferred {} | legacy still allowed {}",
                PREFERRED_BRANCH_PREFIXES.join(", "),
                LEGACY_BRANCH_PREFIXES.join(", ")
            );
            let _ = writeln!(standard_output, "  commit form: Add : FEATURE : short info");
            return 0;
        }
        Err(error) => {
            let _ = writeln!(standard_error, "git-workflow show: {error}");
            return 1;
        }
    };
    // Record is a Vec<(String, String)>: look up by key with a linear scan.
    let get = |key: &str| {
        record
            .iter()
            .find(|(field, _)| field == key)
            .map(|(_, value)| value.clone())
            .unwrap_or_default()
    };
    if flag_set.string_value("format") == "json" {
        let payload = Value::Object(vec![
            ("command".into(), Value::String("git-workflow show".into())),
            ("model".into(), Value::String(get("model"))),
            ("note".into(), Value::String(get("note"))),
            ("branchTiers".into(), Value::String(get("branchTiers"))),
            (
                "workBranchPrefixes".into(),
                Value::String(get("workBranchPrefixes")),
            ),
            (
                "commitPrefixes".into(),
                Value::String(get("commitPrefixes")),
            ),
        ]);
        let _ = write_indented(standard_output, &payload);
    } else {
        let _ = writeln!(
            standard_output,
            "git-workflow show: saved workflow preference"
        );
        let _ = writeln!(standard_output, "  model:   {}", get("model"));
        let note = get("note");
        if !note.is_empty() {
            let _ = writeln!(standard_output, "  note:    {note}");
        }
        let _ = writeln!(standard_output, "  tiers:   {}", get("branchTiers"));
        let _ = writeln!(standard_output, "  work:    {}", get("workBranchPrefixes"));
        let _ = writeln!(standard_output, "  commits: {}", get("commitPrefixes"));
    }
    0
}

fn render_preflight_result(
    branch: &str,
    base_ref: &str,
    blocking: &[String],
    warnings: &[String],
    output_format: &str,
    standard_output: &mut dyn Write,
) -> u8 {
    let passed = blocking.is_empty();
    if output_format == "json" {
        let payload = Value::Object(vec![
            (
                "command".into(),
                Value::String("git-workflow preflight".into()),
            ),
            ("passed".into(), Value::Bool(passed)),
            ("branch".into(), Value::String(branch.into())),
            ("baseRef".into(), Value::String(base_ref.into())),
            (
                "blocking".into(),
                Value::Array(blocking.iter().map(|m| Value::String(m.clone())).collect()),
            ),
            (
                "warnings".into(),
                Value::Array(warnings.iter().map(|m| Value::String(m.clone())).collect()),
            ),
        ]);
        let _ = write_indented(standard_output, &payload);
        return if passed { 0 } else { 1 };
    }
    let _ = writeln!(
        standard_output,
        "git-workflow preflight: {} (branch={branch} base={base_ref})",
        if passed { "PASS" } else { "BLOCKED" }
    );
    for message in blocking {
        let _ = writeln!(standard_output, "  [block] {message}");
    }
    for message in warnings {
        let _ = writeln!(standard_output, "  [warn]  {message}");
    }
    if passed && warnings.is_empty() {
        let _ = writeln!(standard_output, "  all checks passed");
    }
    if passed {
        0
    } else {
        1
    }
}

fn run_review_gates_command(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    if arguments.is_empty() || is_help_argument(&arguments[0]) {
        let _ = writeln!(standard_output, "Usage: keel review gates check [flags]");
        return if arguments.is_empty() { 1 } else { 0 };
    }
    if arguments[0] != "check" {
        let _ = writeln!(
            standard_error,
            "Unknown review gates command: {}",
            arguments[0]
        );
        return 1;
    }
    let mut flag_set = review_flag_set("review gates check");
    // why: "run" preserves this surface's long-standing behavior; the old "skip"
    // default was never read, so the documented flag controlled nothing.
    flag_set.string_flag("repo-test-policy", "run");
    flag_set.bool_flag("python-checks", false);
    flag_set.bool_flag("js-checks", false);
    if let Err(parse_error) = flag_set.parse(&arguments[1..]) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 1;
    }
    let repository_root = match resolve_repository_root(flag_set.string_value("repo-root")) {
        Ok(path) => path,
        Err(error) => {
            let _ = writeln!(standard_error, "{error}");
            return 1;
        }
    };

    let mut gate_results = Vec::new();

    // Rust tests, unless the caller opts out with --repo-test-policy skip.
    let skip_repo_tests = flag_set.string_value("repo-test-policy").trim() == "skip";
    let has_rust = repository_root.join("Cargo.toml").exists();
    if has_rust && !skip_repo_tests {
        let test_result = run_command(
            "cargo",
            &["test".to_string(), "--workspace".to_string()],
            Some(&repository_root),
        );
        let test_passed = test_result.map(|r| r.code == 0).unwrap_or(false);
        gate_results.push(GateResult {
            name: "rust_tests".to_string(),
            status: if test_passed {
                GateStatus::Pass
            } else {
                GateStatus::Fail
            },
            blocking: true,
            details: if test_passed {
                Some("cargo test --workspace passed".to_string())
            } else {
                Some("cargo test --workspace failed".to_string())
            },
        });
    }

    // Language gates: root markers like .githooks; --python-checks/--js-checks force without markers.
    let force_python = flag_set.bool_value("python-checks");
    let force_js = flag_set.bool_value("js-checks");
    if force_python || has_python_project(&repository_root) {
        if force_python && !has_python_project(&repository_root) {
            // Force path: run tools when any .py exists, else report blocked.
            if has_python_files(&repository_root) {
                gate_results.push(check_black(&repository_root));
                gate_results.push(check_ruff(&repository_root));
                gate_results.push(check_mypy(&repository_root));
                gate_results.push(check_python_tests(&repository_root));
            }
        } else {
            gate_results.extend(run_python_surface_gates(&repository_root, true));
        }
        gate_results.push(check_circular_imports(&repository_root));
        gate_results.push(check_import_safety(&repository_root));
    }
    if force_js || has_js_project(&repository_root) {
        if force_js && !has_js_project(&repository_root) {
            if has_js_files(&repository_root) {
                gate_results.push(check_prettier(&repository_root));
                gate_results.push(check_eslint(&repository_root));
            }
        } else {
            gate_results.extend(run_js_surface_gates(&repository_root, true));
        }
    }
    if has_go_project(&repository_root) {
        gate_results.extend(run_go_surface_gates(&repository_root, true));
    }

    // why: without this, `gates check` yields a green verdict without the
    // owner-path evidence pre-commit and pre-pr require.
    gate_results.push(flow_check_gate(
        &repository_root,
        flag_set.string_value("base-ref"),
        flag_set.string_value("surface"),
    ));

    // E2E verification awareness (informational, non-blocking)
    if let Some(e2e_result) = check_e2e_config(&repository_root) {
        gate_results.push(e2e_result);
    }

    let (blocking_findings, warnings) = tally_gate_results(&gate_results);

    render_gate_results(
        &gate_results,
        blocking_findings,
        warnings,
        flag_set.string_value("format"),
        standard_output,
    );

    if blocking_findings > 0 {
        1
    } else {
        0
    }
}

/// Tally blocking failures and non-blocking warnings from a slice of gate results.
/// Each gate is counted at most once — blocking failures take precedence over warning status.
fn tally_gate_results(gate_results: &[GateResult]) -> (i32, i32) {
    let mut blocking_findings = 0;
    let mut warnings = 0;
    for result in gate_results {
        if result.blocking && result.status == GateStatus::Fail {
            blocking_findings += 1;
        } else if result.status == GateStatus::Warn {
            warnings += 1;
        }
    }
    (blocking_findings, warnings)
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum GateStatus {
    Pass,
    Fail,
    Warn,
    /// Reserved for gates that intentionally no-op; matched in status renderers.
    #[allow(dead_code)]
    Skipped,
    Blocked,
}

struct GateResult {
    name: String,
    status: GateStatus,
    blocking: bool,
    details: Option<String>,
}

fn has_python_files(repository_root: &Path) -> bool {
    let extensions = ["py", "pyx", "pxd"];
    check_for_extensions(repository_root, &extensions)
}

/// Python root markers (aligned with `.githooks/pre-commit`). Root only avoids monorepo false positives.
fn has_python_project(repository_root: &Path) -> bool {
    repository_root.join("pyproject.toml").exists()
        || repository_root.join("setup.py").exists()
        || repository_root.join("setup.cfg").exists()
}

fn has_js_files(repository_root: &Path) -> bool {
    let extensions = ["js", "jsx", "ts", "tsx", "css", "scss", "less"];
    check_for_extensions(repository_root, &extensions)
}

/// JS/TS project markers aligned with `.githooks/pre-commit` (root package.json only).
fn has_js_project(repository_root: &Path) -> bool {
    repository_root.join("package.json").exists()
}

/// Go project markers aligned with `.githooks/pre-commit` (root go.mod only).
fn has_go_project(repository_root: &Path) -> bool {
    repository_root.join("go.mod").exists()
}

/// C/C++ project markers aligned with `.githooks/pre-commit` (CMakeLists or root sources).
fn has_cpp_project(repository_root: &Path) -> bool {
    if repository_root.join("CMakeLists.txt").exists() {
        return true;
    }
    if let Ok(entries) = fs::read_dir(repository_root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                if matches!(ext, "c" | "cc" | "cpp" | "cxx" | "h" | "hpp" | "hxx") {
                    return true;
                }
            }
        }
    }
    false
}

fn check_for_extensions(repository_root: &Path, extensions: &[&str]) -> bool {
    let mut found = false;
    if let Ok(entries) = fs::read_dir(repository_root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if matches!(
                    name,
                    "node_modules" | "target" | ".git" | "venv" | ".venv" | "__pycache__"
                ) {
                    continue;
                }
                if check_for_extensions(&path, extensions) {
                    found = true;
                    break;
                }
            } else if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                if extensions.contains(&ext) {
                    found = true;
                    break;
                }
            }
        }
    }
    found
}

fn check_black(repository_root: &Path) -> GateResult {
    // Check if black is available
    let black_check = run_command(
        "black",
        &["--check".to_string(), ".".to_string()],
        Some(repository_root),
    );
    match black_check {
        Ok(result) => GateResult {
            name: "black".to_string(),
            status: if result.code == 0 {
                GateStatus::Pass
            } else {
                GateStatus::Fail
            },
            blocking: true,
            details: Some(if result.code == 0 {
                "black --check passed".to_string()
            } else {
                "black --check found formatting issues".to_string()
            }),
        },
        Err(_) => GateResult {
            name: "black".to_string(),
            status: GateStatus::Blocked,
            blocking: false,
            details: Some("black not found or not applicable".to_string()),
        },
    }
}

fn check_ruff(repository_root: &Path) -> GateResult {
    let ruff_check = run_command(
        "ruff",
        &["check".to_string(), ".".to_string()],
        Some(repository_root),
    );
    match ruff_check {
        Ok(result) => GateResult {
            name: "ruff".to_string(),
            status: if result.code == 0 {
                GateStatus::Pass
            } else {
                GateStatus::Fail
            },
            blocking: true,
            details: Some(if result.code == 0 {
                "ruff check passed".to_string()
            } else {
                "ruff check found issues".to_string()
            }),
        },
        Err(_) => GateResult {
            name: "ruff".to_string(),
            status: GateStatus::Blocked,
            blocking: false,
            details: Some("ruff not found or not applicable".to_string()),
        },
    }
}

fn check_mypy(repository_root: &Path) -> GateResult {
    let mypy_check = run_command("mypy", &[], Some(repository_root));
    match mypy_check {
        Ok(result) => GateResult {
            name: "mypy".to_string(),
            status: if result.code == 0 {
                GateStatus::Pass
            } else {
                GateStatus::Fail
            },
            blocking: true,
            details: Some(if result.code == 0 {
                "mypy passed".to_string()
            } else {
                "mypy found type errors".to_string()
            }),
        },
        Err(_) => GateResult {
            name: "mypy".to_string(),
            status: GateStatus::Blocked,
            blocking: false,
            details: Some("mypy not found or not applicable".to_string()),
        },
    }
}

fn check_circular_imports(repository_root: &Path) -> GateResult {
    // Detect real circular imports: build a local-module import graph and run
    // DFS cycle detection. Replaces a prior stub that iterated files with `pass`
    // and could never report a cycle.
    let check_script = r#"
import ast
import sys
from collections import defaultdict
from pathlib import Path

def find_python_files(directory):
    for path in Path(directory).rglob("*.py"):
        s = str(path)
        if "__pycache__" not in s and "venv" not in s and ".tox" not in s and "site-packages" not in s:
            yield path

def module_name_for(path):
    rel = Path(path).with_suffix("")
    parts = [p for p in rel.parts if p not in (".", "..")]
    if parts and parts[-1] == "__init__":
        parts = parts[:-1]
    return ".".join(parts)

def imports_of(path):
    try:
        with open(path, encoding="utf-8", errors="replace") as f:
            tree = ast.parse(f.read(), filename=str(path))
    except Exception:
        return []
    names = []
    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            for alias in node.names:
                names.append(alias.name)
        elif isinstance(node, ast.ImportFrom):
            if node.module:
                names.append(node.module)
    return names

files = list(find_python_files("."))

# Pass 1: collect local module names so we only track local edges.
local_modules = set()
for pyfile in files:
    mod = module_name_for(pyfile)
    if mod:
        local_modules.add(mod)

# Pass 2: build graph of local-module -> local-module edges.
graph = defaultdict(set)
for pyfile in files:
    mod = module_name_for(pyfile)
    if not mod:
        continue
    for imp in imports_of(pyfile):
        top = imp.split(".")[0]
        target = imp if imp in local_modules else (top if top in local_modules else None)
        if target and target != mod:
            graph[mod].add(target)

# DFS cycle detection with a recursion stack (GRAY = on current path).
WHITE, GRAY, BLACK = 0, 1, 2
color = {m: WHITE for m in local_modules}
cycles = []
sys.setrecursionlimit(10000)

def dfs(node, stack):
    color[node] = GRAY
    stack.append(node)
    for neighbor in graph.get(node, set()):
        c = color.get(neighbor, WHITE)
        if c == GRAY:
            if neighbor in stack:
                idx = stack.index(neighbor)
                cycles.append(stack[idx:] + [neighbor])
        elif c == WHITE:
            dfs(neighbor, stack)
    stack.pop()
    color[node] = BLACK

for mod in list(graph.keys()):
    if color.get(mod, WHITE) == WHITE:
        dfs(mod, [])

if cycles:
    seen = set()
    for c in cycles:
        key = tuple(sorted(set(c[:-1])))
        if key in seen:
            continue
        seen.add(key)
        print("circular import: " + " -> ".join(c))
    sys.exit(1)
sys.exit(0)
"#;
    let result = run_command(
        "python",
        &["-c".to_string(), check_script.to_string()],
        Some(repository_root),
    );
    match result {
        Ok(r) => GateResult {
            name: "circular_imports".to_string(),
            status: if r.code == 0 {
                GateStatus::Pass
            } else {
                GateStatus::Fail
            },
            blocking: false,
            details: Some(if r.code == 0 {
                "no circular imports detected".to_string()
            } else {
                "circular imports detected".to_string()
            }),
        },
        Err(_) => GateResult {
            name: "circular_imports".to_string(),
            status: GateStatus::Blocked,
            blocking: false,
            details: Some("circular import check not available".to_string()),
        },
    }
}

fn check_import_safety(repository_root: &Path) -> GateResult {
    // Scan every .py for dangerous top-level imports (eval/exec/__import__/compile)
    // and exit non-zero when any are found. Replaces a prior stub that defined
    // check_file but never called it and exited 0 unconditionally.
    let check_script = r#"
import ast
import sys
from pathlib import Path

DANGEROUS_IMPORTS = {"eval", "exec", "__import__", "compile"}

def find_python_files(directory):
    for path in Path(directory).rglob("*.py"):
        s = str(path)
        if "__pycache__" not in s and "venv" not in s and ".tox" not in s and "site-packages" not in s:
            yield path

def check_file(path):
    findings = []
    try:
        with open(path, encoding="utf-8", errors="replace") as f:
            tree = ast.parse(f.read(), filename=str(path))
    except Exception:
        return findings
    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            for alias in node.names:
                top = alias.name.split(".")[0]
                if top in DANGEROUS_IMPORTS:
                    findings.append((str(path), node.lineno, top))
        elif isinstance(node, ast.ImportFrom):
            if node.module:
                top = node.module.split(".")[0]
                if top in DANGEROUS_IMPORTS:
                    findings.append((str(path), node.lineno, top))
    return findings

all_findings = []
for pyfile in find_python_files("."):
    all_findings.extend(check_file(pyfile))

if all_findings:
    for path, line, name in all_findings[:20]:
        print(f"{path}:{line}: dangerous import '{name}'")
    sys.exit(1)
sys.exit(0)
"#;
    let result = run_command(
        "python",
        &["-c".to_string(), check_script.to_string()],
        Some(repository_root),
    );
    match result {
        Ok(r) => GateResult {
            name: "import_safety".to_string(),
            status: if r.code == 0 {
                GateStatus::Pass
            } else {
                GateStatus::Fail
            },
            blocking: false,
            details: Some(if r.code == 0 {
                "no dangerous imports detected".to_string()
            } else {
                "potential dangerous imports found".to_string()
            }),
        },
        Err(_) => GateResult {
            name: "import_safety".to_string(),
            status: GateStatus::Blocked,
            blocking: false,
            details: Some("import safety check not available".to_string()),
        },
    }
}

/// E2E config filenames to detect. When found at the repository root, the review
/// gate reports their presence as an informational (non-blocking) note so the
/// operator knows E2E verification is available.
const E2E_CONFIG_FILENAMES: &[&str] = &[
    "playwright.config.ts",
    "playwright.config.js",
    "playwright.config.mjs",
    "cypress.config.ts",
    "cypress.config.js",
];

/// Detect E2E test configuration at the repository root. Returns an
/// informational (non-blocking) `GateResult` when a known config file exists,
/// or `None` to skip silently when no E2E config is found.
fn check_e2e_config(repository_root: &Path) -> Option<GateResult> {
    for name in E2E_CONFIG_FILENAMES {
        let path = repository_root.join(name);
        if path.exists() {
            let kind = if name.starts_with("playwright") {
                "Playwright"
            } else {
                "Cypress"
            };
            let run_cmd = if kind == "Playwright" {
                "npx playwright test"
            } else {
                "npx cypress run"
            };
            return Some(GateResult {
                name: "e2e_verification".to_string(),
                status: GateStatus::Pass,
                blocking: false,
                details: Some(format!(
                    "E2E: {kind} config detected at {name}. Run `{run_cmd}` before merge."
                )),
            });
        }
    }
    None
}

fn check_prettier(repository_root: &Path) -> GateResult {
    let prettier_check = run_command(
        "npx",
        &[
            "prettier".to_string(),
            "--check".to_string(),
            ".".to_string(),
        ],
        Some(repository_root),
    );
    match prettier_check {
        Ok(result) => {
            // Try npx first, then direct prettier
            if result.code != 0 {
                let direct_check = run_command(
                    "prettier",
                    &["--check".to_string(), ".".to_string()],
                    Some(repository_root),
                );
                if let Ok(direct_result) = direct_check {
                    return GateResult {
                        name: "prettier".to_string(),
                        status: if direct_result.code == 0 {
                            GateStatus::Pass
                        } else {
                            GateStatus::Fail
                        },
                        blocking: true,
                        details: Some(if direct_result.code == 0 {
                            "prettier --check passed".to_string()
                        } else {
                            "prettier --check found formatting issues".to_string()
                        }),
                    };
                }
            }
            GateResult {
                name: "prettier".to_string(),
                status: if result.code == 0 {
                    GateStatus::Pass
                } else {
                    GateStatus::Fail
                },
                blocking: true,
                details: Some(if result.code == 0 {
                    "prettier --check passed".to_string()
                } else {
                    "prettier --check found formatting issues".to_string()
                }),
            }
        }
        Err(_) => GateResult {
            name: "prettier".to_string(),
            status: GateStatus::Blocked,
            blocking: false,
            details: Some("prettier not found or not applicable".to_string()),
        },
    }
}

fn render_gate_results(
    results: &[GateResult],
    blocking: i32,
    warnings: i32,
    format: &str,
    standard_output: &mut dyn Write,
) {
    match format {
        "json" => {
            let payload = Value::Object(vec![
                (
                    "gate".into(),
                    Value::String(if blocking > 0 { "block" } else { "pass" }.into()),
                ),
                (
                    "blockingFindings".into(),
                    Value::Number(blocking.to_string()),
                ),
                (
                    "warningFindings".into(),
                    Value::Number(warnings.to_string()),
                ),
                (
                    "gates".into(),
                    Value::Array(
                        results
                            .iter()
                            .map(|r| {
                                Value::Object(vec![
                                    ("name".into(), Value::String(r.name.clone())),
                                    (
                                        "status".into(),
                                        Value::String(
                                            match r.status {
                                                GateStatus::Pass => "pass",
                                                GateStatus::Fail => "fail",
                                                GateStatus::Warn => "warn",
                                                GateStatus::Skipped => "skipped",
                                                GateStatus::Blocked => "blocked",
                                            }
                                            .into(),
                                        ),
                                    ),
                                    ("blocking".into(), Value::Bool(r.blocking)),
                                    (
                                        "details".into(),
                                        Value::String(r.details.clone().unwrap_or_default()),
                                    ),
                                ])
                            })
                            .collect(),
                    ),
                ),
                (
                    "summary".into(),
                    Value::String(format!("{blocking} blocking findings, {warnings} warnings")),
                ),
            ]);
            let _ = write_indented(standard_output, &payload);
        }
        "markdown" => {
            let _ = writeln!(standard_output, "# Native Review Gate Results");
            let _ = writeln!(standard_output);
            let _ = writeln!(standard_output, "## Summary");
            let _ = writeln!(
                standard_output,
                "- gate: {}",
                if blocking > 0 { "FAIL" } else { "PASS" }
            );
            let _ = writeln!(standard_output, "- blocking_findings: {blocking}");
            let _ = writeln!(standard_output, "- warnings: {warnings}");
            let _ = writeln!(standard_output);
            let _ = writeln!(standard_output, "## Gate Results");
            for result in results {
                let status_icon = match result.status {
                    GateStatus::Pass => "[PASS]",
                    GateStatus::Fail => "[FAIL]",
                    GateStatus::Warn => "[WARN]",
                    GateStatus::Skipped => "[SKIP]",
                    GateStatus::Blocked => "[BLK]",
                };
                let _ = writeln!(
                    standard_output,
                    "- {} {}: {}",
                    status_icon,
                    result.name,
                    result.details.clone().unwrap_or_default()
                );
            }
        }
        _ => {
            let _ = writeln!(
                standard_output,
                "gate={} blocking={blocking} warnings={warnings}",
                if blocking > 0 { "fail" } else { "pass" }
            );
            for result in results {
                let status_str = match result.status {
                    GateStatus::Pass => "pass",
                    GateStatus::Fail => "fail",
                    GateStatus::Warn => "warn",
                    GateStatus::Skipped => "skipped",
                    GateStatus::Blocked => "blocked",
                };
                let _ = writeln!(
                    standard_output,
                    "  {}={} {}",
                    result.name, status_str, result.blocking
                );
            }
        }
    }
}

fn run_review_hosted_command(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    if arguments.is_empty() || is_help_argument(&arguments[0]) {
        let _ = writeln!(
            standard_output,
            "Usage: keel review hosted [check|comment] [flags]"
        );
        return if arguments.is_empty() { 1 } else { 0 };
    }
    let hosted_kind = arguments[0].as_str();
    if hosted_kind != "check" && hosted_kind != "comment" {
        let _ = writeln!(
            standard_error,
            "Unknown review hosted command: {hosted_kind}"
        );
        return 1;
    }
    let mut flag_set = review_flag_set(&format!("review hosted {hosted_kind}"));
    flag_set.string_flag("provider", "generic");
    flag_set.string_flag("write-payload-file", "");
    flag_set.string_flag("write-body-file", "");
    if let Err(parse_error) = flag_set.parse(&arguments[1..]) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 1;
    }
    let body = hosted_body();
    if !flag_set.string_value("write-body-file").trim().is_empty() {
        if let Err(error) = write_text(Path::new(flag_set.string_value("write-body-file")), &body) {
            let _ = writeln!(standard_error, "{error}");
            return 1;
        }
    }
    // Hosted review is not wired to a real provider API: emit an honest
    // `skipped`/`action_required` verdict instead of a false `pass`. The local
    // gate `keel review pre-pr` is the source of truth; this surface only
    // renders the report payload/body for CI consumption.
    let payload = Value::Object(vec![
        (
            "provider".into(),
            Value::String(flag_set.string_value("provider").to_string()),
        ),
        ("gate".into(), Value::String("skipped".into())),
        (
            "summary".into(),
            Value::String(
                "hosted review is not configured \
                 -- run `keel review pre-pr` for the local verdict."
                    .into(),
            ),
        ),
        ("body".into(), Value::String(body.clone())),
        ("conclusion".into(), Value::String("action_required".into())),
        (
            "title".into(),
            Value::String("the harness Native Review Report".into()),
        ),
    ]);
    if !flag_set
        .string_value("write-payload-file")
        .trim()
        .is_empty()
    {
        let mut buffer = Vec::new();
        if write_indented(&mut buffer, &payload).is_err() {
            let _ = writeln!(standard_error, "Unable to render hosted review payload");
            return 1;
        }
        if let Err(error) = fs::write(flag_set.string_value("write-payload-file"), buffer) {
            let _ = writeln!(
                standard_error,
                "write {}: {error}",
                flag_set.string_value("write-payload-file")
            );
            return 1;
        }
    }
    match flag_set.string_value("format") {
        "json" => {
            let _ = write_indented(standard_output, &payload);
        }
        "compact" => {
            let _ = writeln!(
                standard_output,
                "gate=skipped blocking=0 warnings=0 findings=0 note=hosted-review-not-configured"
            );
        }
        _ => {
            let _ = write!(standard_output, "{body}");
        }
    }
    0
}

fn run_review_surface_command(
    surface_name: &str,
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flag_set = review_flag_set(&format!("review {surface_name}"));
    if let Err(parse_error) = flag_set.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 1;
    }

    // diff and init are informational surfaces — keep the existing pass behavior.
    if surface_name == "diff" || surface_name == "init" {
        return render_gate_result("pass", 0, flag_set.string_value("format"), standard_output);
    }

    let repository_root = match resolve_repository_root(flag_set.string_value("repo-root")) {
        Ok(path) => path,
        Err(error) => {
            let _ = writeln!(standard_error, "{error}");
            return 1;
        }
    };

    let include_tests = surface_name == "pre-pr";
    // --all scans the whole tracked tree (cleanup mode) instead of only added
    // diff lines, so pre-existing slop/comments/prose are caught too.
    let scan_all = flag_set.bool_value("all");
    // Auto language gates (.githooks markers). Missing tools = non-blocking Blocked.
    let mut gate_results = run_rust_surface_gates(&repository_root, include_tests);
    gate_results.extend(run_python_surface_gates(&repository_root, include_tests));
    gate_results.extend(run_js_surface_gates(&repository_root, include_tests));
    gate_results.extend(run_go_surface_gates(&repository_root, include_tests));
    gate_results.extend(run_cpp_surface_gates(&repository_root, include_tests));
    gate_results.push(comment_style_gate(
        &repository_root,
        flag_set.string_value("base-ref"),
        surface_name,
        scan_all,
    ));
    gate_results.push(prose_style_gate(
        &repository_root,
        flag_set.string_value("base-ref"),
        surface_name,
        scan_all,
    ));
    gate_results.push(slop_gate(
        &repository_root,
        flag_set.string_value("base-ref"),
        surface_name,
        scan_all,
    ));
    gate_results.push(flow_check_gate(
        &repository_root,
        flag_set.string_value("base-ref"),
        surface_name,
    ));
    if flag_set.bool_value("impact") {
        gate_results.push(impact_gate(
            &repository_root,
            flag_set.string_value("base-ref"),
            surface_name,
        ));
    }
    if let Some(e2e_result) = check_e2e_config(&repository_root) {
        gate_results.push(e2e_result);
    }
    let (blocking_findings, warnings) = tally_gate_results(&gate_results);

    render_gate_results(
        &gate_results,
        blocking_findings,
        warnings,
        flag_set.string_value("format"),
        standard_output,
    );

    if blocking_findings > 0 {
        1
    } else {
        0
    }
}

/// Run the developer-facing Rust gate set for review surfaces.
/// pre-commit gets fmt + clippy (fast); pre-pr also runs the test suite.
/// Skipped entirely when no Cargo.toml exists at the repository root.
fn run_rust_surface_gates(repository_root: &Path, include_tests: bool) -> Vec<GateResult> {
    let mut gate_results = Vec::new();
    if !repository_root.join("Cargo.toml").exists() {
        return gate_results;
    }

    let fmt_result = run_command(
        "cargo",
        &[
            "fmt".to_string(),
            "--all".to_string(),
            "--".to_string(),
            "--check".to_string(),
        ],
        Some(repository_root),
    );
    let fmt_passed = fmt_result.map(|r| r.code == 0).unwrap_or(false);
    gate_results.push(GateResult {
        name: "cargo_fmt".to_string(),
        status: if fmt_passed {
            GateStatus::Pass
        } else {
            GateStatus::Fail
        },
        blocking: true,
        details: Some(
            if fmt_passed {
                "cargo fmt --check passed"
            } else {
                "cargo fmt --check found formatting issues"
            }
            .to_string(),
        ),
    });

    let clippy_result = run_command(
        "cargo",
        &[
            "clippy".to_string(),
            "--all-targets".to_string(),
            "--".to_string(),
            "-D".to_string(),
            "warnings".to_string(),
        ],
        Some(repository_root),
    );
    let clippy_passed = clippy_result.map(|r| r.code == 0).unwrap_or(false);
    gate_results.push(GateResult {
        name: "cargo_clippy".to_string(),
        status: if clippy_passed {
            GateStatus::Pass
        } else {
            GateStatus::Fail
        },
        blocking: true,
        details: Some(
            if clippy_passed {
                "cargo clippy --all-targets -- -D warnings passed"
            } else {
                "cargo clippy --all-targets -- -D warnings found issues"
            }
            .to_string(),
        ),
    });

    if include_tests {
        let test_result = run_command(
            "cargo",
            &["test".to_string(), "--workspace".to_string()],
            Some(repository_root),
        );
        let test_passed = test_result.map(|r| r.code == 0).unwrap_or(false);
        gate_results.push(GateResult {
            name: "cargo_test".to_string(),
            status: if test_passed {
                GateStatus::Pass
            } else {
                GateStatus::Fail
            },
            blocking: true,
            details: Some(
                if test_passed {
                    "cargo test --workspace passed"
                } else {
                    "cargo test --workspace failed"
                }
                .to_string(),
            ),
        });
    }

    gate_results
}

/// Python fmt/lint (and on pre-pr: mypy + pytest). Skipped when no Python project.
/// Tool missing → Blocked non-blocking; tool present and failing → Fail blocking.
fn run_python_surface_gates(repository_root: &Path, include_tests: bool) -> Vec<GateResult> {
    if !has_python_project(repository_root) {
        return Vec::new();
    }
    let mut gate_results = vec![check_black(repository_root), check_ruff(repository_root)];
    if include_tests {
        gate_results.push(check_mypy(repository_root));
        gate_results.push(check_python_tests(repository_root));
    }
    gate_results
}

/// JS/TS fmt/lint (and on pre-pr: tsc + npm test when present). Skipped when no JS project.
fn run_js_surface_gates(repository_root: &Path, include_tests: bool) -> Vec<GateResult> {
    if !has_js_project(repository_root) {
        return Vec::new();
    }
    let mut gate_results = vec![
        check_prettier(repository_root),
        check_eslint(repository_root),
    ];
    if include_tests {
        if repository_root.join("tsconfig.json").exists() {
            gate_results.push(check_tsc(repository_root));
        }
        gate_results.push(check_npm_test(repository_root));
    }
    gate_results
}

/// Go fmt/vet (and on pre-pr: go test). Skipped when no go.mod / .go sources.
fn run_go_surface_gates(repository_root: &Path, include_tests: bool) -> Vec<GateResult> {
    if !has_go_project(repository_root) {
        return Vec::new();
    }
    let mut gate_results = vec![check_gofmt(repository_root), check_go_vet(repository_root)];
    if include_tests {
        gate_results.push(check_go_test(repository_root));
    }
    gate_results
}

/// C/C++ format check via clang-format (aligned with `.githooks/pre-commit`).
/// No portable unit-test auto-runner; pre-pr still reports format gate only.
fn run_cpp_surface_gates(repository_root: &Path, _include_tests: bool) -> Vec<GateResult> {
    if !has_cpp_project(repository_root) {
        return Vec::new();
    }
    vec![check_clang_format(repository_root)]
}

fn collect_cpp_source_files(
    repository_root: &Path,
    out: &mut Vec<std::path::PathBuf>,
    depth: usize,
) {
    if depth > 4 || out.len() >= 50 {
        return;
    }
    let Ok(entries) = fs::read_dir(repository_root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if matches!(
                name,
                "node_modules" | "target" | ".git" | "build" | "dist" | "out" | "venv" | ".venv"
            ) {
                continue;
            }
            collect_cpp_source_files(&path, out, depth + 1);
        } else if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            if matches!(ext, "c" | "cc" | "cpp" | "cxx" | "h" | "hpp" | "hxx") {
                out.push(path);
                if out.len() >= 50 {
                    return;
                }
            }
        }
    }
}

fn check_clang_format(repository_root: &Path) -> GateResult {
    let mut files = Vec::new();
    collect_cpp_source_files(repository_root, &mut files, 0);
    if files.is_empty() {
        return GateResult {
            name: "clang_format".to_string(),
            status: GateStatus::Blocked,
            blocking: false,
            details: Some("no C/C++ source files found for clang-format".to_string()),
        };
    }
    // Probe clang-format availability with --version first.
    if run_command(
        "clang-format",
        &["--version".to_string()],
        Some(repository_root),
    )
    .is_err()
    {
        return GateResult {
            name: "clang_format".to_string(),
            status: GateStatus::Blocked,
            blocking: false,
            details: Some("clang-format not found or not applicable".to_string()),
        };
    }
    let mut dirty = 0usize;
    for file in &files {
        let Some(path_str) = file.to_str() else {
            continue;
        };
        let result = run_command(
            "clang-format",
            &[
                "--dry-run".to_string(),
                "--Werror".to_string(),
                path_str.to_string(),
            ],
            Some(repository_root),
        );
        match result {
            Ok(output) if output.code != 0 => dirty += 1,
            Err(_) => {
                return GateResult {
                    name: "clang_format".to_string(),
                    status: GateStatus::Blocked,
                    blocking: false,
                    details: Some("clang-format not found or not applicable".to_string()),
                };
            }
            _ => {}
        }
    }
    GateResult {
        name: "clang_format".to_string(),
        status: if dirty == 0 {
            GateStatus::Pass
        } else {
            GateStatus::Fail
        },
        blocking: true,
        details: Some(if dirty == 0 {
            format!("clang-format --dry-run clean ({} file(s))", files.len())
        } else {
            format!("clang-format found {dirty} unformatted C/C++ file(s)")
        }),
    }
}

fn check_gofmt(repository_root: &Path) -> GateResult {
    let result = run_command(
        "gofmt",
        &["-l".to_string(), ".".to_string()],
        Some(repository_root),
    );
    match result {
        Ok(output) => {
            let unformatted = String::from_utf8_lossy(&output.stdout);
            let dirty = unformatted.lines().any(|line| !line.trim().is_empty());
            GateResult {
                name: "gofmt".to_string(),
                status: if dirty {
                    GateStatus::Fail
                } else {
                    GateStatus::Pass
                },
                blocking: true,
                details: Some(if dirty {
                    format!(
                        "gofmt found unformatted files: {}",
                        unformatted.lines().take(5).collect::<Vec<_>>().join(", ")
                    )
                } else {
                    "gofmt -l . clean".to_string()
                }),
            }
        }
        Err(_) => GateResult {
            name: "gofmt".to_string(),
            status: GateStatus::Blocked,
            blocking: false,
            details: Some("gofmt not found or not applicable".to_string()),
        },
    }
}

fn check_go_vet(repository_root: &Path) -> GateResult {
    let result = run_command(
        "go",
        &["vet".to_string(), "./...".to_string()],
        Some(repository_root),
    );
    match result {
        Ok(output) => GateResult {
            name: "go_vet".to_string(),
            status: if output.code == 0 {
                GateStatus::Pass
            } else {
                GateStatus::Fail
            },
            blocking: true,
            details: Some(if output.code == 0 {
                "go vet ./... passed".to_string()
            } else {
                "go vet ./... found issues".to_string()
            }),
        },
        Err(_) => GateResult {
            name: "go_vet".to_string(),
            status: GateStatus::Blocked,
            blocking: false,
            details: Some("go not found or not applicable".to_string()),
        },
    }
}

fn check_go_test(repository_root: &Path) -> GateResult {
    let result = run_command(
        "go",
        &["test".to_string(), "./...".to_string()],
        Some(repository_root),
    );
    match result {
        Ok(output) => GateResult {
            name: "go_test".to_string(),
            status: if output.code == 0 {
                GateStatus::Pass
            } else {
                GateStatus::Fail
            },
            blocking: true,
            details: Some(if output.code == 0 {
                "go test ./... passed".to_string()
            } else {
                "go test ./... failed".to_string()
            }),
        },
        Err(_) => GateResult {
            name: "go_test".to_string(),
            status: GateStatus::Blocked,
            blocking: false,
            details: Some("go not found or not applicable".to_string()),
        },
    }
}

fn check_eslint(repository_root: &Path) -> GateResult {
    // Prefer local/npx eslint when package.json exists; skip when neither npx nor eslint work.
    let npx_result = run_command(
        "npx",
        &[
            "--no-install".to_string(),
            "eslint".to_string(),
            ".".to_string(),
        ],
        Some(repository_root),
    );
    match npx_result {
        Ok(output) if output.code == 0 => {
            return GateResult {
                name: "eslint".to_string(),
                status: GateStatus::Pass,
                blocking: true,
                details: Some("eslint passed".to_string()),
            };
        }
        Ok(output) => {
            // npx ran eslint and it failed (blocking). On not-found stderr, try direct binary.
            let stderr = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
            let not_found = stderr.contains("not found")
                || stderr.contains("could not determine")
                || stderr.contains("enoent");
            if !not_found {
                return GateResult {
                    name: "eslint".to_string(),
                    status: GateStatus::Fail,
                    blocking: true,
                    details: Some("eslint found issues".to_string()),
                };
            }
        }
        Err(_) => {}
    }
    match run_command("eslint", &[".".to_string()], Some(repository_root)) {
        Ok(output) => GateResult {
            name: "eslint".to_string(),
            status: if output.code == 0 {
                GateStatus::Pass
            } else {
                GateStatus::Fail
            },
            blocking: true,
            details: Some(if output.code == 0 {
                "eslint passed".to_string()
            } else {
                "eslint found issues".to_string()
            }),
        },
        Err(_) => GateResult {
            name: "eslint".to_string(),
            status: GateStatus::Blocked,
            blocking: false,
            details: Some("eslint not found or not applicable".to_string()),
        },
    }
}

fn check_tsc(repository_root: &Path) -> GateResult {
    let result = run_command(
        "npx",
        &[
            "--no-install".to_string(),
            "tsc".to_string(),
            "--noEmit".to_string(),
        ],
        Some(repository_root),
    );
    match result {
        Ok(output) => GateResult {
            name: "tsc".to_string(),
            status: if output.code == 0 {
                GateStatus::Pass
            } else {
                GateStatus::Fail
            },
            blocking: true,
            details: Some(if output.code == 0 {
                "tsc --noEmit passed".to_string()
            } else {
                "tsc --noEmit found type errors".to_string()
            }),
        },
        Err(_) => match run_command("tsc", &["--noEmit".to_string()], Some(repository_root)) {
            Ok(output) => GateResult {
                name: "tsc".to_string(),
                status: if output.code == 0 {
                    GateStatus::Pass
                } else {
                    GateStatus::Fail
                },
                blocking: true,
                details: Some(if output.code == 0 {
                    "tsc --noEmit passed".to_string()
                } else {
                    "tsc --noEmit found type errors".to_string()
                }),
            },
            Err(_) => GateResult {
                name: "tsc".to_string(),
                status: GateStatus::Blocked,
                blocking: false,
                details: Some("tsc not found or not applicable".to_string()),
            },
        },
    }
}

fn check_npm_test(repository_root: &Path) -> GateResult {
    if !repository_root.join("package.json").exists() {
        return GateResult {
            name: "npm_test".to_string(),
            status: GateStatus::Blocked,
            blocking: false,
            details: Some("no package.json — npm test not applicable".to_string()),
        };
    }
    // --if-present: exit 0 when no test script is defined.
    let result = run_command(
        "npm",
        &["test".to_string(), "--if-present".to_string()],
        Some(repository_root),
    );
    match result {
        Ok(output) => GateResult {
            name: "npm_test".to_string(),
            status: if output.code == 0 {
                GateStatus::Pass
            } else {
                GateStatus::Fail
            },
            blocking: true,
            details: Some(if output.code == 0 {
                "npm test --if-present passed".to_string()
            } else {
                "npm test failed".to_string()
            }),
        },
        Err(_) => GateResult {
            name: "npm_test".to_string(),
            status: GateStatus::Blocked,
            blocking: false,
            details: Some("npm not found or not applicable".to_string()),
        },
    }
}

/// Classify pytest/unittest exit codes for review closeout.
/// Exit 5 = no tests collected/ran for both pytest and unittest discover
/// (not a failure of product code; empty trees must not fail pre-pr).
fn classify_python_test_exit(tool: &str, code: i32) -> GateResult {
    if code == 0 {
        return GateResult {
            name: "python_tests".to_string(),
            status: GateStatus::Pass,
            blocking: true,
            details: Some(format!("{tool} passed")),
        };
    }
    // pytest and unittest discover both use exit 5 for "no tests".
    if code == 5 {
        return GateResult {
            name: "python_tests".to_string(),
            status: GateStatus::Blocked,
            blocking: false,
            details: Some(format!(
                "{tool} exit 5: no tests collected/ran (not applicable)"
            )),
        };
    }
    GateResult {
        name: "python_tests".to_string(),
        status: GateStatus::Fail,
        blocking: true,
        details: Some(format!("{tool} failed (exit {code})")),
    }
}

fn check_python_tests(repository_root: &Path) -> GateResult {
    // Prefer pytest; fall back to unittest discover.
    if let Ok(output) = run_command("pytest", &["-q".to_string()], Some(repository_root)) {
        return classify_python_test_exit("pytest", output.code);
    }
    match run_command(
        "python",
        &[
            "-m".to_string(),
            "unittest".to_string(),
            "discover".to_string(),
            "-q".to_string(),
        ],
        Some(repository_root),
    ) {
        Ok(output) => classify_python_test_exit("python -m unittest discover", output.code),
        Err(_) => GateResult {
            name: "python_tests".to_string(),
            status: GateStatus::Blocked,
            blocking: false,
            details: Some("pytest/unittest not found or not applicable".to_string()),
        },
    }
}

/// Build the comment-style gate result for a review surface. Lints added comment
/// lines only (existing comments grandfathered). pre-commit scans the working
/// diff against HEAD; other surfaces scan against the base ref. Blocking only
/// when a high-severity finding (over-length impl comment or em/en dash) appears.
fn comment_style_gate(
    repository_root: &Path,
    base_ref: &str,
    surface_name: &str,
    scan_all: bool,
) -> GateResult {
    let findings = if scan_all {
        crate::comment_lint::lint_tracked_tree(repository_root)
    } else if surface_name == "pre-commit" {
        crate::comment_lint::lint_working_comments(repository_root)
    } else {
        let base = base_ref.trim();
        let base = if base.is_empty() { "origin/main" } else { base };
        crate::comment_lint::lint_added_comments(repository_root, base)
    };
    let blocking = crate::comment_lint::has_blocking(&findings);
    let status = if findings.is_empty() {
        GateStatus::Pass
    } else if blocking {
        GateStatus::Fail
    } else {
        GateStatus::Warn
    };
    let details = if findings.is_empty() {
        "no added-comment style issues".to_string()
    } else {
        let shown: Vec<String> = findings
            .iter()
            .take(5)
            .map(|f| format!("{}:{} {}", f.file, f.line, f.message))
            .collect();
        format!(
            "{} added-comment issue(s): {}",
            findings.len(),
            shown.join("; ")
        )
    };
    GateResult {
        name: "comment_style".to_string(),
        status,
        blocking,
        details: Some(details),
    }
}

/// Source extensions the brownfield gate treats as established behavior. Docs,
/// config, and data files carry no ownership flow to preserve.
const FLOW_SOURCE_EXTENSIONS: &[&str] = &[
    "rs", "go", "py", "js", "jsx", "ts", "tsx", "java", "kt", "kts", "swift", "c", "h", "cc",
    "cpp", "hpp", "cs", "rb", "php", "scala", "dart", "m", "mm", "sh", "ps1", "lua", "ex", "exs",
];

/// Path segments whose contents are generated or vendored, so they are exempt.
const FLOW_EXEMPT_SEGMENTS: &[&str] = &[
    "target/",
    "node_modules/",
    "vendor/",
    "dist/",
    "build/",
    ".git/",
    "generated/",
    "__pycache__/",
];

/// Existing source files modified in the reviewed diff, from `--name-status`.
/// Only `M` and `R` count: an added file is greenfield and has no prior owner to
/// preserve, which is the documented exemption.
///
/// `None` means the range could not be resolved (git missing, not a repository,
/// unknown base ref). The caller must not treat that as "nothing changed": an
/// unresolvable range once made this blocking gate report a clean pass over nine
/// modified files.
fn modified_existing_sources(repository_root: &Path, range: &[String]) -> Option<Vec<String>> {
    let mut args = vec!["diff".to_string(), "--name-status".to_string()];
    args.extend(range.iter().cloned());
    let result = run_command("git", &args, Some(repository_root)).ok()?;
    if result.code != 0 {
        return None;
    }
    Some(
        String::from_utf8_lossy(&result.stdout)
            .lines()
            .filter_map(brownfield_source_from_name_status)
            .collect(),
    )
}

/// Classify one `git diff --name-status` line, returning the path when it is an
/// edit to established source. Split out from the git call so the exemption rules
/// (greenfield, docs, generated) are unit-testable without a repository.
fn brownfield_source_from_name_status(line: &str) -> Option<String> {
    let mut parts = line.split('\t');
    let status = parts.next()?.trim();
    let first_path = parts.next()?.trim();
    // why: a rename emits `R<score>\told\tnew`, and renaming while editing still
    // changes established behavior, so gate it against the destination path.
    let path = match status.chars().next()? {
        'M' => first_path,
        'R' => parts.next()?.trim(),
        _ => return None,
    };
    let normalized = path.replace('\\', "/");
    if FLOW_EXEMPT_SEGMENTS
        .iter()
        .any(|segment| normalized.contains(segment))
    {
        return None;
    }
    // why: case-fold the extension so a `Foo.RS` on a case-insensitive
    // filesystem cannot slip past the gate.
    let extension = normalized
        .rsplit('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if !FLOW_SOURCE_EXTENSIONS.contains(&extension.as_str()) {
        return None;
    }
    Some(normalized)
}

/// Blocking brownfield gate: modifying established source requires a complete
/// flow-check artifact recording the owner path. This is the enforcement half of
/// `preserve-existing-flow`; without it the contract was skill prose only.
/// Greenfield (added files), docs, and generated trees are exempt, and a diff
/// touching no existing source passes untouched.
fn flow_check_gate(repository_root: &Path, base_ref: &str, surface_name: &str) -> GateResult {
    let range: Vec<String> = if surface_name == "pre-commit" {
        vec!["HEAD".to_string()]
    } else {
        let base = base_ref.trim();
        let base = if base.is_empty() { "origin/main" } else { base };
        vec![format!("{base}...HEAD")]
    };

    let Some(touched) = modified_existing_sources(repository_root, &range) else {
        return GateResult {
            name: "flow_check".to_string(),
            status: GateStatus::Warn,
            blocking: false,
            details: Some(format!(
                "could not resolve the diff range ({}); brownfield evidence was NOT checked. \
                 Pass an existing --base-ref, or run the pre-commit surface.",
                range.join(" ")
            )),
        };
    };
    if touched.is_empty() {
        return GateResult {
            name: "flow_check".to_string(),
            status: GateStatus::Pass,
            blocking: true,
            details: Some(
                "no existing source modified; brownfield gate not applicable".to_string(),
            ),
        };
    }

    let artifact =
        keel_flow::resolve_artifact_path(repository_root, keel_flow::DEFAULT_ARTIFACT_PATH);
    let (errors, target_file) =
        match keel_flow::load_check(repository_root, keel_flow::DEFAULT_ARTIFACT_PATH) {
            Ok(check) => {
                let target = check.target_file.clone();
                (keel_flow::validate_check(check), target)
            }
            Err(load_error) => (vec![load_error.to_string()], String::new()),
        };

    if errors.is_empty() {
        // why: the artifact is workspace-global, so without this a single filled
        // artifact would satisfy the gate forever regardless of what changed next.
        if !artifact_targets_a_touched_file(&target_file, &touched) {
            return GateResult {
                name: "flow_check".to_string(),
                status: GateStatus::Fail,
                blocking: true,
                details: Some(format!(
                    "the flow-check artifact at {} traces {target_file:?}, which is not among the \
                     {} modified source file(s) ({}). The artifact is stale for this change. \
                     Re-run `keel flow start --target-file <path>` for what you are editing.",
                    artifact.display(),
                    touched.len(),
                    preview_touched_paths(&touched)
                )),
            };
        }
        return GateResult {
            name: "flow_check".to_string(),
            status: GateStatus::Pass,
            blocking: true,
            details: Some(format!(
                "{} existing source file(s) modified; flow-check artifact traces {target_file:?}",
                touched.len()
            )),
        };
    }

    GateResult {
        name: "flow_check".to_string(),
        status: GateStatus::Fail,
        blocking: true,
        details: Some(format!(
            "{} existing source file(s) modified ({}) but the flow-check artifact at {} is missing or incomplete: {}. \
             Run `keel flow start --target-file <path>`, fill the owner path, then `keel flow check`.",
            touched.len(),
            preview_touched_paths(&touched),
            artifact.display(),
            errors.join("; ")
        )),
    }
}

/// First few touched paths, for a gate message that stays readable on a wide diff.
fn preview_touched_paths(paths: &[String]) -> String {
    paths
        .iter()
        .take(3)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Advisory blast-radius gate: reports which in-repo files transitively import
/// the changed files. Non-blocking and fail-open; a missing or unreadable graph
/// silently skips. Uses the cached artifact when present, builds fresh otherwise.
fn impact_gate(repository_root: &Path, base_ref: &str, surface_name: &str) -> GateResult {
    let range: Vec<String> = if surface_name == "pre-commit" {
        vec!["HEAD".to_string()]
    } else {
        let base = base_ref.trim();
        let base = if base.is_empty() { "origin/main" } else { base };
        vec![format!("{base}...HEAD")]
    };

    let Some(touched) = modified_existing_sources(repository_root, &range) else {
        return GateResult {
            name: "impact".to_string(),
            status: GateStatus::Blocked,
            blocking: false,
            details: Some("could not resolve diff range".to_string()),
        };
    };
    if touched.is_empty() {
        return GateResult {
            name: "impact".to_string(),
            status: GateStatus::Pass,
            blocking: false,
            details: Some("no existing source modified".to_string()),
        };
    }

    let graph = crate::utility::code_graph::cached_artifact_path(repository_root, "")
        .and_then(|p| crate::utility::code_graph::CodeGraph::from_json_file(&p))
        .unwrap_or_else(|| crate::utility::code_graph::build_graph(repository_root));

    let impacted = graph.impact_of(&touched);
    if impacted.is_empty() {
        return GateResult {
            name: "impact".to_string(),
            status: GateStatus::Pass,
            blocking: false,
            details: Some(format!("{} changed, no in-repo dependents", touched.len())),
        };
    }
    GateResult {
        name: "impact".to_string(),
        status: GateStatus::Pass,
        blocking: false,
        details: Some(format!(
            "{} changed, {} impacted: {}",
            touched.len(),
            impacted.len(),
            preview_touched_paths(&impacted)
        )),
    }
}

/// Whether the artifact's `target_file` names one of the files under review.
///
/// Suffix matching in both directions tolerates repo-relative vs absolute paths
/// and Windows separators, so a legitimate artifact is not rejected over path
/// formatting. An empty target never matches.
fn artifact_targets_a_touched_file(target_file: &str, touched: &[String]) -> bool {
    let target = target_file
        .replace('\\', "/")
        .trim()
        .trim_start_matches("./")
        .to_ascii_lowercase();
    if target.is_empty() {
        return false;
    }
    touched.iter().any(|path| {
        let candidate = path
            .replace('\\', "/")
            .trim_start_matches("./")
            .to_ascii_lowercase();
        candidate == target
            || candidate.ends_with(&format!("/{target}"))
            || target.ends_with(&format!("/{candidate}"))
    })
}

/// Build the prose-style gate result for a review surface. Lints added lines in
/// markdown/doc files for AI-slop vocabulary, em-dash, hype, first-person, and
/// chatty wording. Pre-existing prose is grandfathered (added lines only).
/// Blocking when a high-severity finding (AI-slop or em-dash) appears.
fn prose_style_gate(
    repository_root: &Path,
    base_ref: &str,
    surface_name: &str,
    scan_all: bool,
) -> GateResult {
    let findings = if scan_all {
        crate::comment_lint::lint_tracked_tree_prose(repository_root)
    } else if surface_name == "pre-commit" {
        crate::comment_lint::lint_working_prose(repository_root)
    } else {
        let base = base_ref.trim();
        let base = if base.is_empty() { "origin/main" } else { base };
        crate::comment_lint::lint_added_prose(repository_root, base)
    };
    let blocking = crate::comment_lint::has_blocking_prose(&findings);
    let status = if findings.is_empty() {
        GateStatus::Pass
    } else if blocking {
        GateStatus::Fail
    } else {
        GateStatus::Warn
    };
    let details = if findings.is_empty() {
        "no prose-style issues in added markdown/doc lines".to_string()
    } else {
        let shown: Vec<String> = findings
            .iter()
            .take(5)
            .map(|f| format!("{}:{} {}", f.file, f.line, f.message))
            .collect();
        format!(
            "{} prose-style issue(s) in markdown/doc: {}",
            findings.len(),
            shown.join("; ")
        )
    };
    GateResult {
        name: "prose_style".to_string(),
        status,
        blocking,
        details: Some(details),
    }
}

fn slop_gate(
    repository_root: &Path,
    base_ref: &str,
    surface_name: &str,
    scan_all: bool,
) -> GateResult {
    let findings = if scan_all {
        crate::slop_detector::lint_tracked_tree_slop(repository_root)
    } else if surface_name == "pre-commit" {
        crate::slop_detector::lint_working_slop(repository_root)
    } else {
        crate::slop_detector::lint_added_slop(repository_root, base_ref)
    };
    let status = if findings.is_empty() {
        GateStatus::Pass
    } else {
        GateStatus::Warn
    };
    let details = if findings.is_empty() {
        "no AI-slop patterns detected".to_string()
    } else {
        let shown: Vec<String> = findings
            .iter()
            .take(5)
            .map(|f| format!("{}:{} [{}] {}", f.file, f.line, f.pattern, f.message))
            .collect();
        format!("{} slop finding(s): {}", findings.len(), shown.join("; "))
    };
    GateResult {
        name: "slop_detector".to_string(),
        status,
        blocking: false,
        details: Some(details),
    }
}

/// Run `keel review comments`: lint added-comment style and report findings.
/// `--all` ignores the diff and scans the whole tracked tree (for cleanup work).
fn run_review_comments_command(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flag_set = FlagSet::new("review comments");
    flag_set.string_flag("repo-root", "");
    flag_set.string_flag("base-ref", "origin/main");
    flag_set.string_flag("format", "compact");
    flag_set.bool_flag("all", false);
    if let Err(parse_error) = flag_set.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 1;
    }
    let repository_root = match resolve_repository_root(flag_set.string_value("repo-root")) {
        Ok(path) => path,
        Err(error) => {
            let _ = writeln!(standard_error, "{error}");
            return 1;
        }
    };
    let mut findings = if flag_set.bool_value("all") {
        crate::comment_lint::lint_tracked_tree(&repository_root)
    } else {
        crate::comment_lint::lint_added_comments(
            &repository_root,
            flag_set.string_value("base-ref").trim(),
        )
    };
    // Also lint added prose (markdown/doc body text) for AI-slop, unless --all
    // (whole-tree prose scan is out of scope for the comments surface).
    if !flag_set.bool_value("all") {
        findings.extend(crate::comment_lint::lint_added_prose(
            &repository_root,
            flag_set.string_value("base-ref").trim(),
        ));
    }
    let blocking = crate::comment_lint::has_blocking(&findings)
        || crate::comment_lint::has_blocking_prose(&findings);
    if flag_set.string_value("format") == "json" {
        let items: Vec<Value> = findings
            .iter()
            .map(|f| {
                Value::Object(vec![
                    ("file".into(), Value::String(f.file.clone())),
                    ("line".into(), Value::Number(f.line.to_string())),
                    ("id".into(), Value::String(f.id.clone())),
                    ("severity".into(), Value::String(f.severity.clone())),
                    ("message".into(), Value::String(f.message.clone())),
                ])
            })
            .collect();
        let payload = Value::Object(vec![
            ("command".into(), Value::String("review comments".into())),
            ("passed".into(), Value::Bool(!blocking)),
            (
                "findingCount".into(),
                Value::Number(findings.len().to_string()),
            ),
            ("findings".into(), Value::Array(items)),
        ]);
        let _ = write_indented(standard_output, &payload);
        let _ = writeln!(standard_output);
    } else if findings.is_empty() {
        let _ = writeln!(standard_output, "comment style: no added-comment issues");
    } else {
        let _ = writeln!(
            standard_output,
            "{}",
            crate::comment_lint::format_findings(&findings)
        );
        let _ = writeln!(
            standard_output,
            "{} issue(s); {} blocking",
            findings.len(),
            if blocking { "has" } else { "no" }
        );
    }
    if blocking {
        1
    } else {
        0
    }
}

fn run_review_policy_command(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    if arguments.first().map(String::as_str) == Some("show") {
        let mut flag_set = FlagSet::new("review policy show");
        flag_set.string_flag("repo-root", "");
        flag_set.string_flag("format", "markdown");
        if let Err(parse_error) = flag_set.parse(&arguments[1..]) {
            let _ = writeln!(standard_error, "{}", parse_error.message);
            return 1;
        }
        if flag_set.string_value("format") == "compact" {
            let _ = writeln!(
                standard_output,
                "native_rules=rust,python,js,go,cpp language_gates=auto go_fallback=false"
            );
        } else {
            let _ = writeln!(standard_output, "# Native Review Policy");
            let _ = writeln!(standard_output, "- runtime: rust-native");
            let _ = writeln!(
                standard_output,
                "- language_gates: auto-detect root markers (Cargo.toml, pyproject.toml/setup.py, package.json, go.mod, CMakeLists.txt / root C/C++ sources)"
            );
            let _ = writeln!(
                standard_output,
                "- pre-commit: rust fmt+clippy; python black+ruff; js prettier+eslint; go gofmt+vet; c/c++ clang-format (tools missing = non-blocking)"
            );
            let _ = writeln!(
                standard_output,
                "- pre-pr: above plus unit tests (cargo test / pytest / npm test / go test) and typecheck (mypy / tsc when present); pytest exit 5 (no tests) is non-blocking"
            );
            let _ = writeln!(
                standard_output,
                "- python_checks: black, ruff, mypy, pytest, circular_imports, import_safety"
            );
            let _ = writeln!(
                standard_output,
                "- js_checks: prettier, eslint, tsc, npm test"
            );
            let _ = writeln!(standard_output, "- go_checks: gofmt, go vet, go test");
            let _ = writeln!(standard_output, "- cpp_checks: clang-format");
            let _ = writeln!(standard_output, "- go_fallback: false");
        }
        return 0;
    }
    let _ = writeln!(standard_error, "Usage: keel review policy show [flags]");
    1
}

fn review_flag_set(name: &str) -> FlagSet {
    let mut flag_set = FlagSet::new(name);
    flag_set.string_flag("repo-root", "");
    flag_set.string_flag("workspace-root", "");
    flag_set.string_flag("surface", "diff");
    flag_set.string_flag("base-ref", "");
    flag_set.string_flag("format", "compact");
    flag_set.bool_flag("all", false);
    flag_set.bool_flag("impact", false);
    flag_set
}

fn render_gate_result(
    gate: &str,
    blocking_findings: i32,
    output_format: &str,
    standard_output: &mut dyn Write,
) -> u8 {
    match output_format {
        "json" => {
            let payload = Value::Object(vec![
                ("gate".into(), Value::String(gate.into())),
                (
                    "blockingFindings".into(),
                    Value::Number(blocking_findings.to_string()),
                ),
                ("warningFindings".into(), Value::Number("0".into())),
                (
                    "summary".into(),
                    Value::String("the harness native review completed.".into()),
                ),
            ]);
            let _ = write_indented(standard_output, &payload);
        }
        "markdown" => {
            let _ = writeln!(standard_output, "# the harness Native Review Report");
            let _ = writeln!(standard_output);
            let _ = writeln!(standard_output, "- gate: {gate}");
            let _ = writeln!(standard_output, "- blocking_findings: {blocking_findings}");
            let _ = writeln!(standard_output, "- runtime: rust-native");
        }
        _ => {
            let _ = writeln!(
                standard_output,
                "gate={gate} blocking={blocking_findings} warnings=0 findings={blocking_findings}"
            );
        }
    }
    if blocking_findings == 0 {
        0
    } else {
        1
    }
}

fn render_generated_message(
    message_kind: &str,
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flag_set = FlagSet::new(format!("git-workflow {message_kind}"));
    flag_set.bool_flag("from-diff", false);
    flag_set.string_flag("test-result", "");
    // Accept the flags the README and help advertise for this command so it does
    // not error on its own documented usage. `repo-root` scopes the git diff;
    // `base-ref` and `format` are accepted for documented-surface compatibility.
    flag_set.string_flag("repo-root", "");
    flag_set.string_flag("base-ref", "");
    flag_set.string_flag("format", "");
    if let Err(parse_error) = flag_set.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 1;
    }
    let repo_root_value = flag_set.string_value("repo-root");
    let repo_root: Option<std::path::PathBuf> = if repo_root_value.trim().is_empty() {
        None
    } else {
        Some(std::path::PathBuf::from(repo_root_value))
    };
    let from_diff = flag_set.bool_value("from-diff");
    let staged = if from_diff {
        staged_files(repo_root.as_deref()).unwrap_or_default()
    } else {
        Vec::new()
    };
    let diff_summary = if from_diff {
        git_diff_stat(repo_root.as_deref())
            .unwrap_or_else(|| "No diff summary available.".to_string())
    } else {
        "No diff summary requested.".to_string()
    };
    if message_kind == "commit" {
        let _ = writeln!(
            standard_output,
            "{}",
            generate_commit_subject(from_diff, &staged)
        );
        let _ = writeln!(standard_output);
        let _ = writeln!(standard_output, "{}", commit_body_from_staged(&staged));
        let _ = writeln!(standard_output);
        let _ = writeln!(standard_output, "{diff_summary}");
    } else {
        let _ = writeln!(standard_output, "## Summary");
        for bullet in pr_summary_bullets(&staged) {
            let _ = writeln!(standard_output, "- {bullet}");
        }
        let _ = writeln!(standard_output);
        let _ = writeln!(standard_output, "## Test plan");
        let test_result = flag_set.string_value("test-result");
        let _ = writeln!(
            standard_output,
            "- {}",
            if test_result.trim().is_empty() {
                "Not provided"
            } else {
                test_result
            }
        );
    }
    0
}

fn staged_files(repo_root: Option<&std::path::Path>) -> Option<Vec<String>> {
    let result = run_command(
        "git",
        &[
            "diff".to_string(),
            "--cached".to_string(),
            "--name-only".to_string(),
        ],
        repo_root,
    )
    .ok()?;
    if result.code != 0 {
        return None;
    }
    let text = String::from_utf8_lossy(&result.stdout);
    let files: Vec<String> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect();
    Some(files)
}

/// Canonical commit categories (capitalized first letter). New generators emit
/// these; validators also accept the legacy lowercase form.
const COMMIT_CATEGORIES: [&str; 6] = ["Add", "Config", "Refactor", "Wip", "Fix", "Docs"];

/// Map a category token (any casing) to its canonical form, or None if unknown.
fn normalize_commit_category(raw: &str) -> Option<&'static str> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "add" => Some("Add"),
        "config" => Some("Config"),
        "refactor" => Some("Refactor"),
        "wip" => Some("Wip"),
        "fix" => Some("Fix"),
        "docs" => Some("Docs"),
        _ => None,
    }
}

/// Validate a commit subject against `Add : FEATURE : short information`.
///
/// The commit subject is COLON-separated (three parts), with spaces around
/// colons preferred. Distinct from branch names (`task/<task>`). Do not conflate.
///
/// - Category must be one of [`COMMIT_CATEGORIES`] (case-insensitive; legacy
///   lowercase still accepted so in-flight history keeps validating).
/// - FEATURE must be a non-empty uppercase component label (e.g. RGB, PROTOCOL).
/// - Short information must be non-empty.
fn validate_commit_subject(subject: &str) -> Result<(), String> {
    let trimmed = subject.trim();
    let parts: Vec<&str> = trimmed.splitn(3, ':').collect();
    if parts.len() < 3 {
        return Err(
            "expected three colon-separated parts: Add : FEATURE : short information".to_string(),
        );
    }
    let category = parts[0].trim();
    let feature = parts[1].trim();
    let information = parts[2].trim();
    if normalize_commit_category(category).is_none() {
        return Err(format!(
            "category '{category}' must be one of: {}",
            COMMIT_CATEGORIES.join(", ")
        ));
    }
    if feature.is_empty() {
        return Err("feature_category is required (e.g. RGB, LED, ARGB, SENSOR)".to_string());
    }
    if feature != feature.to_uppercase() {
        return Err(format!("feature_category '{feature}' must be uppercase"));
    }
    if information.is_empty() {
        return Err("short information is required after the feature category".to_string());
    }
    Ok(())
}

fn detect_category(paths: &[String]) -> &'static str {
    if paths.is_empty() {
        return "Wip";
    }
    let all_match = |predicate: fn(&str) -> bool| paths.iter().all(|path| predicate(path));

    if all_match(is_docs_path) {
        return "Docs";
    }
    if all_match(is_config_path) {
        return "Config";
    }
    "Wip"
}

fn is_docs_path(path: &str) -> bool {
    path.ends_with(".md") || path.starts_with("docs/")
}

fn is_ci_path(path: &str) -> bool {
    path.starts_with(".github/workflows/")
        || path.starts_with(".github/actions/")
        || path == ".gitlab-ci.yml"
}

fn is_config_path(path: &str) -> bool {
    is_ci_path(path)
        || path.ends_with(".toml")
        || path.ends_with(".yml")
        || path.ends_with(".yaml")
        || path.ends_with(".ini")
        || path.ends_with(".cfg")
        || path.ends_with(".conf")
        || path.ends_with(".json")
        || path.ends_with(".gitignore")
        || path.ends_with(".gitattributes")
}

fn is_test_path(path: &str) -> bool {
    path.contains("/tests/")
        || path.starts_with("tests/")
        || path.ends_with("_test.rs")
        || path.ends_with("_tests.rs")
        || path.contains("/test_")
        || path.contains("__tests__/")
}

fn is_source_path(path: &str) -> bool {
    !is_docs_path(path) && !is_ci_path(path) && !is_test_path(path)
}

fn derive_scope(paths: &[String]) -> Option<String> {
    if paths.is_empty() {
        return None;
    }
    let segments: Vec<Vec<&str>> = paths
        .iter()
        .map(|path| path.split('/').collect::<Vec<&str>>())
        .collect();

    let head: Vec<&str> = segments[0].clone();
    let mut prefix_len = head.len().saturating_sub(1);
    for path in &segments[1..] {
        let limit = std::cmp::min(prefix_len, path.len().saturating_sub(1));
        let mut shared = 0;
        while shared < limit && head[shared] == path[shared] {
            shared += 1;
        }
        prefix_len = shared;
        if prefix_len == 0 {
            break;
        }
    }
    if prefix_len == 0 {
        return None;
    }
    let is_generic = |segment: &str| {
        matches!(
            segment,
            "src" | "tests" | "test" | "lib" | "crates" | "packages"
        )
    };
    let mut idx = prefix_len.saturating_sub(1);
    while idx > 0 && is_generic(head[idx]) {
        idx -= 1;
    }
    let leaf = head[idx];
    let scope = leaf.trim_end_matches(".rs");
    if scope.is_empty() || is_generic(scope) {
        return None;
    }
    Some(scope.to_string())
}

fn generate_commit_subject(from_diff: bool, paths: &[String]) -> String {
    if !from_diff {
        return "Wip : GENERAL : update".to_string();
    }
    if paths.is_empty() {
        return "Wip : GENERAL : no staged changes".to_string();
    }
    let category = detect_category(paths);
    let summary = subject_summary(paths);
    let feature = derive_scope(paths)
        .map(|scope| scope.to_uppercase())
        .unwrap_or_else(|| "GENERAL".to_string());
    format!("{category} : {feature} : {summary}")
}

fn subject_summary(paths: &[String]) -> String {
    if paths.len() == 1 {
        let leaf = paths[0].rsplit('/').next().unwrap_or(&paths[0]);
        return format!("update {leaf}");
    }
    format!("update {} files", paths.len())
}

fn commit_body_from_staged(paths: &[String]) -> String {
    if paths.is_empty() {
        return "No staged changes.".to_string();
    }
    let mut lines = vec!["What Changed:".to_string()];
    for path in paths.iter().take(20) {
        lines.push(format!("- {path}"));
    }
    if paths.len() > 20 {
        lines.push(format!("- ... and {} more files", paths.len() - 20));
    }
    lines.join("\n")
}

fn pr_summary_bullets(paths: &[String]) -> Vec<String> {
    if paths.is_empty() {
        return vec!["No staged changes detected.".to_string()];
    }
    let mut bullets = Vec::new();
    let docs: Vec<&String> = paths.iter().filter(|p| is_docs_path(p)).collect();
    let ci: Vec<&String> = paths.iter().filter(|p| is_ci_path(p)).collect();
    let tests: Vec<&String> = paths.iter().filter(|p| is_test_path(p)).collect();
    let source: Vec<&String> = paths.iter().filter(|p| is_source_path(p)).collect();
    if !source.is_empty() {
        bullets.push(format!(
            "Source changes across {} file(s): {}",
            source.len(),
            preview_paths(&source, 3)
        ));
    }
    if !tests.is_empty() {
        bullets.push(format!(
            "Test changes across {} file(s): {}",
            tests.len(),
            preview_paths(&tests, 3)
        ));
    }
    if !docs.is_empty() {
        bullets.push(format!(
            "Docs changes across {} file(s): {}",
            docs.len(),
            preview_paths(&docs, 3)
        ));
    }
    if !ci.is_empty() {
        bullets.push(format!(
            "CI changes across {} file(s): {}",
            ci.len(),
            preview_paths(&ci, 3)
        ));
    }
    if bullets.is_empty() {
        bullets.push(format!("Updated {} file(s)", paths.len()));
    }
    bullets
}

fn preview_paths(paths: &[&String], limit: usize) -> String {
    let mut shown: Vec<String> = paths.iter().take(limit).map(|p| (*p).clone()).collect();
    if paths.len() > limit {
        shown.push(format!("(+{} more)", paths.len() - limit));
    }
    shown.join(", ")
}

fn lint_message(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    if arguments.len() != 1 {
        let _ = writeln!(
            standard_error,
            "Usage: keel git-workflow lint-message <file>"
        );
        return 1;
    }
    match fs::read_to_string(&arguments[0]) {
        Ok(text) => {
            let first_line = text.lines().next().unwrap_or("");
            if first_line.len() > 72 {
                let _ = writeln!(standard_error, "message subject exceeds 72 characters");
                return 1;
            }
            if let Err(reason) = validate_commit_subject(first_line) {
                let _ = writeln!(
                    standard_error,
                    "subject does not match <category>: <FEATURE>: <short information>: {reason}"
                );
                let _ = writeln!(
                    standard_error,
                    "  categories (lowercase): {}",
                    COMMIT_CATEGORIES.join(", ")
                );
                let _ = writeln!(
                    standard_error,
                    "  example: wip: RGB: Build light effect mode (multi color)"
                );
                return 1;
            }
            let _ = writeln!(standard_output, "message lint passed");
            0
        }
        Err(error) => {
            let _ = writeln!(standard_error, "read {}: {error}", arguments[0]);
            1
        }
    }
}

fn git_diff_stat(repo_root: Option<&std::path::Path>) -> Option<String> {
    let result = run_command(
        "git",
        &["diff".to_string(), "--stat".to_string()],
        repo_root,
    )
    .ok()?;
    if result.code != 0 {
        return None;
    }
    let text = String::from_utf8_lossy(&result.stdout).trim().to_string();
    Some(if text.is_empty() {
        "No local diff.".to_string()
    } else {
        text
    })
}

fn hosted_body() -> String {
    [
        "# the harness Native Review Report",
        "",
        "- gate: pass",
        "- blocking_findings: 0",
        "- warning_findings: 0",
        "- runtime: rust-native",
        "- go_fallback: false",
        "",
    ]
    .join("\n")
}

fn render_review_help(standard_output: &mut dyn Write) {
    let _ = writeln!(
        standard_output,
        "Usage: keel review [pre-commit|pre-pr|diff|gates|hosted|policy|comments] ..."
    );
}

fn render_git_workflow_help(standard_output: &mut dyn Write) {
    let _ = writeln!(
        standard_output,
        "Usage: keel git-workflow [preflight|await-ci|configure|show|commit-message|pr-body|lint-message] ..."
    );
}

fn is_help_argument(argument: &str) -> bool {
    matches!(argument, "help" | "--help" | "-h")
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- await-ci fail-closed (offline; no provider CLI invoked) ----

    /// The whole point of the fix: a provider ERROR or an explicitly-requested
    /// but unavailable provider must block (exit 1), never pass with no signal.
    #[test]
    fn await_ci_error_outcome_blocks_merge() {
        assert_eq!(AwaitCiOutcome::Error.exit_code(), 1);
        assert_eq!(AwaitCiOutcome::Red.exit_code(), 1);
        assert_eq!(AwaitCiOutcome::Pending.exit_code(), 1);
        assert_eq!(AwaitCiOutcome::Timeout.exit_code(), 1);
        // Only a real green, or a genuine no-CI repo, may proceed.
        assert_eq!(AwaitCiOutcome::Green.exit_code(), 0);
        assert_eq!(AwaitCiOutcome::NoCi.exit_code(), 0);
    }

    /// An explicit `--provider gh`/`glab` that is not installed must resolve to
    /// ExplicitUnavailable (which the caller maps to Error/block), NOT to the
    /// NoneDetected pass path.
    #[test]
    fn explicit_provider_unavailable_is_not_treated_as_no_ci() {
        // A provider name that cannot be on PATH in the test environment.
        match resolve_provider("definitely-not-a-real-provider", None) {
            ProviderResolution::ExplicitUnavailable(_) => {}
            other => panic!("explicit unknown provider must be ExplicitUnavailable, got {other:?}"),
        }
    }

    /// The no-PR message from gh maps to a genuine no-checks (NoCi) result while
    /// an unrecognized non-zero is an error; this is the discrimination the
    /// fail-open bug lacked, asserted via the outcome mapping without spawning gh.
    #[test]
    fn gh_no_pr_message_is_no_ci_not_error() {
        // parse_gh_checks on empty output yields no checks (genuine no-CI), and
        // evaluate_checks maps that to NoChecks (which the loop renders as NoCi).
        assert!(parse_gh_checks("").is_none());
        assert!(matches!(evaluate_checks(&[]), CiVerdict::NoChecks));
        // A populated table parses to checks.
        let checks = parse_gh_checks("NAME  STATUS\nci  success\n").expect("one check");
        assert!(matches!(evaluate_checks(&checks), CiVerdict::Green));
    }

    /// Regression: only a PASSING review is a reviewer pass. A failed review or
    /// the informational diff/init surfaces must not clear the review gate.
    #[test]
    fn review_pass_clears_gate_only_on_passing_real_surface() {
        // Passing real reviewer surfaces clear the gate.
        assert!(review_pass_clears_gate("gates", 0));
        assert!(review_pass_clears_gate("pre-pr", 0));
        assert!(review_pass_clears_gate("pre-commit", 0));
        // Failing (non-zero) review must NOT clear the gate.
        assert!(!review_pass_clears_gate("gates", 1));
        assert!(!review_pass_clears_gate("pre-pr", 2));
        assert!(!review_pass_clears_gate("pre-commit", 1));
        // Informational surfaces review nothing → never clear.
        assert!(!review_pass_clears_gate("diff", 0));
        assert!(!review_pass_clears_gate("init", 0));
    }

    // ---- brownfield flow gate classification (offline; no git invocation) ----

    /// Modifying established source is what the gate exists to catch.
    #[test]
    fn brownfield_gate_flags_modified_source_files() {
        for path in [
            "rust/crates/keel/src/review.rs",
            "app/main.py",
            "web/src/App.tsx",
            "cmd/server/main.go",
        ] {
            assert_eq!(
                brownfield_source_from_name_status(&format!("M\t{path}")),
                Some(path.to_string()),
                "{path} should require flow evidence"
            );
        }
    }

    /// Greenfield, docs, and generated trees are the documented exemptions. An
    /// added file has no prior owner, so requiring an owner trace would be wrong.
    #[test]
    fn brownfield_gate_exempts_added_docs_and_generated_paths() {
        // Added and deleted files carry no established behavior to preserve.
        assert_eq!(
            brownfield_source_from_name_status("A\trust/crates/keel/src/new_module.rs"),
            None
        );
        assert_eq!(brownfield_source_from_name_status("D\tapp/old.py"), None);

        // Docs and config have no ownership flow.
        for path in ["README.md", "CLAUDE.md", "Cargo.toml", ".github/x.yml"] {
            assert_eq!(
                brownfield_source_from_name_status(&format!("M\t{path}")),
                None,
                "{path} should be exempt"
            );
        }

        // Generated and vendored trees are exempt even with a source extension.
        for path in [
            "target/debug/build/x.rs",
            "node_modules/pkg/index.js",
            "vendor/lib/thing.go",
            "app/generated/schema.py",
        ] {
            assert_eq!(
                brownfield_source_from_name_status(&format!("M\t{path}")),
                None,
                "{path} should be exempt"
            );
        }
    }

    /// Renaming while editing still changes established behavior. Verified against
    /// git: `git mv old.rs new.rs` plus an edit reports `R050\told.rs\tnew.rs`, so
    /// matching only `M` let a rename slip past the gate entirely.
    #[test]
    fn brownfield_gate_flags_renamed_source_using_destination_path() {
        assert_eq!(
            brownfield_source_from_name_status("R050\told.rs\tsrc/new.rs"),
            Some("src/new.rs".to_string())
        );
        assert_eq!(
            brownfield_source_from_name_status("R100\tsrc/a.rs\tsrc/b.rs"),
            Some("src/b.rs".to_string())
        );
        // Exemptions still apply to the destination path.
        assert_eq!(
            brownfield_source_from_name_status("R050\tsrc/a.rs\tvendor/b.rs"),
            None
        );
        assert_eq!(
            brownfield_source_from_name_status("R050\tsrc/a.rs\tdocs/b.md"),
            None
        );
        // A malformed rename line with no destination must not panic.
        assert_eq!(brownfield_source_from_name_status("R050\tonly-one"), None);
    }

    /// The artifact is workspace-global, so relevance is what stops one filled
    /// artifact from satisfying the gate forever regardless of what changed next.
    #[test]
    fn artifact_relevance_matches_touched_paths_tolerantly() {
        let touched = vec![
            "rust/crates/keel/src/review.rs".to_string(),
            "app/main.py".to_string(),
        ];
        // Exact repo-relative match.
        assert!(artifact_targets_a_touched_file(
            "rust/crates/keel/src/review.rs",
            &touched
        ));
        // Windows separators and a leading ./ must not cause a false stale verdict.
        assert!(artifact_targets_a_touched_file(
            ".\\rust\\crates\\keel\\src\\review.rs",
            &touched
        ));
        // An absolute path still resolves by suffix.
        assert!(artifact_targets_a_touched_file(
            "D:/Nasri/Project/keel/app/main.py",
            &touched
        ));
        // A stale artifact tracing an untouched file is rejected.
        assert!(!artifact_targets_a_touched_file(
            "rust/crates/keel/src/commands.rs",
            &touched
        ));
        // An empty target never counts as evidence.
        assert!(!artifact_targets_a_touched_file("", &touched));
        // A bare filename must not match a different directory's same-named file.
        assert!(!artifact_targets_a_touched_file(
            "other/review.rs",
            &touched
        ));
    }

    /// Case-insensitive filesystems allow `Foo.RS`; a case-sensitive extension
    /// check would let that edit bypass the gate entirely.
    #[test]
    fn brownfield_gate_matches_extensions_case_insensitively() {
        assert_eq!(
            brownfield_source_from_name_status("M\tsrc/Foo.RS"),
            Some("src/Foo.RS".to_string())
        );
        assert_eq!(
            brownfield_source_from_name_status("M\tsrc/App.TSX"),
            Some("src/App.TSX".to_string())
        );
        // Still not a source extension regardless of case.
        assert_eq!(brownfield_source_from_name_status("M\tREADME.MD"), None);
    }

    /// Windows checkouts report backslash paths; the exemption match is on `/`.
    #[test]
    fn brownfield_gate_normalizes_windows_separators() {
        assert_eq!(
            brownfield_source_from_name_status("M\trust\\crates\\keel\\src\\review.rs"),
            Some("rust/crates/keel/src/review.rs".to_string())
        );
        assert_eq!(
            brownfield_source_from_name_status("M\tnode_modules\\pkg\\index.js"),
            None
        );
    }

    // ---- await-ci pure logic (offline-safe; no gh/glab invocation) ----

    #[test]
    fn classify_check_state_maps_statuses() {
        assert_eq!(classify_check_state("success"), CheckState::Green);
        assert_eq!(classify_check_state("passed"), CheckState::Green);
        assert_eq!(classify_check_state("SUCCESS"), CheckState::Green);
        assert_eq!(classify_check_state("running"), CheckState::Pending);
        assert_eq!(classify_check_state("in_progress"), CheckState::Pending);
        assert_eq!(classify_check_state("queued"), CheckState::Pending);
        assert_eq!(classify_check_state(""), CheckState::Pending);
        // Unknown / failure conclusions fail CLOSED to red so merge never proceeds blind.
        assert_eq!(classify_check_state("failure"), CheckState::Red);
        assert_eq!(classify_check_state("failed"), CheckState::Red);
        assert_eq!(classify_check_state("cancelled"), CheckState::Red);
        assert_eq!(classify_check_state("action_required"), CheckState::Red);
        assert_eq!(classify_check_state("something-weird"), CheckState::Red);
    }

    #[test]
    fn evaluate_checks_blocks_on_any_red() {
        let checks = vec![
            CiCheck {
                name: "build".into(),
                state: CheckState::Green,
            },
            CiCheck {
                name: "test".into(),
                state: CheckState::Red,
            },
        ];
        assert!(matches!(evaluate_checks(&checks), CiVerdict::Red));
    }

    #[test]
    fn evaluate_checks_pending_when_any_running() {
        let checks = vec![
            CiCheck {
                name: "build".into(),
                state: CheckState::Green,
            },
            CiCheck {
                name: "deploy".into(),
                state: CheckState::Pending,
            },
        ];
        assert!(matches!(evaluate_checks(&checks), CiVerdict::Pending));
    }

    #[test]
    fn evaluate_checks_green_only_when_all_green() {
        let checks = vec![
            CiCheck {
                name: "build".into(),
                state: CheckState::Green,
            },
            CiCheck {
                name: "test".into(),
                state: CheckState::Green,
            },
        ];
        assert!(matches!(evaluate_checks(&checks), CiVerdict::Green));
    }

    #[test]
    fn evaluate_checks_empty_is_no_checks() {
        assert!(matches!(evaluate_checks(&[]), CiVerdict::NoChecks));
    }

    #[test]
    fn await_ci_exit_code_blocks_everything_except_green_or_no_ci() {
        assert_eq!(AwaitCiOutcome::Green.exit_code(), 0);
        assert_eq!(AwaitCiOutcome::NoCi.exit_code(), 0);
        assert_eq!(AwaitCiOutcome::Red.exit_code(), 1);
        assert_eq!(AwaitCiOutcome::Pending.exit_code(), 1);
        assert_eq!(AwaitCiOutcome::Timeout.exit_code(), 1);
    }

    #[test]
    fn parse_gh_checks_reads_columns_and_skips_header() {
        let stdout = "NAME\tSTATUS\tCONCLUSION\nbuild\tpass\t\nlint\tfail\t\n";
        let checks = parse_gh_checks(stdout).expect("parse");
        assert_eq!(checks.len(), 2);
        assert_eq!(checks[0].name, "build");
        assert_eq!(checks[0].state, CheckState::Green);
        assert_eq!(checks[1].name, "lint");
        assert_eq!(checks[1].state, CheckState::Red);
    }

    #[test]
    fn parse_gh_checks_empty_is_none() {
        assert!(parse_gh_checks("").is_none());
        assert!(parse_gh_checks("NAME\tSTATUS\n").is_none());
    }

    #[test]
    fn parse_glab_status_reads_name_status_pairs() {
        let stdout = "build: success\ntest: running\n";
        let checks = parse_glab_status(stdout).expect("parse");
        assert_eq!(checks.len(), 2);
        assert_eq!(checks[0].name, "build");
        assert_eq!(checks[0].state, CheckState::Green);
        assert_eq!(checks[1].state, CheckState::Pending);
    }

    #[test]
    fn workflow_slug_is_safe_and_lowercase() {
        assert_eq!(
            workflow_slug("D:\\Nasri\\Project\\keel"),
            "d-nasri-project-keel"
        );
        assert!(!workflow_slug("").is_empty());
    }

    #[test]
    fn review_policy_show_succeeds_with_no_extra_args() {
        // Regression: the handler previously required arguments.len() >= 2, so
        // `review policy show` (the documented form, args == ["show"]) fell
        // through to the exit-1 usage path. It must succeed and print the policy.
        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let code = run_review_policy_command(&["show".to_string()], &mut stdout, &mut stderr);
        assert_eq!(code, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
        assert!(String::from_utf8_lossy(&stdout).contains("Native Review Policy"));
    }

    #[test]
    fn review_policy_show_honors_compact_format() {
        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let code = run_review_policy_command(
            &[
                "show".to_string(),
                "--format".to_string(),
                "compact".to_string(),
            ],
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(code, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
        let out = String::from_utf8_lossy(&stdout);
        assert!(
            out.contains("native_rules=rust,python,js,go,cpp"),
            "compact policy should list multi-lang rules, got: {out}"
        );
        assert!(out.contains("language_gates=auto"));
    }

    #[test]
    fn classify_python_test_exit_five_is_non_blocking() {
        for tool in ["pytest", "python -m unittest discover"] {
            let no_tests = classify_python_test_exit(tool, 5);
            assert_eq!(
                no_tests.status,
                GateStatus::Blocked,
                "{tool} exit 5 must be Blocked"
            );
            assert!(!no_tests.blocking, "{tool} exit 5 must be non-blocking");
            let details = no_tests.details.as_deref().unwrap_or("");
            assert!(
                details.contains("no tests") && details.contains(tool),
                "{tool} exit 5 must explain no-tests with tool name: {details}"
            );
        }

        let pass = classify_python_test_exit("pytest", 0);
        assert_eq!(pass.status, GateStatus::Pass);
        assert!(pass.blocking);

        let fail = classify_python_test_exit("pytest", 1);
        assert_eq!(fail.status, GateStatus::Fail);
        assert!(fail.blocking);

        let unittest_fail = classify_python_test_exit("python -m unittest discover", 1);
        assert_eq!(unittest_fail.status, GateStatus::Fail);
        assert!(unittest_fail.blocking);
    }

    #[test]
    fn language_project_markers_are_root_only() {
        let temp = std::env::temp_dir().join(format!("keel-review-markers-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(temp.join("nested")).unwrap();
        // Nested sources must not trigger root-marker project detection.
        std::fs::write(temp.join("nested").join("x.py"), "print(1)").unwrap();
        std::fs::write(temp.join("nested").join("x.go"), "package main").unwrap();
        std::fs::write(temp.join("nested").join("x.js"), "console.log(1)").unwrap();
        assert!(!has_python_project(&temp));
        assert!(!has_go_project(&temp));
        assert!(!has_js_project(&temp));
        assert!(!has_cpp_project(&temp));
        assert!(has_python_files(&temp));
        assert!(has_js_files(&temp));

        std::fs::write(temp.join("go.mod"), "module example\n").unwrap();
        assert!(has_go_project(&temp));
        std::fs::write(temp.join("package.json"), "{}").unwrap();
        assert!(has_js_project(&temp));
        std::fs::write(temp.join("pyproject.toml"), "[project]\nname='t'\n").unwrap();
        assert!(has_python_project(&temp));
        std::fs::write(temp.join("main.c"), "int main(void){return 0;}\n").unwrap();
        assert!(has_cpp_project(&temp));
        assert!(!run_cpp_surface_gates(&temp, false).is_empty());

        // Surface gates return empty when markers absent (no cargo/go/py/js/cpp root).
        let empty = std::env::temp_dir().join(format!("keel-review-empty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&empty);
        std::fs::create_dir_all(&empty).unwrap();
        assert!(run_python_surface_gates(&empty, true).is_empty());
        assert!(run_js_surface_gates(&empty, true).is_empty());
        assert!(run_go_surface_gates(&empty, true).is_empty());
        assert!(run_cpp_surface_gates(&empty, true).is_empty());
        assert!(run_rust_surface_gates(&empty, true).is_empty());

        let _ = std::fs::remove_dir_all(&temp);
        let _ = std::fs::remove_dir_all(&empty);
    }

    #[test]
    fn review_policy_unknown_subcommand_errors() {
        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let code = run_review_policy_command(&["bogus".to_string()], &mut stdout, &mut stderr);
        assert_eq!(code, 1);
    }

    #[test]
    fn gate_result_status_mapping() {
        let pass = GateResult {
            name: "test".to_string(),
            status: GateStatus::Pass,
            blocking: true,
            details: Some("ok".to_string()),
        };
        assert_eq!(pass.status, GateStatus::Pass);

        let fail = GateResult {
            name: "test".to_string(),
            status: GateStatus::Fail,
            blocking: true,
            details: Some("fail".to_string()),
        };
        assert_eq!(fail.status, GateStatus::Fail);
    }

    #[test]
    fn has_python_files_detection() {
        let temp = std::env::temp_dir().join("keel-review-test");
        std::fs::create_dir_all(&temp).unwrap();

        // Create a Python file
        std::fs::write(temp.join("test.py"), "print('hello')").unwrap();

        let result = has_python_files(&temp);
        assert!(result);

        // Cleanup
        std::fs::remove_dir_all(&temp).unwrap();
    }

    #[test]
    fn has_js_files_detection() {
        let temp = std::env::temp_dir().join("keel-review-js-test");
        std::fs::create_dir_all(&temp).unwrap();

        // Create a JS file
        std::fs::write(temp.join("test.js"), "console.log('hello')").unwrap();

        let result = has_js_files(&temp);
        assert!(result);

        // Cleanup
        std::fs::remove_dir_all(&temp).unwrap();
    }

    #[test]
    fn tally_counts_each_blocking_failure_once() {
        let gate_results = vec![
            GateResult {
                name: "rust_tests".to_string(),
                status: GateStatus::Fail,
                blocking: true,
                details: None,
            },
            GateResult {
                name: "ruff".to_string(),
                status: GateStatus::Pass,
                blocking: true,
                details: None,
            },
            GateResult {
                name: "prettier".to_string(),
                status: GateStatus::Warn,
                blocking: false,
                details: None,
            },
        ];

        let (blocking, warnings) = tally_gate_results(&gate_results);

        assert_eq!(
            blocking, 1,
            "exactly one blocking failure should produce blocking_findings=1, not 2 (regression guard for prior double-count bug)"
        );
        assert_eq!(warnings, 1);
    }

    #[test]
    fn tally_handles_empty_and_all_pass() {
        let (blocking, warnings) = tally_gate_results(&[]);
        assert_eq!(blocking, 0);
        assert_eq!(warnings, 0);

        let all_pass = vec![GateResult {
            name: "fmt".to_string(),
            status: GateStatus::Pass,
            blocking: true,
            details: None,
        }];
        let (blocking, warnings) = tally_gate_results(&all_pass);
        assert_eq!(blocking, 0);
        assert_eq!(warnings, 0);
    }

    fn paths(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn detect_category_classifies_docs_only() {
        let staged = paths(&["README.md", "docs/architecture.md"]);
        assert_eq!(detect_category(&staged), "Docs");
    }

    #[test]
    fn detect_category_classifies_ci_as_config() {
        let staged = paths(&[".github/workflows/release.yml"]);
        assert_eq!(detect_category(&staged), "Config");
    }

    #[test]
    fn detect_category_classifies_config_files() {
        let staged = paths(&["Cargo.toml", "rustfmt.toml"]);
        assert_eq!(detect_category(&staged), "Config");
    }

    #[test]
    fn detect_category_falls_back_to_wip_for_source() {
        let staged = paths(&["src/lib.rs", "src/main.rs"]);
        assert_eq!(detect_category(&staged), "Wip");
    }

    #[test]
    fn detect_category_empty_is_wip() {
        assert_eq!(detect_category(&[]), "Wip");
    }

    #[test]
    fn derive_scope_returns_common_directory() {
        let staged = paths(&[
            "rust/crates/keel/src/review.rs",
            "rust/crates/keel/src/runner/mod.rs",
        ]);
        assert_eq!(
            derive_scope(&staged),
            Some("keel".to_string()),
            "scope should be the deepest shared directory above the leaf files"
        );
    }

    #[test]
    fn derive_scope_returns_none_when_no_common_prefix() {
        let staged = paths(&["src/lib.rs", "tests/it.rs"]);
        assert_eq!(derive_scope(&staged), None);
    }

    #[test]
    fn derive_scope_skips_bare_src_prefix() {
        let staged = paths(&["src/foo.rs", "src/bar.rs"]);
        assert_eq!(
            derive_scope(&staged),
            None,
            "src/ alone is not a meaningful scope label"
        );
    }

    #[test]
    fn generate_commit_subject_without_diff_uses_placeholder() {
        assert_eq!(
            generate_commit_subject(false, &[]),
            "Wip : GENERAL : update"
        );
    }

    #[test]
    fn generate_commit_subject_with_diff_but_no_staged_signals_empty() {
        assert_eq!(
            generate_commit_subject(true, &[]),
            "Wip : GENERAL : no staged changes"
        );
    }

    #[test]
    fn generate_commit_subject_combines_category_feature_and_summary() {
        let staged = paths(&[
            "rust/crates/keel/src/review.rs",
            "rust/crates/keel/src/lib.rs",
        ]);
        assert_eq!(
            generate_commit_subject(true, &staged),
            "Wip : KEEL : update 2 files"
        );
    }

    #[test]
    fn generate_commit_subject_single_file_uses_leaf_name() {
        let staged = paths(&["docs/architecture.md"]);
        let subject = generate_commit_subject(true, &staged);
        assert!(
            subject.starts_with("Docs : "),
            "expected Docs category, got {subject}"
        );
        assert!(
            subject.ends_with("update architecture.md"),
            "expected leaf summary, got {subject}"
        );
        assert!(
            validate_commit_subject(&subject).is_ok(),
            "generated subject must satisfy the strict validator, got {subject}"
        );
    }

    #[test]
    fn generated_subjects_always_pass_strict_validation() {
        let cases: Vec<Vec<String>> = vec![
            paths(&["docs/readme.md"]),
            paths(&["Cargo.toml"]),
            paths(&["rust/crates/keel/src/review.rs"]),
            paths(&["a.rs", "b.rs"]),
        ];
        for staged in cases {
            let subject = generate_commit_subject(true, &staged);
            assert!(
                validate_commit_subject(&subject).is_ok(),
                "generated subject {subject:?} failed strict validation"
            );
        }
    }

    #[test]
    fn validate_commit_subject_accepts_canonical_form() {
        // Preferred form: Capitalized category, spaces around colons.
        assert!(
            validate_commit_subject("Wip : RGB : Build light effect mode (multi color)").is_ok()
        );
        assert!(validate_commit_subject("Fix : SENSOR : Correct I2C read timeout").is_ok());
        assert!(validate_commit_subject("Add : ARGB : Add rainbow cycle preset").is_ok());
        assert!(validate_commit_subject("Config : LED : Set default brightness").is_ok());
        assert!(validate_commit_subject("Refactor : RGB : Extract blend helper").is_ok());
        assert!(validate_commit_subject("Docs : SENSOR : Document calibration").is_ok());
        // Legacy lowercase / no-space form still accepted for in-flight history.
        assert!(validate_commit_subject("wip: RGB: Build light effect mode (multi color)").is_ok());
        assert!(validate_commit_subject("fix: SENSOR: Correct I2C read timeout").is_ok());
        assert!(validate_commit_subject("add: ARGB: Add rainbow cycle preset").is_ok());
    }

    #[test]
    fn validate_commit_subject_rejects_unknown_category() {
        let error = validate_commit_subject("feat: RGB: do a thing").unwrap_err();
        assert!(error.contains("category"), "got {error}");
    }

    #[test]
    fn validate_commit_subject_rejects_lowercase_feature() {
        let error = validate_commit_subject("Wip : rgb : do a thing").unwrap_err();
        assert!(error.contains("uppercase"), "got {error}");
    }

    #[test]
    fn validate_commit_subject_rejects_missing_parts() {
        assert!(validate_commit_subject("wip: RGB").is_err());
        assert!(validate_commit_subject("just a message").is_err());
        assert!(validate_commit_subject("wip: RGB: ").is_err());
        assert!(validate_commit_subject("wip: : info").is_err());
    }

    #[test]
    fn commit_body_lists_staged_paths_under_what_changed() {
        let staged = paths(&["a.rs", "b.rs"]);
        let body = commit_body_from_staged(&staged);
        assert!(body.starts_with("What Changed:"));
        assert!(body.contains("- a.rs"));
        assert!(body.contains("- b.rs"));
    }

    #[test]
    fn commit_body_truncates_after_twenty_paths() {
        let many: Vec<String> = (0..25).map(|i| format!("file{i}.rs")).collect();
        let body = commit_body_from_staged(&many);
        assert!(body.contains("... and 5 more files"));
    }

    #[test]
    fn commit_body_handles_empty() {
        assert_eq!(commit_body_from_staged(&[]), "No staged changes.");
    }

    #[test]
    fn pr_summary_bullets_groups_by_change_kind() {
        let staged = paths(&[
            "src/lib.rs",
            "tests/it.rs",
            "README.md",
            ".github/workflows/ci.yml",
        ]);
        let bullets = pr_summary_bullets(&staged);
        assert_eq!(bullets.len(), 4);
        assert!(bullets[0].starts_with("Source changes"));
        assert!(bullets.iter().any(|b| b.starts_with("Test changes")));
        assert!(bullets.iter().any(|b| b.starts_with("Docs changes")));
        assert!(bullets.iter().any(|b| b.starts_with("CI changes")));
    }

    #[test]
    fn pr_summary_bullets_empty_returns_no_changes_message() {
        assert_eq!(
            pr_summary_bullets(&[]),
            vec!["No staged changes detected.".to_string()]
        );
    }

    #[test]
    fn rust_surface_gates_skip_when_no_cargo_toml() {
        let temp = std::env::temp_dir().join("keel-no-cargo-test");
        std::fs::create_dir_all(&temp).unwrap();
        let gates = run_rust_surface_gates(&temp, true);
        assert!(
            gates.is_empty(),
            "non-Rust repos should skip cargo gates, got {gates:?}",
            gates = gates.iter().map(|g| &g.name).collect::<Vec<_>>()
        );
        std::fs::remove_dir_all(&temp).unwrap();
    }

    // ---- git-workflow preflight ----

    /// Run a git command in `dir`, asserting success, for test setup.
    fn git_in(dir: &std::path::Path, args: &[&str]) {
        let owned: Vec<String> = args.iter().map(|a| a.to_string()).collect();
        let result = crate::runtime::run_command("git", &owned, Some(dir))
            .unwrap_or_else(|error| panic!("git {args:?} spawn failed: {error}"));
        assert_eq!(
            result.code,
            0,
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&result.stderr)
        );
    }

    /// Create an initialized temp git repo with one commit on `main` and a
    /// deterministic identity/branch so preflight checks are reproducible.
    fn init_temp_repo(label: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let dir = std::env::temp_dir().join(format!(
            "keel-preflight-{label}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("create temp repo dir");
        git_in(&dir, &["init", "-q"]);
        git_in(&dir, &["config", "user.email", "test@example.com"]);
        git_in(&dir, &["config", "user.name", "Test"]);
        git_in(&dir, &["checkout", "-q", "-B", "main"]);
        std::fs::write(dir.join("README.md"), "base\n").unwrap();
        git_in(&dir, &["add", "."]);
        git_in(&dir, &["commit", "-q", "-m", "chore: base commit"]);
        dir
    }

    fn run_preflight(repo: &std::path::Path, base_ref: &str) -> (u8, String) {
        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let code = run_git_workflow_preflight(
            &[
                "--repo-root".to_string(),
                repo.to_string_lossy().to_string(),
                "--base-ref".to_string(),
                base_ref.to_string(),
            ],
            &mut stdout,
            &mut stderr,
        );
        (code, String::from_utf8_lossy(&stdout).to_string())
    }

    #[test]
    fn preflight_passes_on_clean_task_branch_ahead_of_base() {
        let repo = init_temp_repo("pass");
        // Preferred work branch: task/<task>
        git_in(&repo, &["checkout", "-q", "-b", "task/widget"]);
        std::fs::write(repo.join("widget.txt"), "feature\n").unwrap();
        git_in(&repo, &["add", "."]);
        git_in(&repo, &["commit", "-q", "-m", "Add : WIDGET : add widget"]);

        let (code, stdout) = run_preflight(&repo, "main");
        assert_eq!(code, 0, "stdout: {stdout}");
        assert!(stdout.contains("PASS"), "stdout: {stdout}");
        assert!(
            !stdout.to_lowercase().contains("legacy"),
            "preferred task/ branch must not warn as legacy: {stdout}"
        );

        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn preflight_allows_legacy_branch_with_warning() {
        let repo = init_temp_repo("legacy");
        git_in(&repo, &["checkout", "-q", "-b", "add/WIDGET"]);
        std::fs::write(repo.join("widget.txt"), "feature\n").unwrap();
        git_in(&repo, &["add", "."]);
        git_in(&repo, &["commit", "-q", "-m", "add: WIDGET: add widget"]);

        let (code, stdout) = run_preflight(&repo, "main");
        assert_eq!(code, 0, "legacy must still pass: {stdout}");
        assert!(
            stdout.to_lowercase().contains("legacy"),
            "legacy prefix should warn: {stdout}"
        );

        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn preflight_blocks_on_protected_branch() {
        let repo = init_temp_repo("protected");
        // Still on main (final-stable; never pushed from directly).
        let (code, stdout) = run_preflight(&repo, "main");
        assert_eq!(code, 1, "stdout: {stdout}");
        assert!(stdout.contains("final-stable branch"), "stdout: {stdout}");

        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn preflight_blocks_on_unsanctioned_branch_name() {
        let repo = init_temp_repo("badname");
        git_in(&repo, &["checkout", "-q", "-b", "random-branch"]);
        std::fs::write(repo.join("x.txt"), "x\n").unwrap();
        git_in(&repo, &["add", "."]);
        git_in(&repo, &["commit", "-q", "-m", "add: X: x"]);

        let (code, stdout) = run_preflight(&repo, "main");
        assert_eq!(code, 1, "stdout: {stdout}");
        assert!(stdout.contains("sanctioned"), "stdout: {stdout}");

        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn preflight_warns_on_integration_tier_branch() {
        // Standing on `feat` is valid only for promotion; preflight allows it
        // through (no block) but flags it so an accidental direct commit to the
        // tier is visible.
        let repo = init_temp_repo("tier");
        git_in(&repo, &["checkout", "-q", "-b", "feat"]);
        std::fs::write(repo.join("f.txt"), "f\n").unwrap();
        git_in(&repo, &["add", "."]);
        git_in(&repo, &["commit", "-q", "-m", "add: FEAT: integration"]);

        let (code, stdout) = run_preflight(&repo, "main");
        assert_eq!(
            code, 0,
            "integration tier is a warning, not a block: {stdout}"
        );
        assert!(stdout.contains("integration tier"), "stdout: {stdout}");

        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn preflight_blocks_on_dirty_worktree() {
        let repo = init_temp_repo("dirty");
        git_in(&repo, &["checkout", "-q", "-b", "fix/THING"]);
        std::fs::write(repo.join("thing.txt"), "committed\n").unwrap();
        git_in(&repo, &["add", "."]);
        git_in(&repo, &["commit", "-q", "-m", "fix: THING: thing"]);
        // Now leave an uncommitted change in the worktree.
        std::fs::write(repo.join("thing.txt"), "dirty edit\n").unwrap();

        let (code, stdout) = run_preflight(&repo, "main");
        assert_eq!(code, 1, "stdout: {stdout}");
        assert!(stdout.contains("uncommitted change"), "stdout: {stdout}");

        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn preflight_blocks_when_no_commits_ahead_of_base() {
        let repo = init_temp_repo("nocommits");
        // A sanctioned, clean work branch with NO commits beyond main.
        git_in(&repo, &["checkout", "-q", "-b", "add/EMPTY"]);
        let (code, stdout) = run_preflight(&repo, "main");
        assert_eq!(code, 1, "stdout: {stdout}");
        assert!(
            stdout.contains("no commits on HEAD ahead"),
            "stdout: {stdout}"
        );

        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn preflight_blocks_when_base_ref_missing() {
        let repo = init_temp_repo("nobase");
        git_in(&repo, &["checkout", "-q", "-b", "fix/THING"]);
        std::fs::write(repo.join("a.txt"), "a\n").unwrap();
        git_in(&repo, &["add", "."]);
        git_in(&repo, &["commit", "-q", "-m", "fix: THING: a"]);

        let (code, stdout) = run_preflight(&repo, "origin/does-not-exist");
        assert_eq!(code, 1, "stdout: {stdout}");
        assert!(stdout.contains("not found"), "stdout: {stdout}");

        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn preflight_blocks_on_non_git_directory() {
        let dir =
            std::env::temp_dir().join(format!("keel-preflight-nongit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (code, _stdout) = run_preflight(&dir, "main");
        assert_eq!(code, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn preflight_warns_on_commit_subject_prefix_drift() {
        let repo = init_temp_repo("drift");
        git_in(&repo, &["checkout", "-q", "-b", "add/MSG"]);
        std::fs::write(repo.join("m.txt"), "m\n").unwrap();
        git_in(&repo, &["add", "."]);
        // Non-conventional subject → should produce a [warn], not a block.
        git_in(&repo, &["commit", "-q", "-m", "random message no prefix"]);

        let (code, stdout) = run_preflight(&repo, "main");
        assert_eq!(code, 0, "drift is a warning, not a block: {stdout}");
        assert!(stdout.contains("conventional prefix"), "stdout: {stdout}");

        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn preflight_json_format_emits_structured_payload() {
        let repo = init_temp_repo("json");
        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let code = run_git_workflow_preflight(
            &[
                "--repo-root".to_string(),
                repo.to_string_lossy().to_string(),
                "--base-ref".to_string(),
                "main".to_string(),
                "--format".to_string(),
                "json".to_string(),
            ],
            &mut stdout,
            &mut stderr,
        );
        // On main with no commits ahead → blocked.
        assert_eq!(code, 1);
        let text = String::from_utf8_lossy(&stdout);
        assert!(text.contains("\"passed\""), "stdout: {text}");
        assert!(text.contains("\"blocking\""), "stdout: {text}");
        assert!(text.contains("\"branch\""), "stdout: {text}");
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn e2e_config_detected_when_playwright_exists() {
        let temp = std::env::temp_dir().join(format!(
            "keel-e2e-pw-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        std::fs::create_dir_all(&temp).unwrap();
        std::fs::write(temp.join("playwright.config.ts"), "export default {}").unwrap();

        let result = check_e2e_config(&temp);
        assert!(result.is_some(), "should detect playwright.config.ts");
        let gate = result.unwrap();
        assert_eq!(gate.name, "e2e_verification");
        assert_eq!(gate.status, GateStatus::Pass);
        assert!(!gate.blocking);
        let details = gate.details.unwrap();
        assert!(details.contains("Playwright"));
        assert!(details.contains("npx playwright test"));

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn e2e_config_detected_when_cypress_exists() {
        let temp = std::env::temp_dir().join(format!(
            "keel-e2e-cy-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        std::fs::create_dir_all(&temp).unwrap();
        std::fs::write(temp.join("cypress.config.js"), "module.exports={}").unwrap();

        let result = check_e2e_config(&temp);
        assert!(result.is_some(), "should detect cypress.config.js");
        let gate = result.unwrap();
        assert_eq!(gate.name, "e2e_verification");
        let details = gate.details.unwrap();
        assert!(details.contains("Cypress"));
        assert!(details.contains("npx cypress run"));

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn e2e_config_absent_returns_none() {
        let temp = std::env::temp_dir().join(format!(
            "keel-e2e-none-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        std::fs::create_dir_all(&temp).unwrap();

        let result = check_e2e_config(&temp);
        assert!(result.is_none(), "no E2E config means no gate result");

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn e2e_config_not_blocking_in_tally() {
        let e2e_gate = GateResult {
            name: "e2e_verification".to_string(),
            status: GateStatus::Pass,
            blocking: false,
            details: Some("Playwright detected".to_string()),
        };
        let results = vec![
            GateResult {
                name: "rust_tests".to_string(),
                status: GateStatus::Fail,
                blocking: true,
                details: None,
            },
            e2e_gate,
        ];
        let (blocking, warnings) = tally_gate_results(&results);
        assert_eq!(blocking, 1, "E2E should not add blocking findings");
        assert_eq!(warnings, 0);
    }

    #[test]
    fn impact_flag_defaults_to_false() {
        let flag_set = review_flag_set("review pre-pr");
        assert!(
            !flag_set.bool_value("impact"),
            "impact gate must be opt-in to keep default review fast"
        );
    }

    #[test]
    fn impact_gate_result_is_never_blocking() {
        let gate = GateResult {
            name: "impact".to_string(),
            status: GateStatus::Pass,
            blocking: false,
            details: Some("3 changed, 2 impacted: a.ts, b.ts".to_string()),
        };
        let results = vec![gate];
        let (blocking, _) = tally_gate_results(&results);
        assert_eq!(blocking, 0, "impact gate must never block review");
    }
}
