use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

struct TempTree {
    root: PathBuf,
    home: PathBuf,
}

impl Drop for TempTree {
    fn drop(&mut self) {
        if let Some(parent) = self.root.parent() {
            let _ = fs::remove_dir_all(parent);
        }
    }
}

fn isolated_tree(label: &str) -> TempTree {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let base =
        std::env::temp_dir().join(format!("keel-sdlc-{label}-{}-{nonce}", std::process::id()));
    let root = base.join("workspace");
    let home = base.join("keel-home");
    fs::create_dir_all(&root).expect("create workspace");
    fs::create_dir_all(&home).expect("create home");
    fs::create_dir_all(root.join("src")).expect("create src");
    fs::write(root.join("src/lib.rs"), "pub fn sample() -> u8 { 1 }\n").expect("write lib.rs");
    fs::write(
        root.join("README.md"),
        "# Fixture\n\nThe sample JSON command exits zero, emits schemaVersion 1, and preserves existing commands.\n",
    )
    .expect("write README");
    TempTree { root, home }
}

fn keel_command() -> Command {
    Command::new(env!("CARGO_BIN_EXE_keel"))
}

fn plan_command(tree: &TempTree, arguments: &[&str]) -> Output {
    let mut command = keel_command();
    command.arg("plan").args(arguments).args([
        "--workspace-root",
        tree.root.to_str().expect("UTF-8 root"),
        "--claude-home",
        tree.home.to_str().expect("UTF-8 home"),
        "--json",
    ]);
    command.output().expect("run keel plan command")
}

fn json_output(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).expect("parse JSON output")
}

fn complete_architecture(plan_id: &str) -> String {
    format!(
        "---\nschema_version: 1\nartifact: architecture\nplan_id: {plan_id}\n---\n\nStatus: complete\n\n# Architecture Note\n\n## 1. Current architecture relevant to scope\n\n[verified: CLM-001] The current owner path was read.\n\n## 2. Proposed architecture\n\n[derived: CLM-002] Implement the specified outcome through the existing owner.\n\nInput bound: One bounded plan artifact.\n\nPolicy owner: The existing planner remains the lifecycle owner.\n\n## 3. Components/files/interfaces changed\n\n- Component: src/lib.rs command owner | Requirements: REQ-001 | Acceptance: AC-001\n\n## 4. Data/control flow\n\nThe existing command receives input, the owner applies it, and verification observes the result.\n\n## 5. Alternatives considered\n\nAlternative: Add a second owner.\n\nTradeoff: A second owner duplicates existing policy.\n\n## 6. Why the chosen option fits requirements\n\nChosen option: Extend the established owner.\n\nInfrastructure reuse: Reuse the planner and review gate infrastructure.\n\nConstraint fit: The design maps only REQ-001 and AC-001.\n\n## 7. Risks and mitigations\n\nRisk: A stale design could reach review.\n\nMitigation: Pre-PR review validates the named plan architecture.\n\n## 8. Backward compatibility\n\nCompatibility: Existing fields and behavior remain available.\n\nHost impact: none; host contracts remain unchanged.\n\n## 9. Error handling and fallback semantics\n\nFailure status: Invalid architecture blocks pre-PR review.\n\nFallback: none; repair the canonical architecture note.\n\nVisibility: reviewer output lists the design defect.\n\n## 10. Security/privacy implications\n\nSecurity/privacy: The gate reads one local artifact and no credentials.\n\n## 11. Performance/token impact\n\nToken impact: Architecture input stays bounded.\n\nMeasurement plan: Run the fixed-context budget test.\n\n## 12. Test strategy\n\nVerification: Run review unit tests and planner integration tests.\n\nAcceptance references: AC-001\n\n## 13. Rollback strategy\n\nRollback: Revert the implementation commit.\n\n## 14. Requirement and research references\n\nRequirement references: REQ-001\n\nAcceptance references: AC-001\n\nClaim references: CLM-001, CLM-002\n"
    )
}

const SPECIFIC_REQUEST: &str =
    "Add a sample JSON command that exits zero, emits schemaVersion 1, and preserves existing commands.";

fn setup_ready_plan(tree: &TempTree, request: &str) -> (String, PathBuf) {
    let spec_out = plan_command(tree, &["specify", "--request", request]);
    assert!(spec_out.status.success(), "specify failed");
    let payload = json_output(&spec_out);
    let plan_id = payload["planId"].as_str().unwrap().to_string();
    let plan_path = PathBuf::from(payload["planPath"].as_str().unwrap());

    let res_out = plan_command(tree, &["research", "--plan", &plan_id]);
    assert!(
        res_out.status.success(),
        "research failed: {}",
        String::from_utf8_lossy(&res_out.stderr)
    );

    fs::write(
        plan_path.join("architecture.md"),
        complete_architecture(&plan_id),
    )
    .expect("write architecture note");

    let des_out = plan_command(tree, &["design", "--plan", &plan_id]);
    assert!(
        des_out.status.success(),
        "design failed: {}",
        String::from_utf8_lossy(&des_out.stderr)
    );

    let task_out = plan_command(tree, &["tasks", "--plan", &plan_id]);
    assert!(
        task_out.status.success(),
        "tasks failed: {}",
        String::from_utf8_lossy(&task_out.stderr)
    );

    let chk_out = plan_command(tree, &["check", "--plan", &plan_id]);
    assert!(
        chk_out.status.success(),
        "check failed: {}",
        String::from_utf8_lossy(&chk_out.stderr)
    );

    (plan_id, plan_path)
}

fn check_ready(tree: &TempTree, plan_id: &str) -> std::process::Output {
    plan_command(tree, &["ready", "--plan", plan_id])
}

fn populate_valid_evidence(tree: &TempTree, plan_path: &std::path::Path, plan_id: &str) {
    let sha256_hex = |bytes: &[u8]| -> String {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        format!("{:x}", hasher.finalize())
    };
    let source = fs::read(tree.root.join("src/lib.rs")).expect("read fixture source");
    let output = b"sample JSON command: pass\n";
    fs::write(tree.root.join("command-output.txt"), output).expect("write output");
    fs::create_dir_all(plan_path.join("evidence")).expect("create evidence directory");
    let ticket_path = plan_path.join("task-001.json");
    let mut ticket: Value =
        serde_json::from_str(&fs::read_to_string(&ticket_path).unwrap()).unwrap();
    let recorded_at = chrono::Utc::now().to_rfc3339();
    for subtasks in ticket["layers"]
        .as_object_mut()
        .expect("layers")
        .values_mut()
    {
        for subtask in subtasks.as_array_mut().expect("subtasks") {
            let evidence_type = subtask["expected_evidence_type"].as_str().expect("type");
            let id = subtask["id"].as_str().expect("id").to_string();
            let path = format!("evidence/{id}.json");
            let mut evidence = serde_json::json!({
                "schema_version": 1, "artifact": "task_evidence",
                "plan_id": plan_id, "task_id": "TASK-001", "subtask_id": id,
                "evidence_type": evidence_type, "recorded_at": recorded_at,
                "result": "pass",
                "source_path": "src/lib.rs",
                "source_hash": format!("sha256:{}", sha256_hex(&source)),
                "output_path": "command-output.txt",
                "output_hash": format!("sha256:{}", sha256_hex(output))
            });
            match evidence_type {
                "command" => {
                    evidence["command"] = serde_json::json!("sample JSON command");
                    evidence["exit_code"] = serde_json::json!(0);
                }
                "named_test" => {
                    evidence["test_name"] = serde_json::json!("sample returns one");
                }
                "lint_diagnostic" => {
                    evidence["tool"] = serde_json::json!("fixture linter");
                    evidence["exit_code"] = serde_json::json!(0);
                }
                "source_hash" => {}
                unexpected => panic!("unexpected generated evidence type: {unexpected}"),
            }
            let body = serde_json::to_vec(&evidence).expect("serialize evidence");
            fs::write(plan_path.join(&path), &body).expect("write evidence");
            subtask["status"] = serde_json::json!("done");
            subtask["verification_timestamp"] = serde_json::json!(recorded_at);
            subtask["evidence_ref"] = serde_json::json!({
                "path": path,
                "content_hash": format!("sha256:{}", sha256_hex(&body))
            });
        }
    }
    ticket["status"] = serde_json::json!("done");
    fs::write(&ticket_path, serde_json::to_string_pretty(&ticket).unwrap()).unwrap();
    let tasks_out = plan_command(tree, &["tasks", "--plan", plan_id]);
    assert!(
        tasks_out.status.success(),
        "tasks refresh failed: {}",
        String::from_utf8_lossy(&tasks_out.stderr)
    );
}

#[test]
fn plan_ready_satisfies_all_eleven_items() {
    let tree = isolated_tree("ready-pass");
    let (plan_id, _) = setup_ready_plan(&tree, SPECIFIC_REQUEST);

    let ready_out = check_ready(&tree, &plan_id);
    assert!(
        ready_out.status.success(),
        "plan ready must succeed: stderr={}",
        String::from_utf8_lossy(&ready_out.stderr)
    );
    let ready_val = json_output(&ready_out);
    assert_eq!(ready_val["satisfied"], true);
    assert_eq!(ready_val["passedCount"], 11);
    assert_eq!(ready_val["totalCount"], 11);
    assert_eq!(ready_val["stage"], "ready");

    let items = ready_val["items"].as_array().expect("items array");
    assert_eq!(items.len(), 11);
    for item in items {
        assert_eq!(item["status"], "pass");
    }
}

#[test]
fn plan_ready_fails_when_prerequisite_missing() {
    let tree = isolated_tree("ready-fail");
    let spec_out = plan_command(&tree, &["specify", "--request", SPECIFIC_REQUEST]);
    assert!(spec_out.status.success());
    let payload = json_output(&spec_out);
    let plan_id = payload["planId"].as_str().unwrap().to_string();

    let unready_out = check_ready(&tree, &plan_id);
    assert!(
        !unready_out.status.success(),
        "plan ready must fail on incomplete plan"
    );
    let ready_val = json_output(&unready_out);
    assert_eq!(ready_val["satisfied"], false);
    assert!(ready_val["passedCount"].as_u64().unwrap() < 11);
}

#[test]
fn plan_done_lifecycle_enforcement_and_evidence_gate() {
    let tree = isolated_tree("done-lifecycle");
    let (plan_id, plan_path) = setup_ready_plan(&tree, SPECIFIC_REQUEST);

    let setup_ready = check_ready(&tree, &plan_id);
    assert!(setup_ready.status.success());

    // Before implementation and evidence, plan done must fail
    let done_out_fail = plan_command(&tree, &["done", "--plan", &plan_id]);
    assert!(
        !done_out_fail.status.success(),
        "plan done must fail when evidence is missing"
    );
    let done_val_fail = json_output(&done_out_fail);
    assert_eq!(done_val_fail["satisfied"], false);
    assert_eq!(done_val_fail["items"][0]["status"], "fail");

    populate_valid_evidence(&tree, &plan_path, &plan_id);
    // Now plan done must succeed with all 13 items passed
    let done_out_pass = plan_command(&tree, &["done", "--plan", &plan_id]);
    assert!(
        done_out_pass.status.success(),
        "plan done must pass when all evidence and tasks complete: stderr={}",
        String::from_utf8_lossy(&done_out_pass.stderr)
    );
    let done_val_pass = json_output(&done_out_pass);
    assert_eq!(done_val_pass["satisfied"], true);
    assert_eq!(done_val_pass["passedCount"], 13);
    assert_eq!(done_val_pass["totalCount"], 13);
    assert_eq!(done_val_pass["stage"], "done");
}

#[test]
fn completion_gate_check_with_plan_flag() {
    let tree = isolated_tree("gate-plan");
    let (plan_id, plan_path) = setup_ready_plan(&tree, SPECIFIC_REQUEST);
    let home_str = tree.home.to_str().unwrap();

    // Write a working brief for completion-gate check
    let mut brief_cmd = keel_command();
    brief_cmd.current_dir(&tree.root);
    brief_cmd.args([
        "memory",
        "working-brief",
        "write",
        "--id",
        "brief-gate-test",
        "--request",
        SPECIFIC_REQUEST,
        "--acceptance-criteria",
        "AC-001 is satisfied",
        "--claude-home",
        home_str,
        "--json",
    ]);
    let brief_out = brief_cmd.output().expect("write brief");
    assert!(brief_out.status.success());

    // Completion gate check with --plan should fail because DoD is not satisfied yet
    let mut gate_cmd_fail = keel_command();
    gate_cmd_fail.current_dir(&tree.root);
    gate_cmd_fail.args([
        "memory",
        "completion-gate",
        "check",
        "--brief-id",
        "brief-gate-test",
        "--plan",
        &plan_id,
        "--proof",
        "Verified all contracts",
        "--claude-home",
        home_str,
        "--json",
    ]);
    let gate_out_fail = gate_cmd_fail.output().expect("check completion gate");
    assert!(
        !gate_out_fail.status.success(),
        "completion gate must fail when DoD fails"
    );
    let gate_val_fail = json_output(&gate_out_fail);
    assert_eq!(gate_val_fail["closureReady"], false);

    // Provide evidence and complete tasks
    populate_valid_evidence(&tree, &plan_path, &plan_id);
    // Now completion gate check with --plan must pass!
    let mut gate_cmd_pass = keel_command();
    gate_cmd_pass.current_dir(&tree.root);
    gate_cmd_pass.args([
        "memory",
        "completion-gate",
        "check",
        "--brief-id",
        "brief-gate-test",
        "--plan",
        &plan_id,
        "--proof",
        "requirement-c9e28ef361148869=AC-001",
        "--claude-home",
        home_str,
        "--json",
    ]);
    let gate_out_pass = gate_cmd_pass.output().expect("check completion gate pass");
    assert!(
        gate_out_pass.status.success(),
        "completion gate must pass: stdout={}\nstderr={}",
        String::from_utf8_lossy(&gate_out_pass.stdout),
        String::from_utf8_lossy(&gate_out_pass.stderr)
    );
    let gate_val_pass = json_output(&gate_out_pass);
    assert_eq!(gate_val_pass["closureReady"], true);
    assert_eq!(gate_val_pass["ok"], true);
}
