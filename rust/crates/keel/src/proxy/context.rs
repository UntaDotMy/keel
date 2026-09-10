//! Purpose: Gate dynamic data before it becomes model-visible context.
//! Caller: governed proxy/MCP/memory surfaces that already own execution and storage.
//! Dependencies: the authoritative o200k_base token meter, injection guard, and
//! stable Keel hashing helper.
//! Main Functions: `ContextFirewall::project` and `project_optional`.
//! Side Effects: None. Raw artifacts are written by their existing owner; this
//! module only returns a bounded projection and a recoverable pointer.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::proxy::injection_guard::neutralize_injection;
use crate::proxy::token_meter::TokenMeter;
use crate::utility::hashing::sha256_hex;

/// Conservative default for one dynamic model-visible result. Surface-specific
/// ledgers may ratify a smaller budget; this default is deliberately bounded.
pub const DEFAULT_MAX_DYNAMIC_TOKENS: usize = 4_000;

/// Maximum input accepted by the firewall before token reduction. This is a
/// defense-in-depth bound; command capture itself has an independent limit.
pub const DEFAULT_MAX_INPUT_BYTES: usize = 64 * 1024 * 1024;

/// Safe policy defaults for the fixed/dynamic surfaces that have a smaller
/// budget than a generic command result. These are policy ceilings, not
/// observed measurements; the fixed-context ledger measures the actual value
/// on every run.
pub const DEFAULT_MAX_TOOL_CATALOG_TOKENS: usize = 1_200;
pub const DEFAULT_MAX_MEMORY_PROJECTION_TOKENS: usize = 900;
pub const DEFAULT_MAX_WARNING_POINTER_TOKENS: usize = 30;
pub const DEFAULT_MAX_DISCOVERY_RESULT_TOKENS: usize = 300;
pub const DEFAULT_MAX_SINGLE_RESULT_TOKENS: usize = 1_800;

/// The owner that produced a dynamic context payload.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextSource {
    CommandOutput,
    McpTool,
    Memory,
    Warning,
    Instruction,
    Research,
    Task,
    UiVerification,
    Error,
    Recovery,
}

impl ContextSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::CommandOutput => "command_output",
            Self::McpTool => "mcp_tool",
            Self::Memory => "memory",
            Self::Warning => "warning",
            Self::Instruction => "instruction",
            Self::Research => "research",
            Self::Task => "task",
            Self::UiVerification => "ui_verification",
            Self::Error => "error",
            Self::Recovery => "recovery",
        }
    }

    /// Return the conservative policy ceiling for a surface. Callers may
    /// still provide a narrower `ContextPolicy::with_max_tokens` when a
    /// particular operation has a smaller local budget.
    pub fn default_budget(&self) -> usize {
        match self {
            Self::McpTool => DEFAULT_MAX_SINGLE_RESULT_TOKENS,
            Self::Memory => DEFAULT_MAX_MEMORY_PROJECTION_TOKENS,
            Self::Warning => DEFAULT_MAX_WARNING_POINTER_TOKENS,
            Self::Recovery => DEFAULT_MAX_DISCOVERY_RESULT_TOKENS,
            _ => DEFAULT_MAX_DYNAMIC_TOKENS,
        }
    }
}

impl fmt::Display for ContextSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Cache behavior for a projection. Stable prefixes and deterministic ordering
/// can be cached by a provider; dynamic/volatile values should remain suffixes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheClass {
    Stable,
    Session,
    Dynamic,
    Volatile,
}

/// Bounded lifecycle state for a dynamic context object. The projection path
/// emits `visible`, `compressed`, or `masked`; archived/expired are reserved
/// for durable/reaper owners and are never used as a raw-content fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextLifecycleState {
    Fresh,
    Visible,
    Compressed,
    Masked,
    Archived,
    Expired,
}

/// Policy applied by one firewall instance. The existing fixed-context ledger
/// remains the owner of ratified per-surface budgets; this policy is the
/// reusable dynamic-result default and can be supplied by a caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextPolicy {
    pub max_tokens: usize,
    pub max_input_bytes: usize,
    pub neutralize_injection: bool,
}

impl ContextPolicy {
    /// Read optional operator overrides without making environment state the
    /// source of truth. Invalid values are ignored and the safe defaults stay
    /// active; a parsed zero is retained so an operator can deliberately fail
    /// closed while diagnosing a surface.
    pub fn from_env() -> Self {
        let mut policy = Self::default();
        if let Ok(value) = std::env::var("KEEL_CONTEXT_MAX_DYNAMIC_TOKENS") {
            if let Ok(parsed) = value.trim().parse::<usize>() {
                policy.max_tokens = parsed;
            }
        }
        if let Ok(value) = std::env::var("KEEL_CONTEXT_MAX_INPUT_BYTES") {
            if let Ok(parsed) = value.trim().parse::<usize>() {
                policy.max_input_bytes = parsed;
            }
        }
        policy
    }

    pub fn with_max_tokens(max_tokens: usize) -> Self {
        Self {
            max_tokens,
            ..Self::default()
        }
    }

    pub fn for_surface(max_tokens: usize) -> Self {
        Self::with_max_tokens(max_tokens)
    }

    pub fn with_max_input_bytes(mut self, max_input_bytes: usize) -> Self {
        self.max_input_bytes = max_input_bytes;
        self
    }

    pub fn without_injection_neutralization(mut self) -> Self {
        self.neutralize_injection = false;
        self
    }
}

impl Default for ContextPolicy {
    fn default() -> Self {
        Self {
            max_tokens: DEFAULT_MAX_DYNAMIC_TOKENS,
            max_input_bytes: DEFAULT_MAX_INPUT_BYTES,
            neutralize_injection: true,
        }
    }
}

/// Input owned by a producer. The firewall does not write or fetch the raw
/// artifact; it only validates the pointer and carries it into the projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectionInput {
    pub source: ContextSource,
    pub content: String,
    pub raw_artifact_id: Option<String>,
    pub workspace_id: String,
    pub session_id: String,
    pub request_id: Option<String>,
    pub cache_class: CacheClass,
    pub omitted_items: u32,
}

impl ProjectionInput {
    pub fn new(
        source: ContextSource,
        content: impl Into<String>,
        raw_artifact_id: Option<impl Into<String>>,
        workspace_id: impl Into<String>,
        session_id: impl Into<String>,
    ) -> Self {
        Self {
            source,
            content: content.into(),
            raw_artifact_id: raw_artifact_id.map(Into::into),
            workspace_id: workspace_id.into(),
            session_id: session_id.into(),
            request_id: None,
            cache_class: CacheClass::Dynamic,
            omitted_items: 0,
        }
    }

    pub fn with_request_id(mut self, request_id: impl Into<String>) -> Self {
        self.request_id = Some(request_id.into());
        self
    }

    pub fn with_cache_class(mut self, cache_class: CacheClass) -> Self {
        self.cache_class = cache_class;
        self
    }

    pub fn with_omitted_items(mut self, omitted_items: u32) -> Self {
        self.omitted_items = omitted_items;
        self
    }
}

/// The only object returned for normal model-visible dynamic data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextProjection {
    pub id: String,
    pub source: ContextSource,
    pub summary: String,
    pub token_count: u32,
    /// Exact tokenizer count of the cleaned producer payload before reduction.
    #[serde(rename = "raw_tokens")]
    pub raw_tokens: u32,
    /// Exact tokenizer count of the model-visible projection.
    #[serde(rename = "visible_tokens")]
    pub visible_tokens: u32,
    /// Hard budget applied to this projection.
    #[serde(rename = "budget")]
    pub budget: u32,
    /// Deterministic reducer selected for this projection.
    pub reducer: String,
    pub policy_version: String,
    pub state: ContextLifecycleState,
    /// Creation timestamp is evidence metadata, not part of the stable
    /// model-visible metadata projection.
    #[serde(skip)]
    pub created_at: u64,
    pub raw_artifact_id: Option<String>,
    pub provenance_id: String,
    pub truncated: bool,
    pub omitted_items: u32,
    pub cache_class: CacheClass,
}

/// Wire-safe projection metadata. It intentionally omits `summary` so a
/// protocol response can carry provenance and accounting without duplicating
/// the model-visible text in both `content` and an envelope object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextProjectionMetadata {
    pub id: String,
    pub source: ContextSource,
    pub token_count: u32,
    #[serde(rename = "raw_tokens")]
    pub raw_tokens: u32,
    #[serde(rename = "visible_tokens")]
    pub visible_tokens: u32,
    #[serde(rename = "budget")]
    pub budget: u32,
    pub reducer: String,
    pub policy_version: String,
    pub state: ContextLifecycleState,
    pub raw_artifact_id: Option<String>,
    pub provenance_id: String,
    pub truncated: bool,
    pub omitted_items: u32,
    pub cache_class: CacheClass,
}

/// Provider-reported cache usage. The provider owns these numbers; Keel never
/// infers cache accounting when a provider has not supplied it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderUsage {
    pub provider: String,
    pub cached_input_tokens: Option<usize>,
    pub uncached_input_tokens: Option<usize>,
    pub output_tokens: Option<usize>,
}

/// One measured model-visible surface. Keeping raw and visible counts in the
/// same record prevents a token-saving claim from being based on a theoretical
/// reducer estimate alone.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextMeasurement {
    pub surface: String,
    pub raw_input_tokens: usize,
    pub model_visible_input_tokens: usize,
    pub cached_input_tokens: Option<usize>,
    pub uncached_input_tokens: Option<usize>,
    pub output_tokens: usize,
    pub context_peak_tokens: usize,
    pub reduced_tokens: usize,
    pub saved_tokens: isize,
    pub reduction_ratio: f64,
    pub turn_count: u64,
    pub tool_count: u64,
    pub cache_hit_rate: Option<f64>,
    pub cache_class: CacheClass,
    pub budget_tokens: usize,
    pub soft_budget_tokens: Option<usize>,
    pub hard_budget_tokens: usize,
    pub tokenizer: String,
    pub measurement_timestamp: u64,
    pub implementation_version: String,
    pub status: String,
}

/// Whole-context ledger owned by the firewall/budget engine. It is an
/// in-memory operator-facing ledger; persistent evidence remains owned by the
/// existing event/RawStore paths.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ContextLedger {
    pub measurements: Vec<ContextMeasurement>,
    pub provider_usage: Vec<ProviderUsage>,
}

/// In-memory accounting is transient operator state. Keep a bounded tail so a
/// long-lived session cannot turn observability into an unbounded allocation.
const MAX_LEDGER_MEASUREMENTS: usize = 4_096;
const MAX_PROVIDER_USAGE: usize = 512;

impl ContextLedger {
    pub fn record(&mut self, measurement: ContextMeasurement) {
        if self.measurements.len() >= MAX_LEDGER_MEASUREMENTS {
            let excess = self
                .measurements
                .len()
                .saturating_sub(MAX_LEDGER_MEASUREMENTS)
                .saturating_add(1);
            self.measurements.drain(..excess);
        }
        self.measurements.push(measurement);
    }

    pub fn record_provider_usage(&mut self, usage: ProviderUsage) {
        if self.provider_usage.len() >= MAX_PROVIDER_USAGE {
            let excess = self
                .provider_usage
                .len()
                .saturating_sub(MAX_PROVIDER_USAGE)
                .saturating_add(1);
            self.provider_usage.drain(..excess);
        }
        self.provider_usage.push(usage);
    }

    pub fn status(&self) -> &'static str {
        if self.measurements.iter().any(|row| row.status == "blocked") {
            "blocked"
        } else if self.measurements.iter().any(|row| row.status == "exceeded") {
            "exceeded"
        } else {
            "within_budget"
        }
    }

    pub fn totals(&self) -> ContextTotals {
        let mut totals = ContextTotals::default();
        for row in &self.measurements {
            totals.raw_input_tokens = totals.raw_input_tokens.saturating_add(row.raw_input_tokens);
            totals.model_visible_input_tokens = totals
                .model_visible_input_tokens
                .saturating_add(row.model_visible_input_tokens);
            totals.reduced_tokens = totals.reduced_tokens.saturating_add(row.reduced_tokens);
            totals.saved_tokens = totals.saved_tokens.saturating_add(row.saved_tokens);
            totals.output_tokens = totals.output_tokens.saturating_add(row.output_tokens);
            totals.context_peak_tokens = totals.context_peak_tokens.max(row.context_peak_tokens);
            totals.turn_count = totals.turn_count.max(row.turn_count);
            totals.tool_count = totals.tool_count.saturating_add(row.tool_count);
        }
        totals
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextTotals {
    pub raw_input_tokens: usize,
    pub model_visible_input_tokens: usize,
    pub reduced_tokens: usize,
    pub saved_tokens: isize,
    pub output_tokens: usize,
    pub context_peak_tokens: usize,
    pub turn_count: u64,
    pub tool_count: u64,
}

impl ContextProjection {
    pub fn metadata(&self) -> ContextProjectionMetadata {
        ContextProjectionMetadata {
            id: self.id.clone(),
            source: self.source.clone(),
            token_count: self.token_count,
            raw_tokens: self.raw_tokens,
            visible_tokens: self.visible_tokens,
            budget: self.budget,
            reducer: self.reducer.clone(),
            policy_version: self.policy_version.clone(),
            state: self.state,
            raw_artifact_id: self.raw_artifact_id.clone(),
            provenance_id: self.provenance_id.clone(),
            truncated: self.truncated,
            omitted_items: self.omitted_items,
            cache_class: self.cache_class,
        }
    }
}

/// A projection's provenance identity. It is deliberately not model-visible by
/// default; `provenance_id` is enough to correlate the projection with a
/// producer-side evidence record without inflating context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextProvenance {
    pub id: String,
    pub source: ContextSource,
    pub workspace_id: String,
    pub session_id: String,
    pub request_id: Option<String>,
    pub content_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ContextFirewallError {
    #[error("context projection requires a non-empty workspace identity")]
    MissingWorkspaceIdentity,
    #[error("context projection requires a non-empty session identity")]
    MissingSessionIdentity,
    #[error("context projection contains an invalid raw artifact id")]
    InvalidArtifactId,
    #[error("context projection contains an invalid {field} identity")]
    InvalidIdentity { field: &'static str },
    #[error("context projection {field} identity exceeds the {max_bytes}-byte bound")]
    IdentityTooLarge {
        field: &'static str,
        max_bytes: usize,
    },
    #[error("context payload exceeds the {max_input_bytes}-byte input bound")]
    InputTooLarge { max_input_bytes: usize },
    #[error(
        "context budget exceeded: required {required_tokens} tokens, max {max_tokens}; raw fallback is blocked"
    )]
    BudgetExceeded {
        required_tokens: usize,
        max_tokens: usize,
    },
    #[error("duplicate context projection suppressed")]
    DuplicateSuppressed,
}

/// Stateful within one session/workspace: it remembers content fingerprints so
/// identical memory/tool/output payloads do not get injected twice. Callers may
/// discard the instance at a session boundary to reset the dedupe set.
#[derive(Debug, Clone)]
pub struct ContextFirewall {
    policy: ContextPolicy,
    seen: HashSet<String>,
    ledger: ContextLedger,
    turn_count: u64,
}

/// The budget engine and gateway are intentionally aliases of the one
/// firewall owner, not parallel counters or policy stores. They make the
/// architectural roles discoverable to callers while keeping one source of
/// truth for projection decisions.
pub type ContextBudgetEngine = ContextFirewall;
pub type ContextGateway = ContextFirewall;

/// Bound process-wide session state so a peer cannot grow the gateway map (or
/// each session's duplicate set) without limit by inventing identities. Evicted
/// entries only lose a dedupe optimization; raw artifacts and projections stay
/// owned by their durable stores.
const MAX_SESSION_GATEWAYS: usize = 256;
const MAX_DEDUPE_ENTRIES: usize = 4_096;
const MAX_CONTEXT_ID_BYTES: usize = 512;
const MAX_ARTIFACT_ID_BYTES: usize = 256;
const DEFAULT_CONTEXT_GATEWAY_TTL_SECONDS: u64 = 900;

fn context_gateway_ttl() -> Duration {
    std::env::var("KEEL_CONTEXT_GATEWAY_TTL_SECONDS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(|value| Duration::from_secs(value.clamp(1, 86_400)))
        .unwrap_or_else(|| Duration::from_secs(DEFAULT_CONTEXT_GATEWAY_TTL_SECONDS))
}

fn current_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

struct SessionGatewayEntry {
    gateway: ContextFirewall,
    last_seen: Instant,
}

#[derive(Default)]
struct SessionGateways {
    entries: HashMap<String, SessionGatewayEntry>,
    order: VecDeque<String>,
}

static SESSION_GATEWAYS: LazyLock<Mutex<SessionGateways>> =
    LazyLock::new(|| Mutex::new(SessionGateways::default()));

/// Project through the process-wide session/workspace gateway. Producers keep
/// ownership of raw artifacts; this shared owner only retains bounded
/// deduplication and measurement state so two calls in one identity cannot
/// silently inject the same dynamic payload twice.
pub fn project_scoped(
    policy: ContextPolicy,
    input: ProjectionInput,
) -> Result<ContextProjection, ContextFirewallError> {
    let workspace_id = input.workspace_id.trim();
    let session_id = input.session_id.trim();
    validate_identity_component(workspace_id, "workspace")?;
    validate_identity_component(session_id, "session")?;
    let key = format!("{}\0{}", workspace_id, session_id);
    let mut gateways = SESSION_GATEWAYS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let now = Instant::now();
    let ttl = context_gateway_ttl();
    gateways
        .entries
        .retain(|_, entry| now.saturating_duration_since(entry.last_seen) < ttl);
    let live_keys = gateways.entries.keys().cloned().collect::<HashSet<_>>();
    gateways
        .order
        .retain(|existing| live_keys.contains(existing));
    if !gateways.entries.contains_key(&key) {
        while gateways.entries.len() >= MAX_SESSION_GATEWAYS {
            let evicted = gateways
                .order
                .pop_front()
                .or_else(|| gateways.entries.keys().next().cloned());
            let Some(evicted) = evicted else { break };
            gateways.entries.remove(&evicted);
        }
        gateways.order.push_back(key.clone());
        gateways.entries.insert(
            key.clone(),
            SessionGatewayEntry {
                gateway: ContextFirewall::new(policy.clone()),
                last_seen: now,
            },
        );
    } else {
        // Keep eviction order deterministic and approximate LRU behavior. The
        // bounded map is a memory safeguard, not a source of authorization.
        gateways.order.retain(|existing| existing != &key);
        gateways.order.push_back(key.clone());
    }
    let entry = gateways
        .entries
        .get_mut(&key)
        .expect("session gateway inserted or already present");
    entry.last_seen = now;
    // The session gateway owns dedupe and measurement; callers own policy.
    // Refresh policy per projection so a narrow budget cannot leak across calls.
    entry.gateway.policy = policy;
    entry.gateway.project(input)
}

impl ContextFirewall {
    pub fn new(policy: ContextPolicy) -> Self {
        Self {
            policy,
            seen: HashSet::new(),
            ledger: ContextLedger::default(),
            turn_count: 0,
        }
    }

    pub fn policy(&self) -> &ContextPolicy {
        &self.policy
    }

    pub fn clear_dedupe(&mut self) {
        self.seen.clear();
    }

    pub fn ledger(&self) -> &ContextLedger {
        &self.ledger
    }

    pub fn metrics(&self) -> ContextTotals {
        self.ledger.totals()
    }

    pub fn record_provider_usage(&mut self, usage: ProviderUsage) {
        self.ledger.record_provider_usage(usage);
    }

    /// Advance the logical turn counter used by operator metrics. This is
    /// intentionally explicit because a firewall can serve several projections
    /// in one turn and must not guess turn boundaries from call count.
    pub fn begin_turn(&mut self) {
        self.turn_count = self.turn_count.saturating_add(1);
    }

    /// Project a payload through identity, sensitivity, measurement,
    /// duplicate, reduction, and budget stages. A failure never returns the
    /// original content, so callers cannot accidentally fall back to raw data.
    pub fn project(
        &mut self,
        input: ProjectionInput,
    ) -> Result<ContextProjection, ContextFirewallError> {
        let workspace_id = input.workspace_id.trim();
        if workspace_id.is_empty() {
            return Err(ContextFirewallError::MissingWorkspaceIdentity);
        }
        let session_id = input.session_id.trim();
        if session_id.is_empty() {
            return Err(ContextFirewallError::MissingSessionIdentity);
        }
        validate_identity_component(workspace_id, "workspace")?;
        validate_identity_component(session_id, "session")?;
        if let Some(raw_id) = input.raw_artifact_id.as_deref() {
            validate_artifact_id(raw_id)?;
        }
        if let Some(request_id) = input.request_id.as_deref() {
            validate_identity_component(request_id.trim(), "request")?;
        }
        if input.content.len() > self.policy.max_input_bytes {
            return Err(ContextFirewallError::InputTooLarge {
                max_input_bytes: self.policy.max_input_bytes,
            });
        }

        let (cleaned_content, masked) = if self.policy.neutralize_injection {
            let raw_id = input.raw_artifact_id.as_deref().unwrap_or("unavailable");
            let (cleaned, findings) = neutralize_injection(&input.content, raw_id);
            if input.raw_artifact_id.is_some() {
                (cleaned, !findings.is_empty())
            } else {
                // Preserve the injection guard marker and state when no raw-store
                // owner exists, so the missing recovery path stays explicit.
                (
                    cleaned.replace(
                        "raw available via keel raw unavailable",
                        "raw artifact unavailable",
                    ),
                    !findings.is_empty(),
                )
            }
        } else {
            (input.content.clone(), false)
        };
        let content = cleaned_content.trim().to_string();
        let normalized = normalize_for_identity(&content);
        // Request ids identify provenance, not new context. Exclude them so a
        // fresh transport id cannot re-inject the same payload.
        let dedupe_material = format!("{}\0{}\0{}", workspace_id, session_id, normalized);
        let content_hash = sha256_hex(dedupe_material.as_bytes());
        // Empty command/tool results carry no context and must not poison
        // dedupe or make a later empty result look firewall-blocked.
        let dedupe = !normalized.is_empty();
        if dedupe && self.seen.contains(&content_hash) {
            return Err(ContextFirewallError::DuplicateSuppressed);
        }

        let raw_tokens = TokenMeter::count_text(&content);
        let budget_tokens = self.policy.max_tokens;
        let (summary, truncated, omitted_items, reducer, summary_tokens) = if raw_tokens
            <= self.policy.max_tokens
        {
            (
                content,
                false,
                input.omitted_items,
                "identity-v1".to_string(),
                raw_tokens,
            )
        } else {
            let pointer = recovery_pointer(input.raw_artifact_id.as_deref());
            let pointer_tokens = TokenMeter::count_text(&pointer);
            if pointer_tokens >= self.policy.max_tokens {
                return Err(ContextFirewallError::BudgetExceeded {
                    required_tokens: pointer_tokens,
                    max_tokens: self.policy.max_tokens,
                });
            }
            let prefix_budget = self.policy.max_tokens - pointer_tokens;
            let prefix = semantic_reduce_to_tokens_with_count(&content, prefix_budget, raw_tokens);
            let summary = format!("{}{}", prefix.trim_end(), pointer);
            let summary_tokens = TokenMeter::count_text(&summary);
            if summary_tokens > self.policy.max_tokens {
                return Err(ContextFirewallError::BudgetExceeded {
                    required_tokens: summary_tokens,
                    max_tokens: self.policy.max_tokens,
                });
            }
            let kept_lines = prefix.lines().count();
            let total_lines = content.lines().count();
            let omitted = total_lines
                .saturating_sub(kept_lines)
                .saturating_add(input.omitted_items as usize)
                .min(u32::MAX as usize) as u32;
            (
                summary,
                true,
                omitted,
                "semantic-bounded-v1".to_string(),
                summary_tokens,
            )
        };

        let request_id = input
            .request_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let provenance_material = format!(
            "{}\0{}\0{}\0{}\0{}\0{}",
            workspace_id,
            session_id,
            input.source,
            normalized,
            request_id.unwrap_or(""),
            input.raw_artifact_id.as_deref().unwrap_or("")
        );
        let provenance_hash = sha256_hex(provenance_material.as_bytes());
        let provenance_id = format!("prov-fnv1a:{provenance_hash}");
        let projection_id = format!("projection-fnv1a:{provenance_hash}");
        let token_count = summary_tokens as u32;

        if dedupe {
            if self.seen.len() >= MAX_DEDUPE_ENTRIES {
                // Duplicate suppression is best-effort; reset only the bounded
                // set, leaving raw recovery and provenance unaffected.
                self.seen.clear();
            }
            self.seen.insert(content_hash);
        }
        let model_visible_tokens = summary_tokens;
        let saved_tokens = raw_tokens as isize - model_visible_tokens as isize;
        let reduction_ratio = if raw_tokens == 0 {
            0.0
        } else {
            saved_tokens.max(0) as f64 / raw_tokens as f64
        };
        self.ledger.record(ContextMeasurement {
            surface: input.source.as_str().to_string(),
            raw_input_tokens: raw_tokens,
            model_visible_input_tokens: model_visible_tokens,
            cached_input_tokens: match input.cache_class {
                CacheClass::Stable | CacheClass::Session => Some(model_visible_tokens),
                CacheClass::Dynamic | CacheClass::Volatile => None,
            },
            uncached_input_tokens: match input.cache_class {
                CacheClass::Stable | CacheClass::Session => None,
                CacheClass::Dynamic | CacheClass::Volatile => Some(model_visible_tokens),
            },
            output_tokens: 0,
            context_peak_tokens: model_visible_tokens,
            reduced_tokens: model_visible_tokens,
            saved_tokens,
            reduction_ratio,
            turn_count: self.turn_count,
            tool_count: u64::from(matches!(input.source, ContextSource::McpTool)),
            cache_hit_rate: None,
            cache_class: input.cache_class,
            budget_tokens,
            soft_budget_tokens: None,
            hard_budget_tokens: budget_tokens,
            tokenizer: "o200k_base".to_string(),
            measurement_timestamp: current_unix_seconds(),
            implementation_version: "context-firewall-v1".to_string(),
            status: if model_visible_tokens <= budget_tokens {
                "within_budget".to_string()
            } else {
                "exceeded".to_string()
            },
        });
        Ok(ContextProjection {
            id: projection_id,
            source: input.source,
            summary,
            token_count,
            raw_tokens: raw_tokens.min(u32::MAX as usize) as u32,
            visible_tokens: model_visible_tokens.min(u32::MAX as usize) as u32,
            budget: budget_tokens.min(u32::MAX as usize) as u32,
            reducer,
            policy_version: "context-firewall-v1".to_string(),
            state: if masked {
                ContextLifecycleState::Masked
            } else if truncated {
                ContextLifecycleState::Compressed
            } else {
                ContextLifecycleState::Visible
            },
            created_at: current_unix_seconds(),
            raw_artifact_id: input.raw_artifact_id,
            provenance_id,
            truncated,
            omitted_items,
            cache_class: input.cache_class,
        })
    }

    /// Same fail-closed projection path, with an explicit `None` for a
    /// duplicate. Other errors remain visible to the caller.
    pub fn project_optional(
        &mut self,
        input: ProjectionInput,
    ) -> Result<Option<ContextProjection>, ContextFirewallError> {
        match self.project(input) {
            Ok(projection) => Ok(Some(projection)),
            Err(ContextFirewallError::DuplicateSuppressed) => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Convenience wrapper for producers that have no custom `ProjectionInput`
    /// options yet.
    pub fn project_text(
        &mut self,
        source: ContextSource,
        content: impl Into<String>,
        raw_artifact_id: Option<impl Into<String>>,
        workspace_id: impl Into<String>,
        session_id: impl Into<String>,
    ) -> Result<ContextProjection, ContextFirewallError> {
        self.project(ProjectionInput::new(
            source,
            content,
            raw_artifact_id,
            workspace_id,
            session_id,
        ))
    }
}

impl Default for ContextFirewall {
    fn default() -> Self {
        Self::new(ContextPolicy::from_env())
    }
}

fn validate_artifact_id(raw_id: &str) -> Result<(), ContextFirewallError> {
    let trimmed = raw_id.trim();
    if trimmed.is_empty()
        || trimmed == "."
        || trimmed == ".."
        || trimmed.contains("..")
        || trimmed.contains('/')
        || trimmed.contains('\\')
        || trimmed.chars().any(char::is_whitespace)
    {
        return Err(ContextFirewallError::InvalidArtifactId);
    }
    if trimmed.len() > MAX_ARTIFACT_ID_BYTES {
        return Err(ContextFirewallError::IdentityTooLarge {
            field: "artifact",
            max_bytes: MAX_ARTIFACT_ID_BYTES,
        });
    }
    if trimmed.chars().any(|character| character.is_control()) {
        return Err(ContextFirewallError::InvalidArtifactId);
    }
    Ok(())
}

fn validate_identity_component(
    value: &str,
    field: &'static str,
) -> Result<(), ContextFirewallError> {
    if value.len() > MAX_CONTEXT_ID_BYTES {
        return Err(ContextFirewallError::IdentityTooLarge {
            field,
            max_bytes: MAX_CONTEXT_ID_BYTES,
        });
    }
    if value.chars().any(|character| character.is_control()) {
        return Err(ContextFirewallError::InvalidIdentity { field });
    }
    Ok(())
}

fn normalize_for_identity(content: &str) -> String {
    content
        .replace("\r\n", "\n")
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
}

fn recovery_pointer(raw_artifact_id: Option<&str>) -> String {
    match raw_artifact_id {
        Some(raw_id) => {
            format!("\n[keel] context truncated; recover raw artifact with `keel raw {raw_id}`")
        }
        None => "\n[keel] context truncated; raw artifact unavailable".to_string(),
    }
}

/// Deterministic semantic reducer used before generic truncation. It keeps
/// status/error/policy lines and a small tail of summary lines, then fills any
/// remaining space in source order. A single unstructured line falls back to
/// the exact UTF-8-safe tokenizer cut below.
#[cfg(test)]
fn semantic_reduce_to_tokens(text: &str, max_tokens: usize) -> String {
    let raw_tokens = TokenMeter::count_text(text);
    semantic_reduce_to_tokens_with_count(text, max_tokens, raw_tokens)
}

fn semantic_reduce_to_tokens_with_count(
    text: &str,
    max_tokens: usize,
    raw_tokens: usize,
) -> String {
    if max_tokens == 0 || text.is_empty() {
        return String::new();
    }
    if raw_tokens <= max_tokens {
        return text.to_string();
    }
    let lines = text.lines().collect::<Vec<_>>();
    if lines.len() <= 1 {
        return truncate_to_tokens(text, max_tokens);
    }

    let failure_lines = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| is_failure_identity_line(line))
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    if let Some(&index) = failure_lines.first() {
        let failure_line = lines[index];
        if TokenMeter::count_text(failure_line) > max_tokens {
            return truncate_to_tokens(failure_line, max_tokens);
        }
    }

    let mut priority = failure_lines;
    priority.extend(
        lines
            .iter()
            .enumerate()
            .filter(|(_, line)| is_high_signal_line(line) && !is_failure_identity_line(line))
            .map(|(index, _)| index),
    );
    priority.extend(0..lines.len().min(2));
    priority.extend(lines.len().saturating_sub(2)..lines.len());
    priority.extend(0..lines.len());

    let mut selected = std::collections::BTreeSet::new();
    for index in priority {
        if !selected.insert(index) {
            continue;
        }
        let candidate = selected
            .iter()
            .map(|selected_index| lines[*selected_index])
            .collect::<Vec<_>>()
            .join("\n");
        if TokenMeter::count_text(&candidate) > max_tokens {
            selected.remove(&index);
        }
    }
    let reduced = selected
        .iter()
        .map(|index| lines[*index])
        .collect::<Vec<_>>()
        .join("\n");
    if reduced.is_empty() {
        truncate_to_tokens(text, max_tokens)
    } else {
        reduced
    }
}

fn is_high_signal_line(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    [
        "error",
        "fail",
        "panic",
        "fatal",
        "exception",
        "denied",
        "timeout",
        "exit",
        "status",
        "policy",
        "security",
        "blocked",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn is_failure_identity_line(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    let starts_with_failure_marker = [
        "error",
        "failure",
        "fatal",
        "panic",
        "exception",
        "denied",
        "timeout",
        "blocked",
    ]
    .iter()
    .any(|marker| {
        lower.strip_prefix(marker).is_some_and(|remainder| {
            remainder.is_empty()
                || remainder
                    .chars()
                    .next()
                    .is_some_and(|character| !character.is_ascii_alphanumeric())
        })
    });
    let contains_actionable_failure = ["failed", "timed out", "not found", "assert", "traceback"]
        .iter()
        .any(|needle| lower.contains(needle));
    starts_with_failure_marker || contains_actionable_failure
}

/// Return the longest UTF-8-safe prefix whose exact tokenizer count is within
/// `max_tokens`. This is deterministic and avoids a generative summarizer.
fn truncate_to_tokens(text: &str, max_tokens: usize) -> String {
    TokenMeter::prefix_to_token_budget(text, max_tokens)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncation_is_utf8_safe_and_token_bounded() {
        let text = "こんにちは世界 — diagnostic output";
        let result = truncate_to_tokens(text, 3);
        assert!(result.is_char_boundary(result.len()));
        assert!(TokenMeter::count_text(&result) <= 3);
    }

    #[test]
    fn provenance_is_stable_for_the_same_identity() {
        let mut first = ContextFirewall::new(ContextPolicy::with_max_tokens(50));
        let mut second = ContextFirewall::new(ContextPolicy::with_max_tokens(50));
        let input = ProjectionInput::new(
            ContextSource::Warning,
            "warning: one",
            Some("raw-warning"),
            "workspace",
            "session",
        )
        .with_request_id("request");
        let left = first.project(input.clone()).expect("first projection");
        let right = second.project(input).expect("second projection");
        assert_eq!(left.id, right.id);
        assert_eq!(left.provenance_id, right.provenance_id);
    }

    #[test]
    fn provenance_and_projection_ids_use_sha256_with_legacy_prefixes() {
        let mut firewall = ContextFirewall::new(ContextPolicy::with_max_tokens(50));
        let input = ProjectionInput::new(
            ContextSource::Warning,
            "warning: one",
            Some("raw-warning"),
            "workspace",
            "session",
        )
        .with_request_id("request");
        let projection = firewall.project(input).expect("projection");
        let material = "workspace\0session\0warning\0warning: one\0request\0raw-warning";
        let digest = sha256_hex(material.as_bytes());
        assert_eq!(projection.provenance_id, format!("prov-fnv1a:{digest}"));
        assert_eq!(projection.id, format!("projection-fnv1a:{digest}"));
        assert_eq!(digest.len(), 64);
    }

    #[test]
    fn semantic_reducer_keeps_failure_and_tail_evidence() {
        let mut firewall = ContextFirewall::new(ContextPolicy::with_max_tokens(60));
        let content = (0..80)
            .map(|index| {
                if index == 40 {
                    "error: failed test at src/lib.rs:40".to_string()
                } else if index == 79 {
                    "exit status: 1".to_string()
                } else {
                    format!("noise line {index}")
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        let projection = firewall
            .project(ProjectionInput::new(
                ContextSource::Error,
                content,
                Some("raw-error"),
                "workspace",
                "session",
            ))
            .expect("failure projection remains bounded");
        assert!(projection.summary.contains("failed test"));
        assert!(projection.summary.contains("exit status"));
        assert_eq!(projection.reducer, "semantic-bounded-v1");
        assert!(projection.raw_tokens > projection.visible_tokens);
        assert_eq!(projection.budget, 60);
    }

    #[test]
    fn semantic_reducer_prioritizes_failure_identity_at_a_tight_budget() {
        let failure_line = "error: critical-test-42 failed at src/lib.rs:40";
        let content = [
            "setup output that would consume the available reduction budget",
            "another ordinary line that must not displace the failure identity",
            failure_line,
            "exit status: 1",
        ]
        .join("\n");
        let budget = TokenMeter::count_text(failure_line);
        let reduced = semantic_reduce_to_tokens(&content, budget);
        assert!(reduced.contains("critical-test-42"), "reduced={reduced:?}");
        assert!(reduced.contains("failed"), "reduced={reduced:?}");
        assert!(TokenMeter::count_text(&reduced) <= budget);
    }

    #[test]
    fn semantic_reducer_bounds_an_oversized_failure_identity_without_dropping_it() {
        let content = "error: critical-test-42 failed after a very long diagnostic explanation";
        let reduced = semantic_reduce_to_tokens(content, 2);
        assert!(reduced.starts_with("error"), "reduced={reduced:?}");
        assert!(TokenMeter::count_text(&reduced) <= 2);
    }

    #[test]
    fn transient_ledger_state_keeps_a_bounded_tail() {
        let mut ledger = ContextLedger::default();
        for index in 0..(MAX_LEDGER_MEASUREMENTS + 17) {
            ledger.record(ContextMeasurement {
                surface: format!("surface-{index}"),
                raw_input_tokens: index,
                model_visible_input_tokens: index,
                cached_input_tokens: None,
                uncached_input_tokens: Some(index),
                output_tokens: 0,
                context_peak_tokens: index,
                reduced_tokens: index,
                saved_tokens: 0,
                reduction_ratio: 0.0,
                turn_count: index as u64,
                tool_count: 0,
                cache_hit_rate: None,
                cache_class: CacheClass::Dynamic,
                budget_tokens: 1_000,
                soft_budget_tokens: None,
                hard_budget_tokens: 1_000,
                tokenizer: "o200k_base".to_string(),
                measurement_timestamp: 0,
                implementation_version: "test".to_string(),
                status: "within_budget".to_string(),
            });
        }
        for index in 0..(MAX_PROVIDER_USAGE + 17) {
            ledger.record_provider_usage(ProviderUsage {
                provider: format!("provider-{index}"),
                cached_input_tokens: None,
                uncached_input_tokens: Some(index),
                output_tokens: Some(0),
            });
        }
        assert_eq!(ledger.measurements.len(), MAX_LEDGER_MEASUREMENTS);
        assert_eq!(ledger.provider_usage.len(), MAX_PROVIDER_USAGE);
        assert_eq!(ledger.measurements[0].surface, "surface-17");
        assert_eq!(ledger.provider_usage[0].provider, "provider-17");
    }

    #[test]
    fn expired_scoped_gateway_is_replaced_instead_of_reusing_dedupe_state() {
        let key = format!(
            "context-expiry-workspace-{}\0context-expiry-session-{}",
            std::process::id(),
            std::process::id()
        );
        let workspace = format!("context-expiry-workspace-{}", std::process::id());
        let session = format!("context-expiry-session-{}", std::process::id());
        let input = ProjectionInput::new(
            ContextSource::CommandOutput,
            "expired gateway evidence",
            None::<String>,
            &workspace,
            &session,
        );
        let mut expired = ContextFirewall::default();
        expired.project(input.clone()).expect("seed dedupe state");
        {
            let mut gateways = SESSION_GATEWAYS.lock().expect("session gateway lock");
            gateways.entries.insert(
                key.clone(),
                SessionGatewayEntry {
                    gateway: expired,
                    last_seen: Instant::now() - context_gateway_ttl() - Duration::from_secs(1),
                },
            );
            gateways.order.push_back(key.clone());
        }
        assert!(project_scoped(ContextPolicy::default(), input).is_ok());
        let mut gateways = SESSION_GATEWAYS.lock().expect("session gateway lock");
        gateways.entries.remove(&key);
        gateways.order.retain(|existing| existing != &key);
    }

    #[test]
    fn oversized_context_identity_is_rejected_before_projection() {
        let mut firewall = ContextFirewall::default();
        let result = firewall.project(ProjectionInput::new(
            ContextSource::Memory,
            "bounded",
            Some("a".repeat(MAX_ARTIFACT_ID_BYTES + 1)),
            "workspace",
            "session",
        ));
        assert!(matches!(
            result,
            Err(ContextFirewallError::IdentityTooLarge {
                field: "artifact",
                ..
            })
        ));
    }
}
