use assert_cmd::Command;
use serde_json::Value;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

struct TestTree {
    root: PathBuf,
    home: PathBuf,
}

impl Drop for TestTree {
    fn drop(&mut self) {
        if let Some(parent) = self.root.parent() {
            let _ = fs::remove_dir_all(parent);
        }
    }
}

fn create_test_tree(label: &str) -> TestTree {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let base = std::env::temp_dir().join(format!(
        "keel-ui-verify-{label}-{}-{nonce}",
        std::process::id()
    ));
    let root = base.join("workspace");
    let home = base.join("keel-home");
    fs::create_dir_all(&root).expect("create test workspace");
    fs::create_dir_all(&home).expect("create test home");

    TestTree { root, home }
}

fn keel_cmd(tree: &TestTree) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_keel"));
    cmd.current_dir(&tree.root);
    cmd.env("KEEL_HOME", &tree.home);
    cmd.env("CLAUDE_TARGET_OVERRIDE", &tree.home);
    cmd.env("CLAUDE_SKILLS_HOOK", "test");
    cmd
}

const SAMPLE_PNG_BASE64: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";

#[test]
fn ui_verify_pass_and_rawstore_retrieval() {
    let tree = create_test_tree("pass");
    let fixture_path = tree.root.join("fixture_pass.json");

    let fixture_content = serde_json::json!({
        "criterion": {
            "state_name": "checkout_review",
            "route_or_screen": "/checkout/review",
            "preconditions": ["cart_has_items"],
            "expected_visible_text": ["Order Summary", "Confirm Purchase"],
            "expected_interactions": ["click_confirm"],
            "screenshot_required": true
        },
        "screen_text": ["Cart (2)", "Order Summary", "$49.99", "Confirm Purchase"],
        "interactions": ["click_confirm"],
        "screenshot_base64": SAMPLE_PNG_BASE64
    });

    fs::write(
        &fixture_path,
        serde_json::to_string(&fixture_content).unwrap(),
    )
    .expect("write fixture");

    let assert_res = keel_cmd(&tree)
        .args([
            "verify",
            "ui",
            "--fixture",
            "fixture_pass.json",
            "--task",
            "checkout-review",
            "--adapter",
            "playwright",
            "--json",
        ])
        .assert()
        .success();

    let stdout_str = String::from_utf8(assert_res.get_output().stdout.clone()).unwrap();
    let record: Value = serde_json::from_str(&stdout_str).expect("parse json record");

    assert_eq!(record["state"], "checkout_review");
    assert_eq!(record["adapter"], "playwright");
    assert_eq!(record["verdict"], "pass");
    let raw_id = record["screenshot_id"].as_str().expect("raw_id present");

    let raw_assert = keel_cmd(&tree).args(["raw", raw_id]).assert().success();

    let raw_stdout = String::from_utf8(raw_assert.get_output().stdout.clone()).unwrap();
    assert!(raw_stdout.contains("screenshot:"));
    assert!(raw_stdout.contains("screenshot.png"));

    let raw_path_assert = keel_cmd(&tree)
        .args(["raw", "--path", raw_id])
        .assert()
        .success();

    let raw_dir_str = String::from_utf8(raw_path_assert.get_output().stdout.clone())
        .unwrap()
        .trim()
        .to_string();
    let raw_dir = PathBuf::from(&raw_dir_str);
    assert!(raw_dir.join("screenshot.png").is_file());
    assert!(raw_dir.join("meta.json").is_file());
    assert!(raw_dir.join("stdout.log").is_file());

    let workspace_entries = fs::read_dir(tree.home.join("memories/workspaces"))
        .expect("workspace verification lane")
        .collect::<Result<Vec<_>, _>>()
        .expect("workspace entries");
    assert_eq!(workspace_entries.len(), 1);
    let verification_dir = workspace_entries[0]
        .path()
        .join("ui-verification/checkout-review");
    let manifest: Value = serde_json::from_str(
        &fs::read_to_string(verification_dir.join("manifest.json")).expect("manifest"),
    )
    .expect("manifest json");
    assert_eq!(manifest["schemaVersion"], 1);
    assert_eq!(manifest["status"], "pass");
    let verdicts: Value = serde_json::from_str(
        &fs::read_to_string(verification_dir.join("verdicts.json")).expect("verdicts"),
    )
    .expect("verdicts json");
    assert_eq!(verdicts[0]["schemaVersion"], 1);
    assert_eq!(verdicts[0]["screenshotId"], raw_id);
}

#[test]
fn ui_verify_fail_missing_expected_elements() {
    let tree = create_test_tree("fail");
    let fixture_path = tree.root.join("fixture_fail.json");

    let fixture_content = serde_json::json!({
        "criterion": {
            "state_name": "settings_page",
            "route_or_screen": "/settings",
            "preconditions": [],
            "expected_visible_text": ["Security Keys", "Two-Factor Auth"],
            "expected_interactions": [],
            "screenshot_required": true
        },
        "screen_text": ["General Settings", "Dark Theme"],
        "screenshot_base64": SAMPLE_PNG_BASE64
    });

    fs::write(
        &fixture_path,
        serde_json::to_string(&fixture_content).unwrap(),
    )
    .expect("write fixture");

    let assert_res = keel_cmd(&tree)
        .args([
            "verify",
            "ui",
            "--fixture",
            "fixture_fail.json",
            "--adapter",
            "playwright",
            "--json",
        ])
        .assert()
        .failure()
        .code(1);

    let stdout_str = String::from_utf8(assert_res.get_output().stdout.clone()).unwrap();
    let record: Value = serde_json::from_str(&stdout_str).expect("parse json record");

    assert_eq!(record["state"], "settings_page");
    assert_eq!(record["verdict"], "fail");
    let reasons = record["reasons"].as_array().expect("reasons array");
    let combined_reasons = reasons
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join("; ");
    assert!(combined_reasons.contains("Security Keys"));
    assert!(combined_reasons.contains("Two-Factor Auth"));
}

#[test]
fn ui_verify_needs_human_when_adapter_missing() {
    let tree = create_test_tree("needs_human");
    let fixture_path = tree.root.join("fixture_nh.json");

    let fixture_content = serde_json::json!({
        "criterion": {
            "state_name": "offline_view",
            "route_or_screen": "/offline",
            "preconditions": [],
            "expected_visible_text": ["No Connection"],
            "expected_interactions": [],
            "screenshot_required": true
        },
        "screen_text": ["No Connection"],
        "screenshot_base64": SAMPLE_PNG_BASE64
    });

    fs::write(
        &fixture_path,
        serde_json::to_string(&fixture_content).unwrap(),
    )
    .expect("write fixture");

    let assert_res = keel_cmd(&tree)
        .args([
            "verify",
            "ui",
            "--fixture",
            "fixture_nh.json",
            "--adapter",
            "needs_human",
            "--json",
        ])
        .assert()
        .success();

    let stdout_str = String::from_utf8(assert_res.get_output().stdout.clone()).unwrap();
    let record: Value = serde_json::from_str(&stdout_str).expect("parse json record");

    assert_eq!(record["verdict"], "needs_human");
    assert_ne!(record["verdict"], "pass");
    let reasons = record["reasons"].as_array().expect("reasons array");
    let combined_reasons = reasons
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join("; ");
    assert!(combined_reasons.contains("No automated visual adapter available"));
}

#[test]
fn ui_verify_unclear_maps_to_needs_human() {
    let tree = create_test_tree("unclear");
    let fixture_path = tree.root.join("fixture_unclear.json");

    let fixture_content = serde_json::json!({
        "criterion": {
            "state_name": "complex_chart",
            "route_or_screen": "/analytics",
            "preconditions": [],
            "expected_visible_text": [],
            "expected_interactions": [],
            "screenshot_required": true
        },
        "unclear": true,
        "unclear_reason": "Chart rendered with webgl canvas, cannot parse text DOM",
        "screenshot_base64": SAMPLE_PNG_BASE64
    });

    fs::write(
        &fixture_path,
        serde_json::to_string(&fixture_content).unwrap(),
    )
    .expect("write fixture");

    let assert_res = keel_cmd(&tree)
        .args([
            "verify",
            "ui",
            "--fixture",
            "fixture_unclear.json",
            "--adapter",
            "playwright",
            "--json",
        ])
        .assert()
        .success();

    let stdout_str = String::from_utf8(assert_res.get_output().stdout.clone()).unwrap();
    let record: Value = serde_json::from_str(&stdout_str).expect("parse json record");

    assert_eq!(record["verdict"], "unclear");
    assert_eq!(record["review_status"], "needs_human");
    assert_ne!(record["verdict"], "pass");
    assert_ne!(record["review_status"], "pass");
    let reasons = record["reasons"].as_array().expect("reasons array");
    let combined_reasons = reasons
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join("; ");
    assert!(combined_reasons.contains("Visual inspection required"));
    assert!(combined_reasons.contains("webgl canvas"));
}

#[test]
fn ui_verify_missing_fixture_fails() {
    let tree = create_test_tree("missing_fix");

    keel_cmd(&tree)
        .args(["verify", "ui", "--fixture", "nonexistent.json"])
        .assert()
        .failure()
        .code(1);
}

#[test]
fn ui_verify_rejects_fixture_outside_workspace_boundary() {
    let tree = create_test_tree("outside");
    let outside = tree
        .root
        .parent()
        .expect("fixture parent")
        .join("outside.json");
    fs::write(
        &outside,
        serde_json::to_string(&serde_json::json!({
            "text": "must not be read outside the workspace"
        }))
        .expect("serialize outside fixture"),
    )
    .expect("write outside fixture");

    let assert_res = keel_cmd(&tree)
        .args([
            "verify",
            "ui",
            "--fixture",
            outside.to_str().expect("outside path"),
            "--adapter",
            "playwright",
        ])
        .assert()
        .failure()
        .code(1);
    let stderr = String::from_utf8(assert_res.get_output().stderr.clone()).unwrap();
    assert!(
        stderr.contains("escapes workspace boundary"),
        "stderr: {stderr}"
    );
}
