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

/// Regression: `bridge pre-tool-use` read stdin to EOF whenever `--command` was
/// absent, but only a *shell* tool's decision consults the command. Every host
/// adapter calls it for edit-class tools with no `--command` and no stdin pipe,
/// so on a host whose child inherits a still-open stdin the call blocked until
/// the adapter's timeout and the fail-closed branch denied every edit.
///
/// Holds the write end open and never sends a byte: the process must still exit.
#[test]
fn bridge_pre_tool_use_does_not_block_on_stdin_for_edit_tools() {
    use std::process::{Command as StdCommand, Stdio};
    use std::time::Instant;

    let binary = assert_cmd::cargo::cargo_bin("keel");
    let mut child = StdCommand::new(binary)
        .args([
            "bridge",
            "pre-tool-use",
            "--session",
            "s",
            "--cwd",
            ".",
            "--tool",
            "Edit",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn keel bridge pre-tool-use");

    // Deliberately NOT dropped: stdin stays open for the life of this scope.
    let _held_stdin = child.stdin.take().expect("piped stdin");

    let deadline = Duration::from_secs(10);
    let started = Instant::now();
    loop {
        match child.try_wait().expect("poll child") {
            Some(_) => break,
            None if started.elapsed() > deadline => {
                let _ = child.kill();
                panic!(
                    "bridge pre-tool-use blocked on an open stdin for an edit-class tool; \
                     adapters would time out and deny every edit"
                );
            }
            None => std::thread::sleep(Duration::from_millis(25)),
        }
    }
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
