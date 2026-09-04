//! Purpose: Shared synchronization primitives for the crate's test suite.
//! Caller: `#[cfg(test)] mod tests` blocks across the crate.
//! Dependencies: std process primitives and std::sync::Mutex.
//! Main Functions: ENV_LOCK accessor and deterministic process-tree fixture.
//! Side Effects: None at module scope; tests may spawn a bounded fixture child.
//!
//! Design note: Several modules under `keel` mutate
//! process-global environment variables during their tests
//! (`CLAUDE_TARGET_OVERRIDE`, `CLAUDE_EFFORT`, `CLAUDE_PROJECT_DIR`,
//! `CLAUDE_SKILLS_HOOK`). Cargo runs tests across modules on a shared
//! thread pool, so a per-module Mutex only serializes within its own
//! module — two threads from different modules can still flip the same
//! env var concurrently and each see the other's value through
//! `resolve_claude_home`. The symptom we hit is a `tool_timings` test
//! that wrote its JSONL row, observed a `recall` test overwrite the
//! override, then tried to read the log back from its own (now stale)
//! directory and got `NotFound`.
//!
//! The fix is a single process-wide `ENV_LOCK` that every test which
//! mutates env vars takes before it touches them. Tests that do not
//! touch env vars do not need to hold it.

#![cfg(test)]

use std::ffi::OsStr;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);
const PROCESS_TREE_FIXTURE_ENV: &str = "KEEL_PROCESS_TREE_FIXTURE_ROLE";
const PROCESS_TREE_FIXTURE_TEST: &str = "test_support::descendant_pipe_fixture";

/// Process-wide guard around test-time mutation of environment variables.
/// Writers hold this for their full test body and restore the prior values.
pub(crate) static ENV_LOCK: Mutex<()> = Mutex::new(());

pub(crate) struct TestTempDir {
    path: PathBuf,
}

impl Deref for TestTempDir {
    type Target = Path;

    fn deref(&self) -> &Self::Target {
        &self.path
    }
}

impl AsRef<Path> for TestTempDir {
    fn as_ref(&self) -> &Path {
        &self.path
    }
}

impl AsRef<OsStr> for TestTempDir {
    fn as_ref(&self) -> &OsStr {
        self.path.as_os_str()
    }
}

impl Drop for TestTempDir {
    fn drop(&mut self) {
        for attempt in 0..5 {
            match std::fs::remove_dir_all(&self.path) {
                Ok(()) => return,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
                Err(_) if attempt < 4 => {
                    std::thread::sleep(std::time::Duration::from_millis(25));
                }
                Err(_) => return,
            }
        }
    }
}

/// Create a process-unique test directory and remove it when its owner drops.
/// The numeric suffix prevents parallel test processes from deleting each
/// other's state, while RAII cleanup also runs during unwinding.
pub(crate) fn unique_temp_dir(label: &str) -> TestTempDir {
    let sequence = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("{label}-{}-{sequence}", std::process::id()));
    std::fs::create_dir_all(&path).expect("create unique test directory");
    TestTempDir { path }
}

/// Build the self-hosted leader used by process-tree tests. The leader starts
/// one descendant that inherits its output handles, prints that descendant's
/// PID, and exits. This exercises pipe closure and tree termination without
/// depending on PowerShell cold-start latency or shell-specific job control.
pub(crate) fn descendant_pipe_fixture_command() -> Command {
    let executable = std::env::current_exe().expect("resolve current test executable");
    let mut command = Command::new(executable);
    command
        .args([PROCESS_TREE_FIXTURE_TEST, "--exact", "--nocapture"])
        .env(PROCESS_TREE_FIXTURE_ENV, "leader");
    command
}

pub(crate) fn descendant_pid_from_fixture_output(output: &str) -> Option<u32> {
    output.lines().find_map(|line| {
        line.trim()
            .strip_prefix("KEEL_DESCENDANT_PID=")?
            .parse::<u32>()
            .ok()
    })
}

/// The leader intentionally does not wait for its descendant: exiting first is
/// the condition the process-tree owners under test must clean up.
#[allow(clippy::zombie_processes)]
#[test]
fn descendant_pipe_fixture() {
    match std::env::var(PROCESS_TREE_FIXTURE_ENV).ok().as_deref() {
        Some("leader") => {
            let executable = std::env::current_exe().expect("resolve current test executable");
            let descendant = Command::new(executable)
                .args([PROCESS_TREE_FIXTURE_TEST, "--exact", "--nocapture"])
                .env(PROCESS_TREE_FIXTURE_ENV, "descendant")
                .spawn()
                .expect("spawn descendant fixture");
            println!("KEEL_DESCENDANT_PID={}", descendant.id());
        }
        Some("descendant") => std::thread::sleep(std::time::Duration::from_secs(30)),
        _ => {}
    }
}

#[test]
fn unique_temp_directory_is_removed_on_drop() {
    let directory = unique_temp_dir("keel-raii-temp-test");
    let path = directory.path.clone();
    assert!(path.is_dir());
    drop(directory);
    assert!(!path.exists());
}
