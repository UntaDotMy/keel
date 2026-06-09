//! Purpose: Compact cloud CLI output (aws, az, gcloud) with service-aware reducers.
//! Caller: AdapterRegistry for CommandKind::Cloud commands.
//! Dependencies: CommandAst classification, RunMeta, and shared adapter helpers.
//! Main Functions: CloudAdapter::compact.
//! Side Effects: None; proxy::run persists raw and compact output.
//!
//! Cloud CLIs emit large JSON blobs and wide tables, often carrying secrets
//! (IAM policy documents, lambda env vars, access keys). This adapter:
//!
//! - strips obvious secrets via the shared redactor and a cloud-specific pass,
//! - reduces large JSON to structure-only (keys + array lengths, values elided),
//! - surfaces failures first (non-zero exit shows error signals before data),
//! - keeps small payloads verbatim so short lookups stay readable.
//!
//! Raw output is always saved for recovery, so reduction is non-destructive.

use crate::adapters::common::{
    compact_edges, compact_json_structure, merge_streams, normalized_command,
    redact_possible_secret, signal_lines,
};
use crate::proxy::adapter::{CommandAdapter, CompactResult};
use crate::proxy::command_ast::{CommandAst, CommandKind};
use crate::proxy::raw_store::RunMeta;

pub struct CloudAdapter;

/// Below this many lines, cloud output is small enough to pass through verbatim
/// (after secret redaction) rather than reduce — short `describe`/`get` lookups
/// are more useful whole.
const SMALL_OUTPUT_LINE_LIMIT: usize = 25;

impl CommandAdapter for CloudAdapter {
    fn name(&self) -> &'static str {
        "cloud"
    }

    fn matches(&self, ast: &CommandAst) -> bool {
        ast.detected_kind == CommandKind::Cloud
    }

    fn compact(
        &self,
        stdout: &[u8],
        stderr: &[u8],
        exit_code: i32,
        meta: &RunMeta,
    ) -> CompactResult {
        let merged = merge_streams(stdout, stderr);
        let command = normalized_command(&meta.program, &meta.args);
        let provider = cloud_provider(&meta.program);
        let service = cloud_service(&meta.args);

        // Failures: lead with error signals so the cause is visible without
        // scrolling past data dumps.
        if exit_code != 0 {
            let signals = signal_lines(&merged, 40);
            let rendered = if signals.is_empty() {
                compact_edges(&strip_cloud_secrets(&merged), "cloud error output", 50)
            } else {
                let mut out = String::from("cloud error signals:\n");
                for line in &signals {
                    out.push_str(&format!("- {line}\n"));
                }
                out
            };
            return make_cloud_result(self.name(), &command, rendered, exit_code, meta);
        }

        let line_count = merged.lines().count();
        let trimmed = merged.trim();

        // Small payloads pass through verbatim (secrets still redacted).
        if line_count <= SMALL_OUTPUT_LINE_LIMIT && trimmed.len() < 2000 {
            let rendered = strip_cloud_secrets(&merged);
            return make_cloud_result(self.name(), &command, rendered, exit_code, meta);
        }

        // JSON payloads: reduce to structure-only (keys + array shape), which
        // also drops secret values along with everything else.
        let rendered = if trimmed.starts_with('{') || trimmed.starts_with('[') {
            // compact_json_structure returns the input VERBATIM when the payload
            // is JSON-shaped but unparseable (truncated/trailing-log/BOM), so
            // redact the result to stop that fallback leaking access keys/tokens.
            // On a successful parse strip_cloud_secrets is a no-op (values already
            // elided to type placeholders).
            let structure = strip_cloud_secrets(&compact_json_structure(trimmed));
            format!(
                "{provider} {service}: JSON reduced to structure (values elided; raw saved)\n{structure}"
            )
        } else {
            // Wide tables / line output: redact secrets then edge-compact.
            compact_edges(
                &strip_cloud_secrets(&merged),
                &format!("{provider} {service} output"),
                60,
            )
        };
        make_cloud_result(self.name(), &command, rendered, exit_code, meta)
    }
}

fn make_cloud_result(
    name: &'static str,
    command: &str,
    rendered: String,
    exit_code: i32,
    meta: &RunMeta,
) -> CompactResult {
    let prefix = if exit_code == 0 { "ok" } else { "failed" };
    crate::adapters::common::make_result(
        name,
        format!("{prefix}: {command}"),
        rendered,
        String::new(),
        exit_code,
        meta,
        true,
    )
}

/// Cloud-specific secret stripping on top of the shared line redactor: blank the
/// values of known sensitive keys (access keys, session tokens, passwords,
/// private keys) while keeping the key so the shape stays legible, then run the
/// generic per-line redactor for anything else that looks like a secret.
fn strip_cloud_secrets(text: &str) -> String {
    text.lines()
        .map(|line| {
            let normalized = line.to_ascii_lowercase().replace(['_', '-', ' '], "");
            let is_sensitive_key = [
                "accesskey",
                "secretaccesskey",
                "sessiontoken",
                "password",
                "privatekey",
            ]
            .iter()
            .any(|needle| normalized.contains(needle));
            if is_sensitive_key {
                if let Some(colon) = line.find(':') {
                    let (key, _) = line.split_at(colon);
                    return format!("{key}: \"[redacted cloud secret; see raw output locally]\"");
                }
            }
            redact_possible_secret(line)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn cloud_provider(program: &str) -> String {
    match program_base(program).as_str() {
        "aws" => "aws".to_string(),
        "az" => "azure".to_string(),
        "gcloud" => "gcloud".to_string(),
        other => other.to_string(),
    }
}

/// First positional that names the service/group (e.g. `aws s3`, `gcloud compute`,
/// `az vm`). Flags are skipped so `aws --region x s3 ls` still reports `s3`.
fn cloud_service(args: &[String]) -> String {
    args.iter()
        .find(|arg| !arg.starts_with('-'))
        .cloned()
        .unwrap_or_else(|| "command".to_string())
}

fn program_base(program: &str) -> String {
    program
        .replace('\\', "/")
        .rsplit('/')
        .next()
        .unwrap_or(program)
        .trim_end_matches(".exe")
        .trim_end_matches(".cmd")
        .trim_end_matches(".bat")
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::CloudAdapter;
    use crate::proxy::adapter::CommandAdapter;
    use crate::proxy::raw_store::RunMeta;
    use std::path::PathBuf;

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
            adapter_name: "cloud".to_string(),
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

    #[test]
    fn large_json_reduced_to_structure() {
        // A large JSON array of objects should be reduced to structure-only.
        let mut payload = String::from("[");
        for i in 0..50 {
            if i > 0 {
                payload.push(',');
            }
            payload.push_str(&format!(
                "{{\"FunctionName\":\"fn-{i}\",\"Runtime\":\"nodejs\",\"CodeSize\":1234}}"
            ));
        }
        payload.push(']');
        let result = CloudAdapter.compact(
            payload.as_bytes(),
            b"",
            0,
            &meta("aws", &["lambda", "list-functions"], payload.len()),
        );
        assert!(result.compacted);
        assert!(result.summary.contains("ok: aws lambda list-functions"));
        assert!(result.stdout.contains("structure"));
        // Concrete function names/values must not survive structure reduction.
        assert!(!result.stdout.contains("fn-42"));
    }

    #[test]
    fn secrets_are_redacted_in_passthrough() {
        let payload = "{\n  \"AccessKeyId\": \"AKIAIOSFODNN7EXAMPLE\",\n  \"SecretAccessKey\": \"wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY\"\n}";
        let result = CloudAdapter.compact(
            payload.as_bytes(),
            b"",
            0,
            &meta("aws", &["iam", "create-access-key"], payload.len()),
        );
        assert!(
            result.stdout.contains("redacted cloud secret"),
            "stdout: {}",
            result.stdout
        );
        assert!(!result.stdout.contains("wJalrXUtnFEMI"), "secret leaked");
    }

    #[test]
    fn failure_leads_with_error_signals() {
        let stderr = "An error occurred (AccessDenied) when calling the ListBuckets operation: Access Denied\n";
        let result = CloudAdapter.compact(
            b"",
            stderr.as_bytes(),
            255,
            &meta("aws", &["s3", "ls"], stderr.len()),
        );
        assert_eq!(result.exit_code, 255);
        assert!(result.summary.contains("failed: aws s3 ls"));
        assert!(result.stdout.to_lowercase().contains("denied"));
    }

    #[test]
    fn small_output_passes_through() {
        let payload = "us-east-1\nus-west-2\n";
        let result = CloudAdapter.compact(
            payload.as_bytes(),
            b"",
            0,
            &meta("aws", &["ec2", "describe-regions"], payload.len()),
        );
        assert!(result.stdout.contains("us-east-1"));
        assert!(result.stdout.contains("us-west-2"));
    }
}
