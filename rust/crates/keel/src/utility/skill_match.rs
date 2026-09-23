//! Purpose: Deterministic prompt→skill matcher. Reads the installed
//!   `~/.claude/skills/<name>/SKILL.md` frontmatter, builds an IDF-weighted
//!   keyword model per skill, and decides whether a user prompt is a *strong,
//!   distinctive* match for exactly one skill.
//! Caller: `runner::hook_lifecycle::run_hook_user_prompt_submit` — on a strong,
//!   distinctive match it injects a bounded slice of the matched skill's *actual
//!   body* into the per-prompt context (see `skill_inline_brief`), so the
//!   operative guidance lands whether or not the gateway model chooses to honor
//!   a `Skill("<name>")` tool call. The match itself is the gate; the inline
//!   brief is what makes it model-independent.
//! Dependencies: std::fs, std::path, std::collections; crate::runtime for the
//!   skills directory resolver.
//! Main Functions: match_skill_for_prompt, load_skill_terms,
//!   score_prompt_against_skills, skill_inline_brief.
//! Side Effects: Reads SKILL.md files under the installed skills directory.
//!   Never writes. Any IO failure degrades to "no match" so the hook fails
//!   open to its generic reminder.
//!
//! Why deterministic, not model-driven: this runs inside a UserPromptSubmit
//! hook on every turn. It must be fast, dependency-free, and incapable of
//! mis-routing a generic prompt. The matcher is intentionally conservative —
//! it stays silent unless one skill both clears an absolute score floor and
//! beats the runner-up by a margin on a *distinctive* (corpus-rare) token. A
//! silent matcher is correct; a confidently-wrong one is worse than the
//! existing generic nudge.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::proxy::token_meter::TokenMeter;
use crate::runtime::{safe_path_segment, skills_directory, state_directory};

const SKILL_CATALOG_CACHE_VERSION: u32 = 3;
const SKILL_CATALOG_CACHE_FILE: &str = "skill-catalog-v2.json";
const SKILL_CATALOG_DEFAULT_INTEGRITY_INTERVAL_SECS: u64 = 300;
// J02 content-hash routing cache (300s TTL): prompt plus skills-listing
// fingerprint. Catalog edits miss; discovery failure misses fail-open.
pub const SKILL_ROUTING_CACHE_TTL_SECS: u64 = 300;
const SKILL_ROUTING_CACHE_MAX_ENTRIES: usize = 512;

#[derive(Debug, Clone)]
struct SkillRoutingCacheEntry {
    decision: Option<SkillSelectionDecision>,
    expires_at_secs: u64,
}

struct SkillRoutingCache {
    entries: HashMap<String, SkillRoutingCacheEntry>,
}

static SKILL_ROUTING_CACHE: std::sync::LazyLock<
    std::sync::Mutex<SkillRoutingCache>,
    fn() -> std::sync::Mutex<SkillRoutingCache>,
> = std::sync::LazyLock::new(|| {
    std::sync::Mutex::new(SkillRoutingCache {
        entries: HashMap::new(),
    })
});
static SKILL_ROUTING_CACHE_HITS: AtomicU64 = AtomicU64::new(0);
static SKILL_ROUTING_CACHE_MISSES: AtomicU64 = AtomicU64::new(0);
static SKILL_ROUTING_CACHE_RECOMPUTES: AtomicU64 = AtomicU64::new(0);

/// Observable cache counters: every lookup records exactly one hit or miss,
/// every full recompute records one recompute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SkillRoutingCacheStats {
    pub hits: u64,
    pub misses: u64,
    pub recomputes: u64,
    pub entries: usize,
}

pub fn skill_routing_cache_stats() -> SkillRoutingCacheStats {
    SkillRoutingCacheStats {
        hits: SKILL_ROUTING_CACHE_HITS.load(Ordering::Relaxed),
        misses: SKILL_ROUTING_CACHE_MISSES.load(Ordering::Relaxed),
        recomputes: SKILL_ROUTING_CACHE_RECOMPUTES.load(Ordering::Relaxed),
        entries: SKILL_ROUTING_CACHE
            .lock()
            .map(|cache| cache.entries.len())
            .unwrap_or(0),
    }
}

/// Test hook: drop every cached routing decision, so a test can prove the
/// catalog fingerprint is what invalidates the cache in production.
#[cfg(test)]
pub(crate) fn clear_skill_routing_cache() {
    if let Ok(mut cache) = SKILL_ROUTING_CACHE.lock() {
        cache.entries.clear();
    }
}

fn skill_routing_cache_key(skills_dir: &Path, prompt: &str) -> Option<String> {
    let files = discover_skill_files(skills_dir)?;
    let mut material = String::new();
    material.push_str(prompt.trim());
    material.push('\0');
    for file in &files {
        material.push_str(&file.name);
        material.push('\0');
        material.push_str(&file.size.to_string());
        material.push('\0');
        material.push_str(&file.modified_at_nanos.to_string());
        material.push('\n');
    }
    Some(crate::utility::hashing::fnv1a64_hex(&material))
}

fn skill_routing_cache_get(key: &str, now_secs: u64) -> Option<Option<SkillSelectionDecision>> {
    let cache = SKILL_ROUTING_CACHE.lock().ok()?;
    let entry = cache.entries.get(key)?;
    if entry.expires_at_secs <= now_secs {
        return None;
    }
    SKILL_ROUTING_CACHE_HITS.fetch_add(1, Ordering::Relaxed);
    Some(entry.decision.clone())
}

fn skill_routing_cache_put(key: String, decision: &Option<SkillSelectionDecision>, now_secs: u64) {
    let mut cache = match SKILL_ROUTING_CACHE.lock() {
        Ok(cache) => cache,
        Err(poisoned) => poisoned.into_inner(),
    };
    if cache.entries.len() >= SKILL_ROUTING_CACHE_MAX_ENTRIES {
        cache
            .entries
            .retain(|_, entry| entry.expires_at_secs > now_secs);
        if cache.entries.len() >= SKILL_ROUTING_CACHE_MAX_ENTRIES {
            cache.entries.clear();
        }
    }
    cache.entries.insert(
        key,
        SkillRoutingCacheEntry {
            decision: decision.clone(),
            expires_at_secs: now_secs.saturating_add(SKILL_ROUTING_CACHE_TTL_SECS),
        },
    );
    SKILL_ROUTING_CACHE_RECOMPUTES.fetch_add(1, Ordering::Relaxed);
}

fn skill_routing_cache_miss() {
    SKILL_ROUTING_CACHE_MISSES.fetch_add(1, Ordering::Relaxed);
}

// J08 pending match ledger, reconciled at session end (cited is helpful).
// All IO is fail-open.
const SKILL_MATCH_PENDING_FILE: &str = "skill-match-pending.json";
const SKILL_MATCH_PENDING_MAX: usize = 500;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct PendingSkillMatch {
    skill_name: String,
    predicted_confidence: f64,
    matched_at_secs: u64,
}

/// Outcome of a staged routing decision. `Unknown` is a first-class state rather
/// than a failure: a skill that was used silently leaves no citation, and
/// recording that silence as a mis-route is what taught this loop false
/// negatives. Only `Used` and `Unused` carry evidence and may be recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoutingOutcome {
    /// Positive evidence the routed skill was followed.
    Used,
    /// Positive evidence a competing candidate was followed instead.
    Unused,
    /// No evidence either way; excluded from success rate, calibration, priors.
    Unknown,
}

fn skill_match_pending_path(claude_home: &Path) -> PathBuf {
    state_directory(claude_home).join(SKILL_MATCH_PENDING_FILE)
}

/// Best-effort exclusive lock for a state ledger, keyed by the ledger filename.
/// The caller decides what a failure means: writers proceed unlocked (a rare
/// lost update beats a dropped record) and reconcilers defer the whole ledger.
fn lock_state_ledger(
    claude_home: &Path,
    ledger_file: &str,
) -> std::io::Result<crate::utility::file_lock::ExclusiveLock> {
    let lock_name = format!("{ledger_file}.lock");
    crate::utility::file_lock::lock_exclusive(&state_directory(claude_home), &lock_name)
}

fn record_pending_skill_match(claude_home: &Path, skill_name: &str, predicted_confidence: f64) {
    // why: a contended lock leaves the append unlocked rather than dropping it.
    let _guard = lock_state_ledger(claude_home, SKILL_MATCH_PENDING_FILE).ok();
    let path = skill_match_pending_path(claude_home);
    let mut pending: Vec<PendingSkillMatch> = fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    pending.push(PendingSkillMatch {
        skill_name: skill_name.to_string(),
        predicted_confidence: predicted_confidence.clamp(0.0, 1.0),
        matched_at_secs: now_unix_secs(),
    });
    if pending.len() > SKILL_MATCH_PENDING_MAX {
        let drain = pending.len() - SKILL_MATCH_PENDING_MAX;
        pending.drain(..drain);
    }
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(text) = serde_json::to_string(&pending) {
        let _ = fs::write(&path, text);
    }
}

/// Reconcile staged routing decisions (cited records success); consumes the ledger.
pub fn reconcile_skill_match_outcomes(claude_home: &Path) -> usize {
    // why: defer the ledger when the lock is unavailable; reconciling unlocked
    // could double-count outcomes another process is already scoring.
    let Ok(_guard) = lock_state_ledger(claude_home, SKILL_MATCH_PENDING_FILE) else {
        return 0;
    };
    let path = skill_match_pending_path(claude_home);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(_) => return 0,
    };
    let pending: Vec<PendingSkillMatch> = serde_json::from_str(&text).unwrap_or_default();
    if pending.is_empty() {
        return 0;
    }
    let candidates: HashSet<String> = pending
        .iter()
        .map(|entry| entry.skill_name.clone())
        .collect();
    let cited = cited_skill_names(claude_home, &candidates);
    // A citation for any candidate is evidence the session followed one routing
    // decision; the rest of that batch are misses, and silence is still unknown.
    let any_cited = !cited.is_empty();
    let mut reconciled = 0usize;
    for entry in &pending {
        let outcome = if cited.contains(&entry.skill_name) {
            RoutingOutcome::Used
        } else if any_cited {
            RoutingOutcome::Unused
        } else {
            RoutingOutcome::Unknown
        };
        if outcome == RoutingOutcome::Unknown {
            continue;
        }
        if crate::utility::decision::record_skill_session_outcome(
            claude_home,
            &entry.skill_name,
            outcome == RoutingOutcome::Used,
            entry.predicted_confidence,
        )
        .is_ok()
        {
            reconciled += 1;
        }
    }
    let _ = fs::remove_file(&path);
    reconciled
}

/// Skill names from `candidates` cited in recent observation signatures or
/// details (case-insensitive). Shared by routing and composition reconcile.
fn cited_skill_names(claude_home: &Path, candidates: &HashSet<String>) -> HashSet<String> {
    let mut cited = HashSet::new();
    if candidates.is_empty() {
        return cited;
    }
    if let Ok(rows) = crate::runner::observation::iter_recent_rows_at(claude_home, 1) {
        for row in rows {
            let haystack = format!("{}\n{}", row.signature, row.detail).to_ascii_lowercase();
            for name in candidates {
                if haystack.contains(&name.to_ascii_lowercase()) {
                    cited.insert(name.clone());
                }
            }
        }
    }
    cited
}

/// Staged composition decision awaiting session-end outcome reconcile.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct PendingComposition {
    kind: String,
    skills: Vec<String>,
    considered: Vec<String>,
    confidence: f64,
    matched_at_secs: u64,
}

const COMPOSITION_PENDING_FILE: &str = "composition-pending.json";
const COMPOSITION_PENDING_MAX: usize = 200;

fn composition_pending_path(claude_home: &Path) -> PathBuf {
    state_directory(claude_home).join(COMPOSITION_PENDING_FILE)
}

fn record_pending_composition(
    claude_home: &Path,
    kind: &str,
    skills: &[String],
    considered: &[String],
    confidence: f64,
) {
    // why: a contended lock leaves the append unlocked rather than dropping it.
    let _guard = lock_state_ledger(claude_home, COMPOSITION_PENDING_FILE).ok();
    let path = composition_pending_path(claude_home);
    let mut pending: Vec<PendingComposition> = fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    pending.push(PendingComposition {
        kind: kind.to_string(),
        skills: skills.to_vec(),
        considered: considered.to_vec(),
        confidence: confidence.clamp(0.0, 1.0),
        matched_at_secs: now_unix_secs(),
    });
    if pending.len() > COMPOSITION_PENDING_MAX {
        let drain = pending.len() - COMPOSITION_PENDING_MAX;
        pending.drain(..drain);
    }
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(text) = serde_json::to_string(&pending) {
        let _ = fs::write(&path, text);
    }
}

/// Aggregate composition accuracy for the calibration report.
#[derive(Debug, Clone, Copy, Default, serde::Serialize, serde::Deserialize)]
struct CompositionCalibration {
    total: usize,
    correct: usize,
    compose_total: usize,
    compose_correct: usize,
}

fn composition_calibration_file(claude_home: &Path) -> PathBuf {
    state_directory(claude_home).join("composition-calibration.json")
}

fn load_composition_calibration(claude_home: &Path) -> CompositionCalibration {
    fs::read_to_string(composition_calibration_file(claude_home))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// Reconcile staged composition decisions: compose needs every chosen skill
/// cited, single needs its skill cited, generic needs none considered cited.
pub fn reconcile_composition_outcomes(claude_home: &Path) -> usize {
    // why: defer the ledger when the lock is unavailable; reconciling unlocked
    // could double-count outcomes another process is already scoring.
    let Ok(_guard) = lock_state_ledger(claude_home, COMPOSITION_PENDING_FILE) else {
        return 0;
    };
    let path = composition_pending_path(claude_home);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(_) => return 0,
    };
    let pending: Vec<PendingComposition> = serde_json::from_str(&text).unwrap_or_default();
    if pending.is_empty() {
        return 0;
    }
    let mut names = HashSet::new();
    for entry in &pending {
        names.extend(entry.skills.iter().cloned());
        names.extend(entry.considered.iter().cloned());
    }
    let cited = cited_skill_names(claude_home, &names);
    let mut aggregate = load_composition_calibration(claude_home);
    let mut reconciled = 0usize;
    for entry in &pending {
        let correct = match entry.kind.as_str() {
            "compose" => !entry.skills.is_empty() && entry.skills.iter().all(|s| cited.contains(s)),
            "single" => entry.skills.first().is_some_and(|s| cited.contains(s)),
            _ => !entry.considered.iter().any(|s| cited.contains(s)),
        };
        aggregate.total = aggregate.total.saturating_add(1);
        if correct {
            aggregate.correct = aggregate.correct.saturating_add(1);
        }
        if entry.kind == "compose" {
            aggregate.compose_total = aggregate.compose_total.saturating_add(1);
            if correct {
                aggregate.compose_correct = aggregate.compose_correct.saturating_add(1);
            }
        }
        reconciled += 1;
    }
    if let Some(parent) = composition_calibration_file(claude_home).parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(text) = serde_json::to_string_pretty(&aggregate) {
        let _ = fs::write(composition_calibration_file(claude_home), text);
    }
    let _ = fs::remove_file(&path);
    reconciled
}

/// Composition precision for the calibration report.
pub fn composition_calibration_stats(claude_home: &Path) -> (usize, f64, usize, f64) {
    let aggregate = load_composition_calibration(claude_home);
    let precision = if aggregate.total > 0 {
        aggregate.correct as f64 / aggregate.total as f64
    } else {
        1.0
    };
    let compose_precision = if aggregate.compose_total > 0 {
        aggregate.compose_correct as f64 / aggregate.compose_total as f64
    } else {
        1.0
    };
    (
        aggregate.total,
        precision,
        aggregate.compose_total,
        compose_precision,
    )
}

/// Score floor as a fraction of `ln(corpus_size)`. The floor must scale with
/// corpus size because IDF does: a token present in exactly one skill scores
/// `ln(N)`, which is ~3.56 for the real ~35-skill install but only ~1.95 for a
/// small test corpus. An *absolute* floor calibrated for one corpus size
/// silently mis-fires on another. Expressing it as a fraction of `ln(N)` keeps
/// the bar at "roughly one distinctive token" regardless of how many skills are
/// installed. At 0.75: a unique (df=1) or near-unique (df=2) token clears,
/// while a borderline df=3 token needs a name-boost or a second hit to qualify.
///
/// NOTE (investigated during the s3 skill-loading work): lowering this floor in
/// isolation is inert. The only band it would open is a lone df=3 token, but a
/// df=3 token is shared by three skills, so such a prompt is a three-way tie that
/// the `DISTINCTIVENESS_MARGIN` guard rejects anyway. To beat the margin over a
/// sibling holding the same token the winner must already exceed the 0.75 floor.
/// The real lever for "skills don't load as needed" is therefore the curated
/// keyword tier (`CURATED_SKILL_TRIGGERS`), which routes the high-frequency,
/// non-distinctive specialist vocabulary the statistical tier correctly declines
/// to guess on — not a floor change that the margin guard makes a no-op.
const MIN_SCORE_FACTOR: f64 = 0.75;

/// The winner must beat the runner-up by this factor. Prevents firing when two
/// skills are near-ties (ambiguous prompt) — exactly the case where naming one
/// would be a coin-flip mis-route.
const DISTINCTIVENESS_MARGIN: f64 = 1.25;

/// A token is "distinctive" when it appears in at most this many skills. The
/// winning overlap must include at least one distinctive token, so a pile of
/// generic words can never trigger a match on its own.
const DISTINCTIVE_DF_MAX: usize = 3;

/// Skill-name tokens are the canonical handle for a skill, so a prompt that
/// uses them is a stronger signal than an incidental description-word hit.
const NAME_TOKEN_BOOST: f64 = 1.5;

/// Hard cap on the inline brief injected into per-prompt context. Skill bodies
/// run ~10-15 KB; we inject the description plus the opening body and stop at
/// this many bytes (rounded out to the next line boundary). The cap keeps the
/// per-prompt input-token cost bounded — the full skill is still one
/// `Skill("<name>")` call away for the model that wants the rest — while
/// guaranteeing the operative guidance lands even if the gateway model never
/// makes that call. ~2400 bytes is roughly 600 tokens: enough for a skill's
/// purpose and its first one or two discipline sections, small enough to pay
/// every prompt that distinctively matches.
const INLINE_BRIEF_MAX_BYTES: usize = 2400;

/// Independent skill-surface budgets. MCP list/page budgets must not silently
/// govern skill activation or reference loading.
pub(crate) const SKILL_CATALOG_DEFAULT_TOKENS: usize = 1_200;
pub(crate) const SKILL_S1_TARGET_TOKENS: usize = 2_500;
pub(crate) const SKILL_S1_HARD_TOKENS: usize = 5_000;
pub(crate) const SKILL_S2_DEFAULT_TOKENS: usize = 2_500;
pub(crate) const SKILL_S2_HARD_TOKENS: usize = 5_000;
pub(crate) const SKILL_RESOURCE_MAX_BYTES: usize = 2 * 1024 * 1024;
const DEFAULT_SKILL_ACTIVATION_BUDGET_TOKENS: usize = SKILL_S1_HARD_TOKENS;
const DEFAULT_SKILL_HISTORICAL_SUCCESS: f64 = 0.5;
const COST_WEIGHT: f64 = 0.25;
const REDUNDANCY_WEIGHT: f64 = 0.20;
const CRITICALITY_WEIGHT: f64 = 0.20;
const SUCCESS_WEIGHT: f64 = 0.20;

/// Tokenized term model for one installed skill.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SkillTerms {
    pub name: String,
    /// Every token drawn from name + description + when_to_use.
    pub all_tokens: HashSet<String>,
    /// Tokens drawn from the skill *name* only (e.g. `stripe`, `integration`).
    pub name_tokens: HashSet<String>,
}

/// A confident, distinctive match.
#[derive(Debug, Clone, PartialEq)]
pub struct SkillMatch {
    pub name: String,
    pub score: f64,
}

/// S0 metadata used by the bounded skill selector and catalog projection.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SkillSelectionMetadata {
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub dependencies: Vec<String>,
    #[serde(default)]
    pub activation_cost_tokens: usize,
    #[serde(default)]
    pub task_criticality: f64,
    #[serde(default = "default_historical_success")]
    pub historical_success: f64,
}

impl Default for SkillSelectionMetadata {
    fn default() -> Self {
        Self {
            capabilities: Vec::new(),
            version: "unversioned".to_string(),
            dependencies: Vec::new(),
            activation_cost_tokens: 1,
            task_criticality: 0.5,
            historical_success: DEFAULT_SKILL_HISTORICAL_SUCCESS,
        }
    }
}

fn default_historical_success() -> f64 {
    DEFAULT_SKILL_HISTORICAL_SUCCESS
}

/// Auditable outcome of cost-aware routing. The public `SkillMatch` remains
/// intentionally small for callers that only need a name and relevance score.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SkillSelectionDecision {
    pub name: String,
    pub relevance: f64,
    pub utility: f64,
    pub confidence: f64,
    pub estimated_tokens: usize,
    pub activation_budget_tokens: usize,
    pub redundancy: f64,
    pub task_criticality: f64,
    pub historical_success: f64,
    pub reason: String,
}

/// Resolve the installed skills directory for `claude_home`, load every skill's
/// term model, and return the single distinctive match for `prompt` — or `None`
/// when no skill clears the bar. `None` is the common, correct case for
/// generic prompts; the caller falls back to its generic reminder.
pub fn match_skill_for_prompt(claude_home: &Path, prompt: &str) -> Option<SkillMatch> {
    match_skill_for_prompt_with_details(claude_home, prompt).map(|decision| SkillMatch {
        name: decision.name,
        score: decision.relevance,
    })
}

/// Curated confirmation for the J03 confidence gate: a sub-0.60 statistical
/// winner still routes when the independent curated phrase tier names the
/// same skill and that skill has never recorded a failure. Recorded failures
/// always keep the gate shut: learned evidence beats phrase agreement.
fn confirmed_by_curated_tier(prompt: &str, skill_name: &str, claude_home: &Path) -> bool {
    if curated_skill_for_prompt(prompt) != Some(skill_name) {
        return false;
    }
    let record = crate::utility::decision::load_skill_calibration(claude_home, skill_name);
    if record.epoch < crate::utility::calibration::OUTCOME_SEMANTICS_EPOCH {
        // Pre-epoch evidence counted a silence as a failure, so it carries no
        // authority here either: the curated tier stands on its own.
        return true;
    }
    // why: a successful record is not a reason to withhold confirmation. That
    // read counted every use, so a 14-of-14 skill was treated as a failed one.
    let (total, correct) = record.skill_totals();
    total == correct
}

/// Resolve and record a cost-aware activation decision for an installed corpus.
/// The returned details are bounded S0 telemetry; skill bodies remain on demand.
pub fn match_skill_for_prompt_with_details(
    claude_home: &Path,
    prompt: &str,
) -> Option<SkillSelectionDecision> {
    // J03 gate: silence sub-0.60 matches unless curated-confirmed on clean history.
    fn apply_confidence_gate(
        decision: Option<SkillSelectionDecision>,
        prompt: &str,
        claude_home: &Path,
    ) -> Option<SkillSelectionDecision> {
        // The learned expert is the only evidence trained on real phrasing, so it
        // speaks only where the term model and the curated tier fall silent.
        // why: the head is cached per artifact version; parsing and indexing it
        // per prompt was the router's dominant cost.
        let verdict = || -> Option<(String, f64)> {
            let head = crate::utility::lexical_experts::head(
                &crate::utility::lexical_experts::artifact_path(claude_home),
            )?;
            let accept = head.accept();
            let ranked = head.rank(prompt)?;
            let (name, confidence) =
                crate::utility::lexical_experts::select_installed(&ranked, accept, &|name| {
                    resolve_skill_path(claude_home, name).is_some_and(|path| path.is_file())
                })?;
            Some((name.to_string(), confidence))
        };
        match decision {
            Some(found) if found.confidence < 0.60 => {
                if confirmed_by_curated_tier(prompt, &found.name, claude_home) {
                    return Some(found);
                }
                // Measured: a weak term match for one skill used to veto a head
                // answer for another, and that deleted 3 of 19 correct answers on
                // the host corpus. The head already clears its own fitted accept
                // point inside verdict(), so it speaks and the term match does not.
                let (name, confidence) = verdict()?;
                Some(SkillSelectionDecision {
                    name,
                    relevance: confidence,
                    utility: confidence,
                    confidence,
                    ..found
                })
            }
            None => {
                let (name, confidence) = verdict()?;
                Some(SkillSelectionDecision {
                    name,
                    relevance: confidence,
                    utility: confidence,
                    confidence,
                    estimated_tokens: 1,
                    activation_budget_tokens: 0,
                    redundancy: 0.0,
                    task_criticality: 0.5,
                    historical_success: DEFAULT_SKILL_HISTORICAL_SUCCESS,
                    reason: "learned lexical expert".to_string(),
                })
            }
            gated => gated,
        }
    }
    if prompt.trim().is_empty() {
        return None;
    }
    // J02 fast path: identical prompts over unchanged skills hit the cache.
    let skills_dir = skills_directory(claude_home);
    let cache_key = skill_routing_cache_key(&skills_dir, prompt);
    if let Some(key) = &cache_key {
        if let Some(cached) = skill_routing_cache_get(key, now_unix_secs()) {
            let gated = apply_confidence_gate(cached, prompt, claude_home);
            if let Some(found) = &gated {
                crate::utility::skill_usage::record_skill_match(claude_home, &found.name);
                record_pending_skill_match(claude_home, &found.name, found.confidence);
            }
            return gated;
        }
        skill_routing_cache_miss();
    }
    let mut corpus = load_skill_corpus_for_home(claude_home);
    if corpus.terms.is_empty() {
        return None;
    }
    let prior_store = crate::utility::decision::load_prior_store(claude_home);
    for entry in &mut corpus.catalog {
        entry.use_count = crate::utility::skill_usage::skill_use_count(claude_home, &entry.name);
        entry.historical_success =
            crate::utility::skill_usage::skill_success_rate(claude_home, &entry.name);
        entry.is_quarantined = prior_store.is_quarantined(&entry.name);
    }
    let resolved = resolve_skill_selection(prompt, &corpus.terms, &corpus.catalog);
    // Fail closed on a dangling name: never hand the agent a skill that is not
    // a readable SKILL.md on disk (catalog race, partial install, renamed dir).
    let resolved = resolved.and_then(|found| {
        let path = resolve_skill_path(claude_home, &found.name)?;
        if !path.is_file() {
            return None;
        }
        // Catalog-tied decay: a skill file edited after its calibration
        // record halves that skill once (fail-open, rare path).
        let mtime_ms = path
            .metadata()
            .ok()
            .and_then(|meta| meta.modified().ok())
            .and_then(|modified| {
                modified
                    .duration_since(std::time::UNIX_EPOCH)
                    .ok()
                    .map(|elapsed| elapsed.as_millis() as u64)
            })
            .unwrap_or(0);
        if mtime_ms > 0 {
            crate::utility::decision::decay_calibration_for_skill_file(
                claude_home,
                &found.name,
                mtime_ms,
            );
        }
        Some(found)
    });
    // Calibrate confidence using empirical accuracy history (J03)
    let mut resolved = resolved;
    if let Some(found) = &mut resolved {
        found.confidence = crate::utility::decision::get_calibrated_confidence(
            claude_home,
            &found.name,
            found.confidence,
        );
    }
    let resolved = apply_confidence_gate(resolved, prompt, claude_home);
    // Record match-usage telemetry for skill_list. Fail-open: a write error
    // inside record_skill_match never breaks the match path.
    // J08: stage the (skill, predicted confidence) pair for session-end
    // outcome reconcile (cited → helpful, never cited → mis-routed).
    if let Some(found) = &resolved {
        crate::utility::skill_usage::record_skill_match(claude_home, &found.name);
        record_pending_skill_match(claude_home, &found.name, found.confidence);
    }
    // J02: publish the decision under its content-hash key.
    if let Some(key) = cache_key {
        skill_routing_cache_put(key, &resolved, now_unix_secs());
    }
    resolved
}

/// Resolve skill composition (J11) for a prompt across installed skills.
pub fn match_skill_composition_for_prompt(
    claude_home: &Path,
    prompt: &str,
) -> Option<crate::utility::decision::SkillCompositionDecision> {
    if prompt.trim().is_empty() {
        return None;
    }
    let corpus = load_skill_corpus_for_home(claude_home);
    if corpus.terms.is_empty() {
        return None;
    }
    let prompt_tokens = tokenize(prompt);
    if prompt_tokens.is_empty() {
        return None;
    }
    let candidates: Vec<&SkillTerms> = corpus
        .terms
        .iter()
        .filter(|skill| !is_learned_skill(&skill.name))
        .collect();
    if candidates.is_empty() {
        return None;
    }
    let mut document_frequency: HashMap<&str, usize> = HashMap::new();
    for skill in &candidates {
        for token in &skill.all_tokens {
            *document_frequency.entry(token.as_str()).or_insert(0) += 1;
        }
    }
    let corpus_size = candidates.len() as f64;
    let idf = |token: &str| -> f64 {
        let df = document_frequency.get(token).copied().unwrap_or(0);
        if df == 0 {
            0.0
        } else {
            (corpus_size / df as f64).ln()
        }
    };
    let min_score = MIN_SCORE_FACTOR * corpus_size.ln();
    let mut scored: Vec<(String, f64)> = Vec::new();
    for skill in &candidates {
        let mut score = 0.0;
        let mut has_distinctive = false;
        for token in &prompt_tokens {
            if !skill.all_tokens.contains(token) {
                continue;
            }
            let is_own_name_token = skill.name_tokens.contains(token);
            let weight = if is_own_name_token {
                (corpus_size).ln() * NAME_TOKEN_BOOST
            } else {
                idf(token)
            };
            score += weight;
            let df = document_frequency.get(token.as_str()).copied().unwrap_or(0);
            if is_own_name_token || (df > 0 && df <= DISTINCTIVE_DF_MAX) {
                has_distinctive = true;
            }
        }
        if score > 0.0 && score >= min_score && has_distinctive {
            scored.push((skill.name.clone(), score));
        }
    }
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    if scored.is_empty() {
        return None;
    }
    // J11: normalize raw IDF sums by the top score (evaluator takes 0.0-1.0).
    let top = scored.first().map(|(_, score)| *score).unwrap_or(0.0);
    if top > 0.0 {
        for (_, score) in &mut scored {
            *score = (*score / top).clamp(0.0, 1.0);
        }
    }
    let decision = crate::utility::decision::evaluate_skill_composition(&scored);
    let (kind, skills) = match &decision.choice {
        crate::utility::decision::SkillCompositionChoice::Compose(pair) => {
            ("compose", pair.clone())
        }
        crate::utility::decision::SkillCompositionChoice::Single(name) => {
            ("single", vec![name.clone()])
        }
        crate::utility::decision::SkillCompositionChoice::Generic => ("generic", Vec::new()),
    };
    let considered: Vec<String> = candidates.iter().map(|skill| skill.name.clone()).collect();
    record_pending_composition(claude_home, kind, &skills, &considered, decision.confidence);
    Some(decision)
}

/// Public resolve of `<claude_home>/skills/<name>/SKILL.md` when the skill is
/// installed and readable. Used by MCP skill_route/skill_get so callers always
/// get a concrete path for Read fallback when host `Skill()` is stale.
pub fn installed_skill_path(claude_home: &Path, skill_name: &str) -> Option<std::path::PathBuf> {
    let path = resolve_skill_path(claude_home, skill_name)?;
    if path.is_file() {
        Some(path)
    } else {
        None
    }
}

/// Pure operation-aware resolution over an already-loaded skill corpus: the
/// security-audit boundary first, then IDF statistical scoring, then the curated
/// cross-cutting fallback (only for a curated skill present in `skills`). Extracted
/// [`match_skill_for_prompt`] so the exact production decision can be driven over
/// a fixture corpus with no IO — this is what the behavioral skill-eval
/// (`keel skill-eval`) asserts against, so the eval tests the real
/// activation path rather than a reimplementation of it.
pub fn resolve_skill_for_prompt(prompt: &str, skills: &[SkillTerms]) -> Option<SkillMatch> {
    resolve_skill_selection(prompt, skills, &[]).map(|decision| SkillMatch {
        name: decision.name,
        score: decision.relevance,
    })
}

/// Return the full cost-aware decision used by production routing and evals.
pub fn resolve_skill_selection(
    prompt: &str,
    skills: &[SkillTerms],
    catalog: &[SkillCatalogEntry],
) -> Option<SkillSelectionDecision> {
    if prompt.trim().is_empty() || skills.is_empty() {
        return None;
    }
    if security_audit_override(prompt)
        .is_some_and(|name| skills.iter().any(|skill| skill.name == name))
    {
        return decision_for_named_skill(
            "security-and-compliance-auditor",
            skills,
            catalog,
            "explicit security audit operation",
        );
    }
    if diagnosis_operation_override(prompt)
        .is_some_and(|name| skills.iter().any(|skill| skill.name == name))
    {
        return decision_for_named_skill(
            "systematic-debugging",
            skills,
            catalog,
            "explicit diagnosis operation",
        );
    }
    if let Some(found) = select_cost_aware_skill(prompt, skills, catalog) {
        return Some(found);
    }
    // The IDF matcher stayed silent (no corpus-rare token). Before giving up,
    // try the curated cross-cutting tier: a small set of always-applies skills
    // (reviewer, TDD, debugging, preserve-existing-flow) whose vocabulary is
    // high-frequency across the corpus and therefore never distinctive, even
    // though the prompt clearly calls for them. Only route to a curated skill
    // that is actually installed, so a trimmed install never points at a
    // missing skill.
    if let Some(curated) = curated_skill_for_prompt(prompt) {
        if skills.iter().any(|skill| skill.name == curated) {
            return decision_for_named_skill(curated, skills, catalog, "curated operation trigger");
        }
        return None;
    }
    // K-Native-3: Sub-word semantic centroid match when lexical BM25 is silent.
    // Resolves conceptual and synonym-based developer intents with 0 tokens and <0.5ms offline.
    select_semantic_centroid_skill(prompt, skills, catalog)
}

fn select_semantic_centroid_skill(
    prompt: &str,
    skills: &[SkillTerms],
    catalog: &[SkillCatalogEntry],
) -> Option<SkillSelectionDecision> {
    if prompt.trim().is_empty() || skills.is_empty() || catalog.is_empty() {
        return None;
    }
    use crate::utility::semantic_fast::{SemanticCentroidEngine, SemanticTarget};
    let mut targets = Vec::with_capacity(catalog.len());
    for entry in catalog {
        if entry.is_quarantined || is_learned_skill(&entry.name) {
            continue;
        }
        let desc = format!("{} {}", entry.description, entry.when_to_use);
        let cap_refs: Vec<&str> = entry.capabilities.iter().map(|s| s.as_str()).collect();
        targets.push(SemanticTarget::new(&entry.name, &desc, &cap_refs));
    }
    if targets.is_empty() {
        return None;
    }
    // Fallback is for conceptual queries (>= 4 words) where BM25 lacks rare IDF tokens.
    // Short prompts (<= 3 words) stay silent to avoid inflating prompt context.
    if prompt.split_whitespace().count() <= 3 {
        return None;
    }
    let engine = SemanticCentroidEngine::new(targets);
    let matches = engine.match_query(prompt, 3);
    let best = matches.first()?;
    // Must be at least 0.10 similarity to trigger semantic fallback
    if best.similarity < 0.10 {
        return None;
    }
    let runner_up_sim = matches.get(1).map(|m| m.similarity).unwrap_or(0.0);
    // Distinctiveness margin: best must be noticeably ahead of runner-up if runner-up is strong
    if runner_up_sim > 0.08 && (best.similarity - runner_up_sim) < 0.02 {
        return None;
    }
    validate_skill_dependencies(&best.identifier, catalog).ok()?;
    let (index, skill) = skills
        .iter()
        .enumerate()
        .find(|(_, s)| s.name == best.identifier)?;
    let metadata = selection_metadata(index, skill, catalog);
    let activation_budget = skill_activation_budget_tokens();
    let candidates: Vec<(usize, &SkillTerms)> = skills
        .iter()
        .enumerate()
        .filter(|(_, item)| !is_learned_skill(&item.name))
        .collect();
    let confidence = if runner_up_sim > 0.0 {
        ((best.similarity - runner_up_sim) / best.similarity.max(1e-6)).clamp(0.0, 1.0) as f64
    } else {
        best.confidence
    };
    Some(SkillSelectionDecision {
        name: best.identifier.clone(),
        relevance: (best.similarity * 10.0) as f64,
        utility: (best.similarity * 10.0) as f64
            * (1.0 + SUCCESS_WEIGHT * (metadata.historical_success - 0.5)),
        confidence: confidence.max(0.65), // semantic fallback clears J03 confidence gate
        estimated_tokens: metadata.activation_cost_tokens.max(1),
        activation_budget_tokens: activation_budget,
        redundancy: candidate_redundancy(index, &candidates),
        task_criticality: metadata.task_criticality.clamp(0.0, 1.0),
        historical_success: metadata.historical_success.clamp(0.0, 1.0),
        reason: "semantic centroid match with high cosine similarity (no lexical BM25 match)"
            .to_string(),
    })
}

#[derive(Debug, Clone)]
struct ScoredSkill {
    index: usize,
    relevance: f64,
    distinctive: bool,
    utility: f64,
    redundancy: f64,
    metadata: SkillSelectionMetadata,
}

fn select_cost_aware_skill(
    prompt: &str,
    skills: &[SkillTerms],
    catalog: &[SkillCatalogEntry],
) -> Option<SkillSelectionDecision> {
    let prompt_tokens = tokenize(prompt);
    if prompt_tokens.is_empty() {
        return None;
    }
    let candidates: Vec<(usize, &SkillTerms)> = skills
        .iter()
        .enumerate()
        .filter(|(_, skill)| {
            if is_learned_skill(&skill.name) {
                return false;
            }
            if catalog
                .iter()
                .any(|c| c.name == skill.name && c.is_quarantined)
            {
                let prompt_lower = prompt.to_ascii_lowercase();
                let skill_lower = skill.name.to_ascii_lowercase();
                return prompt_lower.contains(&skill_lower);
            }
            true
        })
        .collect();
    if candidates.is_empty() {
        return None;
    }
    let document_frequency = document_frequency(&candidates);
    let corpus_size = candidates.len() as f64;
    let activation_budget = skill_activation_budget_tokens();
    let mut scored = Vec::with_capacity(candidates.len());
    for (index, skill) in &candidates {
        let metadata = selection_metadata(*index, skill, catalog);
        let estimated_tokens = metadata
            .activation_cost_tokens
            .clamp(1, SKILL_S1_HARD_TOKENS);
        let critical = metadata.task_criticality.clamp(0.0, 1.0);
        if estimated_tokens > activation_budget && critical < 0.9 {
            continue;
        }
        let (relevance, distinctive) =
            skill_relevance(&prompt_tokens, skill, &document_frequency, corpus_size);
        if relevance <= 0.0 || !distinctive {
            continue;
        }
        let redundancy = candidate_redundancy(*index, &candidates);
        let cost_ratio = estimated_tokens as f64 / activation_budget.max(1) as f64;
        let cost_factor = (1.0 - COST_WEIGHT * cost_ratio.min(1.0)).max(0.5);
        let redundancy_factor = 1.0 - REDUNDANCY_WEIGHT * redundancy;
        let criticality_factor = 1.0 + CRITICALITY_WEIGHT * (critical - 0.5);
        let success_factor =
            1.0 + SUCCESS_WEIGHT * (metadata.historical_success.clamp(0.0, 1.0) - 0.5);
        let utility =
            relevance * cost_factor * redundancy_factor * criticality_factor * success_factor;
        scored.push(ScoredSkill {
            index: *index,
            relevance,
            distinctive,
            utility,
            redundancy,
            metadata,
        });
    }
    scored.sort_by(|left, right| {
        right
            .utility
            .partial_cmp(&left.utility)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                right
                    .relevance
                    .partial_cmp(&left.relevance)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| skills[left.index].name.cmp(&skills[right.index].name))
    });
    let best = scored.first()?;
    if !catalog.is_empty() {
        validate_skill_dependencies(&skills[best.index].name, catalog).ok()?;
    }
    let min_score = MIN_SCORE_FACTOR * corpus_size.ln();
    if best.relevance < min_score || !best.distinctive {
        return None;
    }
    let runner_up = scored.get(1).map(|entry| entry.utility).unwrap_or(0.0);
    if runner_up > 0.0 && best.utility < runner_up * DISTINCTIVENESS_MARGIN {
        return None;
    }
    let confidence = if runner_up > 0.0 {
        ((best.utility - runner_up) / best.utility.max(f64::EPSILON)).clamp(0.0, 1.0)
    } else {
        1.0
    };
    Some(SkillSelectionDecision {
        name: skills[best.index].name.clone(),
        relevance: best.relevance,
        utility: best.utility,
        confidence,
        estimated_tokens: best.metadata.activation_cost_tokens.max(1),
        activation_budget_tokens: activation_budget,
        redundancy: best.redundancy,
        task_criticality: best.metadata.task_criticality.clamp(0.0, 1.0),
        historical_success: best.metadata.historical_success.clamp(0.0, 1.0),
        reason: "cost-aware relevance with budget, redundancy, criticality, and historical success"
            .to_string(),
    })
}

fn decision_for_named_skill(
    name: &str,
    skills: &[SkillTerms],
    catalog: &[SkillCatalogEntry],
    reason: &str,
) -> Option<SkillSelectionDecision> {
    let (index, skill) = skills
        .iter()
        .enumerate()
        .find(|(_, skill)| skill.name == name)?;
    if !catalog.is_empty() {
        validate_skill_dependencies(name, catalog).ok()?;
    }
    let metadata = selection_metadata(index, skill, catalog);
    let activation_budget = skill_activation_budget_tokens();
    let candidates: Vec<(usize, &SkillTerms)> = skills
        .iter()
        .enumerate()
        .filter(|(_, item)| !is_learned_skill(&item.name))
        .collect();
    Some(SkillSelectionDecision {
        name: name.to_string(),
        relevance: 0.0,
        utility: 0.0,
        confidence: 1.0,
        estimated_tokens: metadata.activation_cost_tokens.max(1),
        activation_budget_tokens: activation_budget,
        redundancy: candidate_redundancy(index, &candidates),
        task_criticality: metadata.task_criticality.clamp(0.0, 1.0),
        historical_success: metadata.historical_success.clamp(0.0, 1.0),
        reason: reason.to_string(),
    })
}

fn selection_metadata(
    _index: usize,
    skill: &SkillTerms,
    catalog: &[SkillCatalogEntry],
) -> SkillSelectionMetadata {
    catalog
        .iter()
        .find(|entry| entry.name == skill.name)
        .map(SkillCatalogEntry::selection_metadata)
        .unwrap_or_else(|| SkillSelectionMetadata {
            activation_cost_tokens: skill.all_tokens.len().max(1),
            task_criticality: default_task_criticality(&skill.name),
            ..SkillSelectionMetadata::default()
        })
}

fn default_task_criticality(name: &str) -> f64 {
    if matches!(
        name,
        "security-and-compliance-auditor"
            | "preserve-existing-flow"
            | "systematic-debugging"
            | "reviewer"
            | "test-driven-development"
    ) {
        1.0
    } else {
        0.5
    }
}

fn skill_activation_budget_tokens() -> usize {
    let budget = [
        "KEEL_SKILL_ACTIVATION_BUDGET_TOKENS",
        "KEEL_SKILL_ACTIVATION_TOKENS",
    ]
    .iter()
    .find_map(|name| {
        std::env::var(name)
            .ok()
            .and_then(|value| value.trim().parse::<usize>().ok())
            .filter(|value| *value > 0)
    })
    .unwrap_or(DEFAULT_SKILL_ACTIVATION_BUDGET_TOKENS);
    budget.min(1_000_000)
}

fn document_frequency<'a>(candidates: &[(usize, &'a SkillTerms)]) -> HashMap<&'a str, usize> {
    let mut frequency = HashMap::new();
    for (_, skill) in candidates {
        for token in &skill.all_tokens {
            *frequency.entry(token.as_str()).or_insert(0) += 1;
        }
    }
    frequency
}

fn skill_relevance(
    prompt_tokens: &HashSet<String>,
    skill: &SkillTerms,
    document_frequency: &HashMap<&str, usize>,
    corpus_size: f64,
) -> (f64, bool) {
    let mut relevance = 0.0;
    let mut distinctive = false;
    for token in prompt_tokens {
        if !skill.all_tokens.contains(token) {
            continue;
        }
        let df = document_frequency.get(token.as_str()).copied().unwrap_or(0);
        let weight = if skill.name_tokens.contains(token) {
            corpus_size.ln() * NAME_TOKEN_BOOST
        } else if df == 0 {
            0.0
        } else {
            (corpus_size / df as f64).ln()
        };
        relevance += weight;
        if skill.name_tokens.contains(token) || (df > 0 && df <= DISTINCTIVE_DF_MAX) {
            distinctive = true;
        }
    }
    (relevance, distinctive)
}

fn candidate_redundancy(index: usize, candidates: &[(usize, &SkillTerms)]) -> f64 {
    let Some((_, skill)) = candidates.iter().find(|(candidate, _)| *candidate == index) else {
        return 0.0;
    };
    candidates
        .iter()
        .filter(|(candidate, _)| *candidate != index)
        .map(|(_, other)| jaccard(&skill.all_tokens, &other.all_tokens))
        .fold(0.0, f64::max)
}

fn jaccard(left: &HashSet<String>, right: &HashSet<String>) -> f64 {
    let union = left.union(right).count();
    if union == 0 {
        return 0.0;
    }
    left.intersection(right).count() as f64 / union as f64
}

/// Operation intent outranks protocol vocabulary for security audits. Auth
/// implementation and audit skills necessarily share OAuth/token terms, so the
/// explicit read-only audit operation must be resolved before IDF scoring.
fn security_audit_override(prompt: &str) -> Option<&'static str> {
    let tokens = tokenize(prompt);
    let audit = [
        "audit",
        "auditing",
        "review",
        "threat",
        "compliance",
        "vulnerability",
    ]
    .iter()
    .any(|token| tokens.contains(*token));
    let auth = [
        "oauth",
        "oidc",
        "authentication",
        "auth",
        "jwt",
        "credential",
        "credentials",
    ]
    .iter()
    .any(|token| tokens.contains(*token));
    if audit && auth {
        Some("security-and-compliance-auditor")
    } else {
        None
    }
}

/// Explicit broken-behavior diagnosis outranks incidental subsystem nouns.
/// A request such as "memory and skills are not working; find the root cause"
/// is debugging work even though a feature-specific status skill can score
/// highly on repeated words like `memory`.
fn diagnosis_operation_override(prompt: &str) -> Option<&'static str> {
    let lower = prompt.to_ascii_lowercase();
    let diagnosis = [
        "find the root cause",
        "check why",
        "why is this not working",
        "why isn't this working",
        "not working",
        "does not work",
        "doesn't work",
        "diagnose",
        "debug",
        "reproduce the bug",
    ]
    .iter()
    .any(|phrase| lower.contains(phrase));
    let repair = ["fix", "repair", "root cause", "verify"]
        .iter()
        .any(|phrase| lower.contains(phrase));
    (diagnosis && repair).then_some("systematic-debugging")
}

/// Curated cross-cutting skill triggers, evaluated only when the statistical
/// matcher is silent.
///
/// These skills apply to a whole *class* of work but use high-frequency verbs
/// (review, test, debug, edit) — or, for the UI/UX pair, share nearly all their
/// vocabulary with each other — so the IDF matcher correctly treats them as
/// non-distinctive and stays silent, leaving the most-used everyday skills with
/// no inline brief. Each entry lists narrow, verb- or noun-anchored trigger
/// phrases; the first skill whose phrase appears in the (lowercased) prompt
/// wins. Phrases are intentionally specific so ordinary prose ("I reviewed the
/// docs") does not trip them.
///
/// Order matters: earlier entries win ties. `systematic-debugging` precedes
/// `test-driven-development` because "the test is failing" is a debugging ask,
/// not a request to author a new test. The UI and UX phrase sets are kept
/// disjoint (visual-craft vs. research/flow) so the two never collide.
const CURATED_SKILL_TRIGGERS: &[(&str, &[&str])] = &[
    (
        "critic",
        &[
            "critique this",
            "what am i missing",
            "stress-test this approach",
            "stress test this approach",
            "is this sound before i go further",
            "review this implementation adversarially",
            "challenge this implementation",
            "find weaknesses in this implementation",
            "half-baked",
            "half baked",
            "unfinished work",
            "need some critics",
            "critics about",
        ],
    ),
    (
        "reviewer",
        &[
            "review this diff",
            "review the diff",
            "review this pr",
            "review the pr",
            "review this code",
            "review the code",
            "review my code",
            "review my changes",
            "review the changes",
            "code review",
            "ready to merge",
            "ready for merge",
            "before we merge",
            "production ready",
            "production readiness",
            "release readiness",
            "is this ready to ship",
        ],
    ),
    (
        "systematic-debugging",
        &[
            "test is failing",
            "tests are failing",
            "this test is failing",
            "it keeps failing",
            "keeps crashing",
            "flaky test",
            "flaky tests",
            "intermittent failure",
            "debug this",
            "debug the",
            "why is this failing",
            "why does this fail",
            "track down the bug",
            "find the root cause",
            "reproduce the bug",
        ],
    ),
    (
        "test-driven-development",
        &[
            "write a test",
            "write tests",
            "add a test",
            "add tests",
            "add unit tests",
            "test first",
            "test-driven",
            "tdd",
            "red green refactor",
            "cover this with tests",
            "write a failing test",
        ],
    ),
    (
        "preserve-existing-flow",
        &[
            "edit the existing",
            "change the existing",
            "modify the existing",
            "refactor the existing",
            "change existing behavior",
            "before editing",
            "before i edit",
            "without breaking existing",
            "don't break existing",
        ],
    ),
    (
        "running-anvil",
        &[
            "use anvil",
            "run anvil",
            "anvil compile",
            "delivery loop",
            "compile a named bar",
            "multi-piece delivery",
            "implement this change",
            "implement this feature",
        ],
    ),
    // UI visual-craft asks. The two UI/UX skills share almost all their
    // vocabulary (design, ui, ux, interface, experience), so the IDF matcher
    // ties on them and correctly stays silent — leaving obvious UI prompts with
    // no inline guidance (the real-world failure that produced "I don't have a
    // UI/UX skill registered"). These phrases are visual-craft-specific so they
    // route to `ui-design-systems-…` without colliding with the research-shaped
    // UX phrases below.
    (
        "ui-design-systems-and-responsive-interfaces",
        &[
            "typography",
            "responsive layout",
            "responsive design",
            "design system",
            "looks ai-generated",
            "look less ai-generated",
            "less ai-generated",
            "ai-generated look",
            "visual hierarchy",
            "visual polish",
            "visual craft",
            "color palette",
            "color discipline",
            "spacing and",
            "make this look",
            "make it look",
            "style the",
            "css layout",
            "tailwind",
            "component library",
            "wcag",
            "accessible ui",
            "ui polish",
            "ui design",
            "landing page",
            "landing-page",
            "build a landing",
            "dashboard",
            "analytics dashboard",
            "glassmorphism",
            "polish ui",
            "polish the ui",
            "button states",
            "focus states",
            "focus and contrast",
            "contrast and focus",
            "layout and spacing",
            "palette and typography",
            "theming",
            "design tokens",
        ],
    ),
    // UX research / experience-strategy asks. Research- and flow-shaped phrasing,
    // kept disjoint from the visual-craft phrases above so the two never tie.
    (
        "ux-research-and-experience-strategy",
        &[
            "user research",
            "usability",
            "user journey",
            "user journeys",
            "user flow",
            "user flows",
            "conversion funnel",
            "drop-off",
            "personas",
            "wireframe",
            "wireframes",
            "information architecture",
            "ux research",
            "experience strategy",
            "user testing",
            "journey map",
        ],
    ),
    // Git workflow asks. "git" tokens recur across many skills (every closeout
    // and branch skill mentions them), so the IDF matcher rarely finds them
    // distinctive. These verb-anchored phrases route the actual git-operation
    // asks to the specialist.
    (
        "git-expert",
        &[
            "merge conflict",
            "rebase",
            "force push",
            "cherry-pick",
            "git history",
            "rewrite history",
            "squash commits",
            "undo the commit",
            "revert the commit",
            "resolve the conflict",
            "detached head",
            "git workflow",
        ],
    ),
    // Security / compliance review asks. Security vocabulary is sprinkled across
    // the reviewer and auditor skills, so it ties; these phrases pick the auditor
    // for the threat-modeling / compliance class of work specifically.
    (
        "security-and-compliance-auditor",
        &[
            "threat model",
            "threat modeling",
            "security audit",
            "security review",
            "vulnerability",
            "owasp",
            "soc2",
            "gdpr",
            "pen test",
            "penetration test",
            "is this secure",
            "security hardening",
        ],
    ),
    // QA / test-strategy asks (distinct from TDD's moment-to-moment loop and
    // debugging's failing-test trace): coverage strategy, e2e suites, the release
    // ladder, flaky-suite triage at the strategy level.
    (
        "qa-and-automation-engineer",
        &[
            "test strategy",
            "test plan",
            "test coverage",
            "coverage strategy",
            "end-to-end tests",
            "e2e tests",
            "integration test suite",
            "regression suite",
            "release ladder",
            "qa strategy",
            "automation strategy",
        ],
    ),
    // Cloud / DevOps / infra asks. Infra vocabulary spans many skills; these
    // phrases anchor the deploy/pipeline/IaC class to the specialist.
    (
        "cloud-and-devops-expert",
        &[
            "ci/cd",
            "ci pipeline",
            "cd pipeline",
            "github actions",
            "terraform",
            "kubernetes",
            "helm chart",
            "dockerfile",
            "deploy to production",
            "deployment pipeline",
            "infrastructure as code",
            "rollout strategy",
        ],
    ),
    // API contract asks. "api" alone is everywhere; these phrases target the
    // contract-design / versioning / breaking-change class.
    (
        "api-contract-design",
        &[
            "api contract",
            "openapi",
            "breaking change",
            "api versioning",
            "rest endpoint",
            "graphql schema",
            "grpc",
            "json schema",
            "pagination semantics",
            "idempotency key",
            "error taxonomy",
        ],
    ),
    // Auth / identity build asks. Auth words recur across the security and
    // backend skills; these phrases route the build-a-login-flow class to the
    // identity specialist.
    (
        "authentication-and-identity",
        &[
            "oauth",
            "oidc",
            "openid connect",
            "saml",
            "single sign-on",
            "sso flow",
            "jwt",
            "refresh token",
            "session management",
            "password hashing",
            "passkey",
            "webauthn",
            "multi-factor",
            "login flow",
        ],
    ),
    // Observability / incident asks. "monitor"/"alert" are generic; these phrases
    // target the telemetry / SLO / paging / postmortem class.
    (
        "observability-and-incident-response",
        &[
            "slo",
            "sli",
            "error budget",
            "burn rate",
            "alerting rules",
            "paging",
            "on-call",
            "runbook",
            "postmortem",
            "incident response",
            "distributed tracing",
            "opentelemetry",
        ],
    ),
    // Dependency / supply-chain asks. Routes the upgrade/lockfile/SBOM class.
    (
        "dependency-and-supply-chain",
        &[
            "dependency upgrade",
            "bump the version",
            "lockfile",
            "transitive dependency",
            "dependabot",
            "renovate",
            "sbom",
            "supply chain",
            "major version migration",
            "typosquat",
        ],
    ),
    // Data / ML engineering asks. Routes the pipeline / warehouse / model class.
    (
        "data-and-ml-engineering",
        &[
            "etl pipeline",
            "elt pipeline",
            "data pipeline",
            "data warehouse",
            "dbt model",
            "airflow",
            "feature engineering",
            "model training",
            "model serving",
            "drift monitoring",
            "batch ingestion",
            "streaming ingestion",
        ],
    ),
    // Disagreement-adjudication asks ("disagree"/"approach" are high-frequency,
    // so IDF stays silent); these phrases route that class to the specialist.
    (
        "deliberation",
        &[
            "deliberation",
            "deliberate between",
            "adjudicate",
            "conflicting opinions",
            "experts disagree",
            "disagree on the approach",
        ],
    ),
    // Backend systems asks. Backend nouns recur across API/data skills; these
    // phrases anchor the services/messaging/caching class to the specialist.
    (
        "backend-and-data-architecture",
        &[
            "microservice boundaries",
            "message queue",
            "message queues",
            "cache invalidation",
            "read replica",
            "read replicas",
            "connection pool",
        ],
    ),
    // DDD asks. "domain"/"model" are everywhere; these phrases target the
    // bounded-context/aggregate/language class.
    (
        "domain-driven-design",
        &[
            "bounded context",
            "bounded contexts",
            "ubiquitous language",
            "aggregate root",
            "domain event",
            "domain events",
            "context map",
        ],
    ),
];

/// Every curated trigger phrase paired with the skill it must route to, in
/// table order. Shape: (prompt, expected skill). Owner of the benchmark cases.
pub fn curated_skill_cases() -> Vec<(String, String)> {
    CURATED_SKILL_TRIGGERS
        .iter()
        .flat_map(|(skill, phrases)| {
            phrases
                .iter()
                .map(move |phrase| ((*phrase).to_string(), (*skill).to_string()))
        })
        .collect()
}

/// Pure curated-tier lookup (no IO) so the trigger phrases are unit-testable.
/// Returns the first curated skill whose trigger phrase appears in the
/// lowercased prompt, or `None`. Conservative by construction: it errs toward
/// silence, and the caller still gates on the skill being installed.
///
/// Matching is **word-boundary-anchored**, not raw substring. A short trigger
/// like `slo`, `sli`, or `tdd` used to match with a bare `contains`, so it fired
/// inside ordinary words — `slo` inside "slow", `sli` inside "slideshow"/
/// "slightly"/"slicker" — and mis-routed the prompt (finding #17). Anchoring the
/// span to word boundaries fixes that while leaving multi-word phrases (already
/// boundary-delimited by their internal spaces) and long standalone tokens
/// unchanged: they still match whenever they appear as whole words.
///
/// The lowercase pass is ASCII-only (`to_ascii_lowercase`); every trigger phrase
/// is ASCII, so non-ASCII prompt text simply never matches a trigger, which is
/// the intended conservative behavior.
pub fn curated_skill_for_prompt(prompt: &str) -> Option<&'static str> {
    let lowered = prompt.to_ascii_lowercase();
    for (skill, phrases) in CURATED_SKILL_TRIGGERS {
        if phrases
            .iter()
            .any(|phrase| phrase_matches_at_word_boundary(&lowered, phrase))
        {
            return Some(skill);
        }
    }
    None
}

/// Match `phrase` in `lowered` only at word boundaries, so a short curated
/// trigger like `slo` matches the standalone token `slo` (or its plural `slos`)
/// but never fires inside a larger word such as `slow` or `slideshow`.
///
/// A phrase may itself contain internal separators (`ci/cd`, `drop-off`, `error
/// budget`); only the characters immediately before and after the *whole* matched
/// span are boundary-checked, so internal punctuation and spaces are preserved
/// and multi-word phrases still match exactly as before. Both `lowered` and every
/// `phrase` are ASCII-lowercased; a `phrase` is always non-empty ASCII, so all
/// byte indexing here lands on char boundaries (an ASCII match start plus the
/// phrase byte length).
fn phrase_matches_at_word_boundary(lowered: &str, phrase: &str) -> bool {
    if phrase.is_empty() {
        return false;
    }
    let bytes = lowered.as_bytes();
    let phrase_len = phrase.len();
    let mut search_start = 0;
    while let Some(relative) = lowered[search_start..].find(phrase) {
        let start = search_start + relative;
        let end = start + phrase_len;
        let left_ok = start == 0 || !bytes[start - 1].is_ascii_alphanumeric();
        let right_ok = match bytes.get(end).copied() {
            // End of string or a non-alphanumeric char — a clean boundary.
            None => true,
            Some(next) if !next.is_ascii_alphanumeric() => true,
            // Tolerate a single trailing plural `s`: `slo` matches `slos`/`SLOs`
            // but not `slow`. The char after the plural `s` must itself be a
            // boundary, so `slosh` still does not match.
            Some(b's') => bytes
                .get(end + 1)
                .map(|after| !after.is_ascii_alphanumeric())
                .unwrap_or(true),
            Some(_) => false,
        };
        if left_ok && right_ok {
            return true;
        }
        // Advance one byte past this candidate start to look for a later,
        // properly-bounded occurrence. `start` is an ASCII position, so
        // `start + 1` stays on a char boundary.
        search_start = start + 1;
        if search_start >= lowered.len() {
            break;
        }
    }
    false
}

/// Read the matched skill's `SKILL.md` and return a bounded, ready-to-inject
/// brief: the frontmatter `description` followed by the opening of the skill
/// body, truncated to [`INLINE_BRIEF_MAX_BYTES`] on a line boundary. Returns
/// `None` when the skill cannot be read or has no usable content.
///
/// This is the model-independence lever. The per-prompt hook used to *ask* the
/// model to call `Skill("<name>")`; whether that happened depended on the
/// gateway model honoring an injected instruction. Injecting the brief instead
/// means the skill's operative guidance is in the model's input context for
/// this turn no matter what — the `Skill()` call becomes an optional upgrade to
/// the full body, not a prerequisite for any guidance at all.
pub fn skill_inline_brief(claude_home: &Path, skill_name: &str) -> Option<String> {
    let skill_path = resolve_skill_path(claude_home, skill_name)?;
    let text = fs::read_to_string(&skill_path).ok()?;
    inline_brief_from_source(&text)
}

/// Resolve `<skills_dir>/<name>/SKILL.md` for an installed skill, rejecting any
/// `name` that could traverse outside the skills directory or reach a reserved
/// directory. `skill_name` usually comes from a matched installed skill (the
/// matcher only returns names it read from a real directory), but the guard is
/// defensive so a crafted frontmatter `name` — or a caller-supplied name from
/// the MCP surface — can never escape. Delegates the segment-safety check to
/// `runtime::safe_path_segment`, which rejects separators, `.`/`..`, absolute
/// paths, and Windows drive-relative prefixes (`C:foo`) via the OS path parser.
/// Also rejects the `_`/`.`-prefixed reserved directories (`_shared`, hidden)
/// that the discovery traversal hides, so the get/route surface admits exactly
/// the set the catalog enumerates.
fn resolve_skill_path(claude_home: &Path, skill_name: &str) -> Option<std::path::PathBuf> {
    let segment = safe_path_segment(skill_name)?;
    if segment.starts_with('_') || segment.starts_with('.') {
        return None;
    }
    Some(skills_directory(claude_home).join(segment).join("SKILL.md"))
}

/// Read the full SKILL.md body for an installed skill. Returns the resolved
/// path and the complete file text (frontmatter included) so MCP callers can
/// pull the entire skill on demand — the `skill_get` tool's backing — without
/// the brief truncation `skill_inline_brief` applies. `None` when the name is
/// unsafe or the file is missing/unreadable.
pub fn skill_full_body(
    claude_home: &Path,
    skill_name: &str,
) -> Option<(std::path::PathBuf, String)> {
    let skill_path = resolve_skill_path(claude_home, skill_name)?;
    let text = fs::read_to_string(&skill_path).ok()?;
    Some((skill_path, text))
}

/// Bounded S1 projection returned after a skill is selected. The frontmatter
/// is intentionally excluded: S0 already carries routing metadata, while S1
/// should contain actionable core instructions only.
pub(crate) fn skill_core_projection(
    claude_home: &Path,
    skill_name: &str,
) -> Option<(PathBuf, String, usize, usize, bool)> {
    let (path, full_body) = skill_full_body(claude_home, skill_name)?;
    let body = strip_frontmatter_block(&full_body).trim_start();
    let raw_tokens = TokenMeter::count_text(body);
    let budget = configured_skill_tokens("KEEL_SKILL_S1_RESPONSE_TOKENS", SKILL_S1_TARGET_TOKENS)
        .min(SKILL_S1_HARD_TOKENS);
    let visible = truncate_text_to_tokens(body, budget);
    let truncated = visible.len() < body.len();
    let visible_tokens = TokenMeter::count_text(&visible);
    Some((path, visible, raw_tokens, visible_tokens, truncated))
}

/// Load one explicitly requested S2 resource. The resolver rejects traversal,
/// absolute/drive-relative paths, symlinks, and oversized/binary content, so a
/// failed load becomes a bounded classified error instead of a raw fallback.
pub(crate) fn skill_resource_projection(
    claude_home: &Path,
    skill_name: &str,
    resource: &str,
) -> Result<(PathBuf, String, usize, usize, bool), String> {
    let Some(skill_path) = installed_skill_path(claude_home, skill_name) else {
        return Err(format!("skill resource: unknown skill {skill_name:?}"));
    };
    let skill_root = skill_path
        .parent()
        .ok_or_else(|| "skill resource: skill root unavailable".to_string())?;
    let path = resolve_skill_resource_path(skill_root, resource)
        .ok_or_else(|| "skill resource: path is unsafe or missing".to_string())?;
    let metadata = std::fs::symlink_metadata(&path)
        .map_err(|_| "skill resource: file is missing".to_string())?;
    if !metadata.file_type().is_file() {
        return Err("skill resource: path is not a regular file".to_string());
    }
    if metadata.len() > SKILL_RESOURCE_MAX_BYTES as u64 {
        return Err(format!(
            "skill resource: file exceeds the {}-byte resource limit",
            SKILL_RESOURCE_MAX_BYTES
        ));
    }
    let bytes = std::fs::read(&path).map_err(|_| "skill resource: read failed".to_string())?;
    let text = String::from_utf8(bytes)
        .map_err(|_| "skill resource: binary content requires local Read".to_string())?;
    let raw_tokens = TokenMeter::count_text(&text);
    let budget = configured_skill_tokens("KEEL_SKILL_S2_RESPONSE_TOKENS", SKILL_S2_DEFAULT_TOKENS)
        .min(SKILL_S2_HARD_TOKENS);
    let visible = truncate_text_to_tokens(&text, budget);
    let truncated = visible.len() < text.len();
    let visible_tokens = TokenMeter::count_text(&visible);
    Ok((path, visible, raw_tokens, visible_tokens, truncated))
}

fn configured_skill_tokens(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .map(|value| value.min(SKILL_S1_HARD_TOKENS.max(SKILL_S2_HARD_TOKENS)))
        .unwrap_or(default)
}

fn truncate_text_to_tokens(text: &str, max_tokens: usize) -> String {
    if max_tokens == 0 || text.is_empty() {
        return String::new();
    }
    if TokenMeter::count_text(text) <= max_tokens {
        return text.to_string();
    }
    let mut boundaries = text
        .char_indices()
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    boundaries.push(text.len());
    let mut low = 0usize;
    let mut high = boundaries.len();
    while low + 1 < high {
        let middle = (low + high) / 2;
        if TokenMeter::count_text(&text[..boundaries[middle]]) <= max_tokens {
            low = middle;
        } else {
            high = middle;
        }
    }
    let prefix = &text[..boundaries[low]];
    // A line boundary keeps an instruction from ending in the middle of a
    // command while the exact token check above remains authoritative.
    if let Some(cut) = prefix.rfind('\n') {
        let line_prefix = prefix[..cut].trim_end();
        if !line_prefix.is_empty() && TokenMeter::count_text(line_prefix) <= max_tokens {
            return line_prefix.to_string();
        }
    }
    prefix.to_string()
}

fn resolve_skill_resource_path(skill_root: &Path, resource: &str) -> Option<PathBuf> {
    let normalized = resource.trim().replace('\\', "/");
    if normalized.is_empty() || normalized.contains('\0') {
        return None;
    }
    let (base, relative) = if let Some(shared) = normalized.strip_prefix("../_shared/") {
        (skill_root.parent()?.join("_shared"), shared)
    } else {
        if normalized.starts_with('/')
            || normalized.contains(":/")
            || normalized
                .split('/')
                .any(|part| part.is_empty() || part == "..")
        {
            return None;
        }
        (skill_root.to_path_buf(), normalized.as_str())
    };
    if relative.is_empty()
        || relative
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return None;
    }
    let path = base.join(relative);
    // Do not follow a symlink at any component of the requested path.
    let mut current = base;
    for component in relative.split('/') {
        current = current.join(component);
        if std::fs::symlink_metadata(&current)
            .map(|metadata| metadata.file_type().is_symlink())
            .unwrap_or(false)
        {
            return None;
        }
    }
    Some(path)
}

/// One installed skill's catalog row: its directory name (the resolve key for
/// `skill_get`/`skill_route`) plus the two frontmatter fields the harness
/// matcher reads. Backs the MCP `skill_list` tool.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SkillCatalogEntry {
    pub name: String,
    pub description: String,
    pub when_to_use: String,
    /// How many times the matcher has selected this skill (monotonic counter
    /// under `<claude_home>/state/skill-usage/<name>.count`). 0 = never matched.
    pub use_count: u64,
    /// Other installed skill names this skill declares as related (frontmatter
    /// `related_skills`). Surfaced so the matcher can suggest adjacent skills.
    pub related_skills: Vec<String>,
    /// Compact Agent Skills capability tags for S0 routing.
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// Declared skill version, or `unversioned` when absent.
    #[serde(default)]
    pub version: String,
    /// Explicit bounded dependencies requested by this skill.
    #[serde(default)]
    pub dependencies: Vec<String>,
    /// Estimated S1 activation cost measured with the authoritative tokenizer.
    #[serde(default)]
    pub activation_cost_tokens: usize,
    /// Criticality hint in the inclusive range 0..=1.
    #[serde(default)]
    pub task_criticality: f64,
    /// Smoothed success rate from optional outcome counters.
    #[serde(default = "default_historical_success")]
    pub historical_success: f64,
    /// Closed-loop quarantine status from Bayesian prior tracking (K-Native-4).
    #[serde(default)]
    pub is_quarantined: bool,
}

impl SkillCatalogEntry {
    fn selection_metadata(&self) -> SkillSelectionMetadata {
        SkillSelectionMetadata {
            capabilities: self.capabilities.clone(),
            version: if self.version.trim().is_empty() {
                "unversioned".to_string()
            } else {
                self.version.clone()
            },
            dependencies: self.dependencies.clone(),
            activation_cost_tokens: self.activation_cost_tokens.max(1),
            task_criticality: if self.task_criticality.is_finite() {
                self.task_criticality.clamp(0.0, 1.0)
            } else {
                0.5
            },
            historical_success: if self.historical_success.is_finite() {
                self.historical_success.clamp(0.0, 1.0)
            } else {
                DEFAULT_SKILL_HISTORICAL_SUCCESS
            },
        }
    }
}

/// Validate metadata only; dependencies never preload another skill's body.
pub(crate) fn validate_skill_dependencies(
    name: &str,
    catalog: &[SkillCatalogEntry],
) -> Result<(), String> {
    let mut pending = vec![(name, false)];
    let mut active = HashSet::new();
    let mut complete = HashSet::new();
    while let Some((current, exiting)) = pending.pop() {
        if exiting {
            active.remove(current);
            complete.insert(current);
            continue;
        }
        if complete.contains(current) {
            continue;
        }
        if safe_path_segment(current).is_none() || current.starts_with(['_', '.']) {
            return Err(format!("skill dependency has unsafe name `{current}`"));
        }
        let entry = catalog
            .iter()
            .find(|entry| entry.name == current)
            .ok_or_else(|| format!("skill dependency `{current}` is missing or unreadable"))?;
        if !active.insert(current) {
            return Err(format!("circular skill dependency detected at `{current}`"));
        }
        pending.push((current, true));
        for dependency in entry.dependencies.iter().rev() {
            pending.push((dependency.as_str(), false));
        }
    }
    Ok(())
}

/// Enumerate every installed skill under `<claude_home>/skills`, returning the
/// name + `description` + `when_to_use` frontmatter for each. Skips `_shared`,
/// hidden directories, and any directory without a parseable SKILL.md — the
/// same traversal `load_skill_terms` uses. Sorted by name for stable output.
pub fn skill_catalog(claude_home: &Path) -> Vec<SkillCatalogEntry> {
    let skills_dir = skills_directory(claude_home);
    let mut catalog = load_skill_catalog_for_dir_with_cache(
        &skills_dir,
        Some(&skill_catalog_cache_path(claude_home)),
    );
    // Read usage counters outside the parsed cache so telemetry is immediately
    // visible without invalidating or rereading every SKILL.md.
    for entry in &mut catalog {
        entry.use_count = crate::utility::skill_usage::skill_use_count(claude_home, &entry.name);
        entry.historical_success =
            crate::utility::skill_usage::skill_success_rate(claude_home, &entry.name);
    }
    catalog.sort_by(|left, right| left.name.cmp(&right.name));
    catalog
}

pub(crate) fn load_skill_catalog_for_dir(skills_dir: &Path) -> Vec<SkillCatalogEntry> {
    let mut catalog = load_skill_catalog_for_dir_with_cache(
        skills_dir,
        Some(&skill_catalog_cache_path_for_dir(skills_dir)),
    );
    catalog.sort_by(|left, right| left.name.cmp(&right.name));
    catalog
}

fn load_skill_catalog_for_dir_with_cache(
    skills_dir: &Path,
    cache_path: Option<&Path>,
) -> Vec<SkillCatalogEntry> {
    load_skill_corpus(skills_dir, cache_path).catalog
}

/// Cache-free fixed-context projections over one stable, parseable skill set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SkillContextSnapshot {
    pub skill_count: usize,
    pub inline_catalog: String,
    pub full_bodies: String,
}

pub(crate) fn skill_context_snapshot(skills_dir: &Path) -> Option<SkillContextSnapshot> {
    let files = discover_skill_files(skills_dir)?;
    let mut skill_count = 0usize;
    let mut inline_catalog = String::new();
    let mut full_bodies = String::new();

    for file in files {
        let text = fs::read_to_string(&file.path).ok()?;
        let Some(frontmatter) = split_frontmatter(&text) else {
            continue;
        };
        skill_count += 1;
        inline_catalog.push_str("---\n");
        inline_catalog.push_str(frontmatter.trim());
        inline_catalog.push_str("\n---\n");
        full_bodies.push_str(&text);
        if !text.ends_with('\n') {
            full_bodies.push('\n');
        }
    }

    Some(SkillContextSnapshot {
        skill_count,
        inline_catalog,
        full_bodies,
    })
}

/// Pure brief builder (no IO) so truncation behavior is unit-testable. Takes raw
/// SKILL.md text, pulls the frontmatter `description`, drops the frontmatter
/// block, and appends the opening body up to the byte cap on a line boundary.
fn inline_brief_from_source(text: &str) -> Option<String> {
    let body = strip_frontmatter_block(text);
    let description = split_frontmatter(text)
        .and_then(|frontmatter| frontmatter_field(&frontmatter, "description"))
        .unwrap_or_default();

    let mut brief = String::new();
    if !description.trim().is_empty() {
        brief.push_str(description.trim());
        brief.push_str("\n\n");
    }
    brief.push_str(
        truncate_on_line_boundary(
            body.trim_start(),
            INLINE_BRIEF_MAX_BYTES,
            "\n\n[skill brief truncated — call Skill(\"<name>\") or skill_get / Read the installed path for the full body]",
        )
        .trim_end(),
    );

    let trimmed = brief.trim().to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

/// Return everything after the leading `---\n...\n---\n` frontmatter block, or
/// the whole text when there is no frontmatter. Mirrors [`split_frontmatter`]
/// but yields the body rather than the fenced metadata.
fn strip_frontmatter_block(text: &str) -> &str {
    let trimmed_start = text.trim_start_matches(['\u{feff}', ' ', '\t', '\n', '\r']);
    if !trimmed_start.starts_with("---") {
        return text;
    }
    // Skip the opening fence line, then find the closing fence.
    let Some(after_open) = trimmed_start.split_once('\n').map(|(_, rest)| rest) else {
        return text;
    };
    let mut offset = 0usize;
    for line in after_open.split_inclusive('\n') {
        if line.trim_end_matches(['\r', '\n']).trim() == "---" {
            return &after_open[offset + line.len()..];
        }
        offset += line.len();
    }
    // Unterminated frontmatter — no usable body.
    ""
}

/// Truncate `text` to at most `max_bytes`, backing up to the last newline so the
/// result never ends mid-line. Falls back to a hard byte cut on a char boundary
/// when the first line already exceeds the cap. Appends `marker` when content was
/// dropped so the consumer knows it continues; returns the text unchanged (no
/// marker) when it already fits.
///
/// UTF-8 safe: the only byte-position cut is at a `\n` (ASCII) found by
/// `rposition`, and the no-newline fallback explicitly backs `end` up to a char
/// boundary — so it never slices through a multibyte character. Shared by the
/// per-prompt skill brief and the SessionStart workspace digest, which is why the
/// marker is a parameter rather than hard-coded.
pub fn truncate_on_line_boundary(text: &str, max_bytes: usize, marker: &str) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    // Prefer cutting at the last newline within the budget.
    let window = &text.as_bytes()[..max_bytes];
    let cut = window
        .iter()
        .rposition(|&b| b == b'\n')
        .map(|pos| pos + 1)
        .unwrap_or_else(|| {
            // No newline in range — back up to a UTF-8 char boundary.
            let mut end = max_bytes;
            while end > 0 && !text.is_char_boundary(end) {
                end -= 1;
            }
            end
        });
    let mut truncated = text[..cut].trim_end().to_string();
    truncated.push_str(marker);
    truncated
}

/// Read `<skills_dir>/<name>/SKILL.md` for every installed skill and build its
/// term model. Skips `_shared` and any directory without a SKILL.md. Any read
/// or parse failure for one skill drops that skill silently rather than failing
/// the whole match.
pub fn load_skill_terms(skills_dir: &Path) -> Vec<SkillTerms> {
    load_skill_corpus(
        skills_dir,
        Some(&skill_catalog_cache_path_for_dir(skills_dir)),
    )
    .terms
}

/// Every installed skill's own description, for the training seeds: a class per
/// installed skill is what lets the head name a skill with no public corpus.
pub fn load_skill_catalog_for_home(claude_home: &Path) -> Vec<SkillCatalogEntry> {
    load_skill_corpus_for_home(claude_home).catalog
}

fn load_skill_corpus_for_home(claude_home: &Path) -> LoadedSkillCorpus {
    let skills_dir = skills_directory(claude_home);
    let cache_path = skill_catalog_cache_path(claude_home);
    // why: the on-disk cache still costs a parse and an index build per prompt,
    // and an edited skill changes the fingerprint exactly as it changes the file.
    let fingerprint = skill_corpus_fingerprint(&skills_dir, &cache_path);
    if let (Some(fingerprint), Ok(cache)) = (fingerprint.as_ref(), SKILL_CORPUS_CACHE.lock()) {
        if let Some((cached, corpus)) = cache.as_ref() {
            if cached == fingerprint {
                return (**corpus).clone();
            }
        }
    }
    let corpus = load_skill_corpus(&skills_dir, Some(&cache_path));
    if let (Some(fingerprint), Ok(mut cache)) = (fingerprint, SKILL_CORPUS_CACHE.lock()) {
        *cache = Some((fingerprint, Arc::new(corpus.clone())));
    }
    corpus
}

/// The skills listing is the identity: the loader itself rewrites the on-disk
/// cache, so a fingerprint that included that file's own timestamp would miss
/// on every call.
fn skill_corpus_fingerprint(skills_dir: &Path, cache_path: &Path) -> Option<String> {
    let files = discover_skill_files(skills_dir)?;
    let generation = skill_catalog_generation(&files);
    Some(format!(
        "{}::{generation}::{}",
        skills_dir.display(),
        cache_path.display()
    ))
}

type SkillCorpusCache = Option<(String, Arc<LoadedSkillCorpus>)>;
static SKILL_CORPUS_CACHE: LazyLock<Mutex<SkillCorpusCache>> = LazyLock::new(|| Mutex::new(None));

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct SkillCatalogCache {
    version: u32,
    generation: String,
    last_integrity_check_secs: u64,
    entries: Vec<SkillCatalogCacheEntry>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct SkillCatalogCacheEntry {
    name: String,
    size: u64,
    modified_at_nanos: u128,
    content_hash: String,
    terms: SkillTerms,
    catalog: SkillCatalogEntry,
}

#[derive(Debug, Clone)]
struct SkillFileMetadata {
    name: String,
    path: PathBuf,
    size: u64,
    modified_at_nanos: u128,
}

#[derive(Debug, Clone, Default)]
struct LoadedSkillCorpus {
    terms: Vec<SkillTerms>,
    catalog: Vec<SkillCatalogEntry>,
}

fn skill_catalog_cache_path(claude_home: &Path) -> PathBuf {
    state_directory(claude_home).join(SKILL_CATALOG_CACHE_FILE)
}

fn skill_catalog_cache_path_for_dir(skills_dir: &Path) -> PathBuf {
    skills_dir
        .parent()
        .unwrap_or(skills_dir)
        .join("state")
        .join(SKILL_CATALOG_CACHE_FILE)
}

fn skill_catalog_integrity_interval_secs() -> u64 {
    std::env::var("KEEL_SKILL_CATALOG_INTEGRITY_INTERVAL_SECS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(SKILL_CATALOG_DEFAULT_INTEGRITY_INTERVAL_SECS)
        .min(86_400)
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn discover_skill_files(skills_dir: &Path) -> Option<Vec<SkillFileMetadata>> {
    let entries = fs::read_dir(skills_dir).ok()?;
    let mut files = Vec::new();
    for entry in entries.flatten() {
        if !entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || name.starts_with('_') {
            continue;
        }
        let path = entry.path().join("SKILL.md");
        let metadata = match fs::metadata(&path) {
            Ok(metadata) if metadata.is_file() => metadata,
            _ => continue,
        };
        let modified_at_nanos = metadata
            .modified()
            .ok()
            .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        files.push(SkillFileMetadata {
            name,
            path,
            size: metadata.len(),
            modified_at_nanos,
        });
    }
    files.sort_by(|left, right| left.name.cmp(&right.name));
    Some(files)
}

fn skill_catalog_generation(files: &[SkillFileMetadata]) -> String {
    let mut material = String::new();
    for file in files {
        material.push_str(&file.name);
        material.push('\0');
        material.push_str(&file.size.to_string());
        material.push('\0');
        material.push_str(&file.modified_at_nanos.to_string());
        material.push('\n');
    }
    crate::utility::hashing::fnv1a64_hex(&material)
}

fn load_skill_corpus(skills_dir: &Path, cache_path: Option<&Path>) -> LoadedSkillCorpus {
    let Some(files) = discover_skill_files(skills_dir) else {
        return LoadedSkillCorpus::default();
    };
    let generation = skill_catalog_generation(&files);
    let now = now_unix_secs();
    let mut reusable_cache: Option<SkillCatalogCache> = None;
    let mut verify_cached_content = false;
    if let Some(path) = cache_path {
        if let Some(cache) = read_skill_catalog_cache(path) {
            if cache.version == SKILL_CATALOG_CACHE_VERSION && cache.generation == generation {
                let interval = skill_catalog_integrity_interval_secs();
                let age = now.saturating_sub(cache.last_integrity_check_secs);
                if age < interval && cache_entries_match_metadata(&cache.entries, &files) {
                    return corpus_from_cache(cache.entries);
                }
                verify_cached_content = age >= interval;
            }
            if cache.version == SKILL_CATALOG_CACHE_VERSION {
                reusable_cache = Some(cache);
            }
        }
    }

    // Reuse unchanged entries when one skill is added, removed, or edited;
    // periodic checks still catch same-size/same-mtime content changes.
    let entries = parse_skill_catalog_entries_incremental(
        &files,
        reusable_cache
            .as_ref()
            .map(|cache| cache.entries.as_slice()),
        verify_cached_content,
    );
    if let Some(path) = cache_path {
        let cache = SkillCatalogCache {
            version: SKILL_CATALOG_CACHE_VERSION,
            generation,
            last_integrity_check_secs: now,
            entries: entries.clone(),
        };
        write_skill_catalog_cache(path, &cache);
    }
    corpus_from_cache(entries)
}

fn read_skill_catalog_cache(path: &Path) -> Option<SkillCatalogCache> {
    let text = fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

fn cache_entries_match_metadata(
    cached: &[SkillCatalogCacheEntry],
    files: &[SkillFileMetadata],
) -> bool {
    if cached.len() != files.len() {
        return false;
    }
    let by_name: HashMap<&str, &SkillCatalogCacheEntry> = cached
        .iter()
        .map(|entry| (entry.name.as_str(), entry))
        .collect();
    if by_name.len() != cached.len() {
        return false;
    }
    files.iter().all(|file| {
        let Some(entry) = by_name.get(file.name.as_str()) else {
            return false;
        };
        entry.size == file.size
            && entry.modified_at_nanos == file.modified_at_nanos
            && cached_entry_is_semantically_complete(entry, file)
    })
}

fn cached_entry_is_semantically_complete(
    entry: &SkillCatalogCacheEntry,
    file: &SkillFileMetadata,
) -> bool {
    entry.name == file.name
        && entry.catalog.name == file.name
        && entry.terms.name == file.name
        && !entry.content_hash.is_empty()
        && !entry.terms.all_tokens.is_empty()
}

fn cached_entry_matches_file(
    entry: &SkillCatalogCacheEntry,
    file: &SkillFileMetadata,
    verify_content: bool,
) -> bool {
    if !cached_entry_is_semantically_complete(entry, file)
        || entry.size != file.size
        || entry.modified_at_nanos != file.modified_at_nanos
    {
        return false;
    }
    !verify_content
        || fs::read(&file.path)
            .ok()
            .map(|bytes| crate::utility::hashing::fnv1a64_hex(&String::from_utf8_lossy(&bytes)))
            .is_some_and(|hash| hash == entry.content_hash)
}

fn parse_skill_catalog_entries_incremental(
    files: &[SkillFileMetadata],
    cached: Option<&[SkillCatalogCacheEntry]>,
    verify_content: bool,
) -> Vec<SkillCatalogCacheEntry> {
    let cached_by_name: HashMap<&str, &SkillCatalogCacheEntry> = cached
        .unwrap_or_default()
        .iter()
        .map(|entry| (entry.name.as_str(), entry))
        .collect();
    let mut entries = Vec::new();
    for file in files {
        if let Some(cached_entry) = cached_by_name.get(file.name.as_str()) {
            if cached_entry_matches_file(cached_entry, file, verify_content) {
                entries.push((*cached_entry).clone());
                continue;
            }
        }
        if let Some(entry) = parse_skill_catalog_entry(file) {
            entries.push(entry);
        }
    }
    entries
}

fn parse_skill_catalog_entry(file: &SkillFileMetadata) -> Option<SkillCatalogCacheEntry> {
    let text = fs::read_to_string(&file.path).ok()?;
    let terms = skill_terms_from_source(&file.name, &text)?;
    let frontmatter = split_frontmatter(&text)?;
    let body = strip_frontmatter_block(&text);
    let catalog = SkillCatalogEntry {
        name: file.name.clone(),
        description: frontmatter_field(&frontmatter, "description").unwrap_or_default(),
        when_to_use: frontmatter_field(&frontmatter, "when_to_use").unwrap_or_default(),
        use_count: 0,
        related_skills: related_skills_list(&frontmatter),
        capabilities: capability_list(&frontmatter, &file.name),
        version: frontmatter_field(&frontmatter, "version")
            .or_else(|| frontmatter_field(&frontmatter, "skill_version"))
            .map(|value| strip_quotes(value.trim()).to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "unversioned".to_string()),
        dependencies: dependency_list(&frontmatter),
        activation_cost_tokens: TokenMeter::count_text(body).clamp(1, SKILL_S1_HARD_TOKENS),
        task_criticality: frontmatter_number(
            &frontmatter,
            &["task_criticality", "task-criticality", "criticality"],
        )
        .unwrap_or_else(|| default_task_criticality(&file.name))
        .clamp(0.0, 1.0),
        historical_success: DEFAULT_SKILL_HISTORICAL_SUCCESS,
        is_quarantined: false,
    };
    Some(SkillCatalogCacheEntry {
        name: file.name.clone(),
        size: file.size,
        modified_at_nanos: file.modified_at_nanos,
        content_hash: crate::utility::hashing::fnv1a64_hex(&text),
        terms,
        catalog,
    })
}

fn corpus_from_cache(entries: Vec<SkillCatalogCacheEntry>) -> LoadedSkillCorpus {
    let mut corpus = LoadedSkillCorpus::default();
    for entry in entries {
        corpus.terms.push(entry.terms);
        corpus.catalog.push(entry.catalog);
    }
    corpus
}

fn write_skill_catalog_cache(path: &Path, cache: &SkillCatalogCache) {
    let Some(parent) = path.parent() else {
        return;
    };
    if fs::create_dir_all(parent).is_err() {
        return;
    }
    let Ok(serialized) = serde_json::to_vec_pretty(cache) else {
        return;
    };
    // The catalog is disposable derived state. Write a process-unique staging
    // file first; a torn cache is harmless because the next caller rebuilds it.
    let temp = parent.join(format!(
        ".{SKILL_CATALOG_CACHE_FILE}.tmp-{}-{}",
        std::process::id(),
        now_unix_secs()
    ));
    if fs::write(&temp, serialized).is_err() {
        let _ = fs::remove_file(&temp);
        return;
    }
    if fs::rename(&temp, path).is_err() {
        // Windows cannot rename over an existing file. The target is only cache
        // state, so replacing it is safe; failures leave the old valid cache.
        let _ = fs::remove_file(path);
        let _ = fs::rename(&temp, path);
    }
}

/// Build a [`SkillTerms`] from a directory name and raw SKILL.md text.
/// Uses the frontmatter `description` + `when_to_use` plus the name as the
/// matchable surface — exactly the fields the harness matcher reads.
///
/// The returned `name` is the **directory name**, not the frontmatter `name`.
/// `name` is the stable resolve key: `match_skill_for_prompt` hands it to
/// `skill_inline_brief`/`skill_get`, which resolve `<skills>/<name>/SKILL.md` —
/// a directory join. Keying on the frontmatter name would break that join for
/// any skill whose frontmatter name differs from its directory. Tokens from
/// both the directory name and the frontmatter name feed the matchable surface
/// so divergence never costs match signal.
fn skill_terms_from_source(dir_name: &str, text: &str) -> Option<SkillTerms> {
    let frontmatter = split_frontmatter(text)?;
    let description = frontmatter_field(&frontmatter, "description").unwrap_or_default();
    let when_to_use = frontmatter_field(&frontmatter, "when_to_use").unwrap_or_default();
    let frontmatter_name = frontmatter_field(&frontmatter, "name")
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| dir_name.to_string());

    // Tokenize both the directory name and the frontmatter name so the name
    // signal is preserved when they diverge; the resolve key stays the dir name.
    let mut name_tokens = tokenize(&dir_name.replace('-', " "));
    name_tokens.extend(tokenize(&frontmatter_name.replace('-', " ")));
    let mut all_tokens = name_tokens.clone();
    all_tokens.extend(tokenize(&description));
    all_tokens.extend(tokenize(&when_to_use));

    if all_tokens.is_empty() {
        return None;
    }
    Some(SkillTerms {
        name: dir_name.to_string(),
        all_tokens,
        name_tokens,
    })
}

/// Pure scoring core (no IO) so the thresholds are unit-testable against
/// synthetic corpora. Computes per-token IDF across the supplied skills, scores
/// each skill by the IDF-weighted overlap with the prompt tokens, and returns
/// the winner only when it clears [`MIN_SCORE`], beats the runner-up by
/// [`DISTINCTIVENESS_MARGIN`], and shares at least one distinctive token.
pub fn score_prompt_against_skills(prompt: &str, skills: &[SkillTerms]) -> Option<SkillMatch> {
    let prompt_tokens = tokenize(prompt);
    if prompt_tokens.is_empty() || skills.is_empty() {
        return None;
    }

    // Exclude auto-generated `learned-<project>` skills from the statistical
    // corpus entirely — as candidates AND from document-frequency (finding #18).
    // Their name tokens are generic project words (rust, keel, driver, hub,
    // farm); via the own-name distinctiveness boost below they would hijack any
    // prompt containing that word in ANY project ("fix this rust borrow checker
    // error" -> learned-rust), and even without the boost a df=1 name token like
    // `rust` would score `ln(N)` and win globally. A learned skill is meant to
    // surface only inside its own project, which the harness's project-path
    // matcher handles; this global IDF tier must never route to one on a bare
    // language/word token. Keeping them out of `document_frequency` too means
    // their tokens do not distort the IDF of the real skills.
    let candidates: Vec<&SkillTerms> = skills
        .iter()
        .filter(|skill| !is_learned_skill(&skill.name))
        .collect();
    if candidates.is_empty() {
        return None;
    }

    // Document frequency: how many skills contain each token.
    let mut document_frequency: HashMap<&str, usize> = HashMap::new();
    for skill in &candidates {
        for token in &skill.all_tokens {
            *document_frequency.entry(token.as_str()).or_insert(0) += 1;
        }
    }
    let corpus_size = candidates.len() as f64;

    let idf = |token: &str| -> f64 {
        let df = document_frequency.get(token).copied().unwrap_or(0);
        if df == 0 {
            0.0
        } else {
            (corpus_size / df as f64).ln()
        }
    };

    let mut scored: Vec<(usize, f64, bool)> = Vec::with_capacity(candidates.len());
    for (index, skill) in candidates.iter().enumerate() {
        let mut score = 0.0;
        let mut has_distinctive = false;
        for token in &prompt_tokens {
            if !skill.all_tokens.contains(token) {
                continue;
            }
            let is_own_name_token = skill.name_tokens.contains(token);
            // A skill's own name token is its distinctive handle. Sibling
            // skills that mention it by name inflate its corpus document
            // frequency, which would otherwise crush its IDF (ln(N/df)) toward
            // zero and drop the match below the score floor — the RC7 bug where
            // a skill became unreachable by its own name. For the owning skill
            // we therefore score the token as if it were unique (df=1, i.e.
            // ln(N)) and apply the name boost, so naming a skill always reaches
            // it regardless of how many siblings reference it. Non-name tokens
            // use the real corpus IDF.
            let weight = if is_own_name_token {
                corpus_size.ln() * NAME_TOKEN_BOOST
            } else {
                idf(token)
            };
            score += weight;
            // Distinctive if corpus-rare OR this skill's own name token (same
            // principle as the score: naming a skill is distinctive to it).
            let df = document_frequency.get(token.as_str()).copied().unwrap_or(0);
            if is_own_name_token || (df > 0 && df <= DISTINCTIVE_DF_MAX) {
                has_distinctive = true;
            }
        }
        scored.push((index, score, has_distinctive));
    }

    // Highest score wins; ties broken by name for determinism.
    scored.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| candidates[a.0].name.cmp(&candidates[b.0].name))
    });

    let (best_index, best_score, best_distinctive) = scored[0];
    let min_score = MIN_SCORE_FACTOR * corpus_size.ln();
    // why: with one installed skill every weight and the floor are ln(1)=0, so a
    // single shared token matched at score 0, which is no evidence at all.
    if best_score <= 0.0 || best_score < min_score || !best_distinctive {
        return None;
    }
    let runner_up = scored.get(1).map(|entry| entry.1).unwrap_or(0.0);
    if runner_up > 0.0 && best_score < runner_up * DISTINCTIVENESS_MARGIN {
        return None;
    }

    Some(SkillMatch {
        name: candidates[best_index].name.clone(),
        score: best_score,
    })
}

/// A `learned-<project>` skill is auto-generated by the keel learning loop from
/// observed per-project command/edit patterns. Its name tokens are generic
/// project words, so it is excluded from the global statistical matcher (finding
/// #18); it surfaces only through the harness's project-path routing.
fn is_learned_skill(name: &str) -> bool {
    name.starts_with("learned-")
}

/// English stopwords plus prompt-generic verbs/nouns that carry no routing
/// signal. Kept small and deliberate — over-pruning would starve the matcher,
/// under-pruning lets boilerplate dominate the score.
const STOPWORDS: &[&str] = &[
    "the", "and", "for", "are", "but", "not", "you", "all", "any", "can", "her", "was", "one",
    "our", "out", "use", "using", "used", "how", "what", "why", "when", "who", "where", "which",
    "with", "this", "that", "these", "those", "from", "have", "has", "had", "will", "would",
    "should", "could", "into", "your", "yours", "their", "them", "then", "than", "they", "some",
    "such", "want", "need", "needs", "make", "made", "help", "please", "lets", "let", "get", "got",
    "give", "add", "added", "adding", "fix", "fixed", "fixing", "set", "run", "running", "code",
    "file", "files", "project", "thing", "things", "work", "working", "about", "also", "just",
    "like", "now", "new", "old", "here", "there", "more", "most", "much", "many", "very", "able",
    "via", "per", "its", "it's",
];

/// Tokenize text into a lowercase set: split on non-alphanumeric, keep tokens
/// of length ≥ 3, drop stopwords and pure numbers. A set (not a bag) so a
/// prompt cannot inflate a score by repeating a word.
pub fn tokenize(text: &str) -> HashSet<String> {
    let stop: HashSet<&str> = STOPWORDS.iter().copied().collect();
    text.split(|c: char| !c.is_ascii_alphanumeric())
        .filter_map(|raw| {
            let token = raw.trim().to_ascii_lowercase();
            if token.len() < 3 {
                return None;
            }
            if token.chars().all(|c| c.is_ascii_digit()) {
                return None;
            }
            if stop.contains(token.as_str()) {
                return None;
            }
            Some(token)
        })
        .collect()
}

/// Split a `---\n...\n---\n` leading frontmatter block. Returns the frontmatter
/// body (between the fences), or `None` when the file does not open with a
/// fence. Delegates to the canonical `skill_lint` implementation so both
/// callers agree on trim boundaries (BOM/space/tab/CR/LF).
pub(crate) fn split_frontmatter(text: &str) -> Option<String> {
    crate::utility::skill_lint::split_frontmatter(text).map(|(frontmatter, _)| frontmatter)
}

/// Read a top-level frontmatter field. Delegates to `skill_lint` so YAML 1.2
/// `|` / `>` block scalars match the lint parser (a `when_to_use: |` value is
/// the body, not the `|` indicator).
pub(crate) fn frontmatter_field(frontmatter: &str, key: &str) -> Option<String> {
    crate::utility::skill_lint::frontmatter_field(frontmatter, key)
}

/// Parse a `related_skills` frontmatter value into a list of skill names.
/// Accepts three forms authors actually write: YAML flow list
/// (`[reviewer, git-expert]`), comma-separated (`reviewer, git-expert`), or a
/// YAML block list (`- reviewer\n- git-expert`). Names are trimmed; quotes and
/// empty entries are dropped. Returns `Vec<String>` (possibly empty).
fn related_skills_list(frontmatter: &str) -> Vec<String> {
    frontmatter_list(frontmatter, "related_skills")
}

pub(crate) fn frontmatter_list(frontmatter: &str, key: &str) -> Vec<String> {
    // Block-list form: the key line has an empty value, followed by `- name` lines.
    let mut names = Vec::new();
    let mut in_block = false;
    for line in frontmatter.lines() {
        let trimmed = line.trim_end();
        if trimmed.trim().is_empty() || trimmed.trim_start().starts_with('#') {
            continue;
        }
        if in_block && trimmed.trim_start().starts_with("- ") {
            let item = trimmed.trim_start()[2..].trim();
            if !item.is_empty() {
                names.push(strip_quotes(item).to_string());
            }
        } else if !trimmed.starts_with(char::is_whitespace) {
            in_block = false;
            if let Some(colon) = trimmed.find(':') {
                if trimmed[..colon].trim() == key {
                    let rest = trimmed[colon + 1..].trim();
                    if rest.is_empty() {
                        in_block = true;
                        continue;
                    }
                    // Inline value on the key line: flow list or comma string.
                    names.extend(split_related_value(rest));
                }
            }
        }
    }
    names
}

fn capability_list(frontmatter: &str, skill_name: &str) -> Vec<String> {
    let mut values = Vec::new();
    for key in ["capabilities", "capability", "tags"] {
        if let Some(value) = frontmatter_field(frontmatter, key) {
            values.extend(split_related_value(&value));
            if !values.is_empty() {
                break;
            }
        }
    }
    if values.is_empty() {
        values = skill_name
            .split('-')
            .filter(|part| part.len() >= 3)
            .map(str::to_string)
            .collect();
    }
    values
        .into_iter()
        .map(|value| strip_quotes(value.trim()).to_ascii_lowercase())
        .filter(|value| !value.is_empty())
        .take(8)
        .collect()
}

pub(crate) fn dependency_list(frontmatter: &str) -> Vec<String> {
    for key in ["dependencies", "depends_on", "depends-on"] {
        if frontmatter_field(frontmatter, key).is_some() {
            return frontmatter_list(frontmatter, key);
        }
    }
    Vec::new()
}

fn frontmatter_number(frontmatter: &str, keys: &[&str]) -> Option<f64> {
    keys.iter().find_map(|key| {
        frontmatter_field(frontmatter, key)
            .and_then(|value| strip_quotes(value.trim()).parse::<f64>().ok())
    })
}

/// Split an inline `related_skills` value (`[a, b]` or `a, b`) into names.
fn split_related_value(value: &str) -> Vec<String> {
    let inner = value.trim().trim_start_matches('[').trim_end_matches(']');
    inner
        .split(',')
        .map(|s| strip_quotes(s.trim()))
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect()
}

fn strip_quotes(value: &str) -> &str {
    let v = value.trim();
    if (v.starts_with('"') && v.ends_with('"') && v.len() >= 2)
        || (v.starts_with('\'') && v.ends_with('\'') && v.len() >= 2)
    {
        &v[1..v.len() - 1]
    } else {
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn related_skills_parses_flow_list() {
        let fm = "name: x\ndescription: d.\nrelated_skills: [reviewer, git-expert]\n";
        let names = related_skills_list(fm);
        assert_eq!(
            names,
            vec!["reviewer".to_string(), "git-expert".to_string()]
        );
    }

    #[test]
    fn related_skills_parses_comma_string() {
        let fm = "name: x\ndescription: d.\nrelated_skills: reviewer, git-expert\n";
        let names = related_skills_list(fm);
        assert_eq!(
            names,
            vec!["reviewer".to_string(), "git-expert".to_string()]
        );
    }

    #[test]
    fn related_skills_parses_block_list() {
        let fm = "name: x\ndescription: d.\nrelated_skills:\n  - reviewer\n  - git-expert\n";
        let names = related_skills_list(fm);
        assert_eq!(
            names,
            vec!["reviewer".to_string(), "git-expert".to_string()]
        );
    }

    #[test]
    fn related_skills_empty_when_absent() {
        let fm = "name: x\ndescription: d.\n";
        assert!(related_skills_list(fm).is_empty());
    }

    #[test]
    fn frontmatter_field_reads_literal_block_when_to_use() {
        let fm = "name: memory-consolidation\nwhen_to_use: |\n  Use when consolidating recent work into durable memory:\n  - At session end or compaction\n";
        let value = frontmatter_field(fm, "when_to_use").expect("when_to_use");
        assert_ne!(value.trim(), "|");
        assert!(
            value.contains("Use when consolidating recent work"),
            "{value:?}"
        );
    }

    #[test]
    fn related_skills_strips_quotes() {
        let fm = "name: x\nrelated_skills: [\"reviewer\", 'git-expert']\n";
        let names = related_skills_list(fm);
        assert_eq!(
            names,
            vec!["reviewer".to_string(), "git-expert".to_string()]
        );
    }

    #[test]
    fn resolve_skill_path_rejects_traversal_and_reserved_names() {
        let home = Path::new("/fake/home");
        // Traversal / separator / absolute / drive-relative names must yield None.
        for evil in [
            "",
            "   ",
            "..",
            "../evil",
            "../../etc",
            "a/b",
            "a\\b",
            "/abs",
            "C:foo",
            "C:",
        ] {
            assert!(
                resolve_skill_path(home, evil).is_none(),
                "name {evil:?} must be rejected"
            );
        }
        // Reserved/hidden directories the catalog hides are not gettable either.
        assert!(resolve_skill_path(home, "_shared").is_none());
        assert!(resolve_skill_path(home, ".hidden").is_none());
        // An ordinary skill name resolves to <skills>/<name>/SKILL.md.
        let ok = resolve_skill_path(home, "reviewer").expect("ordinary name resolves");
        assert!(ok.ends_with("reviewer/SKILL.md") || ok.ends_with("reviewer\\SKILL.md"));
    }

    fn skill(name: &str, description: &str, when_to_use: &str) -> SkillTerms {
        let name_tokens = tokenize(&name.replace('-', " "));
        let mut all_tokens = name_tokens.clone();
        all_tokens.extend(tokenize(description));
        all_tokens.extend(tokenize(when_to_use));
        SkillTerms {
            name: name.to_string(),
            all_tokens,
            name_tokens,
        }
    }

    fn temp_skill_root(label: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        std::env::temp_dir().join(format!(
            "keel-skill-cache-{label}-{}-{unique}",
            std::process::id()
        ))
    }

    fn write_skill(root: &Path, name: &str, description: &str) {
        let directory = root.join(name);
        fs::create_dir_all(&directory).expect("create skill directory");
        fs::write(
            directory.join("SKILL.md"),
            format!(
                "---\nname: {name}\ndescription: {description}\nwhen_to_use: Use {description}.\n---\nbody\n"
            ),
        )
        .expect("write skill");
    }

    #[test]
    fn dependency_metadata_blocks_missing_and_cyclic_activation() {
        let root = temp_skill_root("dependencies");
        write_skill(&root, "reviewer", "Review code");
        let path = root.join("reviewer/SKILL.md");
        fs::write(&path, "---\nname: reviewer\ndescription: Review code\ndependencies:\n  - helper\n---\nReview carefully.\n").unwrap();
        let missing = load_skill_corpus(&root, None);
        assert_eq!(missing.catalog[0].dependencies, vec!["helper"]);
        assert!(validate_skill_dependencies("reviewer", &missing.catalog)
            .unwrap_err()
            .contains("helper"));
        assert!(
            resolve_skill_selection("review this diff", &missing.terms, &missing.catalog).is_none()
        );

        write_skill(&root, "helper", "Support review");
        let valid = load_skill_corpus(&root, None);
        assert!(validate_skill_dependencies("reviewer", &valid.catalog).is_ok());
        assert_eq!(
            resolve_skill_selection("review this diff", &valid.terms, &valid.catalog)
                .unwrap()
                .name,
            "reviewer"
        );

        fs::write(
            root.join("helper/SKILL.md"),
            "---\nname: helper\ndescription: Support review\ndepends_on: [reviewer]\n---\nHelp.\n",
        )
        .unwrap();
        let cyclic = load_skill_corpus(&root, None);
        assert!(validate_skill_dependencies("reviewer", &cyclic.catalog)
            .unwrap_err()
            .contains("circular"));
        assert!(
            resolve_skill_selection("review this diff", &cyclic.terms, &cyclic.catalog).is_none()
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn dependency_lists_support_indented_and_indentless_yaml_sequences() {
        assert_eq!(dependency_list("dependencies:\n  - reviewer\n\n  # optional support\n  - 'git-expert'\nversion: 1\n"), vec!["reviewer", "git-expert"]);
        assert_eq!(
            dependency_list("depends-on:\n- reviewer\n- git-expert\ndescription: use for review\n"),
            vec!["reviewer", "git-expert"]
        );
    }

    #[test]
    fn skill_catalog_cache_reuses_unchanged_entries_when_one_skill_changes() {
        let skills_dir = temp_skill_root("incremental");
        write_skill(&skills_dir, "alpha", "alpha routing");
        write_skill(&skills_dir, "beta", "beta routing");
        let cache_path = skills_dir.join("state").join(SKILL_CATALOG_CACHE_FILE);

        let first = load_skill_corpus(&skills_dir, Some(&cache_path));
        assert_eq!(first.terms.len(), 2);
        let first_cache = read_skill_catalog_cache(&cache_path).expect("first cache");
        let beta_hash = first_cache
            .entries
            .iter()
            .find(|entry| entry.name == "beta")
            .map(|entry| entry.content_hash.clone())
            .expect("beta cache entry");

        write_skill(&skills_dir, "alpha", "alpha replacement routing");
        let second = load_skill_corpus(&skills_dir, Some(&cache_path));
        assert_eq!(second.terms.len(), 2);
        let alpha = second
            .terms
            .iter()
            .find(|terms| terms.name == "alpha")
            .expect("updated alpha terms");
        assert!(alpha.all_tokens.iter().any(|token| token == "replacement"));
        let second_cache = read_skill_catalog_cache(&cache_path).expect("second cache");
        assert_eq!(
            second_cache
                .entries
                .iter()
                .find(|entry| entry.name == "beta")
                .map(|entry| entry.content_hash.as_str()),
            Some(beta_hash.as_str()),
            "an unchanged skill must retain its cached parse"
        );
        let _ = fs::remove_dir_all(&skills_dir);
    }

    #[test]
    fn skill_catalog_cache_repairs_semantically_incomplete_entries() {
        let skills_dir = temp_skill_root("repair");
        write_skill(&skills_dir, "alpha", "alpha routing");
        write_skill(&skills_dir, "beta", "beta routing");
        let cache_path = skills_dir.join("state").join(SKILL_CATALOG_CACHE_FILE);
        let first = load_skill_corpus(&skills_dir, Some(&cache_path));
        assert_eq!(first.terms.len(), 2);

        let mut damaged = read_skill_catalog_cache(&cache_path).expect("cache to damage");
        let alpha = damaged
            .entries
            .iter_mut()
            .find(|entry| entry.name == "alpha")
            .expect("alpha cache entry");
        alpha.terms.all_tokens.clear();
        alpha.catalog.name.clear();
        write_skill_catalog_cache(&cache_path, &damaged);
        let repaired = load_skill_corpus(&skills_dir, Some(&cache_path));
        assert_eq!(
            repaired
                .terms
                .iter()
                .map(|terms| terms.name.as_str())
                .collect::<Vec<_>>(),
            vec!["alpha", "beta"],
            "a malformed-but-parseable cache must not hide or poison an installed skill"
        );
        assert!(repaired
            .terms
            .iter()
            .all(|terms| !terms.all_tokens.is_empty()));
        let _ = fs::remove_dir_all(&skills_dir);
    }

    /// A representative slice of the real corpus so threshold behavior is
    /// tested against realistic IDF, not a toy two-skill set.
    fn sample_corpus() -> Vec<SkillTerms> {
        vec![
            skill(
                "stripe-integration",
                "Designs and audits Stripe integrations: Checkout, Payment Intents, Subscriptions, Webhooks, Connect, refunds, disputes, 3DS SCA flows.",
                "Stripe payment integration, webhook reconciliation, and PCI-scope decisions.",
            ),
            skill(
                "postgres-migration-safety",
                "Plans and reviews PostgreSQL migrations with lock analysis, expand-and-contract sequencing, backfill strategy, and rollback boundaries.",
                "PostgreSQL schema changes, migrations, backfills, and lock-sensitive deploys.",
            ),
            skill(
                "reviewer",
                "Reviews completed implementation work for production readiness: quality, security, correctness, testing, release risk.",
                "Production-readiness review and quality gate after implementation.",
            ),
            skill(
                "react-performance-audit",
                "React render-cost tracing, memoization, bundle-size analysis, list virtualization, Core Web Vitals on React routes.",
                "React performance profiling and render-cost reduction.",
            ),
            skill(
                "websocket-realtime-design",
                "WebSocket, SSE, fan-out, reconnect resume, backpressure, ordering and dedup, auth lifecycle on long-lived connections.",
                "Realtime transport and connection lifecycle design.",
            ),
            skill(
                "git-expert",
                "Safe Git workflow: branching, conflict resolution, history repair, secret cleanup, rebase strategy.",
                "Version control operations and history repair.",
            ),
            skill(
                "security-and-compliance-auditor",
                "Security reviews, threat modeling, compliance SOC2 GDPR, remediation quality.",
                "Security audit and compliance review.",
            ),
        ]
    }

    #[test]
    fn distinctive_domain_prompt_matches_its_skill() {
        let corpus = sample_corpus();
        let result =
            score_prompt_against_skills("add stripe checkout to the billing page", &corpus);
        assert_eq!(
            result.map(|m| m.name),
            Some("stripe-integration".to_string())
        );
    }

    #[test]
    fn postgres_migration_prompt_matches() {
        let corpus = sample_corpus();
        let result = score_prompt_against_skills(
            "I need to add a column and backfill a large postgres table without locking",
            &corpus,
        );
        assert_eq!(
            result.map(|m| m.name),
            Some("postgres-migration-safety".to_string())
        );
    }

    #[test]
    fn websocket_prompt_matches() {
        let corpus = sample_corpus();
        let result = score_prompt_against_skills(
            "design reconnect and backpressure for our websocket fan-out",
            &corpus,
        );
        assert_eq!(
            result.map(|m| m.name),
            Some("websocket-realtime-design".to_string())
        );
    }

    #[test]
    fn generic_prompt_does_not_match() {
        let corpus = sample_corpus();
        // No distinctive domain token — must stay silent and let the caller
        // fall back to the generic reminder.
        assert_eq!(
            score_prompt_against_skills("can you help me write a small function", &corpus),
            None
        );
    }

    #[test]
    fn empty_prompt_does_not_match() {
        let corpus = sample_corpus();
        assert_eq!(score_prompt_against_skills("", &corpus), None);
        assert_eq!(score_prompt_against_skills("   ", &corpus), None);
    }

    #[test]
    fn name_token_in_prompt_is_a_strong_signal() {
        let corpus = sample_corpus();
        let result = score_prompt_against_skills("run the reviewer on this diff", &corpus);
        assert_eq!(result.map(|m| m.name), Some("reviewer".to_string()));
    }

    #[test]
    fn own_name_token_stays_distinctive_despite_cross_references() {
        // RC7 regression: when many other skills mention a skill by name in
        // their descriptions, that name token's document frequency climbs past
        // DISTINCTIVE_DF_MAX, so the rarity gate alone would stop the skill from
        // matching its own handle. The own-name exemption must keep it
        // reachable. Build a corpus where "reviewer" appears in 4 skills (df=4
        // > 3) but only the reviewer skill has it as a *name* token.
        let corpus = vec![
            skill(
                "reviewer",
                "Production-readiness review and quality gate.",
                "Review a diff before merge.",
            ),
            skill(
                "test-driven-development",
                "RED-GREEN-REFACTOR; complements the reviewer and qa work.",
                "Write a failing test first.",
            ),
            skill(
                "finishing-a-development-branch",
                "Close out a branch: verify, then route non-trivial work through the reviewer.",
                "Branch closeout.",
            ),
            skill(
                "receiving-code-review",
                "Act on reviewer feedback as the author.",
                "Address review comments.",
            ),
        ];
        // df("reviewer") == 4 here, above the rarity gate, yet naming it must
        // still reach the reviewer skill via the own-name exemption.
        let result = score_prompt_against_skills("have the reviewer look at this", &corpus);
        assert_eq!(result.map(|m| m.name), Some("reviewer".to_string()));
    }

    #[test]
    fn curated_tier_routes_cross_cutting_prompts() {
        // RC2: these verb-anchored prompts carry no corpus-rare token, so the
        // IDF matcher stays silent — the curated tier is what routes them.
        assert_eq!(
            curated_skill_for_prompt("please review this diff"),
            Some("reviewer")
        );
        assert_eq!(
            curated_skill_for_prompt("this test is failing intermittently"),
            Some("systematic-debugging")
        );
        assert_eq!(
            curated_skill_for_prompt("write a test for the parser"),
            Some("test-driven-development")
        );
        assert_eq!(
            curated_skill_for_prompt("use anvil to implement this feature"),
            Some("running-anvil")
        );
        assert_eq!(
            curated_skill_for_prompt("I need to modify the existing auth handler"),
            Some("preserve-existing-flow")
        );
    }

    #[test]
    fn curated_tier_prefers_debugging_over_tdd_for_failing_tests() {
        // "the test is failing" is a debugging ask, not a request to author a
        // new test — debugging must win the tie via trigger order.
        assert_eq!(
            curated_skill_for_prompt("the test is failing, help me fix it"),
            Some("systematic-debugging")
        );
    }

    #[test]
    fn curated_tier_silent_on_ordinary_prose() {
        // Must not trip on incidental mentions — only verb-anchored asks.
        assert_eq!(
            curated_skill_for_prompt("I reviewed the docs yesterday"),
            None
        );
        assert_eq!(curated_skill_for_prompt("add a logout button"), None);
        assert_eq!(curated_skill_for_prompt(""), None);
    }

    #[test]
    fn curated_tier_word_boundary_rejects_substring_false_positives() {
        // Finding #17: short curated triggers (`slo`, `sli`) used to match as bare
        // substrings and fired inside ordinary words, mis-routing the prompt to
        // observability. Word-boundary matching must now keep these silent. These
        // assertions FAIL under the old `lowered.contains(phrase)` matcher and pass
        // after the boundary fix.
        assert_eq!(
            curated_skill_for_prompt("the page is loading slow, can you take a look"),
            None,
            "`slo` inside `slow` must not route to observability"
        );
        assert_eq!(
            curated_skill_for_prompt("make the slideshow transition slightly slicker"),
            None,
            "`sli` inside slideshow/slightly/slicker must not route to observability"
        );
        assert_eq!(
            curated_skill_for_prompt("please deslot the widget and reslice the grid"),
            None,
            "`slo`/`sli` embedded mid-word must not trip the trigger"
        );
    }

    #[test]
    fn curated_tier_still_matches_standalone_short_and_plural_tokens() {
        // The boundary fix must NOT regress genuine standalone matches: the short
        // trigger still fires as a whole word and tolerates a plural `s`.
        assert_eq!(
            curated_skill_for_prompt("define an slo and error budget"),
            Some("observability-and-incident-response"),
            "standalone `slo` must still route"
        );
        assert_eq!(
            curated_skill_for_prompt("our slos are being missed this quarter"),
            Some("observability-and-incident-response"),
            "plural `slos` must still route"
        );
        // A multi-word phrase with internal punctuation still matches as before.
        assert_eq!(
            curated_skill_for_prompt("fix the ci/cd pipeline"),
            Some("cloud-and-devops-expert"),
            "internal-separator phrase `ci/cd` must still route"
        );
    }

    #[test]
    fn learned_skill_never_wins_statistical_match_on_bare_token() {
        // Finding #18: an auto-generated `learned-<project>` skill has generic
        // name tokens (here `rust`). The own-name distinctiveness boost used to
        // make it win any prompt containing that word in ANY project. It must now
        // be excluded from the global statistical corpus entirely. This corpus
        // reproduces the hijack: before the fix `resolve_skill_for_prompt` returns
        // `learned-rust`; after, it must not.
        let corpus = vec![
            skill(
                "learned-rust",
                "Learned procedures for the rust project from observed command and edit patterns.",
                "When working in the rust project, apply learned procedures instead of re-deriving the workflow.",
            ),
            skill(
                "reviewer",
                "Reviews completed implementation work for production readiness.",
                "Production-readiness review and quality gate after implementation.",
            ),
            skill(
                "git-expert",
                "Safe Git workflow: branching, conflict resolution, history repair.",
                "Version control operations and history repair.",
            ),
        ];
        let prompt = "fix this rust borrow checker error";
        // The statistical tier must not surface the learned skill at all.
        assert_eq!(
            score_prompt_against_skills(prompt, &corpus),
            None,
            "learned-rust must be excluded from the statistical corpus"
        );
        // And the full resolution (statistical + curated) must never return it.
        assert_ne!(
            resolve_skill_for_prompt(prompt, &corpus).map(|m| m.name),
            Some("learned-rust".to_string()),
            "a bare language token must not route to a learned-<project> skill"
        );
        // A non-learned skill still wins on its own name token — the RC7 boost is
        // preserved for real skills.
        assert_eq!(
            score_prompt_against_skills("run the reviewer on this change", &corpus).map(|m| m.name),
            Some("reviewer".to_string()),
            "own-name boost must still work for non-learned skills"
        );
    }

    #[test]
    fn curated_tier_routes_ui_visual_craft_prompts() {
        // Regression for the real-world failure: the two UI/UX skills share
        // almost all their vocabulary, so the IDF matcher ties and stays silent,
        // and an obvious UI prompt got NO inline guidance — the agent then said
        // "I don't have a UI/UX skill registered." Visual-craft phrasing must now
        // route to ui-design-systems via the curated tier.
        for prompt in [
            "make this dashboard look less ai-generated, improve the typography and spacing",
            "tighten the color discipline and visual hierarchy",
            "add a responsive layout for mobile",
            "build out the design system tokens",
            "make this look more polished",
            "build a landing page for my beauty spa",
            "create a dashboard for healthcare analytics",
            "make this React page look better with glassmorphism",
            "fix the contrast and focus states on this button",
            "choose a color palette and typography for the layout",
        ] {
            assert_eq!(
                curated_skill_for_prompt(prompt),
                Some("ui-design-systems-and-responsive-interfaces"),
                "UI visual-craft prompt must route to ui-design-systems: {prompt:?}"
            );
        }
    }

    #[test]
    fn curated_tier_routes_ux_research_prompts() {
        // Research/flow-shaped phrasing routes to ux-research, disjoint from the
        // visual-craft phrases so the two never collide.
        for prompt in [
            "run some user research on the onboarding",
            "map the user journey through checkout",
            "where is the drop-off in the conversion funnel",
            "let's do usability testing on this",
            "sketch a wireframe for the settings page",
        ] {
            assert_eq!(
                curated_skill_for_prompt(prompt),
                Some("ux-research-and-experience-strategy"),
                "UX research prompt must route to ux-research: {prompt:?}"
            );
        }
    }

    #[test]
    fn curated_tier_ui_and_ux_phrase_sets_are_disjoint() {
        // The whole point of splitting them is that no single prompt should
        // satisfy both sets — that disjointness is what stops the original tie
        // from reappearing inside the curated tier. Spot-check the representative
        // phrases do not cross-match.
        assert_eq!(
            curated_skill_for_prompt("improve the typography"),
            Some("ui-design-systems-and-responsive-interfaces")
        );
        assert_eq!(
            curated_skill_for_prompt("map the user journey"),
            Some("ux-research-and-experience-strategy")
        );
    }

    #[test]
    fn auth_audit_operation_precedes_protocol_vocabulary() {
        let corpus = vec![
            skill(
                "authentication-and-identity",
                "build OAuth OIDC authentication and token refresh flows",
                "building login and session flows",
            ),
            skill(
                "security-and-compliance-auditor",
                "security review and compliance evidence for authentication and OAuth",
                "auditing auth and exploitability",
            ),
        ];
        assert_eq!(
            resolve_skill_for_prompt(
                "audit our OAuth/OIDC authentication and token refresh rotation",
                &corpus,
            )
            .map(|found| found.name),
            Some("security-and-compliance-auditor".to_string())
        );
    }

    #[test]
    fn token_efficiency_audit_is_not_misrouted_as_security() {
        let corpus = vec![
            skill(
                "security-and-compliance-auditor",
                "security review for authentication and OAuth",
                "auditing auth and exploitability",
            ),
            skill(
                "systematic-debugging",
                "find root causes and verify broken behavior",
                "debug behavior that is not working",
            ),
        ];
        let prompt = "audit token efficiency because the system map keeps regenerating; find the root cause, fix it, and verify";

        assert_ne!(
            resolve_skill_for_prompt(prompt, &corpus).map(|found| found.name),
            Some("security-and-compliance-auditor".to_string())
        );
    }

    #[test]
    fn adversarial_half_baked_review_routes_to_critic() {
        assert_eq!(
            curated_skill_for_prompt(
                "review this implementation adversarially so half-baked unfinished work cannot pass"
            ),
            Some("critic")
        );
    }

    #[test]
    fn critic_skill_body_phrases_route_to_critic() {
        for prompt in [
            "critique this",
            "what am i missing in this implementation",
            "stress-test this approach before I go further",
            "is this sound before i go further",
            "i need some critics about the current codebases",
        ] {
            assert_eq!(
                curated_skill_for_prompt(prompt),
                Some("critic"),
                "critic skill body phrase must route: {prompt}"
            );
        }
        assert_eq!(
            curated_skill_for_prompt("the critical path is slow"),
            None,
            "critical must not match critic"
        );
    }

    #[test]
    fn diagnosis_operation_precedes_incidental_feature_vocabulary() {
        let corpus = vec![
            skill(
                "memory-status-reporter",
                "Reports memory health, recall index status, learned instincts, and document counts.",
                "Use for memory status and recall health reports.",
            ),
            skill(
                "systematic-debugging",
                "Finds root causes before changing code and verifies fixes with evidence.",
                "Use when behavior is broken or a feature is not working.",
            ),
        ];
        let prompt = "The install does not work: Anvil, learn, memory, and skills are not being used. Check why, find the root cause, fix it, and verify everything.";

        assert_eq!(
            resolve_skill_for_prompt(prompt, &corpus).map(|found| found.name),
            Some("systematic-debugging".to_string())
        );
    }

    #[test]
    fn tokenize_drops_stopwords_and_short_tokens() {
        let tokens = tokenize("Add the Stripe webhook to a PCI flow");
        assert!(tokens.contains("stripe"));
        assert!(tokens.contains("webhook"));
        assert!(tokens.contains("pci"));
        assert!(tokens.contains("flow"));
        assert!(!tokens.contains("add")); // stopword
        assert!(!tokens.contains("the")); // stopword
        assert!(!tokens.contains("to")); // length < 3
    }

    #[test]
    fn frontmatter_parsing_extracts_fields() {
        let source = "---\nname: stripe-integration\ndescription: Stripe Checkout and webhooks.\nwhen_to_use: Payments.\npaths:\n  - \"**/*stripe*.ts\"\n---\n# body\n";
        let model = skill_terms_from_source("stripe-integration", source).expect("parse");
        assert_eq!(model.name, "stripe-integration");
        assert!(model.all_tokens.contains("stripe"));
        assert!(model.all_tokens.contains("checkout"));
        assert!(model.all_tokens.contains("payments"));
        // The indented `paths:` list value must not leak in as a token.
        assert!(!model.all_tokens.contains("ts"));
    }

    #[test]
    fn source_without_frontmatter_is_skipped() {
        assert!(skill_terms_from_source("x", "# no frontmatter here\nbody\n").is_none());
    }

    #[test]
    fn ambiguous_tie_does_not_match() {
        // Two skills share the same distinctive token and nothing else — the
        // margin guard must keep the matcher silent rather than coin-flip.
        let corpus = vec![
            skill("alpha-tool", "shared widget handling", ""),
            skill("beta-tool", "shared widget handling", ""),
        ];
        assert_eq!(score_prompt_against_skills("widget", &corpus), None);
    }

    #[test]
    fn inline_brief_includes_description_and_body() {
        let source = "---\nname: stripe-integration\ndescription: Stripe Checkout, webhooks, and PCI scope.\n---\n# Stripe integration\n\nAlways verify webhook signatures before trusting the event.\n";
        let brief = inline_brief_from_source(source).expect("brief");
        assert!(brief.contains("Stripe Checkout, webhooks, and PCI scope."));
        assert!(brief.contains("verify webhook signatures"));
        // The frontmatter fence itself must not leak into the brief.
        assert!(!brief.contains("---"));
        assert!(!brief.contains("name: stripe-integration"));
    }

    #[test]
    fn inline_brief_truncates_long_body_on_line_boundary() {
        let mut source = String::from("---\ndescription: D.\n---\n");
        // Build a body well over the cap out of distinct numbered lines.
        for n in 0..400 {
            source.push_str(&format!("line {n} with enough text to add bytes\n"));
        }
        let brief = inline_brief_from_source(&source).expect("brief");
        assert!(
            brief.len() <= INLINE_BRIEF_MAX_BYTES + 120,
            "brief length {} exceeds cap + marker allowance",
            brief.len()
        );
        assert!(brief.contains("[skill brief truncated"));
        // Truncation lands on a line boundary: the last content line before the
        // marker is a whole "line N ..." line, never a fragment.
        let before_marker = brief.split("\n\n[skill brief truncated").next().unwrap();
        assert!(before_marker.ends_with("add bytes"));
    }

    #[test]
    fn inline_brief_none_without_content() {
        // Frontmatter only, empty body, blank description → nothing to inject.
        assert_eq!(inline_brief_from_source("---\ndescription:\n---\n"), None);
        assert_eq!(inline_brief_from_source(""), None);
    }

    #[test]
    fn strip_frontmatter_block_returns_body_after_fence() {
        let body = strip_frontmatter_block("---\nname: x\n---\nhello body\n");
        assert_eq!(body.trim(), "hello body");
        // No frontmatter → whole text is the body.
        assert_eq!(strip_frontmatter_block("plain body").trim(), "plain body");
        // Unterminated frontmatter → no usable body.
        assert_eq!(strip_frontmatter_block("---\nname: x\nno close").trim(), "");
    }

    #[test]
    fn skill_inline_brief_rejects_path_traversal() {
        let dir = std::env::temp_dir();
        assert_eq!(skill_inline_brief(&dir, "../escape"), None);
        assert_eq!(skill_inline_brief(&dir, "a/b"), None);
        assert_eq!(skill_inline_brief(&dir, ""), None);
    }

    #[test]
    fn curated_match_requires_the_skill_to_be_installed() {
        // match_skill_for_prompt gates a curated route on the skill actually
        // existing under <home>/skills/. Build a home whose only installed skill
        // is unrelated, then fire a curated-trigger prompt for a skill that is
        // NOT installed: the IDF matcher is silent and the curated branch must
        // return None rather than point at a missing skill.
        let suffix = format!(
            "{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        );
        let home = std::env::temp_dir().join(format!("curated-gate-{suffix}"));
        let only_skill = skills_directory(&home).join("unrelated-skill");
        fs::create_dir_all(&only_skill).unwrap();
        fs::write(
            only_skill.join("SKILL.md"),
            "---\nname: unrelated-skill\ndescription: Something entirely unrelated to reviewing.\nwhen_to_use: Never for reviews.\n---\nbody\n",
        )
        .unwrap();

        // "review this diff" is a curated trigger for `reviewer`, which is not
        // installed here. Must be None, not a dangling pointer.
        assert_eq!(
            match_skill_for_prompt(&home, "please review this diff"),
            None
        );

        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn curated_tier_routes_expanded_specialist_prompts() {
        // s3: the curated tier was extended to cover specialist skills whose
        // vocabulary is high-frequency across the corpus (so the IDF matcher
        // correctly stays silent) yet which obvious phrasing clearly calls for.
        // Each pair is a representative prompt that previously matched nothing.
        let cases: &[(&str, &str)] = &[
            ("help me resolve this merge conflict", "git-expert"),
            ("I need to rebase onto main", "git-expert"),
            (
                "can you do a threat model for this service",
                "security-and-compliance-auditor",
            ),
            (
                "we need a security audit before launch",
                "security-and-compliance-auditor",
            ),
            (
                "what's our test strategy for this release",
                "qa-and-automation-engineer",
            ),
            (
                "set up e2e tests for checkout",
                "qa-and-automation-engineer",
            ),
            ("fix the ci/cd pipeline", "cloud-and-devops-expert"),
            (
                "write a terraform module for the vpc",
                "cloud-and-devops-expert",
            ),
            (
                "design the openapi contract for users",
                "api-contract-design",
            ),
            (
                "is this a breaking change to the api",
                "api-contract-design",
            ),
            (
                "implement the oauth login flow",
                "authentication-and-identity",
            ),
            (
                "rotate the refresh token on reuse",
                "authentication-and-identity",
            ),
            (
                "define an slo and error budget",
                "observability-and-incident-response",
            ),
            (
                "write the runbook for this postmortem",
                "observability-and-incident-response",
            ),
            (
                "bump the version of this dependency",
                "dependency-and-supply-chain",
            ),
            (
                "generate an sbom for the release",
                "dependency-and-supply-chain",
            ),
            (
                "build the etl pipeline into the warehouse",
                "data-and-ml-engineering",
            ),
            (
                "add drift monitoring to model serving",
                "data-and-ml-engineering",
            ),
        ];
        for (prompt, expected) in cases {
            assert_eq!(
                curated_skill_for_prompt(prompt),
                Some(*expected),
                "expanded curated prompt must route correctly: {prompt:?}"
            );
        }
    }

    #[test]
    fn curated_tier_still_silent_on_ordinary_prose_after_expansion() {
        // The expanded phrase sets must not trip on incidental prose — guard
        // against over-broad triggers introduced by the s3 expansion.
        for prompt in [
            "I read the api docs this morning",
            "the team had a great sprint",
            "let's grab coffee and talk about the data",
            "the security guard waved me through",
            "we deployed a new logo to the site",
        ] {
            assert_eq!(
                curated_skill_for_prompt(prompt),
                None,
                "ordinary prose must not trip a curated trigger: {prompt:?}"
            );
        }
    }

    #[test]
    fn statistical_matcher_keeps_generic_and_tie_silence() {
        // Regression guard for the conservative statistical tier the curated
        // expansion sits beside: a purely generic prompt (no distinctive token)
        // and a two-way tie must both still return None. (During s3 we confirmed
        // a MIN_SCORE_FACTOR loosen is inert because the margin guard rejects the
        // df=3 tie it would open, so the floor stays at 0.75 and these guarantees
        // are unchanged.)
        let corpus = sample_corpus();
        assert_eq!(
            score_prompt_against_skills("can you help me write a small function", &corpus),
            None,
            "generic prompt must stay silent"
        );
        let tie_corpus = vec![
            skill("alpha-tool", "shared widget handling", ""),
            skill("beta-tool", "shared widget handling", ""),
        ];
        assert_eq!(
            score_prompt_against_skills("widget", &tie_corpus),
            None,
            "ambiguous tie must stay silent"
        );
    }
    #[test]
    fn skill_composition_returns_none_for_empty_prompt() {
        let temp = std::env::temp_dir().join(format!("keel-skill-comp-{}", std::process::id()));
        assert!(match_skill_composition_for_prompt(&temp, "").is_none());
        assert!(match_skill_composition_for_prompt(&temp, "   ").is_none());
    }

    fn home_with_skills(label: &str, skills: &[(&str, &str)]) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let home = std::env::temp_dir().join(format!(
            "keel-skill-j02-{label}-{}-{unique}",
            std::process::id()
        ));
        let skills_dir = skills_directory(&home);
        for (name, description) in skills {
            write_skill(&skills_dir, name, description);
        }
        home
    }

    #[test]
    fn skill_routing_cache_hit_skips_recompute() {
        // Lower bounds only: concurrent tests share the process-global counters,
        // and a full cache can evict this key between two calls.
        let home = home_with_skills(
            "hit",
            &[
                ("reviewer", "review code diffs carefully"),
                ("planner", "plan project tasks roadmaps"),
            ],
        );
        let prompt = "reviewer review this code diff j02-hit";
        let skills_dir = skills_directory(&home);
        let key = skill_routing_cache_key(&skills_dir, prompt).expect("listing fingerprint");
        let before = skill_routing_cache_stats();
        let first = match_skill_for_prompt_with_details(&home, prompt);
        // why: presence under this key is deterministic where a global hit
        // counter is not, and it is what makes the repeat prompt cheap.
        assert!(
            skill_routing_cache_get(&key, now_unix_secs()).is_some(),
            "the first call must publish the decision under its key"
        );
        let after = skill_routing_cache_stats();
        assert!(
            after.misses > before.misses,
            "first call must miss at least once"
        );
        assert!(
            after.recomputes > before.recomputes,
            "one miss must recompute at least once"
        );
        assert!(
            after.hits > before.hits,
            "the repeated lookup must count as a hit"
        );
        let second = match_skill_for_prompt_with_details(&home, prompt);
        assert_eq!(first, second, "a cache hit returns the same decision");
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn skill_routing_cache_invalidates_on_catalog_change() {
        let home = home_with_skills("inval", &[("reviewer", "review code diffs carefully")]);
        let skills_dir = skills_directory(&home);
        let prompt = "reviewer review this code diff j02-inval";
        let key_before =
            skill_routing_cache_key(&skills_dir, prompt).expect("listing fingerprint must exist");
        let before = skill_routing_cache_stats();
        let _ = match_skill_for_prompt_with_details(&home, prompt);
        // Any catalog add/remove/edit changes the listing fingerprint, which
        // is the deterministic invalidation signal (no counters involved).
        write_skill(&skills_dir, "planner", "plan project tasks roadmaps");
        let key_after =
            skill_routing_cache_key(&skills_dir, prompt).expect("listing fingerprint must exist");
        assert_ne!(
            key_before, key_after,
            "catalog change must change the cache key"
        );
        let _ = match_skill_for_prompt_with_details(&home, prompt);
        let after = skill_routing_cache_stats();
        let miss_delta = after.misses.saturating_sub(before.misses);
        assert!(
            miss_delta >= 2,
            "both calls must miss across a catalog change"
        );
        // No hits assertion on the global counter: concurrent tests share it.
        // Key inequality above is the deterministic invalidation proof.
        let _ = fs::remove_dir_all(&home);
    }

    /// Skill names shared by the reconcile tests, so each name has one owner.
    const REVIEWER_SKILL: &str = "reviewer";
    const CRITIC_SKILL: &str = "critic";

    #[test]
    fn reconcile_skill_match_outcomes_treats_silence_as_unknown() {
        let home = home_with_skills("reconcile", &[(REVIEWER_SKILL, "review code diffs")]);
        record_pending_skill_match(&home, REVIEWER_SKILL, 0.8);
        assert_eq!(
            reconcile_skill_match_outcomes(&home),
            0,
            "a silently used skill leaves no citation, so no outcome may be recorded"
        );
        let rate = crate::utility::skill_usage::skill_success_rate(&home, REVIEWER_SKILL);
        assert!(
            rate >= 0.5,
            "an unknown outcome must not move the success rate, got {rate}"
        );
        assert_eq!(
            reconcile_skill_match_outcomes(&home),
            0,
            "the pending ledger is consumed"
        );
        let _ = fs::remove_dir_all(&home);
    }

    /// A citation for one candidate is positive evidence the session followed a
    /// different routing decision, so the rest of that batch is a real miss.
    #[test]
    fn reconcile_skill_match_outcomes_records_failure_for_an_uncited_peer() {
        let home = home_with_skills(
            "reconcile-peer",
            &[
                (REVIEWER_SKILL, "review code diffs"),
                (CRITIC_SKILL, "critique an implementation"),
            ],
        );
        record_pending_skill_match(&home, CRITIC_SKILL, 0.8);
        record_pending_skill_match(&home, REVIEWER_SKILL, 0.8);
        crate::runner::observation::record_observation_from_parts(
            &home,
            "Bash",
            r#"{"command":"keel skill_get reviewer"}"#,
            "/",
            "sess-peer",
            false,
        )
        .expect("record observation");
        assert_eq!(
            reconcile_skill_match_outcomes(&home),
            2,
            "both peers are resolved when one of them is cited"
        );
        let missed = crate::utility::skill_usage::skill_success_rate(&home, CRITIC_SKILL);
        assert!(
            missed < 0.5,
            "the uncited peer must record a miss, got {missed}"
        );
        let hit = crate::utility::skill_usage::skill_success_rate(&home, REVIEWER_SKILL);
        assert!(hit > 0.5, "the cited skill must record a hit, got {hit}");
        let _ = fs::remove_dir_all(&home);
    }

    /// The pending ledger serializes concurrent writers: without the lock the
    /// read-modify-write loses entries under contention.
    #[test]
    fn concurrent_pending_match_writes_keep_every_entry() {
        let home = home_with_skills("pending-lock", &[(REVIEWER_SKILL, "review code diffs")]);
        let home_ref = &home;
        std::thread::scope(|scope| {
            for index in 0..8 {
                scope.spawn(move || {
                    record_pending_skill_match(
                        home_ref,
                        REVIEWER_SKILL,
                        0.8 + index as f64 / 1000.0,
                    );
                });
            }
        });
        let text = fs::read_to_string(skill_match_pending_path(&home)).expect("read ledger");
        let pending: Vec<PendingSkillMatch> = serde_json::from_str(&text).expect("parse ledger");
        assert_eq!(pending.len(), 8, "every concurrent append must survive");
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn skill_composition_composes_distinct_strong_domains() {
        let home = home_with_skills(
            "compose",
            &[
                ("alpha-tool", "alpha widget forging furnaces"),
                ("beta-tool", "beta gadget welding workshops"),
            ],
        );
        let decision = match_skill_composition_for_prompt(&home, "alpha-tool beta-tool assistance")
            .expect("dual-domain prompt must resolve");
        match &decision.choice {
            crate::utility::decision::SkillCompositionChoice::Compose(pair) => {
                assert_eq!(pair.len(), 2, "composition stays pairwise")
            }
            other => panic!("expected Compose, got {other:?}"),
        }
        assert!(
            decision.confidence > 0.0 && decision.confidence <= 0.95,
            "confidence must be a bounded probability, got {}",
            decision.confidence
        );
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn decision_cache_routing_speedup_over_recompute() {
        // Plan J02: cached beats recompute (~1.3x on 25 skills: both paths
        // are file-IO dominated). Directional assertion only; the best-of-rounds
        // comparison keeps a loaded hosted runner from inverting the result.
        let bulk: Vec<(String, String)> = (0..23)
            .map(|i| {
                (
                    format!("bulk-skill-{i}"),
                    format!("bulk domain vocabulary number {i} zebrafinch"),
                )
            })
            .collect();
        let mut refs: Vec<(&str, &str)> = vec![
            ("reviewer", "review code diffs carefully"),
            ("planner", "plan project tasks roadmaps"),
        ];
        refs.extend(
            bulk.iter()
                .map(|(name, description)| (name.as_str(), description.as_str())),
        );
        let home = home_with_skills("bench", &refs);
        let prompt = "reviewer review this code diff j02-bench";
        let _warmup = match_skill_for_prompt_with_details(&home, prompt);
        let rounds = 5u32;
        let iterations = 12u32;
        let mut cached_best = std::time::Duration::MAX;
        let mut uncached_best = std::time::Duration::MAX;
        for round in 0..rounds {
            let round_prompt = format!("{prompt} round-{round}");
            let _cached = match_skill_for_prompt_with_details(&home, &round_prompt);
            let start = std::time::Instant::now();
            for _ in 0..iterations {
                let _routing = match_skill_for_prompt_with_details(&home, &round_prompt);
            }
            cached_best = cached_best.min(start.elapsed());
            let start = std::time::Instant::now();
            for i in 0..iterations {
                let unique = format!("{round_prompt} miss-{i}");
                let _uncached = match_skill_for_prompt_with_details(&home, &unique);
            }
            uncached_best = uncached_best.min(start.elapsed());
        }
        println!(
            "routing cache: best cached={cached_best:?} best uncached={uncached_best:?} ratio={:.1}x",
            uncached_best.as_secs_f64() / cached_best.as_secs_f64().max(f64::EPSILON)
        );
        assert!(
            cached_best < uncached_best,
            "cached routing must beat recomputation: best cached={cached_best:?} best uncached={uncached_best:?}"
        );
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn low_calibrated_confidence_stays_silent() {
        // Plan J03 acceptance: sub-0.60 calibrated confidence never auto-applies.
        let skills = &[("reviewer", "review code diffs carefully")];
        let prompt = "reviewer review this code diff j03-gate";
        let clean = home_with_skills("gate-clean", skills);
        assert!(
            match_skill_for_prompt_with_details(&clean, prompt).is_some(),
            "probe prompt must match on a clean home"
        );
        let poisoned = home_with_skills("gate-poisoned", skills);
        // Eighty failures at 1.0 drag bin 9 below the 0.60 gate.
        for _ in 0..80 {
            crate::utility::decision::record_and_save_skill_calibration(
                &poisoned, "reviewer", 1.0, false,
            )
            .expect("record calibration");
        }
        assert!(
            match_skill_for_prompt_with_details(&poisoned, prompt).is_none(),
            "sub-0.60 calibrated confidence must stay silent"
        );
        let _ = fs::remove_dir_all(&clean);
        let _ = fs::remove_dir_all(&poisoned);
    }

    #[test]
    fn a_weak_term_match_does_not_veto_the_head() {
        // Plan J03 used to silence a sub-0.60 term match unless the head named
        // the same skill, which deleted correct head answers on the host corpus.
        let skills = &[
            ("reviewer", "review code diffs carefully"),
            ("planner", "plan project tasks roadmaps"),
        ];
        let prompt = "reviewer review this code diff j03-veto";
        let home = home_with_skills("gate-head", skills);
        write_head_artifact(&home, "planner", &["review", "code", "diff"]);
        for _ in 0..80 {
            crate::utility::decision::record_and_save_skill_calibration(
                &home, "reviewer", 1.0, false,
            )
            .expect("record calibration");
        }
        let found = match_skill_for_prompt_with_details(&home, prompt)
            .expect("the head answers even though the term match was weak");
        assert_eq!(
            found.name, "planner",
            "the head's own answer is returned, not the vetoed term match"
        );
        assert!(
            found.confidence > 0.0,
            "the returned confidence is the head's, not the term model's: {}",
            found.confidence
        );
        let _ = fs::remove_dir_all(&home);
    }

    /// A minimal usable artifact: no word vectors, no centroids, accept at zero,
    /// and one expert holding the given terms so the head ranks them.
    fn write_head_artifact(home: &Path, skill: &str, terms: &[&str]) {
        use crate::utility::lexical_experts::{
            HeldOut, LexicalExpert, LexicalModel, LEXICAL_SCHEMA,
        };
        let artifact = crate::utility::lexical_experts::artifact_path(home);
        if let Some(parent) = artifact.parent() {
            fs::create_dir_all(parent).expect("artifact dir");
        }
        let model = LexicalModel {
            schema: LEXICAL_SCHEMA,
            training_rows: 100,
            skills: 1,
            experts: vec![LexicalExpert {
                name: skill.to_string(),
                bias: 0.0,
                terms: terms
                    .iter()
                    .map(|term| ((*term).to_string(), 5.0))
                    .collect(),
            }],
            idf: terms
                .iter()
                .map(|term| ((*term).to_string(), 1.0))
                .collect(),
            held_out: Some(HeldOut {
                rows: 10,
                decided: 10,
                correct: 9,
                accuracy: 0.9,
                brier: 0.1,
                scale: 1.0,
                accept: 0.0,
                accept_per_skill: Vec::new(),
                operating_points: Vec::new(),
                ece: 0.0,
                entropy_temperatures: Vec::new(),
                per_class: Vec::new(),
                macro_f1: 0.0,
                weighted_f1: 0.0,
                confusion: Default::default(),
                probe_rejection: 0.0,
            }),
            usable: true,
            vector_rows: 0,
            embedding_dim: 0,
            centroids: Vec::new(),
        };
        fs::write(
            &artifact,
            serde_json::to_string(&model).expect("serialize model"),
        )
        .expect("write model");
    }

    #[test]
    fn curated_confirmation_needs_agreement_and_clean_history() {
        let home = home_with_skills("confirm", &[("reviewer", "review code diffs carefully")]);
        assert!(confirmed_by_curated_tier(
            "please review this diff",
            "reviewer",
            &home
        ));
        assert!(!confirmed_by_curated_tier(
            "please review this diff",
            "planner",
            &home
        ));
        assert!(!confirmed_by_curated_tier(
            "what time is it",
            "reviewer",
            &home
        ));
        for _ in 0..5 {
            crate::utility::decision::record_and_save_skill_calibration(
                &home, "reviewer", 0.8, false,
            )
            .expect("record calibration");
        }
        assert!(!confirmed_by_curated_tier(
            "please review this diff",
            "reviewer",
            &home
        ));
        let _ = fs::remove_dir_all(&home);
        // A used-and-correct skill keeps its confirmation: only failures shut it.
        const PROVEN: &str = "reviewer";
        let proven = home_with_skills("confirm-proven", &[(PROVEN, "review code diffs")]);
        for _ in 0..6 {
            crate::utility::decision::record_and_save_skill_calibration(&proven, PROVEN, 0.8, true)
                .expect("record calibration");
        }
        assert!(
            confirmed_by_curated_tier("please review this diff", PROVEN, &proven),
            "six correct outcomes must not read as a poisoned record"
        );
        let _ = fs::remove_dir_all(&proven);
    }

    #[test]
    fn curated_confirmation_routes_weak_margin_match() {
        // Release-rail regression: curated-confirmed weak-margin winner routes.
        let home = home_with_skills(
            "confirm-weak",
            &[
                ("reviewer", "review code diffs carefully before merge"),
                ("review-assistant", "help review code diffs and changes"),
                ("planner", "plan project tasks roadmaps"),
            ],
        );
        let prompt = "review this code before we merge the feature";
        assert_eq!(
            curated_skill_for_prompt(prompt),
            Some("reviewer"),
            "probe prompt must hit the curated tier"
        );
        let found = match_skill_for_prompt_with_details(&home, prompt);
        let found = found.expect("confirmed weak-margin match must route");
        assert_eq!(found.name, "reviewer");
        assert!(
            found.confidence < 0.60,
            "probe must stay below the gate, got {}",
            found.confidence
        );
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn reconcile_skill_match_outcomes_records_success_when_cited() {
        let home = home_with_skills("reconcile-cited", &[("reviewer", "review code diffs")]);
        record_pending_skill_match(&home, "reviewer", 0.8);
        crate::runner::observation::record_observation_from_parts(
            &home,
            "Bash",
            r#"{"command":"keel skill_get reviewer"}"#,
            "/",
            "sess-cited",
            false,
        )
        .expect("record observation");
        assert_eq!(reconcile_skill_match_outcomes(&home), 1);
        let rate = crate::utility::skill_usage::skill_success_rate(&home, "reviewer");
        assert!(rate > 0.5, "cited routing must record success, got {rate}");
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn reconcile_composition_outcomes_scores_choice_kinds() {
        let home = home_with_skills(
            "comp-out",
            &[
                ("alpha-tool", "alpha widget forging furnaces"),
                ("beta-tool", "beta gadget welding workshops"),
            ],
        );
        let a = "alpha-tool".to_string();
        let b = "beta-tool".to_string();
        let pair = [a.clone(), b.clone()];
        let solo = [a.clone()];
        record_pending_composition(&home, "compose", &pair, &pair, 0.85);
        record_pending_composition(&home, "single", &solo, &solo, 0.9);
        record_pending_composition(&home, "generic", &[], &pair, 0.8);
        // Only alpha-tool is cited: single correct, compose and generic wrong.
        crate::runner::observation::record_observation_from_parts(
            &home,
            "Bash",
            r#"{"command":"keel skill_get alpha-tool"}"#,
            "/",
            "sess-comp",
            false,
        )
        .expect("record observation");
        assert_eq!(reconcile_composition_outcomes(&home), 3);
        let (total, precision, compose_total, compose_precision) =
            composition_calibration_stats(&home);
        assert_eq!((total, compose_total), (3, 1));
        assert!((precision - 1.0 / 3.0).abs() < 1e-12, "got {precision}");
        assert_eq!(compose_precision, 0.0);
        assert_eq!(reconcile_composition_outcomes(&home), 0);
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn test_quarantined_skill_is_filtered_from_general_routing() {
        let home = home_with_skills(
            "quarantine-test",
            &[
                (
                    "flaky-auditor",
                    "specialized audit checking routines and diagnostics",
                ),
                (
                    "stable-helper",
                    "general maintenance utilities and cleanup helpers",
                ),
            ],
        );

        // First verify it matches normally
        let match_normal = match_skill_for_prompt_with_details(&home, "perform audit checking");
        assert!(match_normal.is_some(), "should match before quarantine");

        // Now quarantine the skill in PriorStore
        let mut store = crate::utility::decision::load_prior_store(&home);
        store
            .skills
            .entry("flaky-auditor".to_string())
            .or_default()
            .is_quarantined = true;
        crate::utility::decision::save_prior_store(&home, &store).expect("save priors");
        clear_skill_routing_cache();

        // General prompt matching description must now be ignored (quarantined)
        let match_quarantined =
            match_skill_for_prompt_with_details(&home, "perform audit checking");
        assert!(
            match_quarantined.is_none(),
            "quarantined skill must be excluded from general routing"
        );

        // But explicit mention by name still allows it
        let match_explicit =
            match_skill_for_prompt_with_details(&home, "use flaky-auditor to check");
        assert!(
            match_explicit.is_some(),
            "explicit skill name mention should bypass quarantine"
        );

        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn test_semantic_centroid_fallback_routing() {
        let terms = vec![SkillTerms {
            name: "flutter-build-responsive-layout".to_string(),
            all_tokens: ["flutter", "adaptive", "mediaquery"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            name_tokens: ["flutter", "responsive", "layout"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
        }];
        let catalog = vec![
            SkillCatalogEntry {
                name: "flutter-build-responsive-layout".to_string(),
                description: "Build responsive adaptive layouts across screen sizes using constraints and flexbox".to_string(),
                when_to_use: "Use when designing flexible mobile and desktop user interfaces".to_string(),
                use_count: 0,
                related_skills: Vec::new(),
                capabilities: vec!["ui".to_string(), "layout".to_string()],
                version: "1.0.0".to_string(),
                dependencies: Vec::new(),
                activation_cost_tokens: 10,
                task_criticality: 0.5,
                historical_success: 0.8,
                is_quarantined: false,
            },
        ];

        // Prompt with semantic overlap but no distinctive rare tokens for BM25
        let prompt = "How do I make the UI stretch flexibly across different monitor resolutions?";
        let decision = resolve_skill_selection(prompt, &terms, &catalog);
        assert!(
            decision.is_some(),
            "semantic centroid fallback should match conceptual prompt"
        );
        let matched = decision.unwrap();
        assert_eq!(matched.name, "flutter-build-responsive-layout");
        assert!(matched.reason.contains("semantic centroid match"));
    }
}
