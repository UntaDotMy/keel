//! Tests for memory subcommands that exercise multiple submodules through
//! `run_memory_command`.
//! These tests call across module boundaries and are intentionally kept together
//! rather than scattered across the individual submodules.

use super::*;
use crate::test_support::ENV_LOCK;
use crate::utility::working_brief::{create_brief, write_brief};
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
/// Seed one working brief directly through the storage API (no CLI round-trip).
fn seed_brief(claude_home: &std::path::Path, id: &str, request: &str) {
    write_brief(
        claude_home,
        &create_brief(
            id.to_string(),
            request.to_string(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            String::new(),
            "1970-01-01T00:00:00Z".to_string(),
        ),
    )
    .expect("seed brief");
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
fn completion_gate_check_passes_on_fresh_install_and_persists_proof() {
    let temporary_directory = tempdir_under("keel-cg-fresh");
    let claude_home = temporary_directory.join("claude-home");
    fs::create_dir_all(&claude_home).expect("create claude home");
    seed_brief(&claude_home, "wb-pass", "wire pagination on /users");

    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let exit_code = run_memory_command(
        "memory",
        &[
            "completion-gate".to_string(),
            "check".to_string(),
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
        output.contains("brief=wb-pass status=ok"),
        "stdout: {output}"
    );
    assert!(output.contains("working-brief: ok"), "stdout: {output}");
    assert!(output.contains("proof: ok"), "stdout: {output}");
    assert!(output.contains("proof-persisted: ok"), "stdout: {output}");

    // The proof is stored on the brief record; no ledger remains.
    let stored = crate::utility::working_brief::read_brief(&claude_home, "wb-pass")
        .expect("read brief")
        .expect("brief exists");
    assert_eq!(stored.proof, "ladder green");

    let _ = fs::remove_dir_all(&temporary_directory);
}

#[test]
fn completion_gate_check_fails_when_brief_missing() {
    let temporary_directory = tempdir_under("keel-cg-brief-missing");
    let claude_home = temporary_directory.join("claude-home");
    fs::create_dir_all(&claude_home).expect("create claude home");

    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let exit_code = run_memory_command(
        "memory",
        &[
            "completion-gate".to_string(),
            "check".to_string(),
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
    assert!(output.contains("status=fail"), "stdout: {output}");
    assert!(
        output.contains("no working brief with id wb-nope"),
        "stdout: {output}"
    );

    let _ = fs::remove_dir_all(&temporary_directory);
}

#[test]
fn completion_gate_check_without_brief_id_lists_available_ids() {
    let temporary_directory = tempdir_under("keel-cg-list-ids");
    let claude_home = temporary_directory.join("claude-home");
    fs::create_dir_all(&claude_home).expect("create claude home");
    seed_brief(&claude_home, "wb-beta", "second");
    seed_brief(&claude_home, "wb-alpha", "first");

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
    let output = String::from_utf8_lossy(&stdout).to_string();
    assert!(
        output.contains("available brief ids: wb-alpha, wb-beta"),
        "stdout: {output}"
    );

    let _ = fs::remove_dir_all(&temporary_directory);
}

#[test]
fn completion_gate_check_empty_home_points_at_working_brief_write() {
    let temporary_directory = tempdir_under("keel-cg-empty");
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
    let output = String::from_utf8_lossy(&stdout).to_string();
    assert!(
        output.contains("no working briefs exist"),
        "stdout: {output}"
    );
    assert!(output.contains("working-brief write"), "stdout: {output}");

    let _ = fs::remove_dir_all(&temporary_directory);
}

#[test]
fn completion_gate_check_fails_on_whitespace_only_proof() {
    let temporary_directory = tempdir_under("keel-cg-ws-proof");
    let claude_home = temporary_directory.join("claude-home");
    fs::create_dir_all(&claude_home).expect("create claude home");
    seed_brief(&claude_home, "wb-ws", "ship feature");

    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let exit_code = run_memory_command(
        "memory",
        &[
            "completion-gate".to_string(),
            "check".to_string(),
            "--brief-id".to_string(),
            "wb-ws".to_string(),
            "--proof".to_string(),
            "   ".to_string(),
            "--claude-home".to_string(),
            claude_home.to_string_lossy().to_string(),
        ],
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(exit_code, 1);
    let output = String::from_utf8_lossy(&stdout).to_string();
    assert!(output.contains("whitespace only"), "stdout: {output}");
    // A failed proof must not be persisted onto the brief.
    let stored = crate::utility::working_brief::read_brief(&claude_home, "wb-ws")
        .expect("read brief")
        .expect("brief exists");
    assert_eq!(stored.proof, "");

    let _ = fs::remove_dir_all(&temporary_directory);
}

#[test]
fn completion_gate_check_json_emits_structured_payload() {
    let temporary_directory = tempdir_under("keel-cg-json");
    let claude_home = temporary_directory.join("claude-home");
    fs::create_dir_all(&claude_home).expect("create claude home");
    seed_brief(&claude_home, "wb-json", "structured payload");

    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let exit_code = run_memory_command(
        "memory",
        &[
            "completion-gate".to_string(),
            "check".to_string(),
            "--brief-id".to_string(),
            "wb-json".to_string(),
            "--proof".to_string(),
            "tests green".to_string(),
            "--json".to_string(),
            "--claude-home".to_string(),
            claude_home.to_string_lossy().to_string(),
        ],
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(exit_code, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
    let rendered = String::from_utf8_lossy(&stdout).to_string();
    assert!(rendered.contains("\"workingBrief\""), "json: {rendered}");
    assert!(rendered.contains("\"proof\""), "json: {rendered}");
    assert!(rendered.contains("\"proofPersisted\""), "json: {rendered}");
    assert!(rendered.contains("\"ok\": true"), "json: {rendered}");

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
fn completion_gate_help_lists_brief_id_and_drops_record_requirement() {
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
        output.contains("--brief-id"),
        "help should list --brief-id: {output}"
    );
    assert!(
        !output.contains("record-requirement"),
        "record-requirement was removed and must not appear in help: {output}"
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
