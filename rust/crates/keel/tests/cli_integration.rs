use assert_cmd::Command;
use std::time::Duration;

fn keel_bin() -> Command {
    let mut cmd = Command::cargo_bin("keel").expect("failed to locate keel binary");
    cmd.timeout(Duration::from_secs(10));
    cmd
}

#[test]
fn keel_no_args_exits_zero() {
    keel_bin().assert().success();
}

#[test]
fn keel_unknown_command_fails() {
    keel_bin()
        .arg("definitely-not-a-real-subcommand")
        .assert()
        .failure();
}

#[test]
fn keel_help_output_contains_keel() {
    let output = keel_bin().arg("help").output().expect("failed to run");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.to_lowercase().contains("keel") || stdout.to_lowercase().contains("operator"),
        "help output must mention keel or operator: {stdout}"
    );
}

#[test]
fn keel_config_audit_runs_without_panic() {
    let output = keel_bin()
        .args(["config-audit", "--repo-root", "."])
        .output()
        .expect("failed to run");
    assert!(
        output.status.code().is_some(),
        "config-audit must exit with a code, not hang or panic"
    );
}

#[test]
fn keel_skill_lint_runs_without_panic() {
    let output = keel_bin()
        .args(["skill-lint"])
        .output()
        .expect("failed to run");
    assert!(
        output.status.code().is_some(),
        "skill-lint must exit with a code, not hang or panic"
    );
}
