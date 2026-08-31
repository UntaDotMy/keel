use assert_cmd::Command;
use std::path::PathBuf;
use std::time::Duration;

struct TestDirectory(PathBuf);

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn keel_bin() -> Command {
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("keel"));
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

    let binary = assert_cmd::cargo::cargo_bin!("keel");
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

#[test]
fn grok_camel_case_post_tool_use_updates_lifecycle_state() {
    let unique = format!(
        "keel-grok-camel-hook-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock")
            .as_nanos()
    );
    let root = std::env::temp_dir().join(unique);
    let _cleanup = TestDirectory(root.clone());
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&workspace).expect("create isolated hook workspace");

    let payload = r#"{
        "hookEventName": "post_tool_use",
        "sessionId": "grok-session",
        "cwd": "C:/workspace",
        "workspaceRoot": "C:/workspace",
        "permissionMode": "default",
        "toolName": "search_replace",
        "toolInput": { "file_path": "src/lib.rs" },
        "toolUseId": "tool-1",
        "toolInputTruncated": false,
        "toolResult": { "ok": true },
        "durationMs": 42
    }"#;

    keel_bin()
        .args(["hook", "post-tool-use"])
        .current_dir(&workspace)
        .env("KEEL_HOME", &root)
        .env("CLAUDE_TARGET_OVERRIDE", &root)
        .env("CLAUDE_SKILLS_SYSTEM_MAP_REFRESH_INTERVAL", "100")
        .env("CLAUDE_SKILLS_COMMENT_LINT_GATE", "off")
        .env("CLAUDE_SKILLS_GRAPH_CONTEXT_GATE", "off")
        .write_stdin(payload)
        .assert()
        .success();

    let timings_dir = root.join("state").join("tool-timings");
    let timing_path = std::fs::read_dir(&timings_dir)
        .expect("Grok PostToolUse must create tool timings")
        .next()
        .expect("timing row file")
        .expect("timing directory entry")
        .path();
    let timing = std::fs::read_to_string(timing_path).expect("read timing row");
    assert!(timing.contains(r#""tool_name":"search_replace""#));
    assert!(timing.contains(r#""session_id":"grok-session""#));
    assert!(timing.contains(r#""duration_ms":42"#));

    let counter_dir = root.join("state").join("system-map-edit-counter");
    let counter_path = std::fs::read_dir(&counter_dir)
        .expect("Grok edit must update the SYSTEM_MAP counter")
        .next()
        .expect("counter file")
        .expect("counter directory entry")
        .path();
    assert_eq!(
        std::fs::read_to_string(counter_path).expect("read edit counter"),
        "1"
    );

    let observations_dir = root.join("state").join("observations");
    let observation_path = std::fs::read_dir(&observations_dir)
        .expect("Grok edit must create an observation")
        .next()
        .expect("observation row file")
        .expect("observation directory entry")
        .path();
    let observation = std::fs::read_to_string(observation_path).expect("read observation row");
    assert!(observation.contains(r#""tool_name":"search_replace""#));
    assert!(observation.contains(r#""signature":"edit:rs""#));
}
