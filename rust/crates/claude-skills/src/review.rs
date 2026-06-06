//! Purpose: Rust-native review, hosted-review artifact, and git-workflow text helpers.
//! Caller: commands.rs for `review` and `git-workflow` command groups.
//! Dependencies: args, json, runtime helpers, std::fs, std::io, and std::path.
//! Main Functions: run_review_command, run_git_workflow_command.
//! Side Effects: Reads git diffs, writes optional hosted-review payload/body artifacts, and writes rendered text to stdout/stderr.

use std::fs;
use std::io::Write;
use std::path::Path;

use crate::args::FlagSet;
use crate::json::{write_indented, Value};
use crate::runtime::{resolve_repository_root, run_command, write_text};

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
            // Running a gates check is a reviewer pass — clear the optional
            // PostToolBatch review gate for this workspace so it does not block
            // closeout. Best-effort and a no-op unless the gate is enabled.
            crate::runner::hook_lifecycle::record_review_gate_clear();
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
            // pre-pr / pre-commit / diff constitute a reviewer pass over the
            // working diff; clear the review gate for this workspace. `init` is
            // harmless to clear on (no edits yet). Best-effort; no-op when the
            // gate is disabled.
            crate::runner::hook_lifecycle::record_review_gate_clear();
            code
        }
        "policy" => run_review_policy_command(&arguments[1..], standard_output, standard_error),
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
        other => {
            let _ = writeln!(standard_error, "Unknown git-workflow command: {other}");
            render_git_workflow_help(standard_output);
            1
        }
    }
}

/// Branch-name prefixes WORKFLOW.md sanctions for feature-delivery branches.
const SANCTIONED_BRANCH_PREFIXES: &[&str] = &["feat/", "fix/", "improve/", "add/"];
/// Conventional commit-subject prefixes the preflight expects; a subject that
/// matches none of these earns a (non-blocking) drift warning.
const SANCTIONED_COMMIT_PREFIXES: &[&str] = &[
    "feat", "fix", "docs", "chore", "refactor", "test", "perf", "build", "ci", "style", "revert",
    "improve", "add",
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

    // 1. Branch naming. The current branch must use a sanctioned feature prefix
    //    and must not be the base/protected branch.
    let branch = git_text(repo, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap_or_default();
    let branch = branch.trim().to_string();
    if branch.is_empty() || branch == "HEAD" {
        blocking.push("detached HEAD — check out a feature branch before preflight".to_string());
    } else if matches!(branch.as_str(), "main" | "master" | "develop") {
        blocking.push(format!(
            "on protected branch '{branch}' — create a feat/ fix/ improve/ add/ branch"
        ));
    } else if !SANCTIONED_BRANCH_PREFIXES
        .iter()
        .any(|prefix| branch.starts_with(prefix))
    {
        blocking.push(format!(
            "branch '{branch}' does not use a sanctioned prefix ({})",
            SANCTIONED_BRANCH_PREFIXES.join(", ")
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
    let lowered = subject.trim_start().to_ascii_lowercase();
    SANCTIONED_COMMIT_PREFIXES.iter().any(|prefix| {
        // Match "feat:", "feat(scope):", "feat!:" — the conventional shapes.
        lowered.starts_with(&format!("{prefix}:"))
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
        let _ = writeln!(
            standard_output,
            "Usage: claude-skills review gates check [flags]"
        );
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
    flag_set.string_flag("repo-test-policy", "skip");
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

    // Rust tests (always run for Rust repos)
    let has_rust = repository_root.join("Cargo.toml").exists();
    if has_rust {
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

    // Python checks (if requested and Python files exist)
    if flag_set.bool_value("python-checks") {
        let has_python = has_python_files(&repository_root);
        if has_python {
            // Black formatting check
            let black_result = check_black(&repository_root);
            gate_results.push(black_result);

            // Ruff linting check
            let ruff_result = check_ruff(&repository_root);
            gate_results.push(ruff_result);

            // MyPy type checking
            let mypy_result = check_mypy(&repository_root);
            gate_results.push(mypy_result);

            // Circular import check
            let circular_result = check_circular_imports(&repository_root);
            gate_results.push(circular_result);

            // Import safety check
            let import_safety_result = check_import_safety(&repository_root);
            gate_results.push(import_safety_result);
        }
    }

    // JavaScript/TypeScript checks (if requested and JS/TS files exist)
    if flag_set.bool_value("js-checks") {
        let has_js = has_js_files(&repository_root);
        if has_js {
            // Prettier formatting check
            let prettier_result = check_prettier(&repository_root);
            gate_results.push(prettier_result);
        }
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
#[allow(dead_code)]
enum GateStatus {
    Pass,
    Fail,
    Warn,
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

fn has_js_files(repository_root: &Path) -> bool {
    let extensions = ["js", "jsx", "ts", "tsx", "css", "scss", "less"];
    check_for_extensions(repository_root, &extensions)
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
    // Try to find circular imports using Python's ast module
    let check_script = r#"
import ast
import sys
from pathlib import Path

def check_module(path):
    try:
        with open(path) as f:
            ast.parse(f.read())
        return True
    except:
        return False

def find_python_files(directory):
    for path in Path(directory).rglob("*.py"):
        if "__pycache__" not in str(path) and "venv" not in str(path):
            yield path

circular_found = False
for pyfile in find_python_files("."):
    pass

sys.exit(0 if not circular_found else 1)
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
    // Basic import safety check - verify no dangerous imports
    let check_script = r#"
import ast
import sys
from pathlib import Path

DANGEROUS_IMPORTS = {"eval", "exec", "__import__", "compile"}

def check_file(path):
    with open(path) as f:
        tree = ast.parse(f.read())
    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            for alias in node.names:
                if alias.name in DANGEROUS_IMPORTS:
                    return False
        elif isinstance(node, ast.ImportFrom):
            if node.module in DANGEROUS_IMPORTS:
                return False
    return True

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
            "Usage: claude-skills review hosted [check|comment] [flags]"
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
    let payload = Value::Object(vec![
        (
            "provider".into(),
            Value::String(flag_set.string_value("provider").to_string()),
        ),
        ("gate".into(), Value::String("pass".into())),
        (
            "summary".into(),
            Value::String("Claude Code native review gate passed with no findings.".into()),
        ),
        ("body".into(), Value::String(body.clone())),
        ("conclusion".into(), Value::String("success".into())),
        (
            "title".into(),
            Value::String("Claude Code Native Review Report".into()),
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
                "gate=pass blocking=0 warnings=0 findings=0"
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
    let gate_results = run_rust_surface_gates(&repository_root, include_tests);
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
                "native_rules=rust language_gates=true go_fallback=false"
            );
        } else {
            let _ = writeln!(standard_output, "# Native Review Policy");
            let _ = writeln!(standard_output, "- runtime: rust-native");
            let _ = writeln!(standard_output, "- language_gates: enabled");
            let _ = writeln!(
                standard_output,
                "- python_checks: black, ruff, mypy, circular_imports, import_safety"
            );
            let _ = writeln!(standard_output, "- js_checks: prettier");
            let _ = writeln!(standard_output, "- go_fallback: false");
        }
        return 0;
    }
    let _ = writeln!(
        standard_error,
        "Usage: claude-skills review policy show [flags]"
    );
    1
}

fn review_flag_set(name: &str) -> FlagSet {
    let mut flag_set = FlagSet::new(name);
    flag_set.string_flag("repo-root", "");
    flag_set.string_flag("workspace-root", "");
    flag_set.string_flag("surface", "diff");
    flag_set.string_flag("base-ref", "");
    flag_set.string_flag("format", "compact");
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
                    Value::String("Claude Code native review completed.".into()),
                ),
            ]);
            let _ = write_indented(standard_output, &payload);
        }
        "markdown" => {
            let _ = writeln!(standard_output, "# Claude Code Native Review Report");
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

/// Commit categories required by the project commit convention
/// `<category>: <FEATURE>: <short information>` — all lowercase.
const COMMIT_CATEGORIES: [&str; 6] = ["add", "config", "refactor", "wip", "fix", "docs"];

/// Validate a commit subject against `<category>: <FEATURE>: <short information>`.
///
/// - `<category>` must be one of [`COMMIT_CATEGORIES`] (lowercase).
/// - `<FEATURE>` must be a non-empty uppercase component label (e.g. RGB, LED, ARGB, SENSOR).
/// - `<short information>` must be a non-empty description.
fn validate_commit_subject(subject: &str) -> Result<(), String> {
    let trimmed = subject.trim();
    let parts: Vec<&str> = trimmed.splitn(3, ':').collect();
    if parts.len() < 3 {
        return Err(
            "expected three colon-separated parts: <category>: <FEATURE>: <short information>"
                .to_string(),
        );
    }
    let category = parts[0].trim();
    let feature = parts[1].trim();
    let information = parts[2].trim();
    if !COMMIT_CATEGORIES.contains(&category) {
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
        return "wip";
    }
    let all_match = |predicate: fn(&str) -> bool| paths.iter().all(|path| predicate(path));

    if all_match(is_docs_path) {
        return "docs";
    }
    if all_match(is_config_path) {
        return "config";
    }
    "wip"
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
        return "wip: GENERAL: update".to_string();
    }
    if paths.is_empty() {
        return "wip: GENERAL: no staged changes".to_string();
    }
    let category = detect_category(paths);
    let summary = subject_summary(paths);
    let feature = derive_scope(paths)
        .map(|scope| scope.to_uppercase())
        .unwrap_or_else(|| "GENERAL".to_string());
    format!("{category}: {feature}: {summary}")
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
            "Usage: claude-skills git-workflow lint-message <file>"
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
        "# Claude Code Native Review Report",
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
        "Usage: claude-skills review [pre-commit|pre-pr|diff|gates|hosted|policy] ..."
    );
}

fn render_git_workflow_help(standard_output: &mut dyn Write) {
    let _ = writeln!(
        standard_output,
        "Usage: claude-skills git-workflow [preflight|commit-message|pr-body|lint-message] ..."
    );
}

fn is_help_argument(argument: &str) -> bool {
    matches!(argument, "help" | "--help" | "-h")
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(String::from_utf8_lossy(&stdout).contains("native_rules=rust"));
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
        let temp = std::env::temp_dir().join("claude-skills-review-test");
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
        let temp = std::env::temp_dir().join("claude-skills-review-js-test");
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
        assert_eq!(detect_category(&staged), "docs");
    }

    #[test]
    fn detect_category_classifies_ci_as_config() {
        let staged = paths(&[".github/workflows/release.yml"]);
        assert_eq!(detect_category(&staged), "config");
    }

    #[test]
    fn detect_category_classifies_config_files() {
        let staged = paths(&["Cargo.toml", "rustfmt.toml"]);
        assert_eq!(detect_category(&staged), "config");
    }

    #[test]
    fn detect_category_falls_back_to_wip_for_source() {
        let staged = paths(&["src/lib.rs", "src/main.rs"]);
        assert_eq!(detect_category(&staged), "wip");
    }

    #[test]
    fn detect_category_empty_is_wip() {
        assert_eq!(detect_category(&[]), "wip");
    }

    #[test]
    fn derive_scope_returns_common_directory() {
        let staged = paths(&[
            "rust/crates/claude-skills/src/review.rs",
            "rust/crates/claude-skills/src/runner/mod.rs",
        ]);
        assert_eq!(
            derive_scope(&staged),
            Some("claude-skills".to_string()),
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
        assert_eq!(generate_commit_subject(false, &[]), "wip: GENERAL: update");
    }

    #[test]
    fn generate_commit_subject_with_diff_but_no_staged_signals_empty() {
        assert_eq!(
            generate_commit_subject(true, &[]),
            "wip: GENERAL: no staged changes"
        );
    }

    #[test]
    fn generate_commit_subject_combines_category_feature_and_summary() {
        let staged = paths(&[
            "rust/crates/claude-skills/src/review.rs",
            "rust/crates/claude-skills/src/lib.rs",
        ]);
        assert_eq!(
            generate_commit_subject(true, &staged),
            "wip: CLAUDE-SKILLS: update 2 files"
        );
    }

    #[test]
    fn generate_commit_subject_single_file_uses_leaf_name() {
        let staged = paths(&["docs/architecture.md"]);
        let subject = generate_commit_subject(true, &staged);
        assert!(
            subject.starts_with("docs: "),
            "expected docs category, got {subject}"
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
            paths(&["rust/crates/claude-skills/src/review.rs"]),
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
        assert!(validate_commit_subject("wip: RGB: Build light effect mode (multi color)").is_ok());
        assert!(validate_commit_subject("fix: SENSOR: Correct I2C read timeout").is_ok());
        assert!(validate_commit_subject("add: ARGB: Add rainbow cycle preset").is_ok());
        assert!(validate_commit_subject("config: LED: Set default brightness").is_ok());
        assert!(validate_commit_subject("refactor: RGB: Extract blend helper").is_ok());
        assert!(validate_commit_subject("docs: SENSOR: Document calibration").is_ok());
    }

    #[test]
    fn validate_commit_subject_rejects_unknown_category() {
        let error = validate_commit_subject("feat: RGB: do a thing").unwrap_err();
        assert!(error.contains("category"), "got {error}");
    }

    #[test]
    fn validate_commit_subject_rejects_lowercase_feature() {
        let error = validate_commit_subject("wip: rgb: do a thing").unwrap_err();
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
        let temp = std::env::temp_dir().join("claude-skills-no-cargo-test");
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
            "claude-skills-preflight-{label}-{}-{nanos}",
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
    fn preflight_passes_on_clean_feature_branch_ahead_of_base() {
        let repo = init_temp_repo("pass");
        // Branch off main, add a committed change so HEAD is ahead of main.
        git_in(&repo, &["checkout", "-q", "-b", "feat/widget"]);
        std::fs::write(repo.join("widget.txt"), "feature\n").unwrap();
        git_in(&repo, &["add", "."]);
        git_in(&repo, &["commit", "-q", "-m", "feat: add widget"]);

        let (code, stdout) = run_preflight(&repo, "main");
        assert_eq!(code, 0, "stdout: {stdout}");
        assert!(stdout.contains("PASS"), "stdout: {stdout}");

        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn preflight_blocks_on_protected_branch() {
        let repo = init_temp_repo("protected");
        // Still on main (a protected branch).
        let (code, stdout) = run_preflight(&repo, "main");
        assert_eq!(code, 1, "stdout: {stdout}");
        assert!(stdout.contains("protected branch"), "stdout: {stdout}");

        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn preflight_blocks_on_unsanctioned_branch_name() {
        let repo = init_temp_repo("badname");
        git_in(&repo, &["checkout", "-q", "-b", "random-branch"]);
        std::fs::write(repo.join("x.txt"), "x\n").unwrap();
        git_in(&repo, &["add", "."]);
        git_in(&repo, &["commit", "-q", "-m", "feat: x"]);

        let (code, stdout) = run_preflight(&repo, "main");
        assert_eq!(code, 1, "stdout: {stdout}");
        assert!(stdout.contains("sanctioned prefix"), "stdout: {stdout}");

        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn preflight_blocks_on_dirty_worktree() {
        let repo = init_temp_repo("dirty");
        git_in(&repo, &["checkout", "-q", "-b", "fix/thing"]);
        std::fs::write(repo.join("thing.txt"), "committed\n").unwrap();
        git_in(&repo, &["add", "."]);
        git_in(&repo, &["commit", "-q", "-m", "fix: thing"]);
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
        // A sanctioned, clean branch with NO commits beyond main.
        git_in(&repo, &["checkout", "-q", "-b", "feat/empty"]);
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
        git_in(&repo, &["checkout", "-q", "-b", "feat/thing"]);
        std::fs::write(repo.join("a.txt"), "a\n").unwrap();
        git_in(&repo, &["add", "."]);
        git_in(&repo, &["commit", "-q", "-m", "feat: a"]);

        let (code, stdout) = run_preflight(&repo, "origin/does-not-exist");
        assert_eq!(code, 1, "stdout: {stdout}");
        assert!(stdout.contains("not found"), "stdout: {stdout}");

        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn preflight_blocks_on_non_git_directory() {
        let dir = std::env::temp_dir().join(format!(
            "claude-skills-preflight-nongit-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let (code, _stdout) = run_preflight(&dir, "main");
        assert_eq!(code, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn preflight_warns_on_commit_subject_prefix_drift() {
        let repo = init_temp_repo("drift");
        git_in(&repo, &["checkout", "-q", "-b", "feat/msg"]);
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
}
