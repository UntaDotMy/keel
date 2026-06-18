//! Purpose: Compact Git and GitHub CLI output without hiding raw recovery data.
//! Caller: AdapterRegistry for CommandKind::Git commands.
//! Dependencies: CommandAst, RunMeta, and shared adapter helpers.
//! Main Functions: GitAdapter::compact.
//! Side Effects: None; proxy::run persists raw and compact output.

use crate::adapters::common::{compact_edges, make_result};
use crate::proxy::adapter::{CommandAdapter, CompactResult};
use crate::proxy::command_ast::{CommandAst, CommandKind};
use crate::proxy::raw_store::RunMeta;

pub struct GitAdapter;

const LINE_LIMIT: usize = 60;
impl CommandAdapter for GitAdapter {
    fn name(&self) -> &'static str {
        "git"
    }

    fn matches(&self, ast: &CommandAst) -> bool {
        ast.detected_kind == CommandKind::Git
    }

    fn compact(
        &self,
        stdout: &[u8],
        stderr: &[u8],
        exit_code: i32,
        meta: &RunMeta,
    ) -> CompactResult {
        let stdout = String::from_utf8_lossy(stdout);
        let stderr = String::from_utf8_lossy(stderr);

        let compact_stdout = match git_subcommand(&meta.program, &meta.args).as_deref() {
            Some("status") => compact_status(&stdout),
            Some("diff") => compact_diff(&stdout),
            Some("show") => compact_edges(&stdout, "show summary", LINE_LIMIT),
            Some("log") => compact_edges(&stdout, "log summary", LINE_LIMIT),
            Some("branch") => compact_edges(&stdout, "branches", LINE_LIMIT),
            Some("push") | Some("pull") | Some("fetch") => {
                compact_edges(&stdout, "remote update", LINE_LIMIT)
            }
            Some("gh pr") | Some("gh run") => compact_edges(&stdout, "github summary", LINE_LIMIT),
            _ => compact_edges(&stdout, "git output", LINE_LIMIT),
        };
        let compact_stderr = compact_edges(&stderr, "git diagnostics", LINE_LIMIT);
        let compacted = compact_stdout != stdout || compact_stderr != stderr;

        make_result(
            self.name(),
            format!(
                "git {}",
                git_subcommand(&meta.program, &meta.args).unwrap_or_else(|| "command".to_string())
            ),
            compact_stdout,
            compact_stderr,
            exit_code,
            meta,
            compacted,
        )
    }
}

fn git_subcommand(program: &str, args: &[String]) -> Option<String> {
    if program.eq_ignore_ascii_case("gh") || program.to_ascii_lowercase().ends_with("gh.exe") {
        return args.first().map(|arg| format!("gh {arg}"));
    }
    args.iter().find(|arg| !arg.starts_with('-')).cloned()
}

fn compact_status(stdout: &str) -> String {
    let lines: Vec<&str> = stdout.lines().collect();
    if lines.is_empty() {
        return "branch: clean\nchanged: 0 files".to_string();
    }
    let mut branch = String::new();
    let mut changed = Vec::new();
    let mut untracked = Vec::new();
    for line in &lines {
        if line.starts_with("## ") || line.starts_with("On branch ") {
            branch = line
                .trim_start_matches("## ")
                .trim_start_matches("On branch ")
                .to_string();
        } else if line.starts_with("?? ") || line.contains("Untracked files:") {
            untracked.push(line.trim().to_string());
        } else if !line.trim().is_empty()
            && !line.contains("nothing to commit")
            && !line.contains("working tree clean")
        {
            changed.push(line.trim().to_string());
        }
    }
    let mut rendered = String::new();
    rendered.push_str(&format!(
        "branch: {}\nchanged: {} files",
        if branch.is_empty() {
            "unknown"
        } else {
            branch.as_str()
        },
        changed.len()
    ));
    for line in changed.iter().take(12) {
        rendered.push_str(&format!("\n  {line}"));
    }
    if changed.len() > 12 {
        rendered.push_str(&format!("\nomitted changed: {}", changed.len() - 12));
    }
    if !untracked.is_empty() {
        rendered.push_str(&format!("\nuntracked: {} files", untracked.len()));
        for line in untracked.iter().take(8) {
            rendered.push_str(&format!("\n  {line}"));
        }
    }
    rendered
}

fn compact_diff(stdout: &str) -> String {
    let mut files = Vec::new();
    let mut additions = 0usize;
    let mut deletions = 0usize;
    let mut current_file = String::new();
    let mut hunks = Vec::new();
    for line in stdout.lines() {
        if let Some(path) = line.strip_prefix("diff --git ") {
            let parts: Vec<&str> = path.split_whitespace().collect();
            current_file = parts
                .get(1)
                .copied()
                .unwrap_or("")
                .trim_start_matches("b/")
                .to_string();
            if !current_file.is_empty() {
                files.push(current_file.clone());
            }
        } else if line.starts_with('+') && !line.starts_with("+++") {
            additions += 1;
        } else if line.starts_with('-') && !line.starts_with("---") {
            deletions += 1;
        } else if line.starts_with("@@") && !current_file.is_empty() && hunks.len() < 12 {
            hunks.push(format!("- {current_file}: {line}"));
        }
    }
    if files.is_empty() {
        return compact_edges(stdout, "diff summary", LINE_LIMIT);
    }
    let mut rendered = format!(
        "diff summary:\n{} files changed, +{} -{}",
        files.len(),
        additions,
        deletions
    );
    if !hunks.is_empty() {
        rendered.push_str("\n\nimportant hunks:");
        for hunk in hunks {
            rendered.push('\n');
            rendered.push_str(&hunk);
        }
    }
    rendered
}

#[cfg(test)]
mod tests {
    use super::GitAdapter;
    use crate::proxy::adapter::CommandAdapter;
    use crate::proxy::raw_store::RunMeta;
    use std::path::PathBuf;

    #[test]
    fn git_status_compacts_changed_files() {
        let stdout = "## main...origin/main\n M src/lib.rs\n?? tests/new.rs\n";
        let result = GitAdapter.compact(
            stdout.as_bytes(),
            b"",
            0,
            &meta("git status --short --branch", stdout.len()),
        );
        assert!(result.stdout.contains("branch: main"));
        assert!(result.stdout.contains("changed: 1 files"));
        assert!(result.stdout.contains("untracked: 1 files"));
    }

    #[test]
    fn wrapped_git_status_uses_normalized_subcommand() {
        let stdout = "On branch main\n M src/lib.rs\n";
        let result = GitAdapter.compact(
            stdout.as_bytes(),
            b"",
            0,
            &wrapped_meta("bash -lc 'git status'", "git", &["status"]),
        );
        assert!(result.stdout.contains("branch: main"));
        assert_eq!(result.summary, "git status");
    }

    fn meta(command: &str, stdout_bytes: usize) -> RunMeta {
        let (program, args) = if command == "git diff" {
            ("git".to_string(), vec!["diff".to_string()])
        } else {
            (
                "git".to_string(),
                vec![
                    "status".to_string(),
                    "--short".to_string(),
                    "--branch".to_string(),
                ],
            )
        };
        RunMeta {
            raw_id: "raw".to_string(),
            command: command.to_string(),
            program,
            args,
            cwd: PathBuf::from("."),
            started_at: 1,
            duration_ms: 1,
            exit_code: 0,
            adapter_name: "git".to_string(),
            raw_path: PathBuf::from("/tmp/raw"),
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

    fn wrapped_meta(command: &str, program: &str, args: &[&str]) -> RunMeta {
        RunMeta {
            raw_id: "raw".to_string(),
            command: command.to_string(),
            program: program.to_string(),
            args: args.iter().map(|arg| (*arg).to_string()).collect(),
            cwd: PathBuf::from("."),
            started_at: 1,
            duration_ms: 1,
            exit_code: 0,
            adapter_name: "git".to_string(),
            raw_path: PathBuf::from("/tmp/raw"),
            compact_path: PathBuf::new(),
            agent: "test".to_string(),
            workspace: PathBuf::from("."),
            stdout_bytes: 32,
            stderr_bytes: 0,
            compact_stdout_bytes: 0,
            compact_stderr_bytes: 0,
            estimated_tokens_before: 8,
            estimated_tokens_after: 0,
            estimated_tokens_saved: 0,
            savings_pct: 0.0,
            compacted: false,
        }
    }
}
