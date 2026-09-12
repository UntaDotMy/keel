//! Purpose: Execute and verify the host-conformance pipeline from the existing
//! command proxy boundary.
//! Caller: `keel host conformance` and the release/integration conformance test.
//! Dependencies: `HostCapabilities`, `run_proxy`, `RawStore` artifact layout,
//! and the authoritative `TokenMeter`.
//! Side effects: Runs one bounded local fixture command and keeps its raw
//! evidence under an operator-selected or temporary recovery directory.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::proxy::execution::{HostCapabilities, HostGovernanceState};
use crate::proxy::run::run_proxy;
use crate::proxy::token_meter::TokenMeter;

pub const CONFORMANCE_SCHEMA_VERSION: u64 = 1;

const PIPELINE_STAGES: [&str; 8] = [
    "host_action",
    "keel_receives",
    "policy_applies",
    "tool_executes",
    "raw_captured",
    "reducer_runs",
    "budget_applied",
    "bounded_result_returned",
];

/// These are machine states, not prose labels. A report can only be `pass`
/// when every stage has a passing evidence record; unproven host surfaces stay
/// `not_run` and therefore fail a release gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConformanceStatus {
    Pass,
    Partial,
    Blocked,
    NeedsHuman,
    NotRun,
    Indeterminate,
}

impl ConformanceStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Partial => "partial",
            Self::Blocked => "blocked",
            Self::NeedsHuman => "needs_human",
            Self::NotRun => "not_run",
            Self::Indeterminate => "indeterminate",
        }
    }

    pub fn is_pass(self) -> bool {
        self == Self::Pass
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConformanceStage {
    pub name: String,
    pub status: ConformanceStatus,
    /// Evidence is deliberately a bounded fact string, never captured tool
    /// output. Raw output remains recoverable from the referenced artifact.
    pub evidence: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostConformanceReport {
    pub schema_version: u64,
    pub host: String,
    pub governance_state: HostGovernanceState,
    pub status: ConformanceStatus,
    pub protocol: String,
    pub transport: String,
    pub stages: Vec<ConformanceStage>,
    pub execution_id: Option<String>,
    pub raw_artifact_id: Option<String>,
    pub projection_id: Option<String>,
    pub raw_tokens: Option<u32>,
    pub visible_tokens: Option<u32>,
    pub budget_tokens: Option<u32>,
    pub recovery_path: Option<String>,
    pub reason: Option<String>,
    pub generated_at_ms: u128,
}

impl HostConformanceReport {
    pub fn not_run(host: &str, governance_state: HostGovernanceState, reason: &str) -> Self {
        Self {
            schema_version: CONFORMANCE_SCHEMA_VERSION,
            host: host.to_string(),
            governance_state,
            status: ConformanceStatus::NotRun,
            protocol: "keel-command-proxy".to_string(),
            transport: "host-adapter".to_string(),
            stages: PIPELINE_STAGES
                .iter()
                .map(|stage| ConformanceStage {
                    name: (*stage).to_string(),
                    status: ConformanceStatus::NotRun,
                    evidence: "not executed".to_string(),
                })
                .collect(),
            execution_id: None,
            raw_artifact_id: None,
            projection_id: None,
            raw_tokens: None,
            visible_tokens: None,
            budget_tokens: None,
            recovery_path: None,
            reason: Some(reason.to_string()),
            generated_at_ms: now_millis(),
        }
    }

    pub(crate) fn blocked(host: &str, governance_state: HostGovernanceState, reason: &str) -> Self {
        let mut report = Self::not_run(host, governance_state, reason);
        report.status = ConformanceStatus::Blocked;
        report
    }
}

/// Return only hosts for which the matrix claims a complete governance path.
/// Unsupported/partial hosts are intentionally not silently promoted into the
/// conformance release gate.
pub fn governed_hosts() -> Vec<&'static str> {
    HostCapabilities::CLAIMED_HOSTS
        .iter()
        .copied()
        .filter(|host| {
            HostCapabilities::for_agent(host).governance_state() == HostGovernanceState::Governed
        })
        .collect()
}

/// Run a deterministic, isolated fixture through the same proxy owner used by
/// host hooks and validate every evidence stage. This proves the native path;
/// a real third-party host process is still a separate hosted/interactive
/// concern and must not be inferred from this report.
pub fn run(
    host: &str,
    requested_recovery_dir: Option<&Path>,
) -> Result<HostConformanceReport, String> {
    let host = host.trim().to_ascii_lowercase();
    if !HostCapabilities::is_claimed_host(&host) {
        return Err(format!(
            "unknown host '{host}'; known hosts: {}",
            HostCapabilities::CLAIMED_HOSTS.join(", ")
        ));
    }

    let governance_state = HostCapabilities::for_agent(&host).governance_state();
    if governance_state != HostGovernanceState::Governed {
        return Ok(HostConformanceReport::not_run(
            &host,
            governance_state,
            "host lifecycle is not fully governed; no conformance pass is claimed",
        ));
    }

    let recovery_root = requested_recovery_dir
        .map(Path::to_path_buf)
        .unwrap_or_else(|| default_recovery_dir(&host));
    let home = recovery_root.join("home");
    let workspace = recovery_root.join("workspace");
    let raw_root = recovery_root.join("raw");
    for directory in [&home, &workspace, &raw_root] {
        fs::create_dir_all(directory).map_err(|error| {
            format!(
                "create host-conformance directory {}: {error}",
                directory.display()
            )
        })?;
    }

    let session_id = format!("host-conformance-{host}-{}", std::process::id());
    let workspace_id = format!("host-conformance-workspace-{host}");
    let mut environment = EnvironmentGuard::default();
    for name in [
        "KEEL_HOME",
        "CLAUDE_TARGET_OVERRIDE",
        "HOME",
        "USERPROFILE",
        "CLAUDE_SKILLS_HOOK",
        "CLAUDE_PLUGIN_ROOT",
        "CLAUDE_AGENT",
        "CLAUDE_SKILLS_AGENT",
        "CLAUDECODE",
        "CLAUDE_CODE_ENTRYPOINT",
        "CLAUDE_CODE_SESSION_ID",
        "AI_AGENT",
        "CODEX_THREAD_ID",
        "CODEX_CI",
        "KEEL_MCP_WORKSPACE_ID",
        "KEEL_MCP_SESSION_ID",
    ] {
        environment.set(name, None);
    }
    environment.set("KEEL_HOME", Some(home.to_string_lossy().to_string()));
    environment.set(
        "CLAUDE_TARGET_OVERRIDE",
        Some(home.to_string_lossy().to_string()),
    );
    environment.set("HOME", Some(home.to_string_lossy().to_string()));
    environment.set("USERPROFILE", Some(home.to_string_lossy().to_string()));
    environment.set(
        "CLAUDE_PROJECT_DIR",
        Some(workspace.to_string_lossy().to_string()),
    );
    environment.set("CLAUDE_SKILLS_AGENT", Some(host.clone()));
    environment.set("KEEL_MCP_WORKSPACE_ID", Some(workspace_id));
    environment.set("KEEL_MCP_SESSION_ID", Some(session_id));
    if host == "codex" {
        environment.set(
            "CODEX_THREAD_ID",
            Some(format!("host-conformance-thread-{}", std::process::id())),
        );
        environment.set("CODEX_CI", Some("1".to_string()));
    } else {
        environment.set(
            "CLAUDE_CODE_SESSION_ID",
            Some(format!("host-conformance-session-{}", std::process::id())),
        );
    }

    let mut arguments = vec![
        "--ultra".to_string(),
        "--json".to_string(),
        "--recovery-dir".to_string(),
        raw_root.to_string_lossy().to_string(),
        "--".to_string(),
    ];
    arguments.extend(fixture_command());
    let mut output = Vec::new();
    let mut diagnostics = Vec::new();
    let exit_code = run_proxy(&arguments, &mut output, &mut diagnostics);
    let run_json = match serde_json::from_slice::<Value>(&output) {
        Ok(value) => value,
        Err(error) => {
            let mut report = HostConformanceReport::blocked(
                &host,
                governance_state,
                &format!("proxy did not emit a JSON evidence envelope: {error}"),
            );
            report.recovery_path = Some(recovery_root.to_string_lossy().to_string());
            return Ok(report);
        }
    };
    let mut report = evaluate(&host, governance_state, &run_json, &recovery_root);
    if exit_code != 0 && report.status.is_pass() {
        report.status = ConformanceStatus::Blocked;
        report.reason = Some(format!("fixture command exited with code {exit_code}"));
    }
    // Keep compiler warnings and hook diagnostics out of the report body. The
    // child evidence remains available under `recovery_path` for operators.
    let _ = diagnostics;
    Ok(report)
}

fn evaluate(
    host: &str,
    governance_state: HostGovernanceState,
    run_json: &Value,
    recovery_root: &Path,
) -> HostConformanceReport {
    let execution_id = string_field(run_json, "execution_id");
    let raw_artifact_id = string_field(run_json, "raw_id");
    let projection_id = run_json
        .get("context")
        .and_then(|context| context.get("id"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let context = run_json.get("context");
    let raw_tokens = context
        .and_then(|value| value.get("raw_tokens"))
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok());
    let visible_tokens = context
        .and_then(|value| value.get("visible_tokens"))
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok());
    let budget_tokens = context
        .and_then(|value| value.get("budget"))
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok());

    let raw_path = string_field(run_json, "raw_path").map(PathBuf::from);
    let raw_directory = raw_path.as_deref().filter(|path| path.is_dir());
    let receipt = raw_directory
        .and_then(|directory| fs::read_to_string(directory.join("execution.json")).ok())
        .and_then(|contents| serde_json::from_str::<Value>(&contents).ok());

    let host_action = run_json["host_capabilities"]["matrix"]["hostRegistered"]
        == Value::Bool(true)
        && run_json["host_capabilities"]["matrix"]["governanceLevel"]
            == Value::String("GOVERNED".to_string());
    let received = run_json["intercepted"] == Value::Bool(true);
    let policy = receipt
        .as_ref()
        .and_then(|value| value.get("policy_decision"))
        .and_then(Value::as_str)
        == Some("allowed");
    let tool = run_json.get("exit_code").and_then(Value::as_i64) == Some(0)
        && run_json.get("execution_status").and_then(Value::as_str) != Some("unknown");
    let raw_captured = raw_artifact_id.as_deref().is_some_and(|id| !id.is_empty())
        && raw_directory.is_some_and(|directory| {
            ["stdout.log", "stderr.log", "meta.json", "execution.json"]
                .iter()
                .all(|file| directory.join(file).is_file())
        })
        && receipt
            .as_ref()
            .and_then(|value| value.get("raw_artifact_id"))
            .and_then(Value::as_str)
            == raw_artifact_id.as_deref();
    let reducer = run_json.get("compacted").and_then(Value::as_bool) == Some(true)
        && context
            .and_then(|value| value.get("reducer"))
            .and_then(Value::as_str)
            .is_some_and(|value| !value.trim().is_empty());
    let budget = matches!((visible_tokens, budget_tokens), (Some(visible), Some(budget)) if budget > 0 && visible <= budget);
    let bounded = context
        .and_then(|_| run_json.get("summary"))
        .and_then(Value::as_str)
        .zip(budget_tokens)
        .is_some_and(|(summary, budget)| TokenMeter::count_text(summary) <= budget as usize);

    let checks = [
        ("host_action", host_action, "registered governed host"),
        ("keel_receives", received, "intercepted=true"),
        (
            "policy_applies",
            policy,
            "execution receipt policy_decision=allowed",
        ),
        ("tool_executes", tool, "fixture exit_code=0"),
        (
            "raw_captured",
            raw_captured,
            "raw and execution artifacts present",
        ),
        (
            "reducer_runs",
            reducer,
            "compacted=true with reducer metadata",
        ),
        ("budget_applied", budget, "visible_tokens <= budget"),
        (
            "bounded_result_returned",
            bounded,
            "summary token count <= budget",
        ),
    ];
    let stages = checks
        .iter()
        .map(|(name, passed, evidence)| ConformanceStage {
            name: (*name).to_string(),
            status: if *passed {
                ConformanceStatus::Pass
            } else {
                ConformanceStatus::Blocked
            },
            evidence: (*evidence).to_string(),
        })
        .collect::<Vec<_>>();
    let failed = checks
        .iter()
        .filter(|(_, passed, _)| !passed)
        .map(|(name, _, _)| *name)
        .collect::<Vec<_>>();
    HostConformanceReport {
        schema_version: CONFORMANCE_SCHEMA_VERSION,
        host: host.to_string(),
        governance_state,
        status: if failed.is_empty() {
            ConformanceStatus::Pass
        } else {
            ConformanceStatus::Blocked
        },
        protocol: "keel-command-proxy".to_string(),
        transport: "host-adapter".to_string(),
        stages,
        execution_id,
        raw_artifact_id,
        projection_id,
        raw_tokens,
        visible_tokens,
        budget_tokens,
        recovery_path: Some(recovery_root.to_string_lossy().to_string()),
        reason: (!failed.is_empty()).then(|| format!("failed stages: {}", failed.join(", "))),
        generated_at_ms: now_millis(),
    }
}

fn fixture_command() -> Vec<String> {
    if cfg!(windows) {
        vec![
            "cmd.exe".to_string(),
            "/d".to_string(),
            "/c".to_string(),
            "for /L %i in (1,1,96) do @echo keel-conformance-line-%i".to_string(),
        ]
    } else {
        vec![
            "sh".to_string(),
            "-c".to_string(),
            "i=1; while [ \"$i\" -le 96 ]; do printf 'keel-conformance-line-%s\\n' \"$i\"; i=$((i+1)); done".to_string(),
        ]
    }
}

fn default_recovery_dir(host: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "keel-host-conformance-{host}-{}-{}",
        std::process::id(),
        now_millis()
    ))
}

fn string_field(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_string)
}

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default()
}

#[derive(Default)]
struct EnvironmentGuard {
    previous: Vec<(String, Option<OsString>)>,
}

impl EnvironmentGuard {
    fn set(&mut self, name: &str, value: Option<String>) {
        self.previous
            .push((name.to_string(), std::env::var_os(name)));
        match value {
            Some(value) => std::env::set_var(name, value),
            None => std::env::remove_var(name),
        }
    }
}

impl Drop for EnvironmentGuard {
    fn drop(&mut self) {
        for (name, value) in self.previous.drain(..).rev() {
            match value {
                Some(value) => std::env::set_var(name, value),
                None => std::env::remove_var(name),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_governed_hosts_are_not_reported_as_pass() {
        let report = HostConformanceReport::not_run(
            "cowork",
            HostGovernanceState::Unsupported,
            "lifecycle unavailable",
        );
        assert_eq!(report.status, ConformanceStatus::NotRun);
        assert!(!report.status.is_pass());
        assert!(report
            .stages
            .iter()
            .all(|stage| { stage.status == ConformanceStatus::NotRun }));
    }

    #[test]
    fn governed_host_list_matches_matrix() {
        for host in governed_hosts() {
            assert_eq!(
                HostCapabilities::for_agent(host).governance_state(),
                HostGovernanceState::Governed
            );
        }
        assert!(!governed_hosts().is_empty());
    }

    #[test]
    fn fixture_command_is_platform_specific_and_bounded() {
        let command = fixture_command();
        assert!(!command.is_empty());
        assert!(command.len() <= 4);
        assert!(command.iter().all(|part| part.len() < 200));
    }
}
