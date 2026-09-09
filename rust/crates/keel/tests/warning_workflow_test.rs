use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
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
        "keel-warning-test-{label}-{}-{nonce}",
        std::process::id()
    ));
    let root = base.join("workspace");
    let home = base.join("keel-home");
    fs::create_dir_all(&root).expect("create fixture workspace");
    fs::create_dir_all(root.join("lib")).expect("create fixture lib");

    // Initialize git repo in fixture workspace so git rev-parse resolves root
    let _ = Command::new("git").arg("init").current_dir(&root).output();
    let _ = Command::new("git")
        .args(["config", "user.name", "Keel Test"])
        .current_dir(&root)
        .output();
    let _ = Command::new("git")
        .args(["config", "user.email", "test@keel.local"])
        .current_dir(&root)
        .output();

    fs::write(
        root.join("pubspec.yaml"),
        "name: fixture_app\ndependencies:\n  flutter:\n    sdk: flutter\n",
    )
    .expect("write pubspec");
    fs::write(
        root.join("lib").join("main.dart"),
        "void main() {\n  print('hello');\n}\n",
    )
    .expect("write main.dart");

    TestTree { root, home }
}

fn keel_command(tree: &TestTree) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_keel"));
    command.env("CLAUDE_TARGET_OVERRIDE", &tree.home);
    command.env("KEEL_HOME", &tree.home);
    command.env("CLAUDE_SKILLS_HOOK", "test");
    command.current_dir(&tree.root);
    command
}

fn write_mock_flutter(root: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        let script = root.join("flutter.cmd");
        fs::write(
            &script,
            "@echo off\r\nif exist \"%~dp0lib\\clean.marker\" (\r\n  echo No issues found!\r\n  exit /b 0\r\n) else (\r\n  echo info • Avoid print • lib/main.dart:7:3 • avoid_print\r\n  exit /b 0\r\n)\r\n",
        )
        .expect("write mock flutter.cmd");
        script
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;
        let script = root.join("flutter");
        fs::write(
            &script,
            "#!/bin/sh\nif [ -f \"$(dirname \"$0\")/lib/clean.marker\" ]; then\n  echo \"No issues found!\"\n  exit 0\nelse\n  echo \"info • Avoid print • lib/main.dart:7:3 • avoid_print\"\n  exit 0\nfi\n",
        )
        .expect("write mock flutter");
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755))
            .expect("chmod mock flutter");
        script
    }
}

fn complete_architecture(plan_id: &str) -> String {
    format!(
        "---\nschema_version: 1\nartifact: architecture\nplan_id: {plan_id}\n---\n\nStatus: complete\n\n# Architecture Note\n\n## 1. Current architecture relevant to scope\n\n[verified: CLM-001] The pubspec.yaml Flutter project was detected.\n\n## 2. Proposed architecture\n\n[derived: CLM-002] Keep flutter analyze clean.\n\nInput bound: One bounded analyzer stream.\n\nPolicy owner: The warnings ledger owns parsed diagnostics.\n\n## 3. Components/files/interfaces changed\n\n- Component: lib/main.dart | Requirements: REQ-001 | Acceptance: AC-001\n\n## 4. Data/control flow\n\nDiagnostics are reconciled into the workspace warnings ledger.\n\n## 5. Alternatives considered\n\nAlternative: Ignore analyzer warnings.\n\nTradeoff: Warnings are defect precursors.\n\n## 6. Why the chosen option fits requirements\n\nChosen option: Strict verification policy with warning ledger.\n\nInfrastructure reuse: Reuses existing CommandAst and raw recovery.\n\nConstraint fit: Preserves zero warning gate requirement.\n\n## 7. Risks and mitigations\n\nRisk: Flaky warnings block valid work.\n\nMitigation: Fingerprint-based waivers with expiry.\n\n## 8. Backward compatibility\n\nCompatibility: Non-strict commands remain unblocked.\n\nHost impact: none; existing host contracts remain unchanged.\n\n## 9. Error handling and fallback semantics\n\nFailure status: Unresolved new warnings exit non-zero.\n\nFallback: Baseline warnings remain visible without blocking.\n\nVisibility: Status pointer printed to standard error.\n\n## 10. Security/privacy implications\n\nSecurity/privacy: Sanitized diagnostic paths confined to workspace.\n\n## 11. Performance/token impact\n\nToken impact: Pointer stays under 30 tokens.\n\nMeasurement plan: Fixed-context budget tests.\n\n## 12. Test strategy\n\nVerification: Run offline Flutter E2E lifecycle test.\n\nAcceptance references: AC-001\n\n## 13. Rollback strategy\n\nRollback: Revert commits and reopen warnings.\n\n## 14. Requirement and research references\n\nRequirement references: REQ-001\n\nAcceptance references: AC-001\n\nClaim references: CLM-001, CLM-002\n"
    )
}

fn advance_to_tasks(tree: &TestTree) -> (String, PathBuf) {
    let request = "Keep Flutter analyzer clean and verify that zero warnings are introduced.";
    let mut specify = keel_command(tree);
    specify.args([
        "plan",
        "specify",
        "--request",
        request,
        "--workspace-root",
        tree.root.to_str().unwrap(),
        "--claude-home",
        tree.home.to_str().unwrap(),
        "--json",
    ]);
    let output = specify.output().expect("run plan specify");
    assert!(
        output.status.success(),
        "specify failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let payload: Value = serde_json::from_slice(&output.stdout).expect("parse specify json");
    let plan_id = payload["planId"].as_str().expect("plan id").to_string();
    let plan_path = PathBuf::from(payload["planPath"].as_str().expect("plan path"));

    let mut research = keel_command(tree);
    research.args([
        "plan",
        "research",
        "--plan",
        &plan_id,
        "--workspace-root",
        tree.root.to_str().unwrap(),
        "--claude-home",
        tree.home.to_str().unwrap(),
        "--json",
    ]);
    assert!(research
        .output()
        .expect("run plan research")
        .status
        .success());

    fs::write(
        plan_path.join("architecture.md"),
        complete_architecture(&plan_id),
    )
    .expect("write architecture.md");

    let mut design = keel_command(tree);
    design.args([
        "plan",
        "design",
        "--plan",
        &plan_id,
        "--workspace-root",
        tree.root.to_str().unwrap(),
        "--claude-home",
        tree.home.to_str().unwrap(),
        "--json",
    ]);
    assert!(design.output().expect("run plan design").status.success());

    let mut tasks = keel_command(tree);
    tasks.args([
        "plan",
        "tasks",
        "--plan",
        &plan_id,
        "--workspace-root",
        tree.root.to_str().unwrap(),
        "--claude-home",
        tree.home.to_str().unwrap(),
        "--json",
    ]);
    assert!(tasks.output().expect("run plan tasks").status.success());

    (plan_id, plan_path)
}

#[test]
fn flutter_offline_e2e_open_fix_resolved_lifecycle() {
    let tree = test_tree("flutter-lifecycle");
    let mock_flutter = write_mock_flutter(&tree.root);
    let (plan_id, plan_path) = advance_to_tasks(&tree);

    // Initial clean run establishes the "dart" family baseline
    fs::write(tree.root.join("lib").join("clean.marker"), "").expect("write clean marker");
    let mut baseline_run = keel_command(&tree);
    baseline_run.args([
        "run",
        "--",
        mock_flutter.to_str().unwrap(),
        "analyze",
        "--fatal-infos",
        "--fatal-warnings",
    ]);
    let baseline_output = baseline_run.output().expect("run baseline flutter analyze");
    assert!(
        baseline_output.status.success(),
        "baseline run failed: {}",
        String::from_utf8_lossy(&baseline_output.stderr)
    );

    // Assert baseline ledger is clean
    let mut warn_list = keel_command(&tree);
    warn_list.args([
        "warn",
        "list",
        "--repo-root",
        tree.root.to_str().unwrap(),
        "--claude-home",
        tree.home.to_str().unwrap(),
        "--json",
    ]);
    let warn_output = warn_list.output().expect("run warn list");
    assert!(warn_output.status.success());
    let list_json: Value = serde_json::from_slice(&warn_output.stdout).expect("parse warn list");
    assert_eq!(list_json["open"], 0);

    // Remove clean.marker to introduce deliberate analyzer warning
    fs::remove_file(tree.root.join("lib").join("clean.marker")).expect("remove clean marker");

    let mut warning_run = keel_command(&tree);
    warning_run.args([
        "run",
        "--",
        mock_flutter.to_str().unwrap(),
        "analyze",
        "--fatal-infos",
        "--fatal-warnings",
    ]);
    let warning_output = warning_run.output().expect("run warning flutter analyze");
    assert!(warning_output.status.success());
    let stderr = String::from_utf8_lossy(&warning_output.stderr);
    assert!(
        stderr.contains("warnings: 1 open (1 new) — run `keel warn list`"),
        "stderr must carry bounded pointer; got: {stderr}"
    );

    // Assert ledger has 1 open warning
    let mut warn_list_open = keel_command(&tree);
    warn_list_open.args([
        "warn",
        "list",
        "--repo-root",
        tree.root.to_str().unwrap(),
        "--claude-home",
        tree.home.to_str().unwrap(),
        "--json",
    ]);
    let warn_open_output = warn_list_open.output().expect("run warn list open");
    let list_open_json: Value =
        serde_json::from_slice(&warn_open_output.stdout).expect("parse warn list open");
    assert_eq!(list_open_json["open"], 1);
    let findings = list_open_json["warnings"]
        .as_array()
        .expect("warnings array");
    assert_eq!(findings.len(), 1);
    let fingerprint = findings[0]["diagnostic"]["fingerprint"]
        .as_str()
        .expect("fingerprint")
        .to_string();

    // Assert task ticket was updated with warning subtask
    let ticket_file = plan_path.join("task-001.json");
    let ticket_content = fs::read_to_string(&ticket_file).expect("read task ticket");
    let ticket: Value = serde_json::from_str(&ticket_content).expect("parse task ticket");
    let lint_subtasks = ticket["layers"]["lint_warnings"]
        .as_array()
        .expect("lint_warnings subtasks");
    assert!(
        lint_subtasks.iter().any(|subtask| subtask["description"]
            .as_str()
            .map(|desc| desc.contains(&fingerprint))
            .unwrap_or(false)),
        "task ticket must contain subtask for open warning fingerprint {fingerprint}: {ticket}"
    );

    // Assert pre-pr review fails on warnings_gate
    let mut review_pre_pr = keel_command(&tree);
    review_pre_pr.args([
        "review",
        "pre-pr",
        "--repo-root",
        tree.root.to_str().unwrap(),
        "--claude-home",
        tree.home.to_str().unwrap(),
        "--plan",
        &plan_id,
        "--format",
        "json",
    ]);
    let review_output = review_pre_pr.output().expect("run review pre-pr");
    let review_json: Value =
        serde_json::from_slice(&review_output.stdout).expect("parse review pre-pr");
    let gates = review_json["gates"].as_array().expect("gates array");
    let warnings_gate = gates
        .iter()
        .find(|g| g["name"].as_str() == Some("warnings_gate"))
        .expect("warnings_gate must be present in pre-pr");
    assert_eq!(warnings_gate["status"], "fail");
    assert_eq!(warnings_gate["blocking"], true);

    // Fix the fixture (restore clean.marker)
    fs::write(tree.root.join("lib").join("clean.marker"), "").expect("restore clean marker");

    // Re-run strict analyze via Keel
    let mut clean_run = keel_command(&tree);
    clean_run.args([
        "run",
        "--",
        mock_flutter.to_str().unwrap(),
        "analyze",
        "--fatal-infos",
        "--fatal-warnings",
    ]);
    let clean_output = clean_run.output().expect("run clean flutter analyze");
    assert!(clean_output.status.success());

    // Assert warning is now resolved in ledger
    let mut warn_list_resolved = keel_command(&tree);
    warn_list_resolved.args([
        "warn",
        "list",
        "--repo-root",
        tree.root.to_str().unwrap(),
        "--claude-home",
        tree.home.to_str().unwrap(),
        "--json",
    ]);
    let warn_res_output = warn_list_resolved.output().expect("run warn list resolved");
    let list_res_json: Value =
        serde_json::from_slice(&warn_res_output.stdout).expect("parse warn list resolved");
    assert_eq!(list_res_json["open"], 0);
    assert_eq!(list_res_json["resolved"], 1);

    // Assert pre-pr review now passes warnings_gate
    let mut review_pre_pr_pass = keel_command(&tree);
    review_pre_pr_pass.args([
        "review",
        "pre-pr",
        "--repo-root",
        tree.root.to_str().unwrap(),
        "--claude-home",
        tree.home.to_str().unwrap(),
        "--plan",
        &plan_id,
        "--format",
        "json",
    ]);
    let review_pass_output = review_pre_pr_pass.output().expect("run review pre-pr pass");
    let review_pass_json: Value =
        serde_json::from_slice(&review_pass_output.stdout).expect("parse review pass json");
    let gates_pass = review_pass_json["gates"].as_array().expect("gates array");
    let warnings_gate_pass = gates_pass
        .iter()
        .find(|g| g["name"].as_str() == Some("warnings_gate"))
        .expect("warnings_gate present");
    assert_eq!(warnings_gate_pass["status"], "pass");
    assert_eq!(warnings_gate_pass["blocking"], false);
}

#[test]
fn completion_gate_and_waiver_lifecycle_offline() {
    let tree = test_tree("flutter-waiver");
    let mock_flutter = write_mock_flutter(&tree.root);

    // Write a working brief in keel-home
    let mut brief_write = keel_command(&tree);
    brief_write.args([
        "memory",
        "working-brief",
        "write",
        "--id",
        "wb-flutter-test",
        "--request",
        "Test flutter analyzer warning waiver lifecycle",
        "--acceptance-criteria",
        "AC-001: Zero open warnings",
        "--claude-home",
        tree.home.to_str().unwrap(),
    ]);
    let brief_output = brief_write.output().expect("write working brief");
    assert!(
        brief_output.status.success(),
        "brief write failed: {}",
        String::from_utf8_lossy(&brief_output.stderr)
    );

    // Baseline run to initialize the family
    fs::write(tree.root.join("lib").join("clean.marker"), "").expect("write clean marker");
    let mut baseline_run = keel_command(&tree);
    baseline_run.args([
        "run",
        "--",
        mock_flutter.to_str().unwrap(),
        "analyze",
        "--fatal-infos",
        "--fatal-warnings",
    ]);
    assert!(baseline_run
        .output()
        .expect("run baseline")
        .status
        .success());

    // Introduce warning
    fs::remove_file(tree.root.join("lib").join("clean.marker")).expect("remove clean marker");
    let mut warn_run = keel_command(&tree);
    warn_run.args([
        "run",
        "--",
        mock_flutter.to_str().unwrap(),
        "analyze",
        "--fatal-infos",
        "--fatal-warnings",
    ]);
    assert!(warn_run.output().expect("run warning").status.success());

    // Get the fingerprint
    let mut warn_list = keel_command(&tree);
    warn_list.args([
        "warn",
        "list",
        "--repo-root",
        tree.root.to_str().unwrap(),
        "--claude-home",
        tree.home.to_str().unwrap(),
        "--json",
    ]);
    let list_output = warn_list.output().expect("run warn list");
    let list_json: Value = serde_json::from_slice(&list_output.stdout).expect("parse list json");
    assert_eq!(list_json["open"], 1);
    let fingerprint = list_json["warnings"][0]["diagnostic"]["fingerprint"]
        .as_str()
        .expect("fingerprint")
        .to_string();

    // Completion gate check must fail because of open warning
    let mut gate_check = keel_command(&tree);
    gate_check.args([
        "memory",
        "completion-gate",
        "check",
        "--brief-id",
        "wb-flutter-test",
        "--proof",
        "Ran strict flutter analyzer",
        "--claude-home",
        tree.home.to_str().unwrap(),
        "--json",
    ]);
    let gate_output = gate_check.output().expect("check completion gate");
    assert!(
        !gate_output.status.success(),
        "completion gate must fail with open warning"
    );
    let gate_json: Value = serde_json::from_slice(&gate_output.stdout).expect("parse gate json");
    assert_eq!(gate_json["closureReady"], false);

    // Waiver with short reason (< 10 chars) must fail
    let mut short_waive = keel_command(&tree);
    short_waive.args([
        "warn",
        "waive",
        &fingerprint,
        "--reason",
        "too short",
        "--repo-root",
        tree.root.to_str().unwrap(),
        "--claude-home",
        tree.home.to_str().unwrap(),
    ]);
    assert!(!short_waive
        .output()
        .expect("run short waive")
        .status
        .success());

    // Waiver with valid reason (>= 10 chars) succeeds
    let mut valid_waive = keel_command(&tree);
    valid_waive.args([
        "warn",
        "waive",
        &fingerprint,
        "--reason",
        "legitimate waiver reason for offline test",
        "--expires",
        "30d",
        "--repo-root",
        tree.root.to_str().unwrap(),
        "--claude-home",
        tree.home.to_str().unwrap(),
        "--json",
    ]);
    let waive_output = valid_waive.output().expect("run valid waive");
    assert!(
        waive_output.status.success(),
        "waive failed: {}",
        String::from_utf8_lossy(&waive_output.stderr)
    );

    // Assert warn list now shows waived=1, open=0
    let mut warn_list_waived = keel_command(&tree);
    warn_list_waived.args([
        "warn",
        "list",
        "--repo-root",
        tree.root.to_str().unwrap(),
        "--claude-home",
        tree.home.to_str().unwrap(),
        "--json",
    ]);
    let list_waived_json: Value =
        serde_json::from_slice(&warn_list_waived.output().expect("list waived").stdout)
            .expect("parse list waived");
    assert_eq!(list_waived_json["open"], 0);
    assert_eq!(list_waived_json["waived"], 1);

    // Completion gate check now succeeds!
    let mut gate_check_waived = keel_command(&tree);
    gate_check_waived.args([
        "memory",
        "completion-gate",
        "check",
        "--brief-id",
        "wb-flutter-test",
        "--proof",
        "Ran strict flutter analyzer with waiver",
        "--claude-home",
        tree.home.to_str().unwrap(),
        "--json",
    ]);
    let gate_waived_output = gate_check_waived
        .output()
        .expect("check completion gate waived");
    assert!(
        gate_waived_output.status.success(),
        "gate check must pass when waived: {}",
        String::from_utf8_lossy(&gate_waived_output.stderr)
    );
    let gate_waived_json: Value =
        serde_json::from_slice(&gate_waived_output.stdout).expect("parse gate waived json");
    assert_eq!(gate_waived_json["closureReady"], true);
}
