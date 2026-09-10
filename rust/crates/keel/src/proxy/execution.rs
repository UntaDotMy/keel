//! Execution identity and host-capability contracts for governed actions.
//! Caller: `proxy::run` after the command boundary is known; operator stats and
//! recovery tooling read the persisted receipt written by `RawStore`.
//! Side effects: none. Persistence remains owned by `RawStore`.

use serde::{Deserialize, Serialize};

use crate::utility::hashing::fnv1a64_hex;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionStatus {
    Intercepted,
    Executed,
    Reduced,
    Bypassed,
    NotIntercepted,
    Blocked,
    Failed,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum HostGovernanceState {
    Governed,
    PartiallyGoverned,
    Observed,
    Unsupported,
}

impl HostGovernanceState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Governed => "GOVERNED",
            Self::PartiallyGoverned => "PARTIALLY_GOVERNED",
            Self::Observed => "OBSERVED",
            Self::Unsupported => "UNSUPPORTED",
        }
    }
}

impl ExecutionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Intercepted => "intercepted",
            Self::Executed => "executed",
            Self::Reduced => "reduced",
            Self::Bypassed => "bypassed",
            Self::NotIntercepted => "not_intercepted",
            Self::Blocked => "blocked",
            Self::Failed => "failed",
            Self::Unknown => "unknown",
        }
    }

    /// Only an actually executed/reduced action is a passing execution. The
    /// ambiguity states deliberately fail closed for aggregation.
    pub fn is_success(self) -> bool {
        matches!(self, Self::Executed | Self::Reduced)
    }

    pub fn is_explicitly_unprotected(self) -> bool {
        matches!(self, Self::Bypassed | Self::NotIntercepted | Self::Unknown)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostCapabilities {
    pub pre_tool_intercept: bool,
    pub post_tool_reduce: bool,
    pub permission_gate: bool,
    pub dynamic_tool_exposure: bool,
    pub context_injection_control: bool,
    pub session_identity: bool,
    pub execution_receipt: bool,
}

impl HostCapabilities {
    pub fn for_agent(agent: &str) -> Self {
        let normalized = agent.trim().to_ascii_lowercase();
        match normalized.as_str() {
            "claude" | "claude-code" | "claude_code" | "codex" | "codex-cli" | "codex_cli" => {
                Self {
                    pre_tool_intercept: true,
                    post_tool_reduce: true,
                    permission_gate: true,
                    dynamic_tool_exposure: false,
                    context_injection_control: true,
                    session_identity: true,
                    execution_receipt: true,
                }
            }
            "zcode" | "antigravity" | "grok" | "opencode" | "pi" | "commandcode" => Self {
                pre_tool_intercept: true,
                post_tool_reduce: true,
                permission_gate: false,
                dynamic_tool_exposure: false,
                context_injection_control: false,
                session_identity: true,
                execution_receipt: true,
            },
            _ => Self {
                pre_tool_intercept: false,
                post_tool_reduce: false,
                permission_gate: false,
                dynamic_tool_exposure: false,
                context_injection_control: false,
                session_identity: false,
                execution_receipt: false,
            },
        }
    }

    pub fn as_json(self) -> serde_json::Value {
        serde_json::json!({
            "state": self.governance_state().as_str(),
            "preToolIntercept": self.pre_tool_intercept,
            "postToolReduce": self.post_tool_reduce,
            "permissionGate": self.permission_gate,
            "dynamicToolExposure": self.dynamic_tool_exposure,
            "contextInjectionControl": self.context_injection_control,
            "sessionIdentity": self.session_identity,
            "executionReceipt": self.execution_receipt,
        })
    }

    /// Attach the machine-readable host matrix to the legacy capability
    /// booleans. The matrix deliberately records unproven surfaces as
    /// `NOT_PROVEN`/`UNSUPPORTED` instead of inferring support from a host's
    /// name or from Keel's own MCP server capabilities.
    pub fn as_json_for_agent(self, agent: &str) -> serde_json::Value {
        let mut payload = self.as_json();
        let normalized = agent.trim().to_ascii_lowercase();
        let mut unsupported_paths = Vec::new();
        let mut known_limitations = Vec::new();
        if !self.pre_tool_intercept {
            unsupported_paths.push("pre-tool interception");
            known_limitations.push("host does not expose a proven pre-tool interception hook");
        }
        if !self.post_tool_reduce {
            unsupported_paths.push("post-tool reduction");
            known_limitations.push("host does not expose a proven post-tool reduction hook");
        }
        if !self.permission_gate {
            unsupported_paths.push("permission gate");
            known_limitations.push("permission decisions remain outside the proven host path");
        }
        if !self.context_injection_control {
            unsupported_paths.push("context rewrite");
            known_limitations.push("model-visible context rewriting is not proven for this host");
        }
        if !self.session_identity {
            unsupported_paths.push("session identity");
            known_limitations.push("session identity is unavailable");
        }
        if !self.execution_receipt {
            unsupported_paths.push("execution receipt");
            known_limitations.push("execution receipts are unavailable");
        }
        let mcp_support = if self.dynamic_tool_exposure {
            "SUPPORTED"
        } else if self.governance_state() == HostGovernanceState::Unsupported {
            "UNSUPPORTED"
        } else {
            "NOT_PROVEN"
        };
        payload["matrix"] = serde_json::json!({
            "host": normalized,
            "protocol": "keel-command-proxy",
            "transport": "host-adapter",
            "sessionHook": self.session_identity,
            "preToolInterception": self.pre_tool_intercept,
            "postToolInterception": self.post_tool_reduce,
            "contextRewrite": self.context_injection_control,
            "mcpSupport": mcp_support,
            "paginationCompatibility": if self.dynamic_tool_exposure { "SUPPORTED" } else { "NOT_PROVEN" },
            "nativeToolSupport": self.pre_tool_intercept,
            "skillSupport": self.context_injection_control,
            "governanceLevel": self.governance_state().as_str(),
            "unsupportedPaths": unsupported_paths,
            "knownLimitations": known_limitations,
        });
        payload
    }

    pub fn governance_state(self) -> HostGovernanceState {
        if !self.session_identity && !self.execution_receipt {
            return HostGovernanceState::Unsupported;
        }
        if self.pre_tool_intercept && self.post_tool_reduce && self.context_injection_control {
            HostGovernanceState::Governed
        } else if self.pre_tool_intercept || self.post_tool_reduce {
            HostGovernanceState::PartiallyGoverned
        } else {
            HostGovernanceState::Observed
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionIdentity {
    pub schema_version: u64,
    pub execution_id: String,
    pub workspace_id: String,
    pub session_id: String,
    pub request_id: String,
    pub host_adapter: String,
    pub tool_name: String,
    pub command: String,
    pub policy_decision: String,
    pub intercepted: bool,
    pub start_time_ms: u128,
    pub end_time_ms: u128,
    pub raw_artifact_id: Option<String>,
    pub projection_id: Option<String>,
    pub result_state: ExecutionStatus,
}

impl ExecutionIdentity {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        workspace_id: impl Into<String>,
        session_id: impl Into<String>,
        request_id: impl Into<String>,
        host_adapter: impl Into<String>,
        tool_name: impl Into<String>,
        command: impl Into<String>,
        policy_decision: impl Into<String>,
        intercepted: bool,
        start_time_ms: u128,
        end_time_ms: u128,
        raw_artifact_id: Option<String>,
        projection_id: Option<String>,
        result_state: ExecutionStatus,
    ) -> Self {
        let workspace_id = workspace_id.into();
        let session_id = session_id.into();
        let request_id = request_id.into();
        let host_adapter = host_adapter.into();
        let tool_name = tool_name.into();
        let command = command.into();
        let policy_decision = policy_decision.into();
        let material = format!(
            "{workspace_id}\0{session_id}\0{request_id}\0{host_adapter}\0{tool_name}\0{command}\0{start_time_ms}\0{end_time_ms}"
        );
        let execution_id = format!("exec-fnv1a:{}", fnv1a64_hex(&material));
        Self {
            schema_version: 1,
            execution_id,
            workspace_id,
            session_id,
            request_id,
            host_adapter,
            tool_name,
            command,
            policy_decision,
            intercepted,
            start_time_ms,
            end_time_ms,
            raw_artifact_id,
            projection_id,
            result_state,
        }
    }

    pub fn capabilities(&self) -> HostCapabilities {
        HostCapabilities::for_agent(&self.host_adapter)
    }
}

pub fn now_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_host_is_explicitly_uninterceptable() {
        let capabilities = HostCapabilities::for_agent("unregistered-host");
        assert!(!capabilities.pre_tool_intercept);
        assert!(!capabilities.execution_receipt);
    }

    #[test]
    fn status_success_does_not_include_bypass_or_unknown() {
        assert!(ExecutionStatus::Reduced.is_success());
        for status in [
            ExecutionStatus::Bypassed,
            ExecutionStatus::NotIntercepted,
            ExecutionStatus::Unknown,
        ] {
            assert!(!status.is_success());
            assert!(status.is_explicitly_unprotected());
        }
    }

    #[test]
    fn host_state_never_claims_unknown_hosts_are_governed() {
        assert_eq!(
            HostCapabilities::for_agent("claude").governance_state(),
            HostGovernanceState::Governed
        );
        assert_eq!(
            HostCapabilities::for_agent("grok").governance_state(),
            HostGovernanceState::PartiallyGoverned
        );
        assert_eq!(
            HostCapabilities::for_agent("unregistered-host").governance_state(),
            HostGovernanceState::Unsupported
        );
    }

    #[test]
    fn capability_json_exposes_explicit_host_matrix_and_limitations() {
        let payload = HostCapabilities::for_agent("grok").as_json_for_agent("grok");
        let matrix = &payload["matrix"];
        assert_eq!(matrix["host"], "grok");
        assert_eq!(matrix["protocol"], "keel-command-proxy");
        assert_eq!(matrix["transport"], "host-adapter");
        assert_eq!(matrix["governanceLevel"], "PARTIALLY_GOVERNED");
        assert_eq!(matrix["mcpSupport"], "NOT_PROVEN");
        assert!(matrix["unsupportedPaths"]
            .as_array()
            .is_some_and(|paths| paths.iter().any(|path| path == "permission gate")));
        assert!(matrix["knownLimitations"].as_array().is_some_and(|items| {
            items
                .iter()
                .any(|item| item.as_str().unwrap_or_default().contains("permission"))
        }));
    }

    #[test]
    fn execution_id_is_stable_for_same_identity() {
        let args = (
            "workspace",
            "session",
            "request",
            "claude-code",
            "run",
            "cargo test",
            "allowed",
            true,
            1,
            2,
            Some("raw".to_string()),
            Some("projection".to_string()),
            ExecutionStatus::Reduced,
        );
        let left = ExecutionIdentity::new(
            args.0,
            args.1,
            args.2,
            args.3,
            args.4,
            args.5,
            args.6,
            args.7,
            args.8,
            args.9,
            args.10.clone(),
            args.11.clone(),
            args.12,
        );
        let right = ExecutionIdentity::new(
            args.0,
            args.1,
            args.2,
            args.3,
            args.4,
            args.5,
            args.6,
            args.7,
            args.8,
            args.9,
            args.10,
            args.11,
            ExecutionStatus::Reduced,
        );
        assert_eq!(left.execution_id, right.execution_id);
    }
}
