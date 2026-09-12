//! Runtime coverage for the release MCP smoke harness.
//!
//! The packaged archive is exercised by CI; this test keeps the same script
//! executable against the Cargo-built binary during ordinary workspace tests.

use serde_json::Value;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

struct TemporaryReleaseHome(PathBuf);

impl Drop for TemporaryReleaseHome {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("workspace repository root")
        .to_path_buf()
}

fn keel_binary() -> PathBuf {
    if let Some(path) = option_env!("CARGO_BIN_EXE_keel") {
        return PathBuf::from(path);
    }

    let mut path = env::current_exe().expect("resolve integration test executable");
    path.pop();
    path.pop();
    path.push(if cfg!(windows) { "keel.exe" } else { "keel" });
    path
}

fn temporary_release_home() -> TemporaryReleaseHome {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock")
        .as_nanos();
    let root = env::temp_dir().join(format!(
        "keel-release-smoke-test-{}-{nonce}",
        std::process::id()
    ));
    let skill_directory = root.join(".claude/skills/preserve-existing-flow");
    fs::create_dir_all(&skill_directory).expect("create isolated release home");
    fs::copy(
        repository_root().join("preserve-existing-flow/SKILL.md"),
        skill_directory.join("SKILL.md"),
    )
    .expect("stage the smoke skill fixture");
    TemporaryReleaseHome(root)
}

fn required_check<'a>(checks: &'a [Value], name: &str) -> &'a Value {
    checks
        .iter()
        .find(|check| check["name"] == name)
        .unwrap_or_else(|| panic!("release smoke report omitted {name}: {checks:?}"))
}

#[test]
fn release_smoke_script_exercises_restart_and_bounded_projection() {
    let repository = repository_root();
    let release_home = temporary_release_home();
    let keel_home = release_home.0.join(".keel");
    let report_path = release_home.0.join("release-smoke.json");
    let output = Command::new("node")
        .current_dir(&repository)
        .arg(repository.join(".github/release-smoke.mjs"))
        .args([
            "--binary",
            keel_binary().to_str().expect("UTF-8 keel binary path"),
            "--bundle-root",
            repository.to_str().expect("UTF-8 bundle root"),
            "--claude-home",
            keel_home.to_str().expect("UTF-8 keel home"),
            "--report",
            report_path.to_str().expect("UTF-8 report path"),
        ])
        .env("KEEL_RELEASE_SMOKE_NOISY_OUTPUT_BYTES", "20000")
        .output()
        .expect("run release MCP smoke script; Node.js is required");
    assert!(
        output.status.success(),
        "release MCP smoke failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let report: Value =
        serde_json::from_slice(&fs::read(&report_path).expect("read release MCP smoke report"))
            .expect("parse release MCP smoke report");
    assert_eq!(report["schemaVersion"], 1);
    assert_eq!(report["status"], "passed");
    assert_eq!(report["protocolVersion"], "2026-07-28");

    let checks = report["checks"].as_array().expect("smoke checks array");
    for check in checks {
        if let (Some(visible), Some(budget)) = (
            check["visibleTokens"].as_u64(),
            check["budgetTokens"].as_u64(),
        ) {
            assert!(
                visible <= budget,
                "smoke check exceeded its context budget: {check}"
            );
        }
    }

    let noisy = required_check(checks, "noisy-command-bounded-result");
    assert_eq!(noisy["status"], "passed");
    assert_eq!(noisy["truncated"], true);
    assert!(
        noisy["rawTokens"].as_u64().unwrap_or(0) > noisy["visibleTokens"].as_u64().unwrap_or(0)
    );
    assert!(noisy["sourceBytes"].as_u64().unwrap_or(0) >= 20_000);

    let restart = required_check(checks, "restart-and-reconnect");
    assert_eq!(restart["status"], "passed");
    assert!(restart["matchCount"].as_u64().unwrap_or(0) >= 1);
}
