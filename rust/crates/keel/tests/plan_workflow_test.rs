use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

struct TempTree {
    root: PathBuf,
    home: PathBuf,
}

impl Drop for TempTree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(
            self.root
                .parent()
                .expect("fixture root has a parent directory"),
        );
    }
}

fn isolated_tree(label: &str) -> TempTree {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let base =
        std::env::temp_dir().join(format!("keel-plan-{label}-{}-{nonce}", std::process::id()));
    let root = base.join("workspace");
    let home = base.join("keel-home");
    fs::create_dir_all(&root).expect("create fixture workspace");
    fs::write(
        root.join("README.md"),
        "# Fixture\n\nThe sample JSON command exits zero, emits schemaVersion 1, and preserves existing commands.\n",
    )
    .expect("write fixture source anchor");
    TempTree { root, home }
}

fn keel_command() -> Command {
    Command::new(env!("CARGO_BIN_EXE_keel"))
}

fn plan_command(tree: &TempTree, arguments: &[&str]) -> Output {
    let mut command = keel_command();
    command.arg("plan").args(arguments).args([
        "--workspace-root",
        tree.root.to_str().expect("UTF-8 fixture root"),
        "--claude-home",
        tree.home.to_str().expect("UTF-8 fixture home"),
        "--json",
    ]);
    command.output().expect("run keel plan command")
}

fn assert_success(output: &Output, step: &str) {
    assert!(
        output.status.success(),
        "{step} failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn json_output(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).expect("parse planner JSON output")
}

fn specify(tree: &TempTree, request: &str) -> (String, PathBuf) {
    let output = plan_command(tree, &["specify", "--request", request]);
    assert_success(&output, "plan specify");
    let payload = json_output(&output);
    assert_eq!(payload["schemaVersion"], 1);
    assert_eq!(payload["stage"], "specified");
    let plan_id = payload["planId"]
        .as_str()
        .expect("planId string")
        .to_string();
    let plan_path = PathBuf::from(payload["planPath"].as_str().expect("planPath string"));
    assert!(plan_path.starts_with(tree.home.join("memories").join("workspaces")));
    assert_eq!(
        plan_path.file_name().and_then(|name| name.to_str()),
        Some(plan_id.as_str())
    );
    (plan_id, plan_path)
}

fn advance_to_tasks(tree: &TempTree, request: &str) -> (String, PathBuf) {
    let (plan_id, plan_path) = specify(tree, request);
    let research = plan_command(tree, &["research", "--plan", &plan_id]);
    assert_success(&research, "plan research");
    assert_eq!(json_output(&research)["stage"], "researched");
    let tasks = plan_command(tree, &["tasks", "--plan", &plan_id]);
    assert_success(&tasks, "plan tasks");
    assert_eq!(json_output(&tasks)["stage"], "tasked");
    (plan_id, plan_path)
}

fn check_failure(tree: &TempTree, plan_id: &str, expected: &str) {
    let output = plan_command(tree, &["check", "--plan", plan_id]);
    assert!(!output.status.success(), "mutated plan unexpectedly passed");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(expected),
        "planner failure did not contain {expected:?}: {stderr}"
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
    .expect("write mutated JSON artifact");
}

const SPECIFIC_REQUEST: &str =
    "Add a sample JSON command that exits zero, emits schemaVersion 1, and preserves existing commands.";

#[test]
fn plan_round_trip_writes_versioned_grounded_traceable_artifacts() {
    let tree = isolated_tree("round-trip");
    let (plan_id, plan_path) = advance_to_tasks(&tree, SPECIFIC_REQUEST);

    for artifact in [
        "spec.md",
        "research.json",
        "architecture.md",
        "tasks.json",
        "rtm.json",
        "status.json",
    ] {
        assert!(plan_path.join(artifact).is_file(), "missing {artifact}");
    }

    let spec = fs::read_to_string(plan_path.join("spec.md")).expect("read specification");
    assert!(spec.contains("schema_version: 1"));
    assert!(spec.contains(SPECIFIC_REQUEST));
    for section in [
        "User request verbatim",
        "Restated user outcome",
        "Problem statement",
        "Scope",
        "Non-goals",
        "Constraints",
        "Stakeholders/users affected",
        "Current behavior",
        "Desired behavior",
        "Functional requirements",
        "Non-functional requirements",
        "Acceptance criteria",
        "Edge cases",
        "Failure behavior",
        "Security/privacy concerns",
        "Performance and token constraints",
        "Compatibility/host constraints",
        "Rollout and rollback requirements",
        "Assumptions",
        "Ambiguities and clarification decisions",
        "Research references",
        "Verification strategy",
    ] {
        assert!(spec.contains(section), "specification missing {section}");
    }
    for required in [
        "ID: AC-001",
        "Requirement references: REQ-001",
        "Precondition:",
        "Action:",
        "Expected observable outcome:",
        "Negative/failure outcome:",
        "Verification method:",
        "Expected evidence type:",
        "Owner role: verifier",
    ] {
        assert!(
            spec.contains(required),
            "acceptance criterion missing {required}"
        );
    }

    let architecture =
        fs::read_to_string(plan_path.join("architecture.md")).expect("read architecture");
    assert!(architecture.contains("schema_version: 1"));
    assert!(architecture.contains("Claim references: CLM-001"));
    for artifact in ["research.json", "tasks.json", "rtm.json", "status.json"] {
        assert_eq!(
            read_json(&plan_path.join(artifact))["schemaVersion"],
            1,
            "{artifact} schema version"
        );
    }
    assert_eq!(
        read_json(&plan_path.join("research.json"))["claims"][0]["classification"],
        "verified"
    );

    let check = plan_command(&tree, &["check", "--plan", &plan_id]);
    assert_success(&check, "plan check");
    let check_payload = json_output(&check);
    assert_eq!(check_payload["status"], "valid");
    assert_eq!(check_payload["requirements"], 1);
    assert_eq!(check_payload["acceptanceCriteria"], 1);

    let rtm = plan_command(&tree, &["check", "--rtm", "--plan", &plan_id]);
    assert_success(&rtm, "plan check --rtm");
    assert_eq!(
        json_output(&rtm)["rtm"]["entries"][0]["requirementId"],
        "REQ-001"
    );
    assert_eq!(read_json(&plan_path.join("status.json"))["stage"], "valid");
}

#[test]
fn plan_check_rejects_missing_acceptance_mapping_and_evidence_method() {
    let tree = isolated_tree("negative-spec");

    let (missing_ac_id, missing_ac_path) = advance_to_tasks(&tree, SPECIFIC_REQUEST);
    let spec_path = missing_ac_path.join("spec.md");
    let mut spec = fs::read_to_string(&spec_path).expect("read spec");
    let ac_start = spec.find("### AC-001").expect("AC start");
    let ac_end = spec[ac_start..]
        .find("\n## 13.")
        .map(|offset| ac_start + offset)
        .expect("AC end");
    spec.replace_range(ac_start..ac_end, "");
    fs::write(&spec_path, spec).expect("remove AC");
    check_failure(&tree, &missing_ac_id, "REQ-001 has no acceptance criterion");

    let (mapping_id, mapping_path) = advance_to_tasks(&tree, SPECIFIC_REQUEST);
    let mapping_spec_path = mapping_path.join("spec.md");
    let mapping_spec = fs::read_to_string(&mapping_spec_path)
        .expect("read mapping spec")
        .replace("Requirement references: REQ-001", "Requirement references:");
    fs::write(&mapping_spec_path, mapping_spec).expect("remove REQ mapping");
    check_failure(&tree, &mapping_id, "AC-001 has no requirement references");

    let (evidence_id, evidence_path) = advance_to_tasks(&tree, SPECIFIC_REQUEST);
    let evidence_spec_path = evidence_path.join("spec.md");
    let evidence_spec = fs::read_to_string(&evidence_spec_path)
        .expect("read evidence spec")
        .replace(
            "Verification method: Run the generated task verification command and inspect its exit status.",
            "Verification method:",
        );
    fs::write(&evidence_spec_path, evidence_spec).expect("remove evidence method");
    check_failure(&tree, &evidence_id, "AC-001 has no verification method");
}

#[test]
fn plan_check_rejects_unclassified_claims_and_unsupported_schemas() {
    let tree = isolated_tree("negative-json");

    let (claim_id, claim_path) = advance_to_tasks(&tree, SPECIFIC_REQUEST);
    let research_path = claim_path.join("research.json");
    let mut research = read_json(&research_path);
    research["claims"][0]["classification"] = Value::String(String::new());
    write_json(&research_path, &research);
    check_failure(&tree, &claim_id, "CLM-001 is unclassified");

    let (architecture_id, architecture_path) = advance_to_tasks(&tree, SPECIFIC_REQUEST);
    let note_path = architecture_path.join("architecture.md");
    let note = fs::read_to_string(&note_path)
        .expect("read architecture note")
        .replace("[verified: CLM-001]", "[CLM-001]");
    fs::write(&note_path, note).expect("remove architecture classification");
    check_failure(
        &tree,
        &architecture_id,
        "architecture.md claim CLM-001 is unclassified",
    );

    let (schema_id, schema_path) = advance_to_tasks(&tree, SPECIFIC_REQUEST);
    let tasks_path = schema_path.join("tasks.json");
    let mut tasks = read_json(&tasks_path);
    tasks["schemaVersion"] = Value::from(99);
    write_json(&tasks_path, &tasks);
    check_failure(&tree, &schema_id, "tasks.json schemaVersion=99");
}

#[test]
fn vague_material_request_records_interpretations_and_blocks_check() {
    let tree = isolated_tree("vague");
    let (plan_id, plan_path) = specify(&tree, "Make it fast and secure.");
    let spec = fs::read_to_string(plan_path.join("spec.md")).expect("read vague spec");
    assert!(spec.contains("Plausible interpretations:"));
    assert!(spec.contains("Clarification question:"));
    assert!(spec.contains("Decision: unresolved_material_ambiguity"));
    let status = read_json(&plan_path.join("status.json"));
    assert_eq!(status["clarificationRequired"], true);
    check_failure(&tree, &plan_id, "unresolved material ambiguity");
}

#[test]
fn research_without_relevant_local_evidence_blocks_task_progression() {
    let tree = isolated_tree("research-gap");
    fs::write(
        tree.root.join("README.md"),
        "# Oranges\n\nCitrus inventory only.\n",
    )
    .expect("replace fixture source");
    let (plan_id, plan_path) = specify(
        &tree,
        "Add a quantum frobnicator with phase-conjugation telemetry.",
    );
    let output = plan_command(&tree, &["research", "--plan", &plan_id]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr)
        .contains("no indexed local evidence matched the request"));
    assert_eq!(
        read_json(&plan_path.join("research.json"))["status"],
        "insufficient"
    );
    assert_eq!(
        read_json(&plan_path.join("status.json"))["researchStatus"],
        "insufficient"
    );
    assert_eq!(
        read_json(&plan_path.join("status.json"))["stage"],
        "specified"
    );
    let tasks = plan_command(&tree, &["tasks", "--plan", &plan_id]);
    assert!(!tasks.status.success());
}

#[test]
fn plan_ids_cannot_escape_the_workspace_plan_lane() {
    let tree = isolated_tree("path-safety");
    let output = plan_command(&tree, &["check", "--plan", "../escape"]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("single safe path segment"),
        "unexpected traversal diagnostic: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
