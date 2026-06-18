//! Purpose: Compact database CLI output (psql, mysql/mariadb, sqlite3, redis-cli, mongosh)
//!   with result-set-aware reducers.
//! Caller: AdapterRegistry for CommandKind::Database commands.
//! Dependencies: CommandAst classification, RunMeta, and shared adapter helpers.
//! Main Functions: DatabaseAdapter::compact.
//! Side Effects: None; proxy::run persists raw and compact output.
//!
//! Interactive DB clients dump wide result tables and large JSON documents that
//! flood context. This adapter:
//!
//! - leads with query errors on failure (syntax errors, constraint violations),
//! - reduces large row sets to a header + sample rows + an omitted-row count,
//! - reduces large JSON (mongosh) to structure-only,
//! - redacts connection strings / passwords that appear in echoed output,
//! - passes small results through verbatim so quick lookups stay readable.
//!
//! Raw output is always saved for recovery, so reduction is non-destructive.

use crate::adapters::common::{
    compact_edges, compact_json_structure, merge_streams, normalized_command,
    redact_possible_secret, signal_lines,
};
use crate::proxy::adapter::{CommandAdapter, CompactResult};
use crate::proxy::command_ast::{CommandAst, CommandKind};
use crate::proxy::raw_store::RunMeta;

pub struct DatabaseAdapter;

/// Result sets at or below this many rows are small enough to keep verbatim.
const SMALL_ROW_LIMIT: usize = 30;
/// Sample this many leading rows when eliding a large result set.
const SAMPLE_ROWS: usize = 12;

impl CommandAdapter for DatabaseAdapter {
    fn name(&self) -> &'static str {
        "database"
    }

    fn matches(&self, ast: &CommandAst) -> bool {
        ast.detected_kind == CommandKind::Database
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
        let engine = database_engine(&meta.program);

        // Failures: lead with the error so a syntax/constraint problem is visible
        // without scrolling past partial result rows.
        if exit_code != 0 {
            let signals = signal_lines(&merged, 40);
            let rendered = if signals.is_empty() {
                compact_edges(&redact_lines(&merged), "database error output", 50)
            } else {
                let mut out = String::from("database error signals:\n");
                for line in &signals {
                    out.push_str(&format!("- {line}\n"));
                }
                out
            };
            return make_db_result(self.name(), &command, rendered, exit_code, meta);
        }

        let trimmed = merged.trim();
        let line_count = merged.lines().count();

        // mongosh and `--json` style output: reduce JSON to structure-only.
        if trimmed.starts_with('{') || trimmed.starts_with('[') {
            // compact_json_structure elides all values on a successful parse, but
            // returns the input VERBATIM when the payload is JSON-shaped yet
            // unparseable (truncated by a broken pipe, trailing log line, BOM).
            // Redact the result so that fallback path cannot leak connection
            // strings / tokens; on the success path redact_lines is a no-op
            // because the structure has no secret values left.
            let structure = redact_lines(&compact_json_structure(trimmed));
            let rendered = format!(
                "{engine}: JSON result reduced to structure (values elided; raw saved)\n{structure}"
            );
            return make_db_result(self.name(), &command, rendered, exit_code, meta);
        }

        // Small result: pass through verbatim (secrets still redacted).
        if line_count <= SMALL_ROW_LIMIT && trimmed.len() < 2000 {
            return make_db_result(
                self.name(),
                &command,
                redact_lines(&merged),
                exit_code,
                meta,
            );
        }

        // Large tabular result: keep the header and a sample of rows, elide the rest.
        let rendered = compact_result_table(&redact_lines(&merged), &engine);
        make_db_result(self.name(), &command, rendered, exit_code, meta)
    }
}

fn make_db_result(
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

/// Keep the first non-empty line as a header plus a sample of subsequent rows,
/// eliding the middle with a count. Result tables from psql/mysql are
/// header-then-rows, so this keeps the column shape and a representative sample.
fn compact_result_table(text: &str, engine: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= SMALL_ROW_LIMIT {
        return text.to_string();
    }
    let header_index = lines
        .iter()
        .position(|line| !line.trim().is_empty())
        .unwrap_or(0);
    let header = lines
        .get(header_index)
        .map(|line| line.trim())
        .unwrap_or("");
    let data_rows = lines.len().saturating_sub(header_index + 1);
    let sample: Vec<&str> = lines
        .iter()
        .skip(header_index + 1)
        .filter(|line| !line.trim().is_empty())
        .take(SAMPLE_ROWS)
        .copied()
        .collect();
    let mut rendered = format!("{engine}: ~{data_rows} rows\nheader: {header}\nsample:");
    for row in &sample {
        rendered.push_str(&format!("\n  {row}"));
    }
    if data_rows > sample.len() {
        rendered.push_str(&format!(
            "\n  ... omitted {} rows; raw output saved for recovery ...",
            data_rows - sample.len()
        ));
    }
    rendered
}

/// Redact connection strings and password values that DB clients echo, then run
/// the generic per-line secret redactor.
fn redact_lines(text: &str) -> String {
    text.lines()
        .map(|line| {
            let lower = line.to_ascii_lowercase();
            // Connection URIs (postgres://user:pass@host) and password prompts/echoes.
            let has_conn_uri = [
                "postgres://",
                "postgresql://",
                "mysql://",
                "mongodb://",
                "redis://",
            ]
            .iter()
            .any(|scheme| lower.contains(scheme));
            let has_password = lower.replace(['_', '-', ' '], "").contains("password");
            if has_conn_uri || has_password {
                return "[redacted database credential; see raw output locally]".to_string();
            }
            redact_possible_secret(line)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn database_engine(program: &str) -> String {
    match program_base(program).as_str() {
        "psql" => "postgres".to_string(),
        "mysql" | "mariadb" => "mysql".to_string(),
        "sqlite3" => "sqlite".to_string(),
        "redis-cli" => "redis".to_string(),
        "mongosh" | "mongo" => "mongodb".to_string(),
        other => other.to_string(),
    }
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
    use super::DatabaseAdapter;
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
            adapter_name: "database".to_string(),
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
    fn large_result_table_keeps_header_and_samples_rows() {
        let mut payload = String::from("id | name | email\n");
        for i in 0..200 {
            payload.push_str(&format!("{i} | user{i} | u{i}@example.com\n"));
        }
        let result = DatabaseAdapter.compact(
            payload.as_bytes(),
            b"",
            0,
            &meta("psql", &["-c", "SELECT * FROM users"], payload.len()),
        );
        assert!(result.compacted);
        assert!(result.summary.contains("ok: psql"));
        assert!(result.stdout.contains("header: id | name | email"));
        assert!(result.stdout.contains("omitted"));
        // Not every row should survive.
        assert!(!result.stdout.contains("user199"));
    }

    #[test]
    fn small_result_passes_through() {
        let payload = "id | name\n1 | alice\n2 | bob\n";
        let result = DatabaseAdapter.compact(
            payload.as_bytes(),
            b"",
            0,
            &meta(
                "psql",
                &["-c", "SELECT * FROM users LIMIT 2"],
                payload.len(),
            ),
        );
        assert!(result.stdout.contains("alice"));
        assert!(result.stdout.contains("bob"));
    }

    #[test]
    fn query_error_leads_with_signal() {
        let stderr = "ERROR:  syntax error at or near \"SELCT\"\nLINE 1: SELCT * FROM users\n";
        let result = DatabaseAdapter.compact(
            b"",
            stderr.as_bytes(),
            1,
            &meta("psql", &["-c", "SELCT"], stderr.len()),
        );
        assert_eq!(result.exit_code, 1);
        assert!(result.summary.contains("failed: psql"));
        assert!(result.stdout.to_lowercase().contains("syntax error"));
    }

    #[test]
    fn mongosh_json_reduced_to_structure() {
        let mut payload = String::from("[");
        for i in 0..40 {
            if i > 0 {
                payload.push(',');
            }
            payload.push_str(&format!(
                "{{\"_id\":\"{i}\",\"name\":\"doc{i}\",\"size\":123}}"
            ));
        }
        payload.push(']');
        let result = DatabaseAdapter.compact(
            payload.as_bytes(),
            b"",
            0,
            &meta("mongosh", &["--eval", "db.docs.find()"], payload.len()),
        );
        assert!(result.stdout.contains("structure"));
        assert!(!result.stdout.contains("doc39"));
    }

    #[test]
    fn malformed_json_payload_still_redacts_secrets() {
        // Regression: compact_json_structure returns the input VERBATIM when the
        // JSON-shaped payload fails to parse (truncated by a broken pipe, trailing
        // log line). Without redaction on that fallback, connection-string
        // credentials leaked straight to the agent. The payload starts with `{`
        // (so it takes the JSON branch) but is truncated/unparseable, and is long
        // enough to clear the small-output passthrough.
        let mut payload =
            String::from("{\"conn\":\"postgres://admin:s3cr3tpass@db.internal:5432/app\",\n");
        for i in 0..40 {
            payload.push_str(&format!("\"row{i}\":\"value-{i}\",\n"));
        }
        // No closing brace -> serde_json::from_str fails -> verbatim fallback.
        let result = DatabaseAdapter.compact(
            payload.as_bytes(),
            b"",
            0,
            &meta("psql", &["--json", "select * from secrets"], payload.len()),
        );
        assert!(
            !result.stdout.contains("s3cr3tpass"),
            "credential leaked on malformed-JSON fallback: {}",
            result.stdout
        );
    }

    #[test]
    fn connection_uri_is_redacted() {
        let payload =
            "Connecting to postgres://admin:s3cr3tpass@db.internal:5432/app\nrow count: 5\n";
        let result = DatabaseAdapter.compact(
            payload.as_bytes(),
            b"",
            0,
            &meta(
                "psql",
                &["postgres://admin:s3cr3tpass@db.internal/app"],
                payload.len(),
            ),
        );
        assert!(result.stdout.contains("redacted database credential"));
        assert!(!result.stdout.contains("s3cr3tpass"), "credential leaked");
    }
}
