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
            "preToolIntercept": self.pre_tool_intercept,
            "postToolReduce": self.post_tool_reduce,
            "permissionGate": self.permission_gate,
            "dynamicToolExposure": self.dynamic_tool_exposure,
            "contextInjectionControl": self.context_injection_control,
            "sessionIdentity": self.session_identity,
            "executionReceipt": self.execution_receipt,
        })
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
