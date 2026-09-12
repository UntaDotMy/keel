use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

struct TestTree {
    root: PathBuf,
    home: PathBuf,
}

impl Drop for TestTree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(
            self.root
                .parent()
                .expect("fixture root has a parent directory"),
        );
    }
}

fn test_tree(label: &str) -> TestTree {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let base = std::env::temp_dir().join(format!(
        "keel-task-ticket-{label}-{}-{nonce}",
        std::process::id()
    ));
    let root = base.join("workspace");
    let home = base.join("keel-home");
    fs::create_dir_all(&root).expect("create fixture workspace");
    fs::write(
        root.join("planner.rs"),
        "// Public CLI task ticket evidence owner used by the fixture.\n",
    )
    .expect("write fixture source anchor");
    TestTree { root, home }
}

fn plan_command(tree: &TestTree, arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_keel"))
        .arg("plan")
        .args(arguments)
        .args([
            "--workspace-root",
            tree.root.to_str().expect("UTF-8 workspace path"),
            "--claude-home",
            tree.home.to_str().expect("UTF-8 home path"),
            "--json",
        ])
        .output()
        .expect("run planner command")
}

fn assert_success(output: &Output, step: &str) {
    assert!(
        output.status.success(),
        "{step} failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path).expect("read JSON artifact"))
        .expect("parse JSON artifact")
}

fn write_json(path: &Path, value: &Value) {
    fs::write(
        path,
        format!(
            "{}\n",
            serde_json::to_string_pretty(value).expect("render JSON artifact")
        ),
    )
    .expect("write JSON artifact");
}

fn complete_architecture(plan_id: &str) -> String {
    format!(
        "---\nschema_version: 1\nartifact: architecture\nplan_id: {plan_id}\n---\n\nStatus: complete\n\n# Architecture Note\n\n## 1. Current architecture relevant to scope\n\n[verified: CLM-001] The planner.rs public CLI owner was read.\n\n## 2. Proposed architecture\n\n[derived: CLM-002] Extend the existing public CLI owner with task tickets.\n\nInput bound: One bounded JSON ticket.\n\nPolicy owner: The existing planner remains the lifecycle owner.\n\n## 3. Components/files/interfaces changed\n\n- Component: planner.rs public CLI | Requirements: REQ-001 | Acceptance: AC-001\n\n## 4. Data/control flow\n\nThe command compiles a task ticket and the checker resolves its evidence.\n\n## 5. Alternatives considered\n\nAlternative: Replace the aggregate task artifact.\n\nTradeoff: Replacement would break existing consumers.\n\n## 6. Why the chosen option fits requirements\n\nChosen option: Add tickets beside the aggregate artifact.\n\nInfrastructure reuse: Reuse the existing planner directory and RTM.\n\nConstraint fit: Existing fields remain available.\n\n## 7. Risks and mitigations\n\nRisk: A ticket could contain a dangling evidence path.\n\nMitigation: Resolve bounded relative paths and validate content fingerprints.\n\n## 8. Backward compatibility\n\nCompatibility: Existing aggregate task fields remain unchanged.\n\nHost impact: none; existing host contracts remain unchanged.\n\n## 9. Error handling and fallback semantics\n\nFailure status: Invalid ticket evidence exits non-zero.\n\nFallback: Legacy plans without tickets retain aggregate validation.\n\nVisibility: Planner and reviewer output list each defect.\n\n## 10. Security/privacy implications\n\nSecurity/privacy: Reject absolute, traversal, and symlink evidence paths.\n\n## 11. Performance/token impact\n\nToken impact: Ticket input is bounded.\n\nMeasurement plan: Run the fixed-context budget test.\n\n## 12. Test strategy\n\nVerification: Run the task ticket integration tests.\n\nAcceptance references: AC-001\n\n## 13. Rollback strategy\n\nRollback: Revert the implementation commit and retain evidence.\n\n## 14. Requirement and research references\n\nRequirement references: REQ-001\n\nAcceptance references: AC-001\n\nClaim references: CLM-001, CLM-002\n"
    )
}

fn advance_to_tasks(tree: &TestTree) -> (String, PathBuf) {
    let request =
        "Add a public CLI task ticket whose named tests exit 0 and preserve existing task fields.";
    let specify = plan_command(tree, &["specify", "--request", request]);
    assert_success(&specify, "plan specify");
    let payload: Value = serde_json::from_slice(&specify.stdout).expect("parse specify output");
    let plan_id = payload["planId"].as_str().expect("plan id").to_string();
    let plan_path = PathBuf::from(payload["planPath"].as_str().expect("plan path"));
    assert_success(
        &plan_command(tree, &["research", "--plan", &plan_id]),
        "plan research",
    );
    fs::write(
        plan_path.join("architecture.md"),
        complete_architecture(&plan_id),
    )
    .expect("write complete architecture");
    assert_success(
        &plan_command(tree, &["design", "--plan", &plan_id]),
        "plan design",
    );
    assert_success(
        &plan_command(tree, &["tasks", "--plan", &plan_id]),
        "plan tasks",
    );
    (plan_id, plan_path)
}

fn check_failure(tree: &TestTree, plan_id: &str, expected: &str) {
    let output = plan_command(tree, &["check", "--rtm", "--plan", plan_id]);
    assert!(!output.status.success(), "invalid plan unexpectedly passed");
    let standard_error = String::from_utf8_lossy(&output.stderr);
    assert!(
        standard_error.contains(expected),
        "expected {expected:?} in planner error: {standard_error}"
    );
}

fn fnv1a64_hex(content: &str) -> String {
    let mut hash: u64 = 14695981039346656037;
    for byte in content.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(1099511628211);
    }
    format!("{hash:016x}")
}

#[test]
fn tasks_write_hierarchical_scope_derived_tickets_and_complete_rtm_traces() {
    let tree = test_tree("generated-schema");
    let (plan_id, plan_path) = advance_to_tasks(&tree);
    let aggregate = read_json(&plan_path.join("tasks.json"));
    assert_eq!(aggregate["status"], "ready");
    assert_eq!(aggregate["tasks"][0]["status"], "pending");
    assert_eq!(aggregate["ticketFiles"], json!(["task-001.json"]));

    let ticket = read_json(&plan_path.join("task-001.json"));
    assert_eq!(ticket["schema_version"], 1);
    assert_eq!(ticket["artifact"], "task_ticket");
    assert_eq!(ticket["plan_id"], plan_id);
    assert_eq!(ticket["id"], "TASK-001");
    assert_eq!(ticket["requirement_refs"], json!(["REQ-001"]));
    assert_eq!(ticket["acceptance_refs"], json!(["AC-001"]));
    assert_eq!(ticket["status"], "planned");

    let layers = ticket["layers"].as_object().expect("ticket layers object");
    for base_layer in [
        "design",
        "implementation",
        "build",
        "tests",
        "lint_warnings",
        "security",
        "performance_tokens",
        "docs",
        "ui",
        "ux_accessibility",
        "release_rollback",
    ] {
        assert!(layers.contains_key(base_layer), "missing {base_layer}");
    }
    for derived_layer in [
        "implementation",
        "build",
        "tests",
        "lint_warnings",
        "docs",
        "compatibility",
        "security",
        "release_rollback",
    ] {
        let subtasks = layers[derived_layer]
            .as_array()
            .expect("derived layer array");
        assert!(!subtasks.is_empty(), "empty derived layer {derived_layer}");
        let subtask = &subtasks[0];
        for field in [
            "id",
            "description",
            "requirement_refs",
            "acceptance_refs",
            "status",
            "expected_evidence_type",
            "evidence_ref",
            "reason",
            "owner_role",
            "verification_timestamp",
        ] {
            assert!(
                subtask.get(field).is_some(),
                "missing subtask field {field}"
            );
        }
        assert_eq!(subtask["acceptance_refs"], json!(["AC-001"]));
    }

    let check = plan_command(&tree, &["check", "--rtm", "--plan", &plan_id]);
    assert_success(&check, "plan check --rtm");
    let payload: Value = serde_json::from_slice(&check.stdout).expect("parse check output");
    assert_eq!(
        payload["rtm"]["chain"],
        "User request -> Requirement -> Acceptance criterion -> Task -> Checklist subtask -> Evidence"
    );
    let traces = payload["rtm"]["traces"].as_array().expect("RTM traces");
    assert!(!traces.is_empty());
    assert!(traces.iter().all(|trace| {
        trace["userRequestRef"] == "request://submitted"
            && trace["requirementId"] == "REQ-001"
            && trace["acceptanceCriterionId"] == "AC-001"
            && trace["taskId"] == "TASK-001"
            && trace["subtaskId"].as_str().is_some()
            && trace["expectedEvidenceType"].as_str().is_some()
    }));
}

#[test]
fn task_subtasks_accept_the_documented_machine_resolvable_evidence_types() {
    let tree = test_tree("evidence-types");
    let (plan_id, plan_path) = advance_to_tasks(&tree);
    let ticket_path = plan_path.join("task-001.json");
    let original = read_json(&ticket_path);

    for evidence_type in [
        "command",
        "named_test",
        "lint_diagnostic",
        "source_hash",
        "raw_store",
        "screenshot",
        "benchmark",
    ] {
        let mut ticket = original.clone();
        ticket["layers"]["tests"][0]["expected_evidence_type"] =
            Value::String(evidence_type.to_string());
        write_json(&ticket_path, &ticket);
        assert_success(
            &plan_command(&tree, &["tasks", "--plan", &plan_id]),
            evidence_type,
        );
        assert_success(
            &plan_command(&tree, &["check", "--rtm", "--plan", &plan_id]),
            evidence_type,
        );
    }

    let mut unsupported = original;
    unsupported["layers"]["tests"][0]["expected_evidence_type"] =
        Value::String("human_attestation".to_string());
    write_json(&ticket_path, &unsupported);
    check_failure(&tree, &plan_id, "unsupported expected_evidence_type");
}

#[test]
fn completion_rejects_done_without_evidence_and_unjustified_not_applicable() {
    let tree = test_tree("completion-rules");
    let (plan_id, plan_path) = advance_to_tasks(&tree);
    let ticket_path = plan_path.join("task-001.json");
    let original = read_json(&ticket_path);

    let mut missing_evidence = original.clone();
    missing_evidence["layers"]["tests"][0]["status"] = Value::String("done".to_string());
    missing_evidence["layers"]["tests"][0]["verification_timestamp"] =
        Value::String("2026-09-09T00:00:00Z".to_string());
    write_json(&ticket_path, &missing_evidence);
    check_failure(&tree, &plan_id, "done without evidence_ref");

    let mut unjustified = original.clone();
    unjustified["layers"]["docs"][0]["status"] = Value::String("not_applicable".to_string());
    write_json(&ticket_path, &unjustified);
    check_failure(
        &tree,
        &plan_id,
        "not_applicable requires a non-empty reason",
    );

    let mut traversal = original;
    traversal["layers"]["tests"][0]["status"] = Value::String("done".to_string());
    traversal["layers"]["tests"][0]["verification_timestamp"] =
        Value::String("2026-09-09T00:00:00Z".to_string());
    traversal["layers"]["tests"][0]["evidence_ref"] = json!({
        "path": "../outside.json",
        "content_hash": "fnv1a64:0000000000000000"
    });
    write_json(&ticket_path, &traversal);
    check_failure(&tree, &plan_id, "unsafe evidence path");
}

#[test]
fn check_rejects_deleted_derived_layers_and_missing_or_dangling_rtm_links() {
    let tree = test_tree("rtm-links");
    let (plan_id, plan_path) = advance_to_tasks(&tree);
    let ticket_path = plan_path.join("task-001.json");
    let original_ticket = read_json(&ticket_path);
    let original_tasks = read_json(&plan_path.join("tasks.json"));
    let original_rtm = read_json(&plan_path.join("rtm.json"));

    let mut missing_layer = original_ticket.clone();
    missing_layer["layers"]
        .as_object_mut()
        .expect("layers object")
        .remove("tests");
    write_json(&ticket_path, &missing_layer);
    check_failure(&tree, &plan_id, "missing derived layer tests");

    write_json(&ticket_path, &original_ticket);
    let mut missing_trace = original_rtm.clone();
    missing_trace["traces"] = json!([]);
    write_json(&plan_path.join("rtm.json"), &missing_trace);
    check_failure(
        &tree,
        &plan_id,
        "AC-001 has no evidence-producing RTM trace",
    );

    let mut dangling = original_rtm.clone();
    dangling["traces"][0]["subtaskId"] = Value::String("TASK-999-TESTS-999".to_string());
    write_json(&plan_path.join("rtm.json"), &dangling);
    check_failure(&tree, &plan_id, "references unknown subtask");

    let mut dangling_aggregate = original_tasks.clone();
    dangling_aggregate["tasks"][0]["acceptanceCriterionIds"] = json!(["AC-001", "AC-999"]);
    write_json(&plan_path.join("tasks.json"), &dangling_aggregate);
    write_json(&plan_path.join("rtm.json"), &original_rtm);
    check_failure(
        &tree,
        &plan_id,
        "acceptanceCriterionIds do not match its ticket",
    );

    write_json(&plan_path.join("tasks.json"), &original_tasks);
    let mut dangling_entry = original_rtm;
    dangling_entry["entries"][0]["acceptanceCriterionIds"] = json!(["AC-001", "AC-999"]);
    write_json(&plan_path.join("rtm.json"), &dangling_entry);
    check_failure(
        &tree,
        &plan_id,
        "acceptanceCriterionIds do not match TASK-001",
    );
}

#[test]
fn evidence_content_fingerprint_detects_tampering() {
    let tree = test_tree("evidence-tamper");
    let (plan_id, plan_path) = advance_to_tasks(&tree);
    let ticket_path = plan_path.join("task-001.json");
    let mut ticket = read_json(&ticket_path);
    let subtask_id = ticket["layers"]["tests"][0]["id"]
        .as_str()
        .expect("subtask id")
        .to_string();
    let recorded_at = "2026-09-09T00:00:00Z";
    let evidence = json!({
        "schema_version": 1,
        "artifact": "task_evidence",
        "plan_id": plan_id,
        "task_id": "TASK-001",
        "subtask_id": subtask_id,
        "evidence_type": "named_test",
        "recorded_at": recorded_at,
        "test_name": "task ticket contract",
        "result": "pass",
        "output_hash": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    });
    let evidence_body = format!(
        "{}\n",
        serde_json::to_string_pretty(&evidence).expect("render evidence")
    );
    fs::create_dir_all(plan_path.join("evidence")).expect("create evidence directory");
    fs::write(plan_path.join("evidence/tests.json"), &evidence_body)
        .expect("write evidence artifact");
    ticket["layers"]["tests"][0]["status"] = Value::String("done".to_string());
    ticket["layers"]["tests"][0]["verification_timestamp"] = Value::String(recorded_at.to_string());
    ticket["layers"]["tests"][0]["evidence_ref"] = json!({
        "path": "evidence/tests.json",
        "content_hash": format!("fnv1a64:{}", fnv1a64_hex(&evidence_body))
    });
    write_json(&ticket_path, &ticket);

    assert_success(
        &plan_command(&tree, &["tasks", "--plan", &plan_id]),
        "refresh RTM from evidence-bound ticket",
    );
    assert_success(
        &plan_command(&tree, &["check", "--rtm", "--plan", &plan_id]),
        "valid evidence-bound plan",
    );

    let mut tampered = evidence;
    tampered["result"] = Value::String("fail".to_string());
    write_json(&plan_path.join("evidence/tests.json"), &tampered);
    check_failure(
        &tree,
        &plan_id,
        "content_hash does not match evidence artifact",
    );
}

#[test]
fn plan_update_records_subtask_evidence_and_regenerates_aggregates() {
    let tree = test_tree("governed-update");
    let (plan_id, plan_path) = advance_to_tasks(&tree);
    let ticket_path = plan_path.join("task-001.json");
    let mut ticket = read_json(&ticket_path);
    let subtask_id = ticket["layers"]["tests"][0]["id"]
        .as_str()
        .expect("tests subtask id")
        .to_string();
    let recorded_at = "2026-09-12T00:00:00Z";
    let evidence = json!({
        "schema_version": 1,
        "artifact": "task_evidence",
        "plan_id": plan_id,
        "task_id": "TASK-001",
        "subtask_id": subtask_id,
        "evidence_type": "named_test",
        "recorded_at": recorded_at,
        "test_name": "governed plan update",
        "result": "pass",
        "output_hash": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    });
    let evidence_body = format!(
        "{}\n",
        serde_json::to_string_pretty(&evidence).expect("render evidence")
    );
    fs::create_dir_all(plan_path.join("evidence")).expect("create evidence directory");
    fs::write(plan_path.join("evidence/tests.json"), &evidence_body)
        .expect("write evidence artifact");

    let update = plan_command(
        &tree,
        &[
            "update",
            "--plan",
            &plan_id,
            "--task",
            "TASK-001",
            "--subtask",
            &subtask_id,
            "--status",
            "done",
            "--evidence-path",
            "evidence/tests.json",
            "--verification-timestamp",
            recorded_at,
        ],
    );
    assert_success(&update, "plan update");

    ticket = read_json(&ticket_path);
    assert_eq!(ticket["layers"]["tests"][0]["status"], "done");
    assert_eq!(
        ticket["layers"]["tests"][0]["verification_timestamp"],
        recorded_at
    );
    assert_eq!(
        ticket["layers"]["tests"][0]["evidence_ref"]["path"],
        "evidence/tests.json"
    );
    assert_eq!(
        read_json(&plan_path.join("tasks.json"))["tasks"][0]["status"],
        "in_progress"
    );
    let rtm = read_json(&plan_path.join("rtm.json"));
    assert!(rtm["entries"][0]["evidenceRefs"]
        .as_array()
        .expect("RTM evidence refs")
        .iter()
        .any(|reference| reference["path"] == "evidence/tests.json"));
    assert_success(
        &plan_command(&tree, &["check", "--rtm", "--plan", &plan_id]),
        "plan check after update",
    );
}

#[test]
fn plan_update_requires_reasons_and_rejects_traversal_without_mutation() {
    let tree = test_tree("governed-update-validation");
    let (plan_id, plan_path) = advance_to_tasks(&tree);
    let ticket_path = plan_path.join("task-001.json");
    let original_ticket = read_json(&ticket_path);
    let original_tasks = read_json(&plan_path.join("tasks.json"));
    let original_rtm = read_json(&plan_path.join("rtm.json"));
    let original_status = read_json(&plan_path.join("status.json"));
    let docs_id = original_ticket["layers"]["docs"][0]["id"]
        .as_str()
        .expect("docs subtask id")
        .to_string();

    let missing_reason = plan_command(
        &tree,
        &[
            "update",
            "--plan",
            &plan_id,
            "--task",
            "TASK-001",
            "--subtask",
            &docs_id,
            "--status",
            "not_applicable",
        ],
    );
    assert!(!missing_reason.status.success());
    assert!(String::from_utf8_lossy(&missing_reason.stderr).contains("requires --reason"));
    assert_eq!(read_json(&ticket_path), original_ticket);
    assert_eq!(read_json(&plan_path.join("tasks.json")), original_tasks);
    assert_eq!(read_json(&plan_path.join("rtm.json")), original_rtm);
    assert_eq!(read_json(&plan_path.join("status.json")), original_status);

    let tests_id = original_ticket["layers"]["tests"][0]["id"]
        .as_str()
        .expect("tests subtask id")
        .to_string();
    let traversal = plan_command(
        &tree,
        &[
            "update",
            "--plan",
            &plan_id,
            "--task",
            "TASK-001",
            "--subtask",
            &tests_id,
            "--status",
            "done",
            "--evidence-path",
            "../outside.json",
            "--verification-timestamp",
            "2026-09-12T00:00:00Z",
        ],
    );
    assert!(!traversal.status.success());
    assert!(String::from_utf8_lossy(&traversal.stderr).contains("unsafe evidence path"));
    assert_eq!(read_json(&ticket_path), original_ticket);
    assert_eq!(read_json(&plan_path.join("tasks.json")), original_tasks);
    assert_eq!(read_json(&plan_path.join("rtm.json")), original_rtm);
    assert_eq!(read_json(&plan_path.join("status.json")), original_status);

    let human = plan_command(
        &tree,
        &[
            "update",
            "--plan",
            &plan_id,
            "--task",
            "TASK-001",
            "--subtask",
            &docs_id,
            "--status",
            "needs_human",
            "--reason",
            "documentation needs a human decision",
        ],
    );
    assert_success(&human, "needs_human update");
    assert_eq!(
        read_json(&ticket_path)["layers"]["docs"][0]["reason"],
        "documentation needs a human decision"
    );
    assert_eq!(
        read_json(&plan_path.join("tasks.json"))["tasks"][0]["status"],
        "blocked"
    );
}

#[test]
fn plan_update_parent_done_requires_terminal_subtasks() {
    let tree = test_tree("governed-parent-update");
    let (plan_id, plan_path) = advance_to_tasks(&tree);
    let parent_done = plan_command(
        &tree,
        &[
            "update", "--plan", &plan_id, "--task", "TASK-001", "--status", "done",
        ],
    );
    assert!(!parent_done.status.success());
    assert!(String::from_utf8_lossy(&parent_done.stderr).contains("subtasks are unfinished"));

    let parent_progress = plan_command(
        &tree,
        &[
            "update",
            "--plan",
            &plan_id,
            "--task",
            "TASK-001",
            "--status",
            "in_progress",
        ],
    );
    assert_success(&parent_progress, "parent in_progress update");
    assert_eq!(
        read_json(&plan_path.join("task-001.json"))["status"],
        "in_progress"
    );
    assert_eq!(
        read_json(&plan_path.join("tasks.json"))["tasks"][0]["status"],
        "in_progress"
    );
}
