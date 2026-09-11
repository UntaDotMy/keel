use super::*;
use super::{ci::*, diff_gates::*, language_gates::*, messages::*, workflow::*};

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

/// `gh pr checks` exits 8 when checks are pending while still printing the
/// table. A non-zero exit carrying parseable rows is signal (pending), not
/// an error; the gate must read the table, not fail closed on the code.
#[test]
fn gh_pending_exit_code_still_reads_the_check_table() {
    let pending = parse_gh_checks("NAME  STATUS\nci  pending\nbuild  pass\n")
        .expect("two checks despite a pending exit code");
    assert!(matches!(evaluate_checks(&pending), CiVerdict::Pending));
}

/// Regression: only a PASSING review is a reviewer pass. A failed review or
/// the informational diff/init surfaces must not clear the review gate.
#[test]
fn review_pass_clears_gate_only_on_passing_real_surface() {
    // Only a full pre-PR or acceptance-criteria closeout clears the gate.
    assert!(review_pass_clears_gate("pre-pr", 0));
    assert!(review_pass_clears_gate("closeout", 0));
    assert!(!review_pass_clears_gate("gates", 0));
    assert!(!review_pass_clears_gate("pre-commit", 0));
    // Failing (non-zero) review must NOT clear the gate.
    assert!(!review_pass_clears_gate("gates", 1));
    assert!(!review_pass_clears_gate("pre-pr", 2));
    assert!(!review_pass_clears_gate("pre-commit", 1));
    // Informational surfaces review nothing and never clear the gate.
    assert!(!review_pass_clears_gate("diff", 0));
    assert!(!review_pass_clears_gate("init", 0));
}

#[test]
fn warnings_gate_blocks_open_diagnostics_but_not_baseline() {
    let root = crate::test_support::unique_temp_dir("keel-review-warnings");
    let home = root.join("home");
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let ast = crate::proxy::CommandAst::new(
        "dart".to_string(),
        vec!["analyze".to_string(), "--format=machine".to_string()],
        workspace.clone(),
    );
    crate::proxy::warnings::reconcile_capture(
        &home,
        &workspace,
        &ast,
        b"INFO|LINT|AVOID_PRINT|lib/old.dart|4|3|5|Old warning.\n",
        &[],
        0,
        "dart analyze --format=machine",
        "2026-09-09T00:00:00Z",
        "raw-1",
    )
    .unwrap();
    let home_text = home.to_string_lossy();
    let baseline = warnings_gate(&workspace, &home_text);
    assert_eq!(baseline.status, GateStatus::Warn);
    assert!(!baseline.blocking);
    assert!(baseline.details.unwrap().contains("baseline=1"));

    crate::proxy::warnings::reconcile_capture(
        &home,
        &workspace,
        &ast,
        b"INFO|LINT|AVOID_PRINT|lib/old.dart|4|3|5|Old warning.\nINFO|LINT|DEAD_CODE|lib/new.dart|8|2|4|Dead code.\n",
        &[],
        0,
        "dart analyze --format=machine",
        "2026-09-09T00:01:00Z",
        "raw-2",
    )
    .unwrap();
    let open = warnings_gate(&workspace, &home_text);
    assert_eq!(open.status, GateStatus::Fail);
    assert!(open.blocking);
    assert!(open.details.unwrap().contains("open=1"));
    let _ = std::fs::remove_dir_all(root);
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

#[test]
fn research_gate_requires_a_named_plan_for_uncommitted_brownfield_work() {
    let repository = init_research_gate_repo("missing-plan");
    let gate = research_traceability_gate(&repository, "main", "pre-pr", "", "");
    assert!(gate.blocking);
    assert_eq!(gate.status, GateStatus::Fail);
    assert!(gate
        .details
        .as_deref()
        .unwrap_or_default()
        .contains("--plan"));
    let _ = std::fs::remove_dir_all(repository);
}

#[test]
fn research_gate_lists_untraced_claims_from_the_named_plan() {
    let repository = init_research_gate_repo("untraced-claim");
    let keel_home = crate::test_support::unique_temp_dir("keel-research-gate-home");
    let (plan_id, research_path) = create_researched_plan(&repository, &keel_home);
    let passing = research_traceability_gate(
        &repository,
        "main",
        "pre-pr",
        &plan_id,
        keel_home.to_str().expect("UTF-8 Keel home"),
    );
    assert_eq!(passing.status, GateStatus::Pass);
    let mut research: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&research_path).expect("read research artifact"),
    )
    .expect("parse research artifact");
    research["claims"][0]["usedBy"] = serde_json::json!([]);
    std::fs::write(
        &research_path,
        format!(
            "{}\n",
            serde_json::to_string_pretty(&research).expect("render research artifact")
        ),
    )
    .expect("write untraced research artifact");

    let gate = research_traceability_gate(
        &repository,
        "main",
        "pre-pr",
        &plan_id,
        keel_home.to_str().expect("UTF-8 Keel home"),
    );
    assert_eq!(gate.status, GateStatus::Fail);
    assert!(gate
        .details
        .as_deref()
        .unwrap_or_default()
        .contains("CLM-001 has no usedBy IDs"));
    let _ = std::fs::remove_dir_all(repository);
    let _ = std::fs::remove_dir_all(keel_home);
}

#[test]
fn architecture_gate_blocks_incomplete_design_and_passes_complete_design() {
    let repository = init_research_gate_repo("architecture-design");
    let keel_home = crate::test_support::unique_temp_dir("keel-architecture-gate-home");
    let (plan_id, research_path) = create_researched_plan(&repository, &keel_home);
    let architecture_path = research_path
        .parent()
        .expect("research artifact has a plan directory")
        .join("architecture.md");
    let complete = complete_review_architecture(&plan_id);
    std::fs::write(
        &architecture_path,
        complete.replacen("Rollback: Revert", "Rollback-missing: Revert", 1),
    )
    .expect("write incomplete architecture");
    let blocked = architecture_design_gate(
        &repository,
        "main",
        "pre-pr",
        &plan_id,
        keel_home.to_str().expect("UTF-8 Keel home"),
    );
    assert_eq!(blocked.name, "architecture_design");
    assert_eq!(blocked.status, GateStatus::Fail);
    assert!(blocked
        .details
        .as_deref()
        .unwrap_or_default()
        .contains("Rollback"));

    std::fs::write(&architecture_path, complete).expect("write complete architecture");
    let design = [
        "design".to_string(),
        "--plan".to_string(),
        plan_id.clone(),
        "--workspace-root".to_string(),
        repository.to_string_lossy().into_owned(),
        "--claude-home".to_string(),
        keel_home.to_string_lossy().into_owned(),
    ];
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    assert_eq!(
        crate::utility::plan::run_plan_command(&design, &mut stdout, &mut stderr),
        0,
        "design failed: {}",
        String::from_utf8_lossy(&stderr)
    );
    let passing = architecture_design_gate(
        &repository,
        "main",
        "pre-pr",
        &plan_id,
        keel_home.to_str().expect("UTF-8 Keel home"),
    );
    assert_eq!(passing.status, GateStatus::Pass);
    let _ = std::fs::remove_dir_all(repository);
    let _ = std::fs::remove_dir_all(keel_home);
}

#[test]
fn task_evidence_gate_rejects_unjustified_status_and_tampered_evidence() {
    let repository = init_research_gate_repo("task-evidence");
    let keel_home = crate::test_support::unique_temp_dir("keel-task-evidence-gate-home");
    let (plan_id, research_path) = create_researched_plan(&repository, &keel_home);
    let plan_path = research_path.parent().expect("plan directory");
    std::fs::write(
        plan_path.join("architecture.md"),
        complete_review_architecture(&plan_id),
    )
    .expect("write complete architecture");
    for action in ["design", "tasks"] {
        let arguments = [
            action.to_string(),
            "--plan".to_string(),
            plan_id.clone(),
            "--workspace-root".to_string(),
            repository.to_string_lossy().into_owned(),
            "--claude-home".to_string(),
            keel_home.to_string_lossy().into_owned(),
        ];
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        assert_eq!(
            crate::utility::plan::run_plan_command(&arguments, &mut stdout, &mut stderr),
            0,
            "{action} failed: {}",
            String::from_utf8_lossy(&stderr)
        );
    }

    let ticket_path = plan_path.join("task-001.json");
    let mut ticket: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&ticket_path).expect("read task ticket"))
            .expect("parse task ticket");
    let original = ticket.clone();
    ticket["layers"]["docs"][0]["status"] = serde_json::Value::String("not_applicable".to_string());
    std::fs::write(
        &ticket_path,
        format!(
            "{}\n",
            serde_json::to_string_pretty(&ticket).expect("render task ticket")
        ),
    )
    .expect("write unjustified ticket");
    let keel_home_text = keel_home.to_str().expect("UTF-8 Keel home");
    let unjustified = task_evidence_gate(&repository, "main", "pre-pr", &plan_id, keel_home_text);
    assert_eq!(unjustified.status, GateStatus::Fail);
    assert!(unjustified
        .details
        .as_deref()
        .unwrap_or_default()
        .contains("requires a non-empty reason"));

    ticket = original;
    let subtask_id = ticket["layers"]["tests"][0]["id"]
        .as_str()
        .expect("tests subtask id")
        .to_string();
    let recorded_at = "2026-09-09T00:00:00Z";
    let evidence = serde_json::json!({
        "schema_version": 1,
        "artifact": "task_evidence",
        "plan_id": plan_id,
        "task_id": "TASK-001",
        "subtask_id": subtask_id,
        "evidence_type": "named_test",
        "recorded_at": recorded_at,
        "test_name": "review task evidence",
        "result": "pass",
        "output_hash": "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
    });
    let evidence_body = format!(
        "{}\n",
        serde_json::to_string_pretty(&evidence).expect("render evidence")
    );
    std::fs::create_dir_all(plan_path.join("evidence")).expect("create evidence directory");
    std::fs::write(plan_path.join("evidence/tests.json"), &evidence_body).expect("write evidence");
    ticket["layers"]["tests"][0]["status"] = serde_json::Value::String("done".to_string());
    ticket["layers"]["tests"][0]["verification_timestamp"] =
        serde_json::Value::String(recorded_at.to_string());
    ticket["layers"]["tests"][0]["evidence_ref"] = serde_json::json!({
        "path": "evidence/tests.json",
        "content_hash": format!(
            "fnv1a64:{}",
            crate::utility::hashing::fnv1a64_hex(&evidence_body)
        )
    });
    std::fs::write(
        &ticket_path,
        format!(
            "{}\n",
            serde_json::to_string_pretty(&ticket).expect("render evidence ticket")
        ),
    )
    .expect("write evidence ticket");
    let task_refresh = [
        "tasks".to_string(),
        "--plan".to_string(),
        plan_id.clone(),
        "--workspace-root".to_string(),
        repository.to_string_lossy().into_owned(),
        "--claude-home".to_string(),
        keel_home.to_string_lossy().into_owned(),
    ];
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    assert_eq!(
        crate::utility::plan::run_plan_command(&task_refresh, &mut stdout, &mut stderr),
        0,
        "task refresh failed: {}",
        String::from_utf8_lossy(&stderr)
    );
    assert_eq!(
        task_evidence_gate(&repository, "main", "pre-pr", &plan_id, keel_home_text,).status,
        GateStatus::Pass
    );

    let mut tampered = evidence;
    tampered["result"] = serde_json::Value::String("fail".to_string());
    std::fs::write(
        plan_path.join("evidence/tests.json"),
        format!(
            "{}\n",
            serde_json::to_string_pretty(&tampered).expect("render tampered evidence")
        ),
    )
    .expect("tamper evidence");
    let tampered_gate = task_evidence_gate(&repository, "main", "pre-pr", &plan_id, keel_home_text);
    assert_eq!(tampered_gate.status, GateStatus::Fail);
    assert!(tampered_gate
        .details
        .as_deref()
        .unwrap_or_default()
        .contains("content_hash does not match evidence artifact"));
    let _ = std::fs::remove_dir_all(repository);
    let _ = std::fs::remove_dir_all(keel_home);
}

fn init_research_gate_repo(label: &str) -> crate::test_support::TestTempDir {
    let repository = crate::test_support::unique_temp_dir(&format!("keel-research-{label}"));
    std::fs::create_dir_all(repository.join("src")).expect("create source directory");
    git_in(&repository, &["init", "-q"]);
    git_in(&repository, &["config", "user.email", "test@example.com"]);
    git_in(&repository, &["config", "user.name", "Test"]);
    git_in(&repository, &["checkout", "-q", "-B", "main"]);
    std::fs::write(
        repository.join("src/lib.rs"),
        "pub fn value() -> u8 { 1 }\n",
    )
    .expect("write baseline source");
    git_in(&repository, &["add", "."]);
    git_in(&repository, &["commit", "-q", "-m", "base"]);
    std::fs::write(
        repository.join("src/lib.rs"),
        "pub fn value() -> u8 { 2 }\n",
    )
    .expect("modify established source");
    repository
}

fn create_researched_plan(
    repository: &std::path::Path,
    keel_home: &std::path::Path,
) -> (String, std::path::PathBuf) {
    let common = [
        "--workspace-root".to_string(),
        repository.to_string_lossy().into_owned(),
        "--claude-home".to_string(),
        keel_home.to_string_lossy().into_owned(),
        "--json".to_string(),
    ];
    let mut specify = vec![
        "specify".to_string(),
        "--request".to_string(),
        "Change the established src/lib.rs value function while preserving its callers."
            .to_string(),
    ];
    specify.extend(common.clone());
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    assert_eq!(
        crate::utility::plan::run_plan_command(&specify, &mut stdout, &mut stderr),
        0,
        "specify failed: {}",
        String::from_utf8_lossy(&stderr)
    );
    let payload: serde_json::Value = serde_json::from_slice(&stdout).expect("parse specify JSON");
    let plan_id = payload["planId"].as_str().expect("plan id").to_string();
    let plan_path = std::path::PathBuf::from(payload["planPath"].as_str().expect("plan path"));
    let mut research = vec![
        "research".to_string(),
        "--plan".to_string(),
        plan_id.clone(),
        "--claim".to_string(),
        "Chrono parses RFC3339 timestamps.".to_string(),
        "--source-url".to_string(),
        "https://docs.rs/chrono/0.4.45/chrono/struct.DateTime.html".to_string(),
        "--source-type".to_string(),
        "official-doc".to_string(),
        "--retrieved-at".to_string(),
        chrono::Utc::now().to_rfc3339(),
        "--support".to_string(),
        "The current crate documentation exposes DateTime::parse_from_rfc3339.".to_string(),
        "--freshness".to_string(),
        "fresh".to_string(),
        "--used-by".to_string(),
        "REQ-001,AC-001".to_string(),
    ];
    research.extend(common);
    stdout.clear();
    stderr.clear();
    assert_eq!(
        crate::utility::plan::run_plan_command(&research, &mut stdout, &mut stderr),
        0,
        "research failed: {}",
        String::from_utf8_lossy(&stderr)
    );
    (plan_id, plan_path.join("research.json"))
}

fn complete_review_architecture(plan_id: &str) -> String {
    format!(
        "---\nschema_version: 1\nartifact: architecture\nplan_id: {plan_id}\n---\n\nStatus: complete\n\n# Architecture Note\n\n## 1. Current architecture relevant to scope\n\n[verified: CLM-001] The established source owner was read.\n\n## 2. Proposed architecture\n\n[derived: CLM-002] Change only the existing owner path.\n\nInput bound: One bounded plan artifact.\n\nPolicy owner: The existing planner remains the lifecycle owner.\n\n## 3. Components/files/interfaces changed\n\n- Component: established source owner | Requirements: REQ-001 | Acceptance: AC-001\n\n## 4. Data/control flow\n\nThe command updates the established owner and existing callers observe the result.\n\n## 5. Alternatives considered\n\nAlternative: Add a second owner.\n\nTradeoff: A second owner duplicates existing policy.\n\n## 6. Why the chosen option fits requirements\n\nChosen option: Extend the established owner.\n\nInfrastructure reuse: Reuse the planner and review gate infrastructure.\n\nConstraint fit: The design maps only REQ-001 and AC-001.\n\n## 7. Risks and mitigations\n\nRisk: A stale design could reach review.\n\nMitigation: Pre-PR review validates the named plan architecture.\n\n## 8. Backward compatibility\n\nCompatibility: Existing fields and behavior remain available.\n\nHost impact: none; host contracts remain unchanged.\n\n## 9. Error handling and fallback semantics\n\nFailure status: Invalid architecture blocks pre-PR review.\n\nFallback: none; repair the canonical architecture note.\n\nVisibility: reviewer output lists the design defect.\n\n## 10. Security/privacy implications\n\nSecurity/privacy: The gate reads one local artifact and no credentials.\n\n## 11. Performance/token impact\n\nToken impact: Architecture input stays bounded.\n\nMeasurement plan: Run the fixed-context budget test.\n\n## 12. Test strategy\n\nVerification: Run review unit tests and planner integration tests.\n\nAcceptance references: AC-001\n\n## 13. Rollback strategy\n\nRollback: Revert the implementation commit.\n\n## 14. Requirement and research references\n\nRequirement references: REQ-001\n\nAcceptance references: AC-001\n\nClaim references: CLM-001, CLM-002\n"
    )
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
    assert_eq!(
        brownfield_source_from_name_status("R050\tsrc/a.rs\tvendor/b.rs"),
        None
    );
    assert_eq!(
        brownfield_source_from_name_status("R050\tsrc/a.rs\tdocs/b.md"),
        None
    );
    assert_eq!(brownfield_source_from_name_status("R050\tonly-one"), None);
}

#[test]
fn completeness_touched_sources_includes_working_tree_when_range_is_empty() {
    let root = std::env::current_dir().expect("cwd");
    let from_head = changed_sources_including_added(&root, &["HEAD".to_string()]);
    let from_empty_range = completeness_touched_sources(&root, &["HEAD...HEAD".to_string()]);
    assert!(from_head.is_some() && from_empty_range.is_some());
    let head = from_head.unwrap();
    let combined = from_empty_range.unwrap();
    for path in &head {
        assert!(
            combined.iter().any(|item| item == path),
            "working-tree path {path} missing from empty-range union"
        );
    }
}

#[test]
fn completeness_gate_includes_added_source_unlike_flow() {
    assert_eq!(
        completeness_source_from_name_status("A\trust/crates/keel/src/new_module.rs"),
        Some("rust/crates/keel/src/new_module.rs".to_string())
    );
    assert_eq!(
        completeness_source_from_name_status("M\trust/crates/keel/src/review.rs"),
        Some("rust/crates/keel/src/review.rs".to_string())
    );
    assert_eq!(
        completeness_source_from_name_status("R050\told.rs\tsrc/new.rs"),
        Some("src/new.rs".to_string())
    );
    assert_eq!(completeness_source_from_name_status("M\tREADME.md"), None);
}

#[test]
fn completeness_scan_satisfies_after_marker() {
    let _guard = crate::test_support::ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = std::env::temp_dir().join(format!(
        "keel-completeness-review-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let previous = std::env::var("CLAUDE_TARGET_OVERRIDE").ok();
    std::env::set_var("CLAUDE_TARGET_OVERRIDE", &home);
    let workspace = home.join("ws");
    std::fs::create_dir_all(&workspace).expect("ws");
    let cwd = crate::runtime::display_path(&workspace);
    assert!(!crate::runner::hook_lifecycle::completeness_scan_satisfies(
        &cwd, 1
    ));
    crate::runner::hook_lifecycle::record_completeness_gate_clear_for(&workspace, &[], &[], 0);
    assert!(crate::runner::hook_lifecycle::completeness_scan_satisfies(
        &cwd, 0
    ));
    match previous {
        Some(value) => std::env::set_var("CLAUDE_TARGET_OVERRIDE", value),
        None => std::env::remove_var("CLAUDE_TARGET_OVERRIDE"),
    }
    let _ = std::fs::remove_dir_all(&home);
}

/// The review gate binds the marker to content: a scan of one change must not
/// satisfy the gate for a different change, and legacy bare-millisecond
/// markers (no changed set) fail the coverage half while still satisfying the
/// mtime reminder path.
#[test]
fn completeness_cover_requires_recorded_changed_set() {
    let _guard = crate::test_support::ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = std::env::temp_dir().join(format!(
        "keel-completeness-cover-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let previous = std::env::var("CLAUDE_TARGET_OVERRIDE").ok();
    std::env::set_var("CLAUDE_TARGET_OVERRIDE", &home);
    let workspace = home.join("ws");
    std::fs::create_dir_all(&workspace).expect("ws");
    let cwd = crate::runtime::display_path(&workspace);
    let touched = vec!["src/auth.rs".to_string(), "src/db.rs".to_string()];
    // No marker: neither half passes.
    assert!(!crate::runner::hook_lifecycle::completeness_scan_satisfies(
        &cwd, 0
    ));
    assert!(!completeness_scan_covers_changed(&cwd, &touched));
    // Legacy bare-millisecond marker: mtime half passes, coverage never does.
    let dir = home.join("state").join("completeness-gate");
    std::fs::create_dir_all(&dir).expect("marker dir");
    std::fs::write(
        dir.join(format!(
            "{}.scanned",
            crate::runner::hook_lifecycle::completeness_marker_key(&cwd)
        )),
        "9999999999999",
    )
    .expect("legacy marker");
    assert!(crate::runner::hook_lifecycle::completeness_scan_satisfies(
        &cwd, 0
    ));
    assert!(!completeness_scan_covers_changed(&cwd, &touched));
    // JSON scan covering only auth: covers [auth], not [auth, db].
    crate::runner::hook_lifecycle::record_completeness_gate_clear_for(
        &workspace,
        &["auth shape".to_string()],
        &["src/auth.rs".to_string()],
        2,
    );
    assert!(completeness_scan_covers_changed(
        &cwd,
        &["src/auth.rs".to_string()]
    ));
    assert!(!completeness_scan_covers_changed(&cwd, &touched));
    // Full coverage passes.
    crate::runner::hook_lifecycle::record_completeness_gate_clear_for(
        &workspace,
        &["auth shape".to_string()],
        &["src/auth.rs".to_string(), "src/db.rs".to_string()],
        2,
    );
    assert!(completeness_scan_covers_changed(&cwd, &touched));
    let record = crate::runner::hook_lifecycle::completeness_marker_record_for_workspace(&cwd)
        .expect("scan record present");
    assert_eq!(record.sibling_count, 2);
    assert_eq!(record.queries, vec!["auth shape".to_string()]);
    match previous {
        Some(value) => std::env::set_var("CLAUDE_TARGET_OVERRIDE", value),
        None => std::env::remove_var("CLAUDE_TARGET_OVERRIDE"),
    }
    let _ = std::fs::remove_dir_all(&home);
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

#[test]
fn artifact_relevance_requires_every_touched_path() {
    let touched = vec![
        "rust/crates/keel/src/review.rs".to_string(),
        "rust/crates/keel/src/commands.rs".to_string(),
    ];
    assert!(!artifact_targets_all_touched_files(
        &["rust/crates/keel/src/review.rs".to_string()],
        &touched
    ));
    assert!(artifact_targets_all_touched_files(
        &[
            "rust/crates/keel/src/commands.rs".to_string(),
            ".\\rust\\crates\\keel\\src\\review.rs".to_string(),
        ],
        &touched
    ));
}

#[test]
fn flow_gate_rejects_stale_or_false_exemption_evidence() {
    let _guard = crate::test_support::ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let repository = crate::test_support::unique_temp_dir("keel-flow-gate-repo");
    let keel_home = crate::test_support::unique_temp_dir("keel-flow-gate-home");
    let previous_home = std::env::var("KEEL_HOME").ok();
    std::env::set_var("KEEL_HOME", keel_home.as_os_str());
    let source = repository.join("src").join("main.rs");
    std::fs::create_dir_all(source.parent().expect("source parent")).expect("source directory");
    std::fs::write(&source, "fn main() {}\n").expect("initial source");
    for arguments in [
        vec!["init"],
        vec!["config", "user.email", "keel-tests@example.invalid"],
        vec!["config", "user.name", "Keel Tests"],
        vec!["add", "."],
        vec!["commit", "-m", "Add : TEST : seed repository"],
    ] {
        let status = std::process::Command::new("git")
            .args(arguments)
            .current_dir(&repository)
            .status()
            .expect("git command");
        assert!(status.success());
    }
    std::fs::write(&source, "fn main() { println!(\"one\"); }\n").expect("first edit");

    let mut check = keel_flow::new_template_check("src/main.rs", "main");
    check.current_behavior = "program entry point".into();
    check.entry_point = "main".into();
    check.producer = "main".into();
    check.source_of_truth = "src/main.rs".into();
    check.storage_state_queue_owner = "Not found".into();
    check.side_effect_owner = "main".into();
    check.consumers = vec!["process".into()];
    check.cleanup_recovery_path = "process exit".into();
    check.edit_boundary = "src/main.rs".into();
    check.validation_needed = vec!["flow gate".into()];
    check.validation_evidence = vec!["test fixture".into()];
    let finalized = keel_flow::finalize_check(&repository, check).expect("finalize check");
    keel_flow::write_check(
        &repository,
        keel_flow::DEFAULT_ARTIFACT_PATH,
        finalized.clone(),
    )
    .expect("write check");
    assert_eq!(
        flow_check_gate(&repository, "", "pre-commit").status,
        GateStatus::Pass
    );

    std::fs::write(&source, "fn main() { println!(\"two\"); }\n").expect("later edit");
    let stale = flow_check_gate(&repository, "", "pre-commit");
    assert_eq!(stale.status, GateStatus::Fail);
    assert!(stale.details.unwrap_or_default().contains("stale"));

    let mut exempt = keel_flow::finalize_check(&repository, finalized).expect("refinalize check");
    exempt.docs_only = true;
    keel_flow::write_check(&repository, keel_flow::DEFAULT_ARTIFACT_PATH, exempt)
        .expect("write exemption");
    let false_exemption = flow_check_gate(&repository, "", "pre-commit");
    assert_eq!(false_exemption.status, GateStatus::Fail);
    assert!(false_exemption
        .details
        .unwrap_or_default()
        .contains("claims an exemption"));

    match previous_home {
        Some(value) => std::env::set_var("KEEL_HOME", value),
        None => std::env::remove_var("KEEL_HOME"),
    }
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
    let slug = workflow_slug("D:\\Nasri\\Project\\keel");
    assert!(slug.starts_with("d-nasri-project-keel-"));
    assert!(slug.chars().all(|character| character.is_ascii_lowercase()
        || character.is_ascii_digit()
        || character == '-'));
    assert!(!workflow_slug("").is_empty());
}

#[test]
fn workflow_slug_keeps_distinct_long_paths_distinct() {
    let shared = "C:/work/this-is-a-very-long-workspace-prefix-that-used-to-be-truncated-before-the-distinguishing-segment/";
    let first = workflow_slug(&format!("{shared}alpha"));
    let second = workflow_slug(&format!("{shared}beta"));

    assert_ne!(
        first, second,
        "long workspace paths must not share one record store"
    );
    assert!(first.len() <= 64);
    assert!(second.len() <= 64);
}

#[test]
fn workflow_slug_matches_the_canonical_workspace_lane() {
    let raw = r"D:\Nasri\Project\keel";
    assert_eq!(
        workflow_slug(raw),
        crate::utility::system_map::workspace_key(raw)
    );
}

#[test]
fn workflow_preferences_copy_from_the_legacy_lane() {
    let repository = crate::test_support::unique_temp_dir("keel-workflow-migrate-repo");
    let home = crate::test_support::unique_temp_dir("keel-workflow-migrate-home");
    let legacy_slug = crate::utility::system_map::sanitize_key(&repository.to_string_lossy());
    let legacy_store = crate::utility::record_store::RecordStore::new(
        &home,
        &format!("memories/workspaces/{legacy_slug}/git-workflow"),
    );
    legacy_store
        .write_record(
            WORKFLOW_PREF_RECORD_ID,
            &vec![("model".to_string(), "four-tier".to_string())],
        )
        .expect("legacy preference");

    let canonical = workflow_pref_store(&repository, &home);
    assert!(canonical
        .read_record(WORKFLOW_PREF_RECORD_ID)
        .expect("canonical preference")
        .is_some());
    assert!(legacy_store.record_path(WORKFLOW_PREF_RECORD_ID).is_file());
}

#[test]
fn git_workflow_configure_rejects_an_unsupported_model() {
    let repository = crate::test_support::unique_temp_dir("keel-workflow-model");
    let keel_home = crate::test_support::unique_temp_dir("keel-workflow-model-home");
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = run_git_workflow_configure(
        &[
            "--repo-root".into(),
            repository.to_string_lossy().into_owned(),
            "--claude-home".into(),
            keel_home.to_string_lossy().into_owned(),
            "--model".into(),
            "invented-model".into(),
        ],
        &mut stdout,
        &mut stderr,
    );

    assert_eq!(code, 1);
    assert!(String::from_utf8_lossy(&stderr).contains("supported model is four-tier"));
}

#[test]
fn review_policy_show_succeeds_with_no_extra_args() {
    // The handler accepts the documented one-argument `review policy show` form.
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
    let temp = crate::test_support::unique_temp_dir("keel-review-test");
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
    let temp = crate::test_support::unique_temp_dir("keel-review-js-test");
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
    assert!(validate_commit_subject("Wip : RGB : Build light effect mode (multi color)").is_ok());
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
    let temp = crate::test_support::unique_temp_dir("keel-no-cargo-test");
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
    let dir = std::env::temp_dir().join(format!("keel-preflight-nongit-{}", std::process::id()));
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
    // Non-conventional subject to should produce a [warn], not a block.
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
    // Main with no commits ahead is blocked.
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
#[test]
fn compact_gate_output_includes_actionable_details() {
    let results = vec![GateResult {
        name: "rust_tests".to_string(),
        status: GateStatus::Fail,
        blocking: true,
        details: Some("cargo test failed: see stderr; rerun cargo test --workspace".to_string()),
    }];
    let mut output = Vec::new();
    render_gate_results(&results, 1, 0, "compact", &mut output);
    let rendered = String::from_utf8(output).expect("utf8");
    assert!(rendered.contains("rust_tests=fail"));
    assert!(rendered.contains("cargo test failed"));
    assert!(rendered.contains("rerun cargo test"));
}

#[test]
fn gate_status_honest_semantics_and_serialization() {
    let variants = [
        (GateStatus::Pass, "pass", "[PASS]", true, false),
        (GateStatus::Fail, "fail", "[FAIL]", false, true),
        (GateStatus::Warn, "warn", "[WARN]", false, false),
        (GateStatus::Skipped, "skipped", "[SKIP]", false, true),
        (
            GateStatus::NotApplicable,
            "not_applicable",
            "[N/A]",
            false,
            false,
        ),
        (
            GateStatus::NeedsHuman,
            "needs_human",
            "[HUMAN]",
            false,
            true,
        ),
        (GateStatus::Unclear, "unclear", "[UNCLEAR]", false, true),
        (GateStatus::Blocked, "blocked", "[BLK]", false, true),
        (GateStatus::Expired, "expired", "[EXPIRED]", false, true),
        (GateStatus::NotRun, "not_run", "[NOTRUN]", false, true),
        (
            GateStatus::Indeterminate,
            "indeterminate",
            "[INDET]",
            false,
            true,
        ),
        (
            GateStatus::PolicyViolation,
            "policy_violation",
            "[POLICY]",
            false,
            true,
        ),
    ];
    for (status, name, icon, is_pass, is_blocking) in variants {
        assert_eq!(status.as_str(), name);
        assert_eq!(status.icon(), icon);
        assert_eq!(status.is_pass(), is_pass);
        assert_eq!(status.is_blocking(), is_blocking);
        let json = serde_json::to_string(&status).expect("serialize status");
        assert_eq!(json, format!("\"{name}\""));
        let deserialized: GateStatus = serde_json::from_str(&json).expect("deserialize status");
        assert_eq!(deserialized, status);
    }
}

/// The plan's honesty rule: no aggregate may collapse a non-pass state to pass.
/// Exactly one variant is a pass, and every other variant is either blocking or
/// an explicitly non-blocking `Warn`/`NotApplicable`, so a caller that asks
/// `is_pass()` or `is_blocking()` can never read uncertainty as success.
#[test]
fn no_gate_status_can_be_read_as_a_pass() {
    let variants = [
        GateStatus::Pass,
        GateStatus::Fail,
        GateStatus::Warn,
        GateStatus::Skipped,
        GateStatus::NotApplicable,
        GateStatus::NeedsHuman,
        GateStatus::Unclear,
        GateStatus::Blocked,
        GateStatus::Expired,
        GateStatus::NotRun,
        GateStatus::Indeterminate,
        GateStatus::PolicyViolation,
    ];
    let passes = variants.iter().filter(|status| status.is_pass()).count();
    assert_eq!(passes, 1, "only one status may report as a pass");

    let names: Vec<&str> = variants.iter().map(|status| status.as_str()).collect();
    let unique: std::collections::BTreeSet<&str> = names.iter().copied().collect();
    assert_eq!(unique.len(), names.len(), "status names must be unique");

    for status in &variants {
        if status.is_pass() {
            continue;
        }
        let non_blocking = !status.is_blocking();
        assert!(
            !non_blocking || matches!(status, GateStatus::Warn | GateStatus::NotApplicable),
            "{} is non-pass and non-blocking, so an aggregate could read it as success",
            status.as_str()
        );
    }
}

#[test]
fn tally_gate_results_honest_counts() {
    let results = vec![
        GateResult {
            name: "pass_gate".to_string(),
            status: GateStatus::Pass,
            blocking: false,
            details: None,
        },
        GateResult {
            name: "fail_gate".to_string(),
            status: GateStatus::Fail,
            blocking: true,
            details: None,
        },
        GateResult {
            name: "warn_gate".to_string(),
            status: GateStatus::Warn,
            blocking: false,
            details: None,
        },
        GateResult {
            name: "human_gate".to_string(),
            status: GateStatus::NeedsHuman,
            blocking: true,
            details: None,
        },
        GateResult {
            name: "unclear_gate".to_string(),
            status: GateStatus::Unclear,
            blocking: true,
            details: None,
        },
        GateResult {
            name: "skipped_gate".to_string(),
            status: GateStatus::Skipped,
            blocking: true,
            details: None,
        },
        GateResult {
            name: "na_gate".to_string(),
            status: GateStatus::NotApplicable,
            blocking: false,
            details: None,
        },
        GateResult {
            name: "blocked_gate".to_string(),
            status: GateStatus::Blocked,
            blocking: true,
            details: None,
        },
    ];
    let (blocking, warnings) = tally_gate_results(&results);
    assert_eq!(blocking, 5);
    assert_eq!(warnings, 1);
}

#[test]
fn gate_status_rendering_all_formats() {
    let results = vec![
        GateResult {
            name: "gate_human".to_string(),
            status: GateStatus::NeedsHuman,
            blocking: true,
            details: Some("visual check needed".to_string()),
        },
        GateResult {
            name: "gate_na".to_string(),
            status: GateStatus::NotApplicable,
            blocking: false,
            details: Some("not applicable".to_string()),
        },
        GateResult {
            name: "gate_unclear".to_string(),
            status: GateStatus::Unclear,
            blocking: true,
            details: Some("ambiguous result".to_string()),
        },
    ];

    let mut json_out = Vec::new();
    render_gate_results(&results, 2, 0, "json", &mut json_out);
    let json_str = String::from_utf8(json_out).expect("utf8 json");
    assert!(json_str.contains("\"needs_human\""));
    assert!(json_str.contains("\"not_applicable\""));
    assert!(json_str.contains("\"unclear\""));

    let mut md_out = Vec::new();
    render_gate_results(&results, 2, 0, "markdown", &mut md_out);
    let md_str = String::from_utf8(md_out).expect("utf8 md");
    assert!(md_str.contains("[HUMAN]"));
    assert!(md_str.contains("[N/A]"));
    assert!(md_str.contains("[UNCLEAR]"));

    let mut compact_out = Vec::new();
    render_gate_results(&results, 2, 0, "compact", &mut compact_out);
    let compact_str = String::from_utf8(compact_out).expect("utf8 compact");
    assert!(compact_str.contains("gate_human=needs_human"));
    assert!(compact_str.contains("gate_na=not_applicable"));
    assert!(compact_str.contains("gate_unclear=unclear"));
}

fn check_precommit_impact(repo: &Path) -> GateResult {
    impact_gate(repo, "main", "pre-commit")
}

fn assert_non_blocking_gate(result: &GateResult, expected: GateStatus) {
    assert_eq!(result.status, expected);
    assert!(!result.blocking);
}

#[test]
fn impact_gate_unresolvable_diff_returns_warn() {
    let temp = crate::test_support::unique_temp_dir("keel-impact-unresolvable");
    let result = impact_gate(&temp, "HEAD~1", "pre-commit");
    assert_non_blocking_gate(&result, GateStatus::Warn);
    assert!(result
        .details
        .unwrap_or_default()
        .contains("could not resolve diff range"));
}

#[test]
fn impact_gate_empty_touched_returns_not_applicable() {
    let repository = crate::test_support::unique_temp_dir("keel-impact-empty");
    git_in(&repository, &["init", "-q"]);
    git_in(&repository, &["config", "user.email", "test@example.com"]);
    git_in(&repository, &["config", "user.name", "Test"]);
    git_in(&repository, &["checkout", "-q", "-B", "main"]);
    std::fs::write(repository.join("README.md"), "# Clean\n").expect("write readme");
    git_in(&repository, &["add", "."]);
    git_in(&repository, &["commit", "-q", "-m", "init"]);

    let clean_result = check_precommit_impact(&repository);
    assert_non_blocking_gate(&clean_result, GateStatus::NotApplicable);
    assert!(clean_result
        .details
        .unwrap_or_default()
        .contains("no existing source modified"));
}

#[test]
fn impact_gate_missing_graph_without_flow_blocks() {
    let _guard = crate::test_support::ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let repository = init_research_gate_repo("impact-missing-flow");
    let result = check_precommit_impact(&repository);
    assert_eq!(result.status, GateStatus::Fail);
    assert!(result.blocking);
    let details = result.details.unwrap_or_default();
    assert!(details.contains("code graph unavailable and flow evidence is missing"));
    assert!(details.contains("keel code-graph build"));
}

#[test]
fn impact_gate_missing_graph_with_valid_flow_warns() {
    let _guard = crate::test_support::ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let repository = init_research_gate_repo("impact-valid-flow");
    let (head, diff_fingerprint) = keel_flow::repository_state(&repository).expect("repo state");
    let check = keel_flow::Check {
        version: keel_flow::SCHEMA_VERSION,
        target_file: "src/lib.rs".to_string(),
        target_files: vec!["src/lib.rs".to_string()],
        current_behavior: "Existing behavior remains unchanged.".to_string(),
        entry_point: "value".to_string(),
        producer: "value producer".to_string(),
        source_of_truth: "value owner".to_string(),
        storage_state_queue_owner: "Not found".to_string(),
        side_effect_owner: "none".to_string(),
        consumers: vec!["caller".to_string()],
        cleanup_recovery_path: "none".to_string(),
        edit_boundary: "src/lib.rs only".to_string(),
        validation_needed: vec!["cargo test".to_string()],
        validation_evidence: vec!["cargo test passed".to_string()],
        repository_head: head,
        diff_fingerprint,
        finalized_at: "2026-09-09T00:00:00Z".to_string(),
        ..keel_flow::Check::default()
    };
    keel_flow::write_check(&repository, keel_flow::DEFAULT_ARTIFACT_PATH, check)
        .expect("write flow check");

    let valid_result = check_precommit_impact(&repository);
    assert_non_blocking_gate(&valid_result, GateStatus::Warn);
    let details = valid_result.details.unwrap_or_default();
    assert!(details.contains("code graph unavailable; impact check skipped"));
    assert!(details.contains("keel code-graph build"));
}

#[test]
fn acceptance_criteria_evaluation_honest_format() {
    let repository = init_research_gate_repo("ac-eval-test");
    let keel_home = crate::test_support::unique_temp_dir("keel-ac-eval-home");
    let (plan_id, research_path) = create_researched_plan(&repository, &keel_home);
    let plan_path = research_path.parent().expect("plan directory");
    std::fs::write(
        plan_path.join("architecture.md"),
        complete_review_architecture(&plan_id),
    )
    .expect("write complete architecture");
    for action in ["design", "tasks"] {
        let arguments = [
            action.to_string(),
            "--plan".to_string(),
            plan_id.clone(),
            "--workspace-root".to_string(),
            repository.to_string_lossy().into_owned(),
            "--claude-home".to_string(),
            keel_home.to_string_lossy().into_owned(),
        ];
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        assert_eq!(
            crate::utility::plan::run_plan_command(&arguments, &mut stdout, &mut stderr),
            0
        );
    }
    let keel_home_text = keel_home.to_str().expect("UTF-8 Keel home");
    let eval_ac = || {
        crate::utility::plan::evaluate_acceptance_criteria(&repository, keel_home_text, &plan_id)
            .expect("evaluate ac")
    };

    let (status, summary) = eval_ac();
    assert_eq!(status, GateStatus::Fail);
    assert!(summary.contains("AC-001: fail | missing traceability to implementation evidence"));

    let ticket_path = plan_path.join("task-001.json");
    let mut ticket: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&ticket_path).expect("read ticket"))
            .expect("parse ticket");
    let subtask_id = ticket["layers"]["tests"][0]["id"]
        .as_str()
        .expect("tests subtask id")
        .to_string();
    let evidence = serde_json::json!({
        "schema_version": 1,
        "artifact": "task_evidence",
        "plan_id": plan_id,
        "task_id": "TASK-001",
        "subtask_id": subtask_id,
        "evidence_type": "named_test",
        "recorded_at": "2026-09-09T00:00:00Z",
        "test_name": "review task evidence",
        "result": "pass",
        "output_hash": "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        "raw_store_id": "RAW-12345"
    });
    let evidence_body = format!("{}\n", serde_json::to_string_pretty(&evidence).unwrap());
    std::fs::create_dir_all(plan_path.join("evidence")).unwrap();
    std::fs::write(plan_path.join("evidence/tests.json"), &evidence_body).unwrap();
    ticket["layers"]["tests"][0]["status"] = serde_json::Value::String("done".to_string());
    ticket["layers"]["tests"][0]["verification_timestamp"] =
        serde_json::Value::String("2026-09-09T00:00:00Z".to_string());
    ticket["layers"]["tests"][0]["evidence_ref"] = serde_json::json!({
        "path": "evidence/tests.json",
        "content_hash": format!("fnv1a64:{}", crate::utility::hashing::fnv1a64_hex(&evidence_body))
    });
    std::fs::write(
        &ticket_path,
        format!("{}\n", serde_json::to_string_pretty(&ticket).unwrap()),
    )
    .unwrap();

    let task_refresh = [
        "tasks".to_string(),
        "--plan".to_string(),
        plan_id.clone(),
        "--workspace-root".to_string(),
        repository.to_string_lossy().into_owned(),
        "--claude-home".to_string(),
        keel_home.to_string_lossy().into_owned(),
    ];
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    assert_eq!(
        crate::utility::plan::run_plan_command(&task_refresh, &mut stdout, &mut stderr),
        0,
        "pass task refresh: {}",
        String::from_utf8_lossy(&stderr)
    );

    let (pass_status, pass_summary) = eval_ac();
    assert_eq!(pass_status, GateStatus::Pass);
    assert!(pass_summary.contains("AC-001: pass | evidence: RAW-12345 | verified by:"));

    let human_evidence = serde_json::json!({
        "schema_version": 1,
        "artifact": "task_evidence",
        "plan_id": plan_id,
        "task_id": "TASK-001",
        "subtask_id": subtask_id,
        "evidence_type": "named_test",
        "recorded_at": "2026-09-09T00:00:00Z",
        "test_name": "review ui check",
        "result": "needs_human",
        "output_hash": "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
        "reason": "layout verification requires visual check",
        "screenshot": "artifacts/screen.png"
    });
    let human_body = format!(
        "{}\n",
        serde_json::to_string_pretty(&human_evidence).unwrap()
    );
    std::fs::write(plan_path.join("evidence/tests.json"), &human_body).unwrap();

    let (human_status, human_summary) = eval_ac();
    assert_eq!(human_status, GateStatus::NeedsHuman);
    assert!(human_summary.contains("AC-001: needs_human | reason: layout verification requires visual check | screenshot: artifacts/screen.png"));
}

#[test]
fn parse_gh_checks_handles_tab_delimited_names_with_spaces() {
    let output = "CI gate\tpass\t3s\thttps://example.com/job/1\nvalidate-linux\tpass\t1m\thttps://example.com/job/2\n";
    let checks = super::ci::parse_gh_checks(output).expect("parsed checks");
    assert_eq!(checks.len(), 2);
    assert_eq!(checks[0].name, "CI gate");
    assert_eq!(checks[0].state, super::ci::CheckState::Green);
    assert_eq!(checks[1].name, "validate-linux");
    assert_eq!(checks[1].state, super::ci::CheckState::Green);
}
