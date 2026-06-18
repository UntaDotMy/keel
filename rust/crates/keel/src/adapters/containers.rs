//! Purpose: Compact docker, kubectl, and container orchestration output.
//! Caller: AdapterRegistry for CommandKind::Container commands.
//! Dependencies: CommandAst classification, RunMeta, and shared adapter helpers.
//! Main Functions: ContainersAdapter::compact.
//! Side Effects: None; proxy::run persists raw and compact output.

use crate::adapters::common::{
    compact_edges, dedup_lines, make_result, merge_streams, normalized_command, signal_lines,
};
use crate::proxy::adapter::{CommandAdapter, CompactResult};
use crate::proxy::command_ast::{CommandAst, CommandKind};
use crate::proxy::raw_store::RunMeta;

pub struct ContainersAdapter;

impl CommandAdapter for ContainersAdapter {
    fn name(&self) -> &'static str {
        "containers"
    }

    fn matches(&self, ast: &CommandAst) -> bool {
        ast.detected_kind == CommandKind::Container
    }

    fn compact(
        &self,
        stdout: &[u8],
        stderr: &[u8],
        exit_code: i32,
        meta: &RunMeta,
    ) -> CompactResult {
        let merged = merge_streams(stdout, stderr);
        let sub = container_subcommand(&meta.program, &meta.args);
        let command = normalized_command(&meta.program, &meta.args);

        let rendered = match sub.as_deref() {
            Some("ps")
            | Some("get pods")
            | Some("get deployments")
            | Some("get services")
            | Some("get nodes") => compact_table_rows(&merged, &sub.unwrap_or_default()),
            Some("logs") => {
                let deduped = dedup_lines(&merged);
                let signals = signal_lines(&deduped, 40);
                if signals.is_empty() {
                    compact_edges(&deduped, "container logs", 60)
                } else {
                    let mut out = String::from("container log signals:\n");
                    for line in &signals {
                        out.push_str(&format!("- {line}\n"));
                    }
                    out
                }
            }
            Some("images") | Some("list") | Some("ls") | Some("history") | Some("repo") => {
                compact_table_rows(&merged, &sub.unwrap_or_default())
            }
            Some("status") | Some("get") => compact_edges(&merged, "helm status", 50),
            Some("compose ps") => compact_table_rows(&merged, "compose services"),
            Some("compose") => compact_edges(&merged, "compose output", 40),
            _ => {
                let deduped = dedup_lines(&merged);
                let signals = signal_lines(&deduped, 40);
                if signals.is_empty() {
                    compact_edges(&deduped, "container output", 60)
                } else {
                    let mut out = String::from("container signals:\n");
                    for line in &signals {
                        out.push_str(&format!("- {line}\n"));
                    }
                    out
                }
            }
        };

        let prefix = if exit_code == 0 { "ok" } else { "failed" };
        make_result(
            self.name(),
            format!("{prefix}: {command}"),
            rendered,
            String::new(),
            exit_code,
            meta,
            true,
        )
    }
}

fn container_subcommand(program: &str, args: &[String]) -> Option<String> {
    let program_lower = program.to_ascii_lowercase();
    if program_lower.contains("kubectl") {
        return kubectl_noun_verb(args);
    }
    if program_lower.contains("helm") {
        return helm_subcommand(args);
    }
    if program_lower.contains("docker")
        || program_lower.contains("podman")
        || program_lower.contains("nerdctl")
    {
        return docker_subcommand(args);
    }
    args.first().cloned()
}

fn docker_subcommand(args: &[String]) -> Option<String> {
    let first = args.first()?;
    match first.as_str() {
        "compose" => {
            let second = args.get(1).map(String::as_str);
            Some(match second {
                Some("ps") => "compose ps".to_string(),
                Some(other) => format!("compose {other}"),
                None => "compose".to_string(),
            })
        }
        other => Some(other.to_string()),
    }
}

fn kubectl_noun_verb(args: &[String]) -> Option<String> {
    let verb = args.first()?;
    let resource = args.get(1).map(String::as_str);
    match (verb.as_str(), resource) {
        ("get", Some(r)) => Some(format!("get {r}")),
        ("describe", Some(r)) => Some(format!("describe {r}")),
        ("logs", _) => Some("logs".to_string()),
        ("apply", _) => Some("apply".to_string()),
        ("delete", Some(r)) => Some(format!("delete {r}")),
        _ => Some(verb.clone()),
    }
}

fn helm_subcommand(args: &[String]) -> Option<String> {
    let verb = args.first().map(String::as_str).unwrap_or("");
    match verb {
        "list" | "ls" | "history" | "repo" => Some(verb.to_string()),
        "install" | "upgrade" | "uninstall" | "rollback" | "template" => args
            .get(1)
            .map(|r| format!("{verb} {r}"))
            .or_else(|| Some(verb.to_string())),
        "status" | "get" | "show" | "test" | "verify" => args
            .get(1)
            .map(|r| format!("{verb} {r}"))
            .or_else(|| Some(verb.to_string())),
        _ => Some(verb.to_string()),
    }
}

fn compact_table_rows(text: &str, label: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() {
        return format!("{label}: (empty)");
    }
    let header_idx = lines.iter().position(|l| !l.trim().is_empty()).unwrap_or(0);
    let data_lines = if header_idx < lines.len() - 1 {
        lines[header_idx + 1..]
            .iter()
            .filter(|l| !l.trim().is_empty())
            .count()
    } else {
        0
    };
    let header = lines.get(header_idx).map(|l| l.trim()).unwrap_or("");
    let mut rendered = format!("{label}: {data_lines} entries\nheader: {header}");
    let sample_lines: Vec<&str> = lines
        .iter()
        .skip(header_idx + 1)
        .filter(|l| !l.trim().is_empty())
        .take(12)
        .copied()
        .collect();
    if !sample_lines.is_empty() {
        rendered.push_str("\nsample:");
        for line in &sample_lines {
            rendered.push_str(&format!("\n  {line}"));
        }
    }
    if data_lines > sample_lines.len() {
        rendered.push_str(&format!(
            "\n  ... omitted {} entries; raw output saved for recovery ...",
            data_lines - sample_lines.len()
        ));
    }
    rendered
}

#[cfg(test)]
mod tests {
    use super::ContainersAdapter;
    use crate::proxy::adapter::CommandAdapter;
    use crate::proxy::raw_store::RunMeta;
    use std::path::PathBuf;

    #[test]
    fn docker_ps_compacts_table() {
        let stdout = "CONTAINER ID   IMAGE     COMMAND   STATUS         NAMES\nabc123   nginx     ...       Up 2 hours     web\ndef456   redis     ...       Up 2 hours     cache\n";
        let result = ContainersAdapter.compact(
            stdout.as_bytes(),
            b"",
            0,
            &meta("docker", &["ps"], stdout.len()),
        );
        assert!(result.compacted);
        assert!(result.summary.contains("ok: docker ps"));
        assert!(result.stdout.contains("2 entries"));
        assert!(result.stdout.contains("CONTAINER ID"));
    }

    #[test]
    fn kubectl_get_pods_compacts_table() {
        let stdout =
            "NAME                          READY   STATUS    RESTARTS   AGE\napi-7d8f9b-abc   1/1     Running   0          5d\nweb-5c4d3e-def   2/2     Running   1          3d\n";
        let result = ContainersAdapter.compact(
            stdout.as_bytes(),
            b"",
            0,
            &meta("kubectl", &["get", "pods"], stdout.len()),
        );
        assert!(result.compacted);
        assert!(result.summary.contains("ok: kubectl get pods"));
        assert!(result.stdout.contains("2 entries"));
    }

    #[test]
    fn docker_logs_deduplicates() {
        let stdout = "error: connection refused\nerror: connection refused\nerror: connection refused\ninfo: retrying\n";
        let result = ContainersAdapter.compact(
            stdout.as_bytes(),
            b"",
            0,
            &meta("docker", &["logs", "web"], stdout.len()),
        );
        assert!(result.compacted);
        assert!(result.summary.contains("ok: docker logs web"));
        // Should collapse repeated lines and detect signal
        assert!(result.stdout.contains("connection refused"));
    }

    fn meta(program: &str, args: &[&str], stdout_bytes: usize) -> RunMeta {
        RunMeta {
            raw_id: "raw".to_string(),
            command: format!("{program} {}", args.join(" ")),
            program: program.to_string(),
            args: args.iter().map(|a| (*a).to_string()).collect(),
            cwd: PathBuf::from("."),
            started_at: 1,
            duration_ms: 1,
            exit_code: 0,
            adapter_name: "containers".to_string(),
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
}
