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
    /// Hosts keel claims a governance story for. Kept beside the table so the
    /// matrix can never describe a host the installer does not wire, and so an
    /// unregistered name stays distinguishable from a registered-but-ungoverned
    /// one. `tests::every_claimed_host_has_explicit_metadata` holds this against
    /// the installer's own platform list.
    pub const CLAIMED_HOSTS: &'static [&'static str] = &[
        "claude",
        "codex",
        "opencode",
        "pi",
        "omp",
        "cursor",
        "cowork",
        "commandcode",
        "grok",
        "zcode",
        "antigravity",
    ];

    /// No proven interception surface at all. Unknown names resolve here too;
    /// [`Self::is_claimed_host`] is what tells the two apart.
    const fn ungoverned() -> Self {
        Self {
            pre_tool_intercept: false,
            post_tool_reduce: false,
            permission_gate: false,
            dynamic_tool_exposure: false,
            context_injection_control: false,
            session_identity: false,
            execution_receipt: false,
        }
    }

    pub fn is_claimed_host(agent: &str) -> bool {
        let normalized = agent.trim().to_ascii_lowercase();
        Self::CLAIMED_HOSTS.iter().any(|host| {
            normalized == *host
                || normalized
                    .strip_prefix(host)
                    .is_some_and(|rest| rest.starts_with('-') || rest.starts_with('_'))
        })
    }

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
            // Cursor's hooks (`cursor/hooks/hooks.json`) return permission
            // decisions, so interception and the gate are proven; rewrite is not.
            "cursor" => Self {
                pre_tool_intercept: true,
                post_tool_reduce: true,
                permission_gate: true,
                dynamic_tool_exposure: false,
                context_injection_control: false,
                session_identity: true,
                execution_receipt: true,
            },
            // OMP is wired at its own `~/.omp/agent` tree through the same
            // `keel-pi.ts` extension seam as Pi, so it shares Pi's proven surface.
            "zcode" | "antigravity" | "grok" | "opencode" | "pi" | "omp" | "commandcode" => Self {
                pre_tool_intercept: true,
                post_tool_reduce: true,
                permission_gate: false,
                dynamic_tool_exposure: false,
                context_injection_control: false,
                session_identity: true,
                execution_receipt: true,
            },
            // Claude Desktop has no hook system or plugin API, so keel registers
            // MCP only and every interception surface stays unproven.
            "cowork" | "desktop" => Self::ungoverned(),
            _ => Self::ungoverned(),
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
            // A name keel has never wired must not read the same as a wired host
            // whose interception surface is genuinely absent.
            "hostRegistered": Self::is_claimed_host(&normalized),
            "protocol": "keel-command-proxy",
            "transport": "host-adapter",
            "sessionHook": self.session_identity,
            "preToolInterception": self.pre_tool_intercept,
            "postToolInterception": self.post_tool_reduce,
            "permissionGate": self.permission_gate,
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

    /// The whole matrix for every claimed host, in a stable order. This is the
    /// machine-readable form §18 requires and the only place the full set is
    /// enumerated, so a host cannot be added to the installer and silently keep
    /// the unknown-host default here.
    pub fn claimed_matrix() -> serde_json::Value {
        let hosts = Self::CLAIMED_HOSTS
            .iter()
            .map(|host| Self::for_agent(host).as_json_for_agent(host)["matrix"].clone())
            .collect::<Vec<_>>();
        serde_json::json!({
            "schemaVersion": 1,
            "protocol": "keel-command-proxy",
            "transport": "host-adapter",
            "hosts": hosts,
        })
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
        assert_eq!(matrix["hostRegistered"], true);
        assert!(matrix["unsupportedPaths"]
            .as_array()
            .is_some_and(|paths| paths.iter().any(|path| path == "permission gate")));
        assert!(matrix["knownLimitations"].as_array().is_some_and(|items| {
            items
                .iter()
                .any(|item| item.as_str().unwrap_or_default().contains("permission"))
        }));
    }

    /// The installer's platform list and this table must describe the same set.
    /// A host added to one and not the other would silently report the unknown
    /// default, which is the failure §18 forbids.
    #[test]
    fn every_installer_platform_has_explicit_host_metadata() {
        for platform in [
            "opencode",
            "codex",
            "pi",
            "cursor",
            "cowork",
            "commandcode",
            "grok",
            "omp",
            "zcode",
            "antigravity",
        ] {
            assert!(
                HostCapabilities::is_claimed_host(platform),
                "{platform} is wired by the installer but missing from CLAIMED_HOSTS"
            );
            let matrix = HostCapabilities::for_agent(platform).as_json_for_agent(platform);
            assert_eq!(matrix["matrix"]["hostRegistered"], true, "{platform}");
            assert_eq!(matrix["matrix"]["host"], platform, "{platform}");
        }
    }

    /// Every claimed host answers from its own entry, never the fall-through.
    #[test]
    fn claimed_hosts_never_resolve_through_the_unknown_default() {
        for host in HostCapabilities::CLAIMED_HOSTS {
            assert!(
                HostCapabilities::is_claimed_host(host),
                "{host} is listed but not recognised"
            );
        }
        assert!(!HostCapabilities::is_claimed_host("unregistered-host"));
        let unknown =
            HostCapabilities::for_agent("unregistered-host").as_json_for_agent("unregistered-host");
        assert_eq!(unknown["matrix"]["hostRegistered"], false);
        assert_eq!(unknown["matrix"]["governanceLevel"], "UNSUPPORTED");

        // Cursor ships hook files that return permission decisions, so it must
        // not share the unknown-host default it used to fall through to.
        let cursor = HostCapabilities::for_agent("cursor").as_json_for_agent("cursor");
        assert_eq!(cursor["matrix"]["preToolInterception"], true);
        assert_eq!(cursor["matrix"]["hostRegistered"], true);
        assert_eq!(cursor["matrix"]["governanceLevel"], "PARTIALLY_GOVERNED");

        // Claude Desktop has no lifecycle hooks; it stays unproven rather than
        // inheriting the Claude Code surface through the shared substring.
        let cowork = HostCapabilities::for_agent("cowork").as_json_for_agent("cowork");
        assert_eq!(cowork["matrix"]["preToolInterception"], false);
        assert_eq!(cowork["matrix"]["hostRegistered"], true);
        assert_eq!(cowork["matrix"]["governanceLevel"], "UNSUPPORTED");
    }

    #[test]
    fn claimed_matrix_enumerates_every_host_once() {
        let matrix = HostCapabilities::claimed_matrix();
        let hosts = matrix["hosts"].as_array().expect("hosts array");
        assert_eq!(hosts.len(), HostCapabilities::CLAIMED_HOSTS.len());
        let names = hosts
            .iter()
            .map(|host| host["host"].as_str().expect("host name").to_string())
            .collect::<Vec<_>>();
        let unique = names.iter().collect::<std::collections::BTreeSet<_>>();
        assert_eq!(unique.len(), names.len(), "a host was enumerated twice");
        for host in hosts {
            assert_eq!(host["hostRegistered"], true);
            assert!(
                host["governanceLevel"].is_string(),
                "every host needs an explicit governance level"
            );
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
