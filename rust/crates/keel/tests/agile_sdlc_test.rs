use serde_json::Value;
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
        "---\nschema_version: 1\nartifact: architecture\nplan_id: {plan_id}\n---\n\nStatus: complete\n\n# Architecture Note\n\n## 1. Current architecture relevant to scope\n\n[verified: CLM-001] The current owner path was read.\n\n## 2. Proposed architecture\n\n[derived: CLM-002] Implement the specified outcome through the existing owner.\n\nInput bound: One bounded plan artifact.\n\nPolicy owner: The existing planner remains the lifecycle owner.\n\n## 3. Components/files/interfaces changed\n\n- Component: existing planner owner | Requirements: REQ-001 | Acceptance: AC-001\n\n## 4. Data/control flow\n\nThe existing command receives input, the owner applies it, and verification observes the result.\n\n## 5. Alternatives considered\n\nAlternative: Add a second planning owner.\n\nTradeoff: A second owner would duplicate lifecycle policy.\n\n## 6. Why the chosen option fits requirements\n\nChosen option: Extend the existing owner.\n\nInfrastructure reuse: Reuse the existing planner artifacts and status output.\n\nConstraint fit: The change stays within REQ-001 and AC-001.\n\n## 7. Risks and mitigations\n\nRisk: The implementation could drift outside the request.\n\nMitigation: Validate the requirement and acceptance mappings before tasks.\n\n## 8. Backward compatibility\n\nCompatibility: Existing behavior and artifact fields remain available.\n\nHost impact: none; existing host contracts remain unchanged.\n\n## 9. Error handling and fallback semantics\n\nFailure status: Invalid planning evidence exits non-zero.\n\nFallback: none; repair the canonical artifact.\n\nVisibility: operator and reviewer output lists the failure.\n\n## 10. Security/privacy implications\n\nSecurity/privacy: Do not record credentials or unrelated user data.\n\n## 11. Performance/token impact\n\nToken impact: Planning input remains bounded.\n\nMeasurement plan: Run the planner and fixed-context budget tests.\n\n## 12. Test strategy\n\nVerification: Run the generated acceptance command and planner integration tests.\n\nAcceptance references: AC-001\n\n## 13. Rollback strategy\n\nRollback: Revert the implementation commit and retain plan evidence.\n\n## 14. Requirement and research references\n\nRequirement references: REQ-001\n\nAcceptance references: AC-001\n\nClaim references: CLM-001, CLM-002\n"
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

fn mark_tasks_complete(plan_path: &std::path::Path) {
    let tasks_path = plan_path.join("tasks.json");
    let mut tasks_val: Value =
        serde_json::from_str(&fs::read_to_string(&tasks_path).unwrap()).unwrap();
    if let Some(tasks_arr) = tasks_val.get_mut("tasks").and_then(Value::as_array_mut) {
        for t in tasks_arr {
            t.as_object_mut()
                .unwrap()
                .insert("status".to_string(), Value::String("complete".to_string()));
            if let Some(ticket_file) = t.get("ticketFile").and_then(Value::as_str) {
                let ticket_path = plan_path.join(ticket_file);
                if let Ok(ticket_str) = fs::read_to_string(&ticket_path) {
                    let mut ticket_val: Value = serde_json::from_str(&ticket_str).unwrap();
                    ticket_val
                        .as_object_mut()
                        .unwrap()
                        .insert("status".to_string(), Value::String("complete".to_string()));
                    if let Some(layers) =
                        ticket_val.get_mut("layers").and_then(Value::as_object_mut)
                    {
                        for (_layer, subtasks) in layers {
                            if let Some(sub_arr) = subtasks.as_array_mut() {
                                for st in sub_arr {
                                    st.as_object_mut().unwrap().insert(
                                        "status".to_string(),
                                        Value::String("complete".to_string()),
                                    );
                                }
                            }
                        }
                    }
                    fs::write(&ticket_path, serde_json::to_string(&ticket_val).unwrap()).unwrap();
                }
            }
        }
    }
    fs::write(&tasks_path, serde_json::to_string(&tasks_val).unwrap()).unwrap();
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

    // Record evidence fixture
    let evidence_rel = "evidence_test.json";
    let evidence_content = serde_json::json!({
        "output_hash": "fnv-12345678abcdef",
        "result": "pass"
    });
    fs::write(
        plan_path.join(evidence_rel),
        serde_json::to_string(&evidence_content).unwrap(),
    )
    .expect("write evidence fixture");

    // Update RTM with evidence reference
    let rtm_path = plan_path.join("rtm.json");
    let mut rtm_val: Value = serde_json::from_str(&fs::read_to_string(&rtm_path).unwrap()).unwrap();
    if let Some(traces) = rtm_val.get_mut("traces").and_then(Value::as_array_mut) {
        for trace in traces {
            trace.as_object_mut().unwrap().insert(
                "evidenceRef".to_string(),
                serde_json::json!({
                    "path": evidence_rel,
                    "kind": "fixture",
                    "hash": "fnv-12345678abcdef"
                }),
            );
        }
    }
    fs::write(&rtm_path, serde_json::to_string(&rtm_val).unwrap()).unwrap();

    mark_tasks_complete(&plan_path);

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
    let evidence_rel = "evidence_test.json";
    let evidence_content = serde_json::json!({
        "output_hash": "fnv-gate-pass-123",
        "result": "pass"
    });
    fs::write(
        plan_path.join(evidence_rel),
        serde_json::to_string(&evidence_content).unwrap(),
    )
    .expect("write evidence fixture");

    let rtm_path = plan_path.join("rtm.json");
    let mut rtm_val: Value = serde_json::from_str(&fs::read_to_string(&rtm_path).unwrap()).unwrap();
    if let Some(traces) = rtm_val.get_mut("traces").and_then(Value::as_array_mut) {
        for trace in traces {
            trace.as_object_mut().unwrap().insert(
                "evidenceRef".to_string(),
                serde_json::json!({
                    "path": evidence_rel,
                    "kind": "fixture",
                    "hash": "fnv-gate-pass-123"
                }),
            );
        }
    }
    fs::write(&rtm_path, serde_json::to_string(&rtm_val).unwrap()).unwrap();

    mark_tasks_complete(&plan_path);

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
        "Verified all contracts with passing evidence",
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
