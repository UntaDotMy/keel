use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

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
    let base = std::env::temp_dir().join(format!(
        "keel-research-gap-{label}-{}-{nonce}",
        std::process::id()
    ));
    let root = base.join("workspace");
    let home = base.join("keel-home");
    fs::create_dir_all(&root).expect("create fixture workspace");
    fs::write(
        root.join("README.md"),
        "# Fixture\n\nThe local index contains MCP protocol notes and a sample JSON command for this test.\n",
    )
    .expect("write fixture source anchor");
    TempTree { root, home }
}

fn plan_command(tree: &TempTree, arguments: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_keel"));
    command.arg("plan").args(arguments).args([
        "--workspace-root",
        tree.root.to_str().expect("UTF-8 fixture root"),
        "--claude-home",
        tree.home.to_str().expect("UTF-8 fixture home"),
        "--json",
    ]);
    command.output().expect("run keel plan command")
}

fn json_output(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).expect("parse planner JSON output")
}

fn specify(tree: &TempTree, request: &str) -> (String, PathBuf) {
    let output = plan_command(tree, &["specify", "--request", request]);
    assert!(
        output.status.success(),
        "specify failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let payload = json_output(&output);
    (
        payload["planId"].as_str().expect("planId").to_string(),
        PathBuf::from(payload["planPath"].as_str().expect("planPath")),
    )
}

#[test]
fn current_external_request_rejects_local_index_fallback() {
    let tree = isolated_tree("external-current");
    let (plan_id, plan_path) = specify(&tree, "Check current MCP protocol behavior");
    let output = plan_command(&tree, &["research", "--plan", &plan_id]);

    assert!(
        !output.status.success(),
        "local fallback unexpectedly passed"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("cannot use local-index")
            || stderr.contains("requires an external host source"),
        "missing external provenance diagnostic: {stderr}"
    );
    let research: Value = serde_json::from_str(
        &fs::read_to_string(plan_path.join("research.json")).expect("research artifact"),
    )
    .expect("parse research artifact");
    assert_eq!(research["researchSource"], "local-index");
    let status: Value = serde_json::from_str(
        &fs::read_to_string(plan_path.join("status.json")).expect("status artifact"),
    )
    .expect("parse status artifact");
    assert_eq!(status["researchStatus"], "invalid");
}

#[test]
fn completed_artifact_requires_explicit_origin_and_non_truncated_projection() {
    let tree = isolated_tree("artifact-envelope");
    let (plan_id, plan_path) = specify(&tree, "Add a sample JSON command");
    let researched = plan_command(&tree, &["research", "--plan", &plan_id]);
    assert!(
        researched.status.success(),
        "stable local research failed: {}",
        String::from_utf8_lossy(&researched.stderr)
    );

    let research_path = plan_path.join("research.json");
    let mut research: Value =
        serde_json::from_str(&fs::read_to_string(&research_path).expect("research artifact"))
            .expect("parse research artifact");
    research
        .as_object_mut()
        .expect("research object")
        .remove("truncated");
    research["researchSource"] = Value::String("untrusted-adapter".to_string());
    fs::write(
        &research_path,
        format!(
            "{}\n",
            serde_json::to_string_pretty(&research).expect("render research")
        ),
    )
    .expect("write mutated research artifact");

    let checked = plan_command(&tree, &["check", "--plan", &plan_id]);
    assert!(
        !checked.status.success(),
        "invalid research envelope passed"
    );
    let stderr = String::from_utf8_lossy(&checked.stderr);
    assert!(
        stderr.contains("truncated flag"),
        "missing truncation diagnostic: {stderr}"
    );
    assert!(
        stderr.contains("researchSource") && stderr.contains("unsupported"),
        "missing origin diagnostic: {stderr}"
    );
}
