#![allow(unused_imports)]
//! Rust-native review and git-workflow command facade.

use std::io::Write;
use std::path::Path;

use crate::args::FlagSet;
use crate::json::{write_indented, Value};
use crate::runtime::run_command;

mod ci;
mod closeout;
mod diff_gates;
mod hosted;
mod language_gates;
mod messages;
mod workflow;

pub(super) use ci::{
    ci_run_matches_head, classify_check_state, cleaned_header, cli_available, detect_ci_provider,
    evaluate_checks, parse_gh_checks, parse_glab_status, query_checks_gh, query_checks_glab,
    query_provider_checks, render_await_ci_result, resolve_provider, run_git_workflow_await_ci,
    AwaitCiOutcome, CheckState, CiCheck, CiProvider, CiQuery, CiVerdict, ProviderResolution,
};

pub(super) use diff_gates::{
    artifact_targets_a_touched_file, brownfield_source_from_name_status,
    changed_sources_including_added, collect_review_gate_results, comment_style_gate,
    completeness_check_gate, completeness_source_from_name_status, completeness_touched_sources,
    flow_check_gate, impact_gate, modified_existing_sources, newest_source_mtime_ms,
    preview_touched_paths, prose_style_gate, run_review_surface_command, slop_gate,
    FLOW_EXEMPT_SEGMENTS, FLOW_SOURCE_EXTENSIONS,
};

pub(super) use hosted::{
    run_review_comments_command, run_review_hosted_command, run_review_policy_command,
};

pub(super) use language_gates::{
    check_black, check_circular_imports, check_clang_format, check_e2e_config, check_eslint,
    check_for_extensions, check_go_test, check_go_vet, check_gofmt, check_import_safety,
    check_mypy, check_npm_test, check_prettier, check_python_tests, check_ruff, check_tsc,
    classify_python_test_exit, collect_cpp_source_files, has_cpp_project, has_go_project,
    has_js_files, has_js_project, has_python_files, has_python_project, render_gate_results,
    run_cpp_surface_gates, run_go_surface_gates, run_js_surface_gates, run_python_surface_gates,
    run_review_gates_command, run_rust_surface_gates, tally_gate_results, GateResult, GateStatus,
    E2E_CONFIG_FILENAMES,
};

pub(super) use messages::{
    changed_files_against_base, commit_body_from_staged, derive_scope, detect_category,
    generate_commit_subject, git_diff_stat, git_diff_stat_against_base, is_ci_path, is_config_path,
    is_docs_path, is_source_path, is_test_path, lint_message, normalize_commit_category,
    pr_summary_bullets, preview_paths, render_generated_message, staged_files, subject_summary,
    validate_commit_subject, COMMIT_CATEGORIES,
};

pub(super) use workflow::{
    commit_subject_has_sanctioned_prefix, render_preflight_result, run_git_workflow_configure,
    run_git_workflow_preflight, run_git_workflow_show, truncate_subject, workflow_pref_store,
    workflow_slug, DEFAULT_BRANCH_TIERS, LEGACY_BRANCH_PREFIXES, PREFERRED_BRANCH_PREFIXES,
    SANCTIONED_BRANCH_PREFIXES, SANCTIONED_COMMIT_PREFIXES, WORKFLOW_PREF_RECORD_ID,
};
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
        "closeout" => {
            closeout::run_review_closeout_command(&arguments[1..], standard_output, standard_error)
        }
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

/// Run a git command in `repo`, returning raw stdout (callers trim as needed)
/// on exit code 0, or `None` on non-zero exit or spawn failure.
pub(super) fn git_text(repo: Option<&Path>, args: &[&str]) -> Option<String> {
    let owned: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
    let result = run_command("git", &owned, repo).ok()?;
    if result.code != 0 {
        return None;
    }
    Some(String::from_utf8_lossy(&result.stdout).to_string())
}

/// Run a git command and return its exit code (or None on spawn failure).
pub(super) fn git_exit_code(repo: Option<&Path>, args: &[&str]) -> Option<i32> {
    let owned: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
    run_command("git", &owned, repo)
        .ok()
        .map(|result| result.code)
}

/// Run a git command and split stdout into non-empty trimmed lines.
pub(super) fn git_lines(repo: Option<&Path>, args: &[&str]) -> Option<Vec<String>> {
    git_text(repo, args).map(|text| {
        text.lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect()
    })
}
pub(super) fn review_flag_set(name: &str) -> FlagSet {
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

pub(super) fn render_gate_result(
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
            let _ = write_indented(standard_output, &payload); // broken pipes do not change gate status
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
        "Usage: keel review [closeout|pre-commit|pre-pr|diff|gates|hosted|policy|comments] ..."
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
mod tests;
