//! Tests for memory subcommands that exercise multiple submodules through the
//! `run_memory_command` or `run_orchestration_command` dispatch entry points.
//! These tests call across module boundaries and are intentionally kept together
//! rather than scattered across the individual submodules.

use super::*;
use crate::test_support::ENV_LOCK;
use crate::utility::workflow_ledger::{
    close_entry, create_entry, format_timestamp_iso8601, write_entry, Entry,
};
use std::fs;

fn tempdir_under(label: &str) -> std::path::PathBuf {
    let unique_suffix: u128 = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let candidate = std::env::temp_dir().join(format!("{label}-{unique_suffix}"));
    fs::create_dir_all(&candidate).expect("create tempdir");
    candidate
}

fn seeded_open_entry(claude_home: &std::path::Path, id: &str, request: &str) -> Entry {
    let entry = create_entry(
        id.to_string(),
        request.to_string(),
        "feature".to_string(),
        format_timestamp_iso8601(0),
    );
    write_entry(claude_home, &entry).expect("seed open entry");
    entry
}

fn task_id_from_stdout(stdout: &[u8]) -> String {
    let text = String::from_utf8_lossy(stdout);
    text.lines()
        .find_map(|line| line.trim().strip_prefix("orchestration task begin: id="))
        .map(|id| id.trim().to_string())
        .expect("begin output must include an id")
}

#[test]
fn memory_scope_defaults_to_global_workspace_reference_map() {
    let _guard = ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temporary_directory = tempdir_under("keel-memory-scope-global");
    let claude_home = temporary_directory.join("claude-home");
    let workspace_root = temporary_directory.join("workspace");
    fs::create_dir_all(&workspace_root).expect("create workspace");
    fs::write(workspace_root.join("README.md"), "# Workspace\n").expect("write readme");
    let previous_override = std::env::var("CLAUDE_TARGET_OVERRIDE").ok();
    std::env::set_var("CLAUDE_TARGET_OVERRIDE", &claude_home);

    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let exit_code = run_memory_command(
        "memory",
        &[
            "scope".to_string(),
            "resolve".to_string(),
            "--workspace-root".to_string(),
            workspace_root.to_string_lossy().to_string(),
            "--create-missing".to_string(),
            "--refresh-system-map".to_string(),
            "--format".to_string(),
            "compact".to_string(),
        ],
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(exit_code, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
    let output = String::from_utf8_lossy(&stdout);
    assert!(output.contains("system_map_path="));
    let workspace_key =
        crate::utility::system_map::sanitize_key(&crate::runtime::display_path(&workspace_root));
    let expected_system_map = claude_home
        .join("memories")
        .join("workspaces")
        .join(workspace_key)
        .join("reference")
        .join("SYSTEM_MAP.md");
    assert!(expected_system_map.is_file());
    assert!(!workspace_root.join("SYSTEM_MAP.md").exists());
    let system_map = fs::read_to_string(expected_system_map).expect("read system map");
    assert!(system_map.contains("# SYSTEM_MAP"));
    assert!(system_map.contains("README.md"));

    if let Some(previous_value) = previous_override {
        std::env::set_var("CLAUDE_TARGET_OVERRIDE", previous_value);
    } else {
        std::env::remove_var("CLAUDE_TARGET_OVERRIDE");
    }
    let _ = fs::remove_dir_all(&temporary_directory);
}

#[test]
fn memory_remember_natural_form_records_and_is_retrievable() {
    let home = tempdir_under("keel-remember-natural");
    let h = home.to_string_lossy().to_string();

    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let code = run_memory_command(
        "memory",
        &[
            "remember".to_string(),
            "--family".to_string(),
            "research-cache".to_string(),
            "--title".to_string(),
            "Kiro-Go embed pattern".to_string(),
            "--text".to_string(),
            "go:embed must live in main package".to_string(),
            "--claude-home".to_string(),
            h.clone(),
        ],
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(code, 0, "stderr: {}", String::from_utf8_lossy(&stderr));

    let mut stdout2: Vec<u8> = Vec::new();
    let mut stderr2: Vec<u8> = Vec::new();
    let code = run_memory_command(
        "memory",
        &[
            "remember".to_string(),
            "--question".to_string(),
            "alias question".to_string(),
            "--answer".to_string(),
            "alias answer".to_string(),
            "--claude-home".to_string(),
            h.clone(),
        ],
        &mut stdout2,
        &mut stderr2,
    );
    assert_eq!(code, 0, "stderr: {}", String::from_utf8_lossy(&stderr2));

    let mut lookup_out: Vec<u8> = Vec::new();
    let mut lookup_err: Vec<u8> = Vec::new();
    let code = run_memory_command(
        "memory",
        &[
            "research-cache".to_string(),
            "lookup".to_string(),
            "--query".to_string(),
            "embed".to_string(),
            "--claude-home".to_string(),
            h.clone(),
        ],
        &mut lookup_out,
        &mut lookup_err,
    );
    assert_eq!(code, 0, "stderr: {}", String::from_utf8_lossy(&lookup_err));
    let out = String::from_utf8_lossy(&lookup_out);
    assert!(
        out.contains("go:embed must live in main package"),
        "stdout: {out}"
    );

    let _ = fs::remove_dir_all(&home);
}

#[test]
fn memory_remember_rejects_missing_fields_and_unsupported_family() {
    let home = tempdir_under("keel-remember-reject");
    let h = home.to_string_lossy().to_string();

    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let code = run_memory_command(
        "memory",
        &[
            "remember".to_string(),
            "--title".to_string(),
            "only a title".to_string(),
            "--claude-home".to_string(),
            h.clone(),
        ],
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(code, 1);
    assert!(
        String::from_utf8_lossy(&stderr).contains("required"),
        "stderr: {}",
        String::from_utf8_lossy(&stderr)
    );

    let mut stdout2: Vec<u8> = Vec::new();
    let mut stderr2: Vec<u8> = Vec::new();
    let code = run_memory_command(
        "memory",
        &[
            "remember".to_string(),
            "--family".to_string(),
            "entity".to_string(),
            "--title".to_string(),
            "t".to_string(),
            "--text".to_string(),
            "x".to_string(),
            "--claude-home".to_string(),
            h.clone(),
        ],
        &mut stdout2,
        &mut stderr2,
    );
    assert_eq!(code, 1);
    assert!(
        String::from_utf8_lossy(&stderr2).contains("no record verb"),
        "stderr: {}",
        String::from_utf8_lossy(&stderr2)
    );

    let _ = fs::remove_dir_all(&home);
}

#[test]
fn orchestration_unknown_subcommand_returns_error() {
    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let exit_code =
        run_orchestration_command(&["bogus-action".to_string()], &mut stdout, &mut stderr);
    assert_eq!(exit_code, 1);
    let stderr_text = String::from_utf8_lossy(&stderr).to_string();
    assert!(
        stderr_text.contains("Unknown orchestration command: bogus-action"),
        "stderr: {stderr_text}"
    );
}

#[test]
fn orchestration_help_lists_documented_subcommands() {
    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let exit_code = run_orchestration_command(&["--help".to_string()], &mut stdout, &mut stderr);
    assert_eq!(exit_code, 0);
    let stdout_text = String::from_utf8_lossy(&stdout).to_string();
    assert!(
        stdout_text.contains("resume-status"),
        "stdout: {stdout_text}"
    );
    assert!(
        stdout_text.contains("runtime-preflight"),
        "stdout: {stdout_text}"
    );
    assert!(
        !stdout_text.contains("checkpoint"),
        "stub subcommand still in help: {stdout_text}"
    );
    assert!(
        !stdout_text.contains("route-plan"),
        "stale subcommand still in help: {stdout_text}"
    );
}

#[test]
fn orchestration_runtime_preflight_reports_probe_status() {
    let temporary_directory = tempdir_under("keel-orchestration-preflight");
    let claude_home = temporary_directory.join("claude-home");

    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let exit_code = run_orchestration_command(
        &[
            "runtime-preflight".to_string(),
            "--claude-home".to_string(),
            claude_home.to_string_lossy().to_string(),
        ],
        &mut stdout,
        &mut stderr,
    );
    let stdout_text = String::from_utf8_lossy(&stdout).to_string();
    assert!(
        stdout_text.contains("orchestration runtime-preflight:"),
        "stdout: {stdout_text}"
    );
    assert!(
        stdout_text.contains("claude_home:"),
        "stdout: {stdout_text}"
    );
    assert!(stdout_text.contains("ledger:"), "stdout: {stdout_text}");
    assert!(stdout_text.contains("git:"), "stdout: {stdout_text}");
    assert!(
        claude_home.join("workflow").is_dir(),
        "ledger dir not created"
    );
    assert!(
        exit_code == 0 || exit_code == 1,
        "unexpected exit: {exit_code}"
    );

    let _ = fs::remove_dir_all(&temporary_directory);
}

#[test]
fn orchestration_runtime_preflight_json_emits_structured_payload() {
    let temporary_directory = tempdir_under("keel-orchestration-preflight-json");
    let claude_home = temporary_directory.join("claude-home");

    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let _exit_code = run_orchestration_command(
        &[
            "runtime-preflight".to_string(),
            "--claude-home".to_string(),
            claude_home.to_string_lossy().to_string(),
            "--json".to_string(),
        ],
        &mut stdout,
        &mut stderr,
    );
    let stdout_text = String::from_utf8_lossy(&stdout).to_string();
    assert!(stdout_text.contains("\"ok\":"), "stdout: {stdout_text}");
    assert!(
        stdout_text.contains("\"claudeHome\""),
        "stdout: {stdout_text}"
    );
    assert!(
        stdout_text.contains("\"ledgerDirectory\""),
        "stdout: {stdout_text}"
    );
    assert!(stdout_text.contains("\"git\""), "stdout: {stdout_text}");

    let _ = fs::remove_dir_all(&temporary_directory);
}

#[test]
fn orchestration_resume_status_lists_open_entries() {
    let temporary_directory = tempdir_under("keel-orchestration-resume-status");
    let claude_home = temporary_directory.join("claude-home");
    fs::create_dir_all(&claude_home).expect("create claude home");
    seeded_open_entry(&claude_home, "wf-eeee", "wire orchestration dispatch");

    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let exit_code = run_orchestration_command(
        &[
            "resume-status".to_string(),
            "--claude-home".to_string(),
            claude_home.to_string_lossy().to_string(),
        ],
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(exit_code, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
    let output = String::from_utf8_lossy(&stdout).to_string();
    assert!(
        output.contains("orchestration resume-status: open=1"),
        "stdout: {output}"
    );
    assert!(output.contains("wf-eeee"), "stdout: {output}");
    assert!(
        output.contains("wire orchestration dispatch"),
        "stdout: {output}"
    );

    let _ = fs::remove_dir_all(&temporary_directory);
}

#[test]
fn orchestration_task_unknown_action_returns_error() {
    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let exit_code = run_orchestration_command(
        &["task".to_string(), "begn".to_string()],
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(exit_code, 1);
    let stderr_text = String::from_utf8_lossy(&stderr).to_string();
    assert!(
        stderr_text.contains("Unknown orchestration task action: begn"),
        "stderr: {stderr_text}"
    );
}

#[test]
fn orchestration_task_begin_without_task_flag_reports_required() {
    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let exit_code = run_orchestration_command(
        &["task".to_string(), "begin".to_string()],
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(exit_code, 1);
    let stderr_text = String::from_utf8_lossy(&stderr).to_string();
    assert!(
        stderr_text.contains("--task is required"),
        "stderr: {stderr_text}"
    );
}

#[test]
fn orchestration_checkpoint_succeeds_and_reports_snapshot() {
    let temporary_directory = tempdir_under("keel-orch-checkpoint-empty");
    let claude_home = temporary_directory.join("claude-home");
    fs::create_dir_all(&claude_home).expect("create claude home");
    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let exit_code = run_orchestration_command(
        &[
            "checkpoint".to_string(),
            "--claude-home".to_string(),
            claude_home.to_string_lossy().to_string(),
        ],
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(exit_code, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
    let stdout_text = String::from_utf8_lossy(&stdout).to_string();
    assert!(
        stdout_text.contains("open tasks: 0"),
        "stdout: {stdout_text}"
    );
    let _ = fs::remove_dir_all(&temporary_directory);
}

#[test]
fn working_brief_write_round_trips_via_show() {
    let temporary_directory = tempdir_under("keel-wb-write-show");
    let claude_home = temporary_directory.join("claude-home");
    fs::create_dir_all(&claude_home).expect("create claude home");

    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let exit_code = run_memory_command(
        "memory",
        &[
            "working-brief".to_string(),
            "write".to_string(),
            "--id".to_string(),
            "wb-show-1".to_string(),
            "--request".to_string(),
            "ship pagination on /users".to_string(),
            "--constraints".to_string(),
            "must not break /users|no n+1 queries".to_string(),
            "--acceptance-criteria".to_string(),
            "limit=20 default|expose nextCursor".to_string(),
            "--assumptions".to_string(),
            "cursor encoding stays opaque".to_string(),
            "--claude-home".to_string(),
            claude_home.to_string_lossy().to_string(),
        ],
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(exit_code, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
    let write_stdout = String::from_utf8_lossy(&stdout).to_string();
    assert!(
        write_stdout.contains("memory working-brief write: id=wb-show-1"),
        "stdout: {write_stdout}"
    );
    assert!(
        write_stdout.contains("constraints: 2 entries"),
        "stdout: {write_stdout}"
    );

    let mut show_stdout: Vec<u8> = Vec::new();
    let mut show_stderr: Vec<u8> = Vec::new();
    let show_code = run_memory_command(
        "memory",
        &[
            "working-brief".to_string(),
            "show".to_string(),
            "--id".to_string(),
            "wb-show-1".to_string(),
            "--claude-home".to_string(),
            claude_home.to_string_lossy().to_string(),
        ],
        &mut show_stdout,
        &mut show_stderr,
    );
    assert_eq!(
        show_code,
        0,
        "stderr: {}",
        String::from_utf8_lossy(&show_stderr)
    );
    let show_text = String::from_utf8_lossy(&show_stdout).to_string();
    assert!(show_text.contains("id: wb-show-1"), "stdout: {show_text}");
    assert!(
        show_text.contains("request: ship pagination on /users"),
        "stdout: {show_text}"
    );
    assert!(
        show_text.contains("- must not break /users"),
        "stdout: {show_text}"
    );
    assert!(
        show_text.contains("- limit=20 default"),
        "stdout: {show_text}"
    );
    assert!(
        show_text.contains("- cursor encoding stays opaque"),
        "stdout: {show_text}"
    );

    let _ = fs::remove_dir_all(&temporary_directory);
}

#[test]
fn working_brief_write_requires_request() {
    let temporary_directory = tempdir_under("keel-wb-write-required");
    let claude_home = temporary_directory.join("claude-home");
    fs::create_dir_all(&claude_home).expect("create claude home");

    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let exit_code = run_memory_command(
        "memory",
        &[
            "working-brief".to_string(),
            "write".to_string(),
            "--claude-home".to_string(),
            claude_home.to_string_lossy().to_string(),
        ],
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(exit_code, 1);
    let stderr_text = String::from_utf8_lossy(&stderr).to_string();
    assert!(
        stderr_text.contains("--request is required"),
        "stderr: {stderr_text}"
    );

    let _ = fs::remove_dir_all(&temporary_directory);
}

#[test]
fn working_brief_show_unknown_id_returns_error() {
    let temporary_directory = tempdir_under("keel-wb-show-missing");
    let claude_home = temporary_directory.join("claude-home");
    fs::create_dir_all(&claude_home).expect("create claude home");

    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let exit_code = run_memory_command(
        "memory",
        &[
            "working-brief".to_string(),
            "show".to_string(),
            "--id".to_string(),
            "wb-missing".to_string(),
            "--claude-home".to_string(),
            claude_home.to_string_lossy().to_string(),
        ],
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(exit_code, 1);
    let stderr_text = String::from_utf8_lossy(&stderr).to_string();
    assert!(
        stderr_text.contains("no brief with id wb-missing"),
        "stderr: {stderr_text}"
    );

    let _ = fs::remove_dir_all(&temporary_directory);
}

#[test]
fn working_brief_list_empty_emits_action_hint() {
    let temporary_directory = tempdir_under("keel-wb-list-empty");
    let claude_home = temporary_directory.join("claude-home");
    fs::create_dir_all(&claude_home).expect("create claude home");

    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let exit_code = run_memory_command(
        "memory",
        &[
            "working-brief".to_string(),
            "list".to_string(),
            "--claude-home".to_string(),
            claude_home.to_string_lossy().to_string(),
        ],
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(exit_code, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
    let output = String::from_utf8_lossy(&stdout).to_string();
    assert!(
        output.contains("memory working-brief list: directory="),
        "stdout: {output}"
    );
    assert!(output.contains("count=0"), "stdout: {output}");
    assert!(
        output.contains("keel memory working-brief write --request"),
        "stdout: {output}"
    );

    let _ = fs::remove_dir_all(&temporary_directory);
}

#[test]
fn working_brief_list_renders_multiple_briefs_in_order() {
    let temporary_directory = tempdir_under("keel-wb-list-multi");
    let claude_home = temporary_directory.join("claude-home");
    fs::create_dir_all(&claude_home).expect("create claude home");

    for (id, request) in [("wb-alpha", "first request"), ("wb-beta", "second request")] {
        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let exit_code = run_memory_command(
            "memory",
            &[
                "working-brief".to_string(),
                "write".to_string(),
                "--id".to_string(),
                id.to_string(),
                "--request".to_string(),
                request.to_string(),
                "--claude-home".to_string(),
                claude_home.to_string_lossy().to_string(),
            ],
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(exit_code, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
    }

    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let exit_code = run_memory_command(
        "memory",
        &[
            "working-brief".to_string(),
            "list".to_string(),
            "--claude-home".to_string(),
            claude_home.to_string_lossy().to_string(),
        ],
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(exit_code, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
    let output = String::from_utf8_lossy(&stdout).to_string();
    assert!(output.contains("count=2"), "stdout: {output}");
    let alpha_pos = output.find("wb-alpha").expect("wb-alpha listed");
    let beta_pos = output.find("wb-beta").expect("wb-beta listed");
    assert!(
        alpha_pos < beta_pos,
        "expected wb-alpha before wb-beta in: {output}"
    );

    let _ = fs::remove_dir_all(&temporary_directory);
}

#[test]
fn working_brief_write_json_emits_structured_payload() {
    let temporary_directory = tempdir_under("keel-wb-write-json");
    let claude_home = temporary_directory.join("claude-home");
    fs::create_dir_all(&claude_home).expect("create claude home");

    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let exit_code = run_memory_command(
        "memory",
        &[
            "working-brief".to_string(),
            "write".to_string(),
            "--id".to_string(),
            "wb-json".to_string(),
            "--request".to_string(),
            "json brief".to_string(),
            "--claude-home".to_string(),
            claude_home.to_string_lossy().to_string(),
            "--json".to_string(),
        ],
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(exit_code, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
    let output = String::from_utf8_lossy(&stdout).to_string();
    assert!(output.contains("\"written\": true"), "stdout: {output}");
    assert!(output.contains("\"id\": \"wb-json\""), "stdout: {output}");
    assert!(
        output.contains("\"request\": \"json brief\""),
        "stdout: {output}"
    );

    let _ = fs::remove_dir_all(&temporary_directory);
}

#[test]
fn working_brief_unknown_subcommand_returns_error() {
    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let exit_code = run_memory_command(
        "memory",
        &["working-brief".to_string(), "bogus".to_string()],
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(exit_code, 1);
    let stderr_text = String::from_utf8_lossy(&stderr).to_string();
    assert!(
        stderr_text.contains("Unknown memory working-brief action: bogus"),
        "stderr: {stderr_text}"
    );
}

#[test]
fn completion_gate_check_passes_for_open_entry_with_brief_and_proof() {
    let temporary_directory = tempdir_under("keel-cg-pass");
    let claude_home = temporary_directory.join("claude-home");
    fs::create_dir_all(&claude_home).expect("create claude home");
    seeded_open_entry(&claude_home, "wf-pass", "wire pagination");
    crate::utility::working_brief::write_brief(
        &claude_home,
        &crate::utility::working_brief::create_brief(
            "wb-pass".into(),
            "wire pagination on /users".into(),
            vec!["no n+1".into()],
            vec!["limit=20".into()],
            Vec::new(),
            String::new(),
            format_timestamp_iso8601(0),
        ),
    )
    .expect("seed brief");

    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let exit_code = run_memory_command(
        "memory",
        &[
            "completion-gate".to_string(),
            "check".to_string(),
            "--id".to_string(),
            "wf-pass".to_string(),
            "--brief-id".to_string(),
            "wb-pass".to_string(),
            "--proof".to_string(),
            "ladder green".to_string(),
            "--claude-home".to_string(),
            claude_home.to_string_lossy().to_string(),
        ],
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(exit_code, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
    let output = String::from_utf8_lossy(&stdout).to_string();
    assert!(
        output.contains("completion-gate check: id=wf-pass status=ok"),
        "stdout: {output}"
    );
    assert!(output.contains("entry: ok"), "stdout: {output}");
    assert!(output.contains("open: ok"), "stdout: {output}");
    assert!(output.contains("working-brief: ok"), "stdout: {output}");
    assert!(output.contains("proof: ok"), "stdout: {output}");
    assert!(
        output.contains("ready to close with keel workflow finish"),
        "stdout: {output}"
    );

    let _ = fs::remove_dir_all(&temporary_directory);
}

#[test]
fn completion_gate_check_fails_when_entry_missing() {
    let temporary_directory = tempdir_under("keel-cg-missing");
    let claude_home = temporary_directory.join("claude-home");
    fs::create_dir_all(claude_home.join("workflow")).expect("create ledger dir");

    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let exit_code = run_memory_command(
        "memory",
        &[
            "completion-gate".to_string(),
            "check".to_string(),
            "--id".to_string(),
            "wf-missing".to_string(),
            "--claude-home".to_string(),
            claude_home.to_string_lossy().to_string(),
        ],
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(exit_code, 1);
    let output = String::from_utf8_lossy(&stdout).to_string();
    assert!(
        output.contains("completion-gate check: id=wf-missing status=fail"),
        "stdout: {output}"
    );
    assert!(
        output.contains("entry: fail -> no ledger entry with id wf-missing"),
        "stdout: {output}"
    );
    assert!(
        output.contains("hint: resolve failing probes"),
        "stdout: {output}"
    );

    let _ = fs::remove_dir_all(&temporary_directory);
}

#[test]
fn completion_gate_check_fails_when_entry_already_closed() {
    let temporary_directory = tempdir_under("keel-cg-closed");
    let claude_home = temporary_directory.join("claude-home");
    fs::create_dir_all(&claude_home).expect("create claude home");
    let open = seeded_open_entry(&claude_home, "wf-closed", "rotate secrets");
    let closed = close_entry(open, format_timestamp_iso8601(1), "done".to_string());
    write_entry(&claude_home, &closed).expect("seed closed entry");

    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let exit_code = run_memory_command(
        "memory",
        &[
            "completion-gate".to_string(),
            "check".to_string(),
            "--id".to_string(),
            "wf-closed".to_string(),
            "--claude-home".to_string(),
            claude_home.to_string_lossy().to_string(),
        ],
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(exit_code, 1);
    let output = String::from_utf8_lossy(&stdout).to_string();
    assert!(output.contains("entry: ok"), "stdout: {output}");
    assert!(
        output.contains("open: fail -> entry wf-closed is closed (expected open)"),
        "stdout: {output}"
    );

    let _ = fs::remove_dir_all(&temporary_directory);
}

#[test]
fn completion_gate_check_fails_when_brief_missing() {
    let temporary_directory = tempdir_under("keel-cg-brief-missing");
    let claude_home = temporary_directory.join("claude-home");
    fs::create_dir_all(&claude_home).expect("create claude home");
    seeded_open_entry(&claude_home, "wf-bm", "ship feature");

    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let exit_code = run_memory_command(
        "memory",
        &[
            "completion-gate".to_string(),
            "check".to_string(),
            "--id".to_string(),
            "wf-bm".to_string(),
            "--brief-id".to_string(),
            "wb-nope".to_string(),
            "--claude-home".to_string(),
            claude_home.to_string_lossy().to_string(),
        ],
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(exit_code, 1);
    let output = String::from_utf8_lossy(&stdout).to_string();
    assert!(
        output.contains("working-brief: fail -> no working brief with id wb-nope"),
        "stdout: {output}"
    );

    let _ = fs::remove_dir_all(&temporary_directory);
}

#[test]
fn completion_gate_check_skips_optional_probes_when_flags_absent() {
    let temporary_directory = tempdir_under("keel-cg-skip");
    let claude_home = temporary_directory.join("claude-home");
    fs::create_dir_all(&claude_home).expect("create claude home");
    seeded_open_entry(&claude_home, "wf-skip", "minimal smoke");

    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let exit_code = run_memory_command(
        "memory",
        &[
            "completion-gate".to_string(),
            "check".to_string(),
            "--id".to_string(),
            "wf-skip".to_string(),
            "--claude-home".to_string(),
            claude_home.to_string_lossy().to_string(),
        ],
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(exit_code, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
    let output = String::from_utf8_lossy(&stdout).to_string();
    assert!(output.contains("status=ok"), "stdout: {output}");
    assert!(
        !output.contains("working-brief:"),
        "expected no working-brief probe in: {output}"
    );
    assert!(
        !output.contains("proof:"),
        "expected no proof probe in: {output}"
    );

    let _ = fs::remove_dir_all(&temporary_directory);
}

#[test]
fn completion_gate_check_requires_id() {
    let temporary_directory = tempdir_under("keel-cg-no-id");
    let claude_home = temporary_directory.join("claude-home");
    fs::create_dir_all(&claude_home).expect("create claude home");

    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let exit_code = run_memory_command(
        "memory",
        &[
            "completion-gate".to_string(),
            "check".to_string(),
            "--claude-home".to_string(),
            claude_home.to_string_lossy().to_string(),
        ],
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(exit_code, 1);
    let stderr_text = String::from_utf8_lossy(&stderr).to_string();
    assert!(
        stderr_text.contains("--id is required"),
        "stderr: {stderr_text}"
    );

    let _ = fs::remove_dir_all(&temporary_directory);
}

#[test]
fn completion_gate_check_json_emits_structured_payload() {
    let temporary_directory = tempdir_under("keel-cg-json");
    let claude_home = temporary_directory.join("claude-home");
    fs::create_dir_all(&claude_home).expect("create claude home");
    seeded_open_entry(&claude_home, "wf-json", "structured payload");

    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let exit_code = run_memory_command(
        "memory",
        &[
            "completion-gate".to_string(),
            "check".to_string(),
            "--id".to_string(),
            "wf-json".to_string(),
            "--proof".to_string(),
            "ladder green".to_string(),
            "--claude-home".to_string(),
            claude_home.to_string_lossy().to_string(),
            "--json".to_string(),
        ],
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(exit_code, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
    let output = String::from_utf8_lossy(&stdout).to_string();
    assert!(output.contains("\"ok\": true"), "stdout: {output}");
    assert!(output.contains("\"id\": \"wf-json\""), "stdout: {output}");
    assert!(output.contains("\"entry\""), "stdout: {output}");
    assert!(output.contains("\"open\""), "stdout: {output}");
    assert!(output.contains("\"proof\""), "stdout: {output}");
    assert!(
        !output.contains("\"workingBrief\""),
        "expected no workingBrief field when --brief-id absent: {output}"
    );

    let _ = fs::remove_dir_all(&temporary_directory);
}

#[test]
fn completion_gate_unknown_subcommand_returns_error() {
    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let exit_code = run_memory_command(
        "memory",
        &["completion-gate".to_string(), "bogus".to_string()],
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(exit_code, 1);
    let stderr_text = String::from_utf8_lossy(&stderr).to_string();
    assert!(
        stderr_text.contains("Unknown memory completion-gate action: bogus"),
        "stderr: {stderr_text}"
    );
}

#[test]
fn orchestration_task_begin_progress_complete_round_trips() {
    let temporary_directory = tempdir_under("keel-orch-task-lifecycle");
    let claude_home = temporary_directory.join("claude-home");
    fs::create_dir_all(&claude_home).expect("create claude home");
    let home_arg = claude_home.to_string_lossy().to_string();

    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let exit = run_orchestration_command(
        &[
            "task".to_string(),
            "begin".to_string(),
            "--task".to_string(),
            "wire sync_commands".to_string(),
            "--phase".to_string(),
            "implement".to_string(),
            "--claude-home".to_string(),
            home_arg.clone(),
        ],
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(exit, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
    let id = task_id_from_stdout(&stdout);
    assert!(claude_home
        .join("orchestration/tasks")
        .join(format!("{id}.json"))
        .is_file());

    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let exit = run_orchestration_command(
        &[
            "task".to_string(),
            "progress".to_string(),
            "--id".to_string(),
            id.clone(),
            "--note".to_string(),
            "tests passing".to_string(),
            "--claude-home".to_string(),
            home_arg.clone(),
        ],
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(exit, 0, "stderr: {}", String::from_utf8_lossy(&stderr));

    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let exit = run_orchestration_command(
        &[
            "task".to_string(),
            "complete".to_string(),
            "--id".to_string(),
            id.clone(),
            "--claude-home".to_string(),
            home_arg.clone(),
            "--json".to_string(),
        ],
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(exit, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
    let out = String::from_utf8_lossy(&stdout).to_string();
    assert!(out.contains("\"status\": \"done\""), "stdout: {out}");
    assert!(out.contains("\"completedAt\""), "stdout: {out}");

    let _ = fs::remove_dir_all(&temporary_directory);
}

#[test]
fn orchestration_task_begin_requires_task_description() {
    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let exit = run_orchestration_command(
        &["task".to_string(), "begin".to_string()],
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(exit, 1);
    assert!(String::from_utf8_lossy(&stderr).contains("--task is required"));
}

#[test]
fn orchestration_task_progress_unknown_id_errors() {
    let temporary_directory = tempdir_under("keel-orch-task-missing");
    let claude_home = temporary_directory.join("claude-home");
    fs::create_dir_all(&claude_home).expect("create claude home");
    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let exit = run_orchestration_command(
        &[
            "task".to_string(),
            "progress".to_string(),
            "--id".to_string(),
            "task-nope".to_string(),
            "--claude-home".to_string(),
            claude_home.to_string_lossy().to_string(),
        ],
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(exit, 1);
    assert!(String::from_utf8_lossy(&stderr).contains("no task with id task-nope"));
    let _ = fs::remove_dir_all(&temporary_directory);
}

#[test]
fn orchestration_task_list_open_only_excludes_done() {
    let temporary_directory = tempdir_under("keel-orch-task-list");
    let claude_home = temporary_directory.join("claude-home");
    fs::create_dir_all(&claude_home).expect("create claude home");
    let home_arg = claude_home.to_string_lossy().to_string();

    let mut out = Vec::new();
    let mut err = Vec::new();
    run_orchestration_command(
        &[
            "task".to_string(),
            "begin".to_string(),
            "--task".to_string(),
            "open one".to_string(),
            "--claude-home".to_string(),
            home_arg.clone(),
        ],
        &mut out,
        &mut err,
    );
    let mut out2 = Vec::new();
    let mut err2 = Vec::new();
    run_orchestration_command(
        &[
            "task".to_string(),
            "begin".to_string(),
            "--task".to_string(),
            "done one".to_string(),
            "--claude-home".to_string(),
            home_arg.clone(),
        ],
        &mut out2,
        &mut err2,
    );
    let done_id = task_id_from_stdout(&out2);
    let mut out3 = Vec::new();
    let mut err3 = Vec::new();
    run_orchestration_command(
        &[
            "task".to_string(),
            "complete".to_string(),
            "--id".to_string(),
            done_id,
            "--claude-home".to_string(),
            home_arg.clone(),
        ],
        &mut out3,
        &mut err3,
    );

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let exit = run_orchestration_command(
        &[
            "task".to_string(),
            "list".to_string(),
            "--open-only".to_string(),
            "--claude-home".to_string(),
            home_arg,
        ],
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(exit, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
    let text = String::from_utf8_lossy(&stdout).to_string();
    assert!(text.contains("1 task(s)"), "stdout: {text}");
    assert!(text.contains("open one"), "stdout: {text}");
    assert!(!text.contains("done one"), "stdout: {text}");
    let _ = fs::remove_dir_all(&temporary_directory);
}

#[test]
fn orchestration_checkpoint_counts_open_tasks_and_persists_snapshot() {
    let temporary_directory = tempdir_under("keel-orch-checkpoint");
    let claude_home = temporary_directory.join("claude-home");
    fs::create_dir_all(&claude_home).expect("create claude home");
    let home_arg = claude_home.to_string_lossy().to_string();

    let mut out = Vec::new();
    let mut err = Vec::new();
    run_orchestration_command(
        &[
            "task".to_string(),
            "begin".to_string(),
            "--task".to_string(),
            "in flight".to_string(),
            "--claude-home".to_string(),
            home_arg.clone(),
        ],
        &mut out,
        &mut err,
    );

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let exit = run_orchestration_command(
        &[
            "checkpoint".to_string(),
            "--note".to_string(),
            "before compaction".to_string(),
            "--claude-home".to_string(),
            home_arg,
            "--json".to_string(),
        ],
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(exit, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
    let text = String::from_utf8_lossy(&stdout).to_string();
    assert!(text.contains("\"openTasks\": \"1\""), "stdout: {text}");
    assert!(text.contains("before compaction"), "stdout: {text}");
    let checkpoint_dir = claude_home.join("orchestration/checkpoints");
    let count = fs::read_dir(&checkpoint_dir)
        .expect("checkpoints dir exists")
        .count();
    assert_eq!(count, 1, "exactly one checkpoint record expected");
    let _ = fs::remove_dir_all(&temporary_directory);
}

#[test]
fn memory_report_aliases_status_summary() {
    let temporary_directory = tempdir_under("keel-memory-report");
    let claude_home = temporary_directory.join("claude-home");
    fs::create_dir_all(&claude_home).expect("create claude home");
    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let exit = run_memory_command(
        "memory",
        &[
            "report".to_string(),
            "--claude-home".to_string(),
            claude_home.to_string_lossy().to_string(),
        ],
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(exit, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
    let text = String::from_utf8_lossy(&stdout).to_string();
    assert!(text.contains("family record counts"), "stdout: {text}");
    let _ = fs::remove_dir_all(&temporary_directory);
}

#[test]
fn memory_index_rebuilds_recall_index() {
    let temporary_directory = tempdir_under("keel-memory-index");
    let claude_home = temporary_directory.join("claude-home");
    fs::create_dir_all(&claude_home).expect("create claude home");
    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let exit = run_memory_command(
        "memory",
        &[
            "index".to_string(),
            "--claude-home".to_string(),
            claude_home.to_string_lossy().to_string(),
        ],
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(exit, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
    assert!(
        claude_home.join("recall-index.sqlite3").is_file(),
        "recall index must be created by `memory index`"
    );
    let _ = fs::remove_dir_all(&temporary_directory);
}

#[test]
fn memory_hook_redirects_to_lifecycle_hook_surface() {
    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let exit = run_memory_command("memory", &["hook".to_string()], &mut stdout, &mut stderr);
    assert_eq!(exit, 1);
    let stderr_text = String::from_utf8_lossy(&stderr).to_string();
    assert!(
        stderr_text.contains("keel hook install"),
        "stderr must point at the real hook surface: {stderr_text}"
    );
}

#[test]
fn working_brief_record_summary_round_trips() {
    let temporary_directory = tempdir_under("keel-wb-record-summary");
    let claude_home = temporary_directory.join("claude-home");
    fs::create_dir_all(&claude_home).expect("create claude home");

    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let exit_code = run_memory_command(
        "memory",
        &[
            "working-brief".to_string(),
            "write".to_string(),
            "--id".to_string(),
            "wb-sum-1".to_string(),
            "--request".to_string(),
            "ship auth".to_string(),
            "--claude-home".to_string(),
            claude_home.to_string_lossy().to_string(),
        ],
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(exit_code, 0, "stderr: {}", String::from_utf8_lossy(&stderr));

    let mut summary_stdout: Vec<u8> = Vec::new();
    let mut summary_stderr: Vec<u8> = Vec::new();
    let exit_code = run_memory_command(
        "memory",
        &[
            "working-brief".to_string(),
            "record-summary".to_string(),
            "--id".to_string(),
            "wb-sum-1".to_string(),
            "--summary".to_string(),
            "Auth shipped: JWT + refresh token flow".to_string(),
            "--claude-home".to_string(),
            claude_home.to_string_lossy().to_string(),
        ],
        &mut summary_stdout,
        &mut summary_stderr,
    );
    assert_eq!(
        exit_code,
        0,
        "stderr: {}",
        String::from_utf8_lossy(&summary_stderr)
    );
    let output = String::from_utf8_lossy(&summary_stdout).to_string();
    assert!(output.contains("summary_id=wbs-"), "stdout: {output}");
    assert!(output.contains("brief_id=wb-sum-1"), "stdout: {output}");
    assert!(output.contains("JWT + refresh token"), "stdout: {output}");

    let _ = fs::remove_dir_all(&temporary_directory);
}

#[test]
fn working_brief_record_summary_requires_id_and_summary() {
    let temporary_directory = tempdir_under("keel-wb-record-summary-required");
    let claude_home = temporary_directory.join("claude-home");
    fs::create_dir_all(&claude_home).expect("create claude home");

    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let exit_code = run_memory_command(
        "memory",
        &[
            "working-brief".to_string(),
            "record-summary".to_string(),
            "--claude-home".to_string(),
            claude_home.to_string_lossy().to_string(),
        ],
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(exit_code, 1);
    let stderr_text = String::from_utf8_lossy(&stderr).to_string();
    assert!(
        stderr_text.contains("--id is required"),
        "stderr: {stderr_text}"
    );

    let mut stdout2: Vec<u8> = Vec::new();
    let mut stderr2: Vec<u8> = Vec::new();
    let exit_code = run_memory_command(
        "memory",
        &[
            "working-brief".to_string(),
            "record-summary".to_string(),
            "--id".to_string(),
            "wb-x".to_string(),
            "--claude-home".to_string(),
            claude_home.to_string_lossy().to_string(),
        ],
        &mut stdout2,
        &mut stderr2,
    );
    assert_eq!(exit_code, 1);
    let stderr_text = String::from_utf8_lossy(&stderr2).to_string();
    assert!(
        stderr_text.contains("--summary is required"),
        "stderr: {stderr_text}"
    );

    let _ = fs::remove_dir_all(&temporary_directory);
}

#[test]
fn working_brief_record_summary_unknown_brief_returns_error() {
    let temporary_directory = tempdir_under("keel-wb-record-summary-missing");
    let claude_home = temporary_directory.join("claude-home");
    fs::create_dir_all(&claude_home).expect("create claude home");

    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let exit_code = run_memory_command(
        "memory",
        &[
            "working-brief".to_string(),
            "record-summary".to_string(),
            "--id".to_string(),
            "wb-nonexistent".to_string(),
            "--summary".to_string(),
            "test".to_string(),
            "--claude-home".to_string(),
            claude_home.to_string_lossy().to_string(),
        ],
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(exit_code, 1);
    let stderr_text = String::from_utf8_lossy(&stderr).to_string();
    assert!(
        stderr_text.contains("no brief with id wb-nonexistent"),
        "stderr: {stderr_text}"
    );

    let _ = fs::remove_dir_all(&temporary_directory);
}

#[test]
fn working_brief_record_summary_json_emits_structured_payload() {
    let temporary_directory = tempdir_under("keel-wb-record-summary-json");
    let claude_home = temporary_directory.join("claude-home");
    fs::create_dir_all(&claude_home).expect("create claude home");

    run_memory_command(
        "memory",
        &[
            "working-brief".to_string(),
            "write".to_string(),
            "--id".to_string(),
            "wb-sj".to_string(),
            "--request".to_string(),
            "test".to_string(),
            "--claude-home".to_string(),
            claude_home.to_string_lossy().to_string(),
        ],
        &mut Vec::new(),
        &mut Vec::new(),
    );

    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let exit_code = run_memory_command(
        "memory",
        &[
            "working-brief".to_string(),
            "record-summary".to_string(),
            "--id".to_string(),
            "wb-sj".to_string(),
            "--summary".to_string(),
            "done".to_string(),
            "--claude-home".to_string(),
            claude_home.to_string_lossy().to_string(),
            "--json".to_string(),
        ],
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(exit_code, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
    let output = String::from_utf8_lossy(&stdout).to_string();
    assert!(output.contains("\"recorded\": true"), "stdout: {output}");
    assert!(output.contains("\"briefId\""), "stdout: {output}");
    assert!(output.contains("\"summaryId\""), "stdout: {output}");

    let _ = fs::remove_dir_all(&temporary_directory);
}

#[test]
fn completion_gate_record_requirement_round_trips() {
    let temporary_directory = tempdir_under("keel-cg-record-req");
    let claude_home = temporary_directory.join("claude-home");
    fs::create_dir_all(&claude_home).expect("create claude home");
    seeded_open_entry(&claude_home, "wf-req", "wire auth");

    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let exit_code = run_memory_command(
        "memory",
        &[
            "completion-gate".to_string(),
            "record-requirement".to_string(),
            "--id".to_string(),
            "wf-req".to_string(),
            "--requirement".to_string(),
            "JWT refresh flow must be implemented".to_string(),
            "--status".to_string(),
            "pending".to_string(),
            "--claude-home".to_string(),
            claude_home.to_string_lossy().to_string(),
        ],
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(exit_code, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
    let output = String::from_utf8_lossy(&stdout).to_string();
    assert!(output.contains("requirement_id=cgr-"), "stdout: {output}");
    assert!(output.contains("entry_id=wf-req"), "stdout: {output}");
    assert!(output.contains("JWT refresh flow"), "stdout: {output}");
    assert!(output.contains("status: pending"), "stdout: {output}");

    let _ = fs::remove_dir_all(&temporary_directory);
}

#[test]
fn completion_gate_record_requirement_requires_id_and_requirement() {
    let temporary_directory = tempdir_under("keel-cg-record-req-required");
    let claude_home = temporary_directory.join("claude-home");
    fs::create_dir_all(&claude_home).expect("create claude home");

    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let exit_code = run_memory_command(
        "memory",
        &[
            "completion-gate".to_string(),
            "record-requirement".to_string(),
            "--claude-home".to_string(),
            claude_home.to_string_lossy().to_string(),
        ],
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(exit_code, 1);
    let stderr_text = String::from_utf8_lossy(&stderr).to_string();
    assert!(
        stderr_text.contains("--id is required"),
        "stderr: {stderr_text}"
    );

    let mut stdout2: Vec<u8> = Vec::new();
    let mut stderr2: Vec<u8> = Vec::new();
    let exit_code = run_memory_command(
        "memory",
        &[
            "completion-gate".to_string(),
            "record-requirement".to_string(),
            "--id".to_string(),
            "wf-x".to_string(),
            "--claude-home".to_string(),
            claude_home.to_string_lossy().to_string(),
        ],
        &mut stdout2,
        &mut stderr2,
    );
    assert_eq!(exit_code, 1);
    let stderr_text = String::from_utf8_lossy(&stderr2).to_string();
    assert!(
        stderr_text.contains("--requirement is required"),
        "stderr: {stderr_text}"
    );

    let _ = fs::remove_dir_all(&temporary_directory);
}

#[test]
fn completion_gate_record_requirement_unknown_entry_returns_error() {
    let temporary_directory = tempdir_under("keel-cg-record-req-missing");
    let claude_home = temporary_directory.join("claude-home");
    fs::create_dir_all(&claude_home).expect("create claude home");

    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let exit_code = run_memory_command(
        "memory",
        &[
            "completion-gate".to_string(),
            "record-requirement".to_string(),
            "--id".to_string(),
            "wf-nope".to_string(),
            "--requirement".to_string(),
            "test".to_string(),
            "--claude-home".to_string(),
            claude_home.to_string_lossy().to_string(),
        ],
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(exit_code, 1);
    let stderr_text = String::from_utf8_lossy(&stderr).to_string();
    assert!(
        stderr_text.contains("no ledger entry with id wf-nope"),
        "stderr: {stderr_text}"
    );

    let _ = fs::remove_dir_all(&temporary_directory);
}

#[test]
fn completion_gate_record_requirement_json_emits_structured_payload() {
    let temporary_directory = tempdir_under("keel-cg-record-req-json");
    let claude_home = temporary_directory.join("claude-home");
    fs::create_dir_all(&claude_home).expect("create claude home");
    seeded_open_entry(&claude_home, "wf-rj", "structured");

    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let exit_code = run_memory_command(
        "memory",
        &[
            "completion-gate".to_string(),
            "record-requirement".to_string(),
            "--id".to_string(),
            "wf-rj".to_string(),
            "--requirement".to_string(),
            "must work".to_string(),
            "--claude-home".to_string(),
            claude_home.to_string_lossy().to_string(),
            "--json".to_string(),
        ],
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(exit_code, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
    let output = String::from_utf8_lossy(&stdout).to_string();
    assert!(output.contains("\"recorded\": true"), "stdout: {output}");
    assert!(output.contains("\"requirementId\""), "stdout: {output}");
    assert!(output.contains("\"entryId\""), "stdout: {output}");
    assert!(output.contains("\"requirement\""), "stdout: {output}");

    let _ = fs::remove_dir_all(&temporary_directory);
}

#[test]
fn consolidate_empty_home_reports_no_records() {
    let temporary_directory = tempdir_under("keel-consolidate-empty");
    let claude_home = temporary_directory.join("claude-home");
    fs::create_dir_all(&claude_home).expect("create claude home");

    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let exit_code = run_memory_command(
        "memory",
        &[
            "consolidate".to_string(),
            "--claude-home".to_string(),
            claude_home.to_string_lossy().to_string(),
        ],
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(exit_code, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
    let output = String::from_utf8_lossy(&stdout).to_string();
    assert!(
        output.contains("no records to consolidate"),
        "stdout: {output}"
    );

    let _ = fs::remove_dir_all(&temporary_directory);
}

#[test]
fn consolidate_counts_family_records() {
    let temporary_directory = tempdir_under("keel-consolidate-counts");
    let claude_home = temporary_directory.join("claude-home");
    fs::create_dir_all(&claude_home).expect("create claude home");

    run_memory_command(
        "memory",
        &[
            "working-brief".to_string(),
            "write".to_string(),
            "--id".to_string(),
            "wb-con1".to_string(),
            "--request".to_string(),
            "first".to_string(),
            "--claude-home".to_string(),
            claude_home.to_string_lossy().to_string(),
        ],
        &mut Vec::new(),
        &mut Vec::new(),
    );

    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let exit_code = run_memory_command(
        "memory",
        &[
            "consolidate".to_string(),
            "--claude-home".to_string(),
            claude_home.to_string_lossy().to_string(),
        ],
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(exit_code, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
    let output = String::from_utf8_lossy(&stdout).to_string();
    assert!(output.contains("total records"), "stdout: {output}");

    let _ = fs::remove_dir_all(&temporary_directory);
}

#[test]
fn consolidate_json_emits_structured_payload() {
    let temporary_directory = tempdir_under("keel-consolidate-json");
    let claude_home = temporary_directory.join("claude-home");
    fs::create_dir_all(&claude_home).expect("create claude home");

    run_memory_command(
        "memory",
        &[
            "working-brief".to_string(),
            "write".to_string(),
            "--id".to_string(),
            "wb-cj".to_string(),
            "--request".to_string(),
            "json test".to_string(),
            "--claude-home".to_string(),
            claude_home.to_string_lossy().to_string(),
        ],
        &mut Vec::new(),
        &mut Vec::new(),
    );

    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let exit_code = run_memory_command(
        "memory",
        &[
            "consolidate".to_string(),
            "--claude-home".to_string(),
            claude_home.to_string_lossy().to_string(),
            "--json".to_string(),
        ],
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(exit_code, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
    let output = String::from_utf8_lossy(&stdout).to_string();
    assert!(output.contains("\"families\""), "stdout: {output}");
    assert!(output.contains("\"totalRecords\""), "stdout: {output}");

    let _ = fs::remove_dir_all(&temporary_directory);
}

#[test]
fn working_brief_help_lists_record_summary() {
    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let exit_code = run_memory_command(
        "memory",
        &["working-brief".to_string(), "--help".to_string()],
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(exit_code, 0);
    let output = String::from_utf8_lossy(&stdout).to_string();
    assert!(
        output.contains("record-summary"),
        "help should list record-summary: {output}"
    );
}

#[test]
fn completion_gate_help_lists_record_requirement() {
    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let exit_code = run_memory_command(
        "memory",
        &["completion-gate".to_string(), "--help".to_string()],
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(exit_code, 0);
    let output = String::from_utf8_lossy(&stdout).to_string();
    assert!(
        output.contains("record-requirement"),
        "help should list record-requirement: {output}"
    );
}

#[test]
fn memory_help_lists_consolidate() {
    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let exit_code = run_memory_command("memory", &["--help".to_string()], &mut stdout, &mut stderr);
    assert_eq!(exit_code, 0);
    let output = String::from_utf8_lossy(&stdout).to_string();
    assert!(
        output.contains("consolidate"),
        "help should list consolidate: {output}"
    );
}
