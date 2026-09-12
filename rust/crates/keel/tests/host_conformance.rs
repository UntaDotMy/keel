//! Purpose: Verify the public host-conformance command reaches the real native
//! proxy path and fails closed for an unsupported host.
//! Caller: `cargo test -p keel --test host_conformance` and CI release gates.
//! Side effects: Uses a unique temporary recovery root and removes only that
//! test-owned directory after each assertion.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use assert_cmd::Command;
use serde_json::Value;

fn unique_root(label: &str) -> PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "keel-host-conformance-{label}-{}-{suffix}",
        std::process::id()
    ))
}

#[test]
fn governed_hosts_complete_the_proxy_evidence_chain() {
    let root = unique_root("governed");
    let output = Command::new(assert_cmd::cargo::cargo_bin!("keel"))
        .args([
            "host",
            "conformance",
            "--json",
            "--recovery-dir",
            root.to_str().expect("temporary path is UTF-8"),
        ])
        .output()
        .expect("run host conformance command");
    assert!(
        output.status.success(),
        "host conformance failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let payload: Value = serde_json::from_slice(&output.stdout).expect("valid JSON report");
    assert_eq!(payload["summary"]["allPass"], true);
    let reports = payload["reports"].as_array().expect("reports array");
    assert!(!reports.is_empty());
    for report in reports {
        assert_eq!(report["governance_state"], "GOVERNED");
        assert_eq!(report["status"], "pass");
        let stages = report["stages"].as_array().expect("stage array");
        assert_eq!(stages.len(), 8);
        assert!(stages.iter().all(|stage| stage["status"] == "pass"));
        assert!(report["raw_artifact_id"]
            .as_str()
            .is_some_and(|id| !id.is_empty()));
        assert!(report["projection_id"]
            .as_str()
            .is_some_and(|id| !id.is_empty()));
        assert!(report["visible_tokens"]
            .as_u64()
            .is_some_and(|tokens| { tokens <= report["budget_tokens"].as_u64().unwrap_or(0) }));
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn unsupported_hosts_are_not_reported_as_passing() {
    let root = unique_root("unsupported");
    let output = Command::new(assert_cmd::cargo::cargo_bin!("keel"))
        .args([
            "host",
            "conformance",
            "--host",
            "cowork",
            "--json",
            "--recovery-dir",
            root.to_str().expect("temporary path is UTF-8"),
        ])
        .output()
        .expect("run unsupported host conformance command");
    assert_eq!(output.status.code(), Some(2));
    let payload: Value = serde_json::from_slice(&output.stdout).expect("valid JSON report");
    assert_eq!(payload["summary"]["allPass"], false);
    assert_eq!(payload["reports"][0]["status"], "not_run");
    assert_eq!(payload["reports"][0]["governance_state"], "UNSUPPORTED");
    assert!(payload["reports"][0]["stages"]
        .as_array()
        .expect("stage array")
        .iter()
        .all(|stage| stage["status"] == "not_run"));
    let _ = std::fs::remove_dir_all(root);
}
