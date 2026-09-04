use super::*;
use crate::runtime::{resolve_claude_home, resolve_repository_root};
use crate::utility::record_store::RecordStore;

/// Preferred work-branch prefix going forward (`task/<task>`). Integration
/// stays `feat` (a bare branch name);
/// work must NOT use `feat/` because Git cannot store both `refs/heads/feat`
/// and `refs/heads/feat/...` at once.
pub(crate) const PREFERRED_BRANCH_PREFIXES: &[&str] = &["task/"];

/// Legacy work-branch prefixes still accepted so in-flight branches keep
/// working. Preflight warns (does not block) and asks for `task/` going forward.
pub(crate) const LEGACY_BRANCH_PREFIXES: &[&str] = &[
    "add/",
    "config/",
    "refactor/",
    "wip/",
    "fix/",
    "docs/",
    "feature/",
];

/// All prefixes preflight will allow (preferred + legacy). Unknown prefixes block.
pub(crate) const SANCTIONED_BRANCH_PREFIXES: &[&str] = &[
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
pub(crate) const DEFAULT_BRANCH_TIERS: &str = "main <- dev <- feat <- task/<task>";

/// Conventional commit-subject prefixes the preflight expects; a subject that
/// matches none of these (and fails the keel colon form) earns a non-blocking
/// drift warning. Includes lowercase category tokens used by both the new
/// `Add : FEATURE : info` form and legacy `add: FEATURE: info`.
pub(crate) const SANCTIONED_COMMIT_PREFIXES: &[&str] = &[
    "feat", "fix", "docs", "chore", "refactor", "test", "perf", "build", "ci", "style", "revert",
    "improve", "add", "config", "wip",
];

/// Run the native Git workflow preflight described in WORKFLOW.md: block on
/// branch naming, dirty worktrees, empty diffs, and missing committed history
/// against the target base ref; warn on commit-subject prefix drift. Returns 0
/// when no blocking check fails, 1 otherwise. Warnings never change the exit
/// code. A non-git directory or unreadable git state is a blocking failure —
/// preflight cannot vouch for what it cannot inspect.
pub(crate) fn run_git_workflow_preflight(
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

    // Confirm the repository is a work tree; otherwise a green result is invalid.
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

    // 1. Branch naming. Hierarchy: main from dev from feat from task/<task>
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
        warnings.push(format!(
            "on integration tier '{branch}' — only valid when promoting upward (feat→dev→main), not for hands-on work"
        ));
    } else if PREFERRED_BRANCH_PREFIXES
        .iter()
        .any(|prefix| branch.starts_with(prefix))
    {
        // Preferred: task/<task>. Use separate flat task names for parallel work;
        // Git cannot create task/x/y while the task/x branch ref exists.
    } else if LEGACY_BRANCH_PREFIXES
        .iter()
        .any(|prefix| branch.starts_with(prefix))
    {
        warnings.push(format!(
            "branch '{branch}' uses a legacy work-branch prefix — keep working; new branches should use task/<task>"
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

    // Validate the base ref before evaluating committed history and the diff.
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

pub(crate) fn commit_subject_has_sanctioned_prefix(subject: &str) -> bool {
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

pub(crate) fn truncate_subject(subject: &str) -> String {
    const MAX: usize = 60;
    if subject.chars().count() <= MAX {
        subject.to_string()
    } else {
        let truncated: String = subject.chars().take(MAX).collect();
        format!("{truncated}…")
    }
}

// persist the chosen branch+commit workflow to the global per-workspace
// memory lane so it survives sessions; this records the model, not new formats.

/// The four-tier model is the supported default; `configure` records the user's
/// choice (and notes) so `show` and later sessions recall it.
pub(crate) const WORKFLOW_PREF_RECORD_ID: &str = "active";

pub(crate) fn workflow_pref_store(repository_root: &Path, claude_home: &Path) -> RecordStore {
    let slug = workflow_slug(&repository_root.to_string_lossy());
    let canonical_group = format!("memories/workspaces/{slug}/git-workflow");
    let canonical_record = claude_home
        .join(&canonical_group)
        .join(format!("{WORKFLOW_PREF_RECORD_ID}.json"));
    if !canonical_record.is_file() {
        for alias in
            crate::utility::system_map::workspace_key_aliases(&repository_root.to_string_lossy())
                .into_iter()
                .skip(1)
        {
            let legacy_record = claude_home
                .join("memories")
                .join("workspaces")
                .join(alias)
                .join("git-workflow")
                .join(format!("{WORKFLOW_PREF_RECORD_ID}.json"));
            if legacy_record.is_file() {
                if let Some(parent) = canonical_record.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                if std::fs::copy(&legacy_record, &canonical_record).is_err() {
                    return RecordStore::new(
                        claude_home,
                        &legacy_record
                            .parent()
                            .and_then(|path| path.strip_prefix(claude_home).ok())
                            .map(|path| path.to_string_lossy().replace('\\', "/"))
                            .unwrap_or_else(|| canonical_group.clone()),
                    );
                }
                break;
            }
        }
    }
    RecordStore::new(claude_home, &canonical_group)
}

/// Slug a workspace path into a safe directory segment (mirrors the SYSTEM_MAP
/// per-workspace lane naming).
pub(crate) fn workflow_slug(raw: &str) -> String {
    crate::utility::system_map::workspace_key(raw)
}

pub(crate) fn run_git_workflow_configure(
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
    if model != "four-tier" {
        let _ = writeln!(
            standard_error,
            "git-workflow configure: supported model is four-tier; received '{}'",
            if model.is_empty() { "<empty>" } else { &model }
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

pub(crate) fn run_git_workflow_show(
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

pub(crate) fn render_preflight_result(
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
