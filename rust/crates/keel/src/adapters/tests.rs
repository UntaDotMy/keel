//! Purpose: Compact high-volume test-runner output into pass/fail signal and rerun hints.
//! Caller: AdapterRegistry for cargo, nextest, pytest, go test, npm/yarn/pnpm, jest, vitest, and similar test commands.
//! Dependencies: CommandAst classification, RawRun capture, RunMeta, and TokenMeter.
//! Main Functions: TestAdapter::compact.
//! Side Effects: None; proxy::run persists raw and compact output.

use crate::adapters::common::{make_result, merge_streams, normalized_command};
use crate::proxy::adapter::{CommandAdapter, CompactResult};
use crate::proxy::command_ast::{CommandAst, CommandKind};
use crate::proxy::raw_store::RunMeta;

pub struct TestAdapter;

const MAX_SUMMARY_LINES: usize = 12;
const MAX_FAILURE_LINES: usize = 28;

impl CommandAdapter for TestAdapter {
    fn name(&self) -> &'static str {
        "tests"
    }

    fn matches(&self, ast: &CommandAst) -> bool {
        ast.detected_kind == CommandKind::Test
    }

    fn compact(
        &self,
        stdout: &[u8],
        stderr: &[u8],
        exit_code: i32,
        meta: &RunMeta,
    ) -> CompactResult {
        let merged = merge_streams(stdout, stderr);
        let summary_lines = collect_summary_lines(&merged);
        let failure_lines = collect_failure_lines(&merged);
        let mut compact_stdout = String::new();

        if exit_code == 0 {
            if summary_lines.is_empty() {
                compact_stdout.push_str("completed successfully");
            } else {
                compact_stdout.push_str(&summary_lines.join("\n"));
            }
        } else {
            if summary_lines.is_empty() {
                compact_stdout.push_str("test command failed");
            } else {
                compact_stdout.push_str(&summary_lines.join("\n"));
            }
            if !failure_lines.is_empty() {
                compact_stdout.push_str("\n\nfailures:");
                for line in &failure_lines {
                    compact_stdout.push('\n');
                    compact_stdout.push_str(line);
                }
            }
            let rerun = rerun_hint(&meta.program, &meta.args, &failure_lines);
            compact_stdout.push_str(&format!("\n\nrerun:\n{rerun}"));
        }

        let compact_stderr = String::new();
        make_result(
            self.name(),
            normalized_command(&meta.program, &meta.args),
            compact_stdout,
            compact_stderr,
            exit_code,
            meta,
            true,
        )
    }
}

fn collect_summary_lines(text: &str) -> Vec<String> {
    let mut lines = Vec::new();
    for line in text.lines().rev() {
        let normalized = line.to_ascii_lowercase();
        let is_summary = normalized.contains("test result:")
            || normalized.contains(" passed")
            || normalized.contains(" failed")
            || normalized.contains(" failures")
            || normalized.contains(" tests")
            || normalized.contains("error:")
            || normalized.contains("failed:");
        if is_summary && !line.trim().is_empty() {
            lines.push(line.trim().to_string());
        }
        if lines.len() >= MAX_SUMMARY_LINES {
            break;
        }
    }
    lines.reverse();
    lines
}

fn collect_failure_lines(text: &str) -> Vec<String> {
    let mut failures = Vec::new();
    for line in text.lines() {
        let normalized = line.to_ascii_lowercase();
        let is_signal = normalized.contains("::")
            || normalized.contains("assert")
            || normalized.contains("panic")
            || normalized.contains("traceback")
            || normalized.contains("error:")
            || normalized.contains("failed")
            || normalized.contains("exception")
            || normalized.contains("expected")
            || normalized.contains("actual");
        if is_signal && !line.trim().is_empty() {
            failures.push(line.trim().to_string());
        }
        if failures.len() >= MAX_FAILURE_LINES {
            failures
                .push("... additional failure context omitted; use raw recovery ...".to_string());
            break;
        }
    }
    failures
}

fn rerun_hint(program: &str, args: &[String], failure_lines: &[String]) -> String {
    let command = normalized_command(program, args);
    let test_ids: Vec<&str> = failure_lines
        .iter()
        .filter_map(|line| line.split_whitespace().find(|part| part.contains("::")))
        .take(8)
        .collect();
    if program.eq_ignore_ascii_case("pytest") && !test_ids.is_empty() {
        return format!("pytest {} -q", test_ids.join(" "));
    }
    if program.eq_ignore_ascii_case("pytest") && !args.iter().any(|arg| arg == "-q") {
        format!("{command} -q")
    } else if program.eq_ignore_ascii_case("cargo")
        && args.iter().any(|arg| arg == "test")
        && !command.contains("-- --nocapture")
    {
        format!("{command} -- --nocapture")
    } else {
        command
    }
}

#[cfg(test)]
#[allow(clippy::module_inception)]
mod tests {
    use super::TestAdapter;
    use crate::proxy::adapter::CommandAdapter;
    use crate::proxy::raw_store::RunMeta;
    use std::path::PathBuf;

    #[test]
    fn pytest_pass_is_short_summary() {
        let stdout = "test_a.py .\n143 passed, 2 skipped in 12.4s\n";
        let result = TestAdapter.compact(
            stdout.as_bytes(),
            b"",
            0,
            &meta("pytest tests -q", stdout.len()),
        );
        assert!(result.compacted);
        assert!(result.stdout.contains("143 passed"));
        assert!(!result.stdout.contains("failures:"));
    }

    #[test]
    fn pytest_fail_keeps_test_id_message_and_rerun() {
        let stdout = "tests/api/test_users.py::test_create_user FAILED\nE AssertionError: expected 201, got 500\ntests/api/test_users.py:88\n2 failed, 143 passed in 12.8s\n";
        let result = TestAdapter.compact(
            stdout.as_bytes(),
            b"",
            1,
            &meta("pytest tests -q", stdout.len()),
        );
        assert!(result.stdout.contains("2 failed"));
        assert!(result
            .stdout
            .contains("tests/api/test_users.py::test_create_user"));
        assert!(result.stdout.contains("expected 201"));
        assert!(result.stdout.contains("rerun:"));
    }

    #[test]
    fn wrapped_pytest_rerun_hint_is_unwrapped() {
        let stdout = "tests/api/test_users.py::test_create_user FAILED\nE AssertionError: expected 201, got 500\n2 failed, 143 passed in 12.8s\n";
        let result = TestAdapter.compact(
            stdout.as_bytes(),
            b"",
            1,
            &wrapped_meta(
                "bash -lc 'pytest tests'",
                "pytest",
                &["tests"],
                stdout.len(),
            ),
        );
        assert!(result
            .stdout
            .contains("rerun:\npytest tests/api/test_users.py::test_create_user -q"));
        assert!(!result.stdout.contains("bash -lc"));
    }

    fn meta(command: &str, stdout_bytes: usize) -> RunMeta {
        let (program, args) = if command == "cargo test" {
            ("cargo".to_string(), vec!["test".to_string()])
        } else {
            (
                "pytest".to_string(),
                vec!["tests".to_string(), "-q".to_string()],
            )
        };
        RunMeta {
            raw_id: "test-raw".to_string(),
            command: command.to_string(),
            program,
            args,
            cwd: PathBuf::from("."),
            started_at: 1,
            duration_ms: 1,
            exit_code: 1,
            adapter_name: "tests".to_string(),
            raw_path: PathBuf::from("/tmp/test-raw"),
            compact_path: PathBuf::new(),
            agent: "test".to_string(),
            workspace: PathBuf::from("."),
            stdout_bytes,
            stderr_bytes: 0,
            compact_stdout_bytes: 0,
            compact_stderr_bytes: 0,
            estimated_tokens_before: stdout_bytes / 4,
            estimated_tokens_after: 0,
            estimated_tokens_saved: 0,
            savings_pct: 0.0,
            compacted: false,
        }
    }

    fn wrapped_meta(command: &str, program: &str, args: &[&str], stdout_bytes: usize) -> RunMeta {
        RunMeta {
            raw_id: "test-raw".to_string(),
            command: command.to_string(),
            program: program.to_string(),
            args: args.iter().map(|arg| (*arg).to_string()).collect(),
            cwd: PathBuf::from("."),
            started_at: 1,
            duration_ms: 1,
            exit_code: 1,
            adapter_name: "tests".to_string(),
            raw_path: PathBuf::from("/tmp/test-raw"),
            compact_path: PathBuf::new(),
            agent: "test".to_string(),
            workspace: PathBuf::from("."),
            stdout_bytes,
            stderr_bytes: 0,
            compact_stdout_bytes: 0,
            compact_stderr_bytes: 0,
            estimated_tokens_before: stdout_bytes / 4,
            estimated_tokens_after: 0,
            estimated_tokens_saved: 0,
            savings_pct: 0.0,
            compacted: false,
        }
    }
}
