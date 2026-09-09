use serde_json::Value;
use std::fs;
use std::path::PathBuf;
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

struct PlanFixture {
    id: String,
    path: PathBuf,
}

const COMPONENTS_HEADER: &str = "## 3. Components/files/interfaces changed";
const DUPLICATE_POLICY_COMPONENTS: &str =
    "Policy owner: A second owner.\n\n## 3. Components/files/interfaces changed";

fn isolated_tree(label: &str) -> TestTree {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let base = std::env::temp_dir().join(format!(
        "keel-architecture-{label}-{}-{nonce}",
        std::process::id()
    ));
    let root = base.join("workspace");
    let home = base.join("keel-home");
    fs::create_dir_all(root.join("src")).expect("create fixture workspace");
    fs::write(
        root.join("src/lib.rs"),
        "pub fn architecture_owner() -> &'static str { \"planner\" }\n",
    )
    .expect("write architecture source anchor");
    TestTree { root, home }
}

fn plan_command(tree: &TestTree, arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_keel"))
        .arg("plan")
        .args(arguments)
        .args([
            "--workspace-root",
            tree.root.to_str().expect("UTF-8 fixture root"),
            "--claude-home",
            tree.home.to_str().expect("UTF-8 fixture home"),
            "--json",
        ])
        .output()
        .expect("run planner command")
}

fn json_output(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).expect("parse planner JSON")
}

fn assert_success(output: &Output, step: &str) {
    assert!(
        output.status.success(),
        "{step} failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn specified_researched_plan(tree: &TestTree) -> PlanFixture {
    let request = "Extend the planner architecture gate with explicit traceability.";
    let specify = plan_command(tree, &["specify", "--request", request]);
    assert_success(&specify, "plan specify");
    let payload = json_output(&specify);
    let plan = PlanFixture {
        id: payload["planId"].as_str().expect("plan id").to_string(),
        path: PathBuf::from(payload["planPath"].as_str().expect("plan path")),
    };
    let research = plan_command(tree, &["research", "--plan", &plan.id]);
    assert_success(&research, "plan research");
    plan
}

fn valid_architecture(plan_id: &str) -> String {
    format!(
        "---\n\
schema_version: 1\n\
artifact: architecture\n\
plan_id: {plan_id}\n\
---\n\
\n\
Status: complete\n\
\n\
# Architecture Note\n\
\n\
## 1. Current architecture relevant to scope\n\
\n\
[verified: CLM-001] The existing planner owns architecture.md and task progression.\n\
\n\
## 2. Proposed architecture\n\
\n\
[derived: CLM-002] Add a design-validation stage inside the existing planner owner.\n\
\n\
Input bound: One bounded architecture.md artifact in the existing plan directory.\n\
\n\
Policy owner: utility::plan remains the only architecture lifecycle owner.\n\
\n\
## 3. Components/files/interfaces changed\n\
\n\
- Component: utility::plan design stage | Requirements: REQ-001 | Acceptance: AC-001\n\
\n\
## 4. Data/control flow\n\
\n\
The operator edits the scaffold, plan design validates it, and plan tasks consumes the status.\n\
\n\
## 5. Alternatives considered\n\
\n\
Alternative: Store a second design JSON artifact beside architecture.md.\n\
\n\
Tradeoff: Structured JSON is easier to parse but duplicates the existing design source of truth.\n\
\n\
## 6. Why the chosen option fits requirements\n\
\n\
Chosen option: Validate the existing architecture.md artifact before task compilation.\n\
\n\
Infrastructure reuse: Reuse PlanPaths, status.json, and existing plan validation.\n\
\n\
Constraint fit: One owner and bounded local artifacts preserve the request boundary.\n\
\n\
## 7. Risks and mitigations\n\
\n\
Risk: A stale design could authorize tasks after research changes.\n\
\n\
Mitigation: Research refresh resets design and task status to pending.\n\
\n\
## 8. Backward compatibility\n\
\n\
Compatibility: Existing plan artifacts stay in place and gain additive status.\n\
\n\
Host impact: none; host-neutral CLI behavior remains unchanged.\n\
\n\
## 9. Error handling and fallback semantics\n\
\n\
Failure status: Invalid or incomplete design returns non-zero before tasks are written.\n\
\n\
Fallback: none; the operator must repair the canonical architecture note.\n\
\n\
Visibility: operator and reviewer output lists every design defect.\n\
\n\
## 10. Security/privacy implications\n\
\n\
Security/privacy: The validator reads one local plan file and records no credentials.\n\
\n\
## 11. Performance/token impact\n\
\n\
Token impact: Only operator help changes fixed text; architecture content stays plan-scoped.\n\
\n\
Measurement plan: Run fixed_context_budget_test and compare the runtime ledger.\n\
\n\
## 12. Test strategy\n\
\n\
Verification: Run architecture_design_test, planner tests, review tests, and the workspace suite.\n\
\n\
Acceptance references: AC-001\n\
\n\
## 13. Rollback strategy\n\
\n\
Rollback: Revert the Phase 4 commit and retain the prior plan evidence.\n\
\n\
## 14. Requirement and research references\n\
\n\
Requirement references: REQ-001\n\
\n\
Acceptance references: AC-001\n\
\n\
Claim references: CLM-001, CLM-002\n"
    )
}

fn write_architecture(plan: &PlanFixture, body: &str) {
    fs::write(plan.path.join("architecture.md"), body).expect("write architecture note");
}

fn design_command(tree: &TestTree, plan: &PlanFixture) -> Output {
    plan_command(tree, &["design", "--plan", &plan.id])
}

fn assert_design_failure(output: &Output, expected: &str) {
    assert!(
        !output.status.success(),
        "invalid design unexpectedly passed"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(expected),
        "design failure did not contain {expected:?}: {stderr}"
    );
}

fn assert_plan_design_failure(tree: &TestTree, plan: &PlanFixture, expected: &str) {
    assert_design_failure(&design_command(tree, plan), expected);
}

#[test]
fn complete_design_is_required_before_tasks() {
    let tree = isolated_tree("required-before-tasks");
    let required_plan = specified_researched_plan(&tree);
    let premature_tasks = plan_command(&tree, &["tasks", "--plan", &required_plan.id]);
    assert_design_failure(&premature_tasks, "architecture.md status is not complete");

    write_architecture(&required_plan, &valid_architecture(&required_plan.id));
    let design = design_command(&tree, &required_plan);
    assert_success(&design, "plan design");
    assert_eq!(json_output(&design)["stage"], "designed");

    let tasks = plan_command(&tree, &["tasks", "--plan", &required_plan.id]);
    assert_success(&tasks, "plan tasks after design");
}

#[test]
fn component_mappings_require_existing_requirement_and_acceptance_ids() {
    let tree = isolated_tree("component-mappings");
    let mapping_plan = specified_researched_plan(&tree);
    let complete = valid_architecture(&mapping_plan.id);

    let missing_acceptance = complete.replace(" | Acceptance: AC-001", " | Acceptance:");
    write_architecture(&mapping_plan, &missing_acceptance);
    assert_plan_design_failure(&tree, &mapping_plan, "has no acceptance mappings");

    let unknown_requirement = complete.replace("Requirements: REQ-001", "Requirements: REQ-999");
    write_architecture(&mapping_plan, &unknown_requirement);
    assert_plan_design_failure(&tree, &mapping_plan, "unknown requirement REQ-999");

    write_architecture(&mapping_plan, &complete);
    assert_success(&design_command(&tree, &mapping_plan), "mapped design");

    let out_of_order = complete
        .replace(
            COMPONENTS_HEADER,
            "## TEMP. Components/files/interfaces changed",
        )
        .replace("## 4. Data/control flow", COMPONENTS_HEADER)
        .replace(
            "## TEMP. Components/files/interfaces changed",
            "## 4. Data/control flow",
        );
    write_architecture(&mapping_plan, &out_of_order);
    assert_plan_design_failure(&tree, &mapping_plan, "not in the required order");
}

#[test]
fn design_requires_tradeoffs_fallback_visibility_measurement_rollback_and_reuse() {
    let tree = isolated_tree("required-decisions");
    let decision_plan = specified_researched_plan(&tree);
    let complete = valid_architecture(&decision_plan.id);
    let cases = [
        ("Tradeoff:", "Tradeoff-missing:", "has no Tradeoff"),
        ("Visibility:", "Visibility-missing:", "has no Visibility"),
        (
            "Visibility: operator and reviewer output lists every design defect.",
            "Visibility: hidden",
            "Visibility must name operator, user, reviewer, or status output",
        ),
        (
            "Measurement plan:",
            "Measurement-plan-missing:",
            "has no Measurement plan",
        ),
        ("Rollback:", "Rollback-missing:", "has no Rollback"),
        (
            "Infrastructure reuse:",
            "Infrastructure-reuse-missing:",
            "has no Infrastructure reuse",
        ),
        (
            "Infrastructure reuse: Reuse PlanPaths, status.json, and existing plan validation.",
            "Infrastructure reuse: none",
            "must name reused Keel infrastructure",
        ),
        ("Input bound:", "Input-bound-missing:", "has no Input bound"),
        (
            "Policy owner:",
            "Policy-owner-missing:",
            "has no Policy owner",
        ),
        (
            "Host impact: none; host-neutral CLI behavior remains unchanged.",
            "Host impact: changes an adapter",
            "must preserve all 11 adapter contracts",
        ),
        (
            COMPONENTS_HEADER,
            DUPLICATE_POLICY_COMPONENTS,
            "more than one Policy owner",
        ),
    ];

    for (target, replacement, expected) in cases {
        write_architecture(&decision_plan, &complete.replacen(target, replacement, 1));
        assert_plan_design_failure(&tree, &decision_plan, expected);
    }
}

#[test]
fn design_rejects_architecture_above_the_bounded_context_limit() {
    let tree = isolated_tree("bounded-context");
    let bounded_plan = specified_researched_plan(&tree);
    write_architecture(&bounded_plan, &"x".repeat(65_537));
    assert_plan_design_failure(&tree, &bounded_plan, "exceeds the 65536-byte input bound");
}

#[test]
fn design_before_research_preserves_the_specified_stage() {
    let tree = isolated_tree("design-before-research");
    let specify = plan_command(
        &tree,
        &[
            "specify",
            "--request",
            "Extend the planner architecture gate with explicit traceability.",
        ],
    );
    assert_success(&specify, "plan specify");
    let payload = json_output(&specify);
    let plan = PlanFixture {
        id: payload["planId"].as_str().expect("plan id").to_string(),
        path: PathBuf::from(payload["planPath"].as_str().expect("plan path")),
    };
    write_architecture(&plan, &valid_architecture(&plan.id));
    assert_plan_design_failure(&tree, &plan, "research.json status is not complete");
    let status: Value = serde_json::from_str(
        &fs::read_to_string(plan.path.join("status.json")).expect("read status"),
    )
    .expect("parse status");
    assert_eq!(status["stage"], "specified");
    assert_eq!(status["architectureStatus"], "invalid");
}

#[test]
fn research_refresh_invalidates_prior_design_and_tasks() {
    let tree = isolated_tree("research-invalidates-design");
    let refresh_plan = specified_researched_plan(&tree);
    write_architecture(&refresh_plan, &valid_architecture(&refresh_plan.id));
    assert_success(&design_command(&tree, &refresh_plan), "initial design");
    let tasks = plan_command(&tree, &["tasks", "--plan", &refresh_plan.id]);
    assert_success(&tasks, "initial tasks");

    let refreshed = plan_command(&tree, &["research", "--plan", &refresh_plan.id]);
    assert_success(&refreshed, "research refresh");
    let stale_tasks = plan_command(&tree, &["tasks", "--plan", &refresh_plan.id]);
    assert_design_failure(&stale_tasks, "architecture.md status is not complete");
}
