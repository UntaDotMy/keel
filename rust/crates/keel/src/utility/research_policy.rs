//! Purpose: Validate planner research provenance and time-sensitive source freshness.
//! Caller: utility::plan and review::diff_gates.
//! Dependencies: chrono, serde_json, and optional project policy in keel.filters.toml.
//! Main Functions: ResearchPolicy::load, validate_research_artifact.
//! Side Effects: Reads project policy; validation itself is pure.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use chrono::{DateTime, NaiveDate, Utc};
use serde::Deserialize;
use serde_json::Value;

/// Fallback window for an externally fresh source whose type declares none.
pub(crate) const DEFAULT_MAX_AGE_DAYS: i64 = 90;

/// Hard bounds for the compact research artifact. These mirror the existing
/// recall input/result limits: research may retain the evidence needed to
/// explain a decision, but cannot turn a plan artifact into a raw source dump.
pub(crate) const MAX_RESEARCH_ARTIFACT_BYTES: usize = 32 * 1024;
pub(crate) const MAX_RESEARCH_QUERY_BYTES: usize = 4 * 1024;
pub(crate) const MAX_RESEARCH_SOURCES: usize = 8;
pub(crate) const MAX_RESEARCH_CLAIMS: usize = 16;
pub(crate) const MAX_RESEARCH_CONFLICTS: usize = 8;
pub(crate) const MAX_RESEARCH_USED_BY: usize = 32;
pub(crate) const MAX_RESEARCH_REFERENCES: usize = 32;
pub(crate) const MAX_RESEARCH_ID_BYTES: usize = 128;
pub(crate) const MAX_RESEARCH_URL_BYTES: usize = 2 * 1024;
pub(crate) const MAX_RESEARCH_EVIDENCE_BYTES: usize = 4 * 1024;
pub(crate) const MAX_RESEARCH_CLAIM_BYTES: usize = 4 * 1024;
pub(crate) const MAX_RESEARCH_RATIONALE_BYTES: usize = 4 * 1024;

/// The evidence class required by the submitted research query.
///
/// This is intentionally a small, exact-token classifier. It does not claim
/// to understand arbitrary natural language; it provides a conservative
/// admission boundary so local checkout evidence cannot masquerade as current
/// external evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResearchRequirement {
    ExternalCurrent,
    LocalCurrent,
    Stable,
}

pub(crate) fn classify_research_requirement(request: &str) -> ResearchRequirement {
    let words: BTreeSet<String> = request
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(|word| word.to_ascii_lowercase())
        .collect();
    let has_any = |terms: &[&str]| terms.iter().any(|term| words.contains(*term));
    let external_terms = [
        "advisory",
        "api",
        "cloud",
        "dependency",
        "endpoint",
        "external",
        "library",
        "mcp",
        "package",
        "platform",
        "product",
        "protocol",
        "release",
        "sdk",
        "security",
        "service",
        "specification",
        "standard",
        "vendor",
        "version",
        "vulnerability",
        "web",
    ];
    let local_terms = [
        "branch",
        "checkout",
        "code",
        "crate",
        "file",
        "filesystem",
        "implementation",
        "local",
        "path",
        "repo",
        "repository",
        "source",
        "test",
        "tests",
        "workspace",
    ];
    let current_terms = [
        "current", "latest", "live", "newest", "now", "recent", "today",
    ];

    // Explicit external nouns win over local wording (for example, "current
    // repository dependency state" still depends on external package facts).
    if has_any(&external_terms) || (has_any(&current_terms) && !has_any(&local_terms)) {
        ResearchRequirement::ExternalCurrent
    } else if has_any(&local_terms) || has_any(&current_terms) {
        ResearchRequirement::LocalCurrent
    } else {
        ResearchRequirement::Stable
    }
}

/// Per-source-type freshness windows. The plan forbids one universal window for
/// every fact: evidence whose subject changes fast must be re-checked sooner
/// than evidence that describes a released artifact.
///
/// - `official-doc`: version-bound. Documentation describes released behavior,
///   so the window is long enough to cover a release cadence.
/// - `repository`: live state. A repository's default branch moves continuously,
///   so a shallow window is used.
/// - `issue`: rapid. Issue status is the fastest-changing evidence Keel reads.
fn default_window_days(source_type: &str) -> i64 {
    match source_type {
        "official-doc" => 90,
        "repository" => 30,
        "issue" => 7,
        _ => DEFAULT_MAX_AGE_DAYS,
    }
}

type Issues = Vec<String>;
type Uses<'a> = BTreeSet<&'a str>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct ResearchPolicy {
    /// Operator override applied to every externally fresh source. When unset,
    /// each source type uses its own declared window.
    pub max_age_days: Option<i64>,
}

impl ResearchPolicy {
    /// The freshness window for one source type: the explicit override when the
    /// operator set one, otherwise the type's declared window.
    fn window_days_for(&self, source_type: &str) -> i64 {
        self.max_age_days
            .unwrap_or_else(|| default_window_days(source_type))
    }
}

#[derive(Debug, Default, Deserialize)]
struct ProjectPolicyFile {
    #[serde(default)]
    research: ProjectResearchPolicy,
}

#[derive(Debug, Default, Deserialize)]
struct ProjectResearchPolicy {
    max_age_days: Option<i64>,
}

struct FreshnessCheck<'a> {
    source_type: &'a str,
    freshness: &'a str,
    source: &'a Value,
    retrieved_at: Option<DateTime<Utc>>,
    policy: ResearchPolicy,
    now: DateTime<Utc>,
    id: &'a str,
}

impl ResearchPolicy {
    pub(crate) fn load(workspace_root: &Path) -> Result<Self, String> {
        let policy_path = workspace_root.join("keel.filters.toml");
        if !policy_path.is_file() {
            return Ok(Self::default());
        }
        let body = fs::read_to_string(&policy_path)
            .map_err(|error| format!("read {}: {error}", policy_path.display()))?;
        let project: ProjectPolicyFile = toml::from_str(&body)
            .map_err(|error| format!("parse {}: {error}", policy_path.display()))?;
        let max_age_days = project.research.max_age_days;
        if max_age_days.is_some_and(|days| days <= 0) {
            return Err(format!(
                "{} research.max_age_days must be greater than zero; freshness cannot be disabled",
                policy_path.display()
            ));
        }
        Ok(Self { max_age_days })
    }
}

pub(crate) fn validate_research_artifact(
    research: &Value,
    uses: &Uses<'_>,
    policy: ResearchPolicy,
    now: DateTime<Utc>,
) -> Issues {
    let mut issues = Vec::new();
    validate_artifact_bounds(research, &mut issues);
    if string_field(research, "status") != Some("complete") {
        issues.push("research.json status is not complete".to_string());
    }

    let query = required_field(research, "query", "research.json", &mut issues);
    let research_source = required_field(research, "researchSource", "research.json", &mut issues);
    validate_bounded_text(
        research,
        "researchSource",
        "research.json",
        MAX_RESEARCH_ID_BYTES,
        &mut issues,
    );
    let truncated = match research.get("truncated") {
        Some(Value::Bool(value)) => Some(*value),
        Some(_) => {
            issues.push("research.json truncated must be a boolean".to_string());
            None
        }
        None => {
            issues.push("research.json has no truncated flag".to_string());
            None
        }
    };
    if truncated == Some(true) {
        issues.push("research.json is truncated; complete research must be refreshed".to_string());
    }

    let mut source_ids = BTreeSet::new();
    let sources = match value_array(research, "sources") {
        Some(sources) if !sources.is_empty() => {
            for source in sources.iter().take(MAX_RESEARCH_SOURCES) {
                validate_source(source, uses, policy, now, &mut source_ids, &mut issues);
            }
            Some(&sources[..sources.len().min(MAX_RESEARCH_SOURCES)])
        }
        Some(_) => {
            issues.push("research.json has no source records".to_string());
            Some(&[] as &[Value])
        }
        None => {
            issues.push("research.json has no source records".to_string());
            None
        }
    };
    if let Some(request_source) = research.get("requestSource") {
        validate_source(
            request_source,
            uses,
            policy,
            now,
            &mut source_ids,
            &mut issues,
        );
    }

    if let (Some(query), Some(research_source), Some(sources)) = (query, research_source, sources) {
        validate_research_origin(
            research_source,
            classify_research_requirement(query),
            sources,
            &mut issues,
        );
    }

    let Some(claims) = value_array(research, "claims") else {
        issues.push("research.json has no claims array".to_string());
        return issues;
    };
    if claims.is_empty() {
        issues.push("research.json has no factual claims".to_string());
    }
    let bounded_claims = &claims[..claims.len().min(MAX_RESEARCH_CLAIMS)];
    let mut claim_ids = BTreeSet::new();
    for claim in bounded_claims {
        validate_claim(claim, uses, &source_ids, &mut claim_ids, &mut issues);
    }
    validate_claim_relations(research, bounded_claims, &claim_ids, &mut issues);
    issues
}

fn validate_artifact_bounds(research: &Value, issues: &mut Issues) {
    match serde_json::to_vec(research) {
        Ok(serialized) if serialized.len() > MAX_RESEARCH_ARTIFACT_BYTES => issues.push(format!(
            "research.json exceeds the {MAX_RESEARCH_ARTIFACT_BYTES}-byte compact-artifact bound"
        )),
        Err(error) => issues.push(format!(
            "research.json cannot be serialized for bounds: {error}"
        )),
        Ok(_) => {}
    }
    validate_optional_bounded_text(
        research,
        "query",
        "research.json",
        MAX_RESEARCH_QUERY_BYTES,
        issues,
    );
    for (field_name, maximum) in [
        ("sources", MAX_RESEARCH_SOURCES),
        ("claims", MAX_RESEARCH_CLAIMS),
        ("conflicts", MAX_RESEARCH_CONFLICTS),
    ] {
        if let Some(value) = research.get(field_name) {
            match value.as_array() {
                Some(values) if values.len() > maximum => issues.push(format!(
                    "research.json {field_name} count {} exceeds the {maximum}-item bound",
                    values.len()
                )),
                Some(_) => {}
                None => issues.push(format!("research.json {field_name} must be an array")),
            }
        }
    }
}

fn validate_research_origin(
    origin: &str,
    requirement: ResearchRequirement,
    sources: &[Value],
    issues: &mut Issues,
) {
    if !matches!(origin, "host" | "cache" | "local-index") {
        issues.push(format!(
            "research.json researchSource {origin:?} is unsupported; expected host, cache, or local-index"
        ));
        return;
    }

    let external_sources = sources
        .iter()
        .filter(|source| {
            !matches!(
                string_field(source, "sourceType"),
                Some("local-code" | "user-request")
            )
        })
        .count();
    match origin {
        "local-index" => {
            if sources.iter().any(|source| {
                string_field(source, "sourceType") != Some("local-code")
                    || string_field(source, "freshness") != Some("local-only")
            }) {
                issues.push(
                    "research.json local-index origin requires only local-code/local-only sources"
                        .to_string(),
                );
            }
            if requirement == ResearchRequirement::ExternalCurrent {
                issues.push(
                    "current external research cannot use local-index evidence; host research or a fresh cache record is required"
                        .to_string(),
                );
            }
        }
        "cache" => {
            for source in sources {
                if string_field(source, "cacheId").is_none() {
                    issues.push(format!(
                        "{} cache-origin source has no cacheId",
                        string_field(source, "sourceId").unwrap_or("research source")
                    ));
                }
            }
            if requirement == ResearchRequirement::ExternalCurrent && external_sources == 0 {
                issues.push(
                    "current external research cache evidence must include an external source"
                        .to_string(),
                );
            }
        }
        "host" if requirement == ResearchRequirement::ExternalCurrent && external_sources == 0 => {
            issues.push(
                "current external research requires an external host source; local-only evidence is insufficient"
                    .to_string(),
            );
        }
        "host" => {}
        _ => unreachable!("research origin was checked above"),
    }
}

fn validate_bounded_text(
    value: &Value,
    field_name: &str,
    id: &str,
    maximum_bytes: usize,
    issues: &mut Issues,
) {
    if let Some(field) = value.get(field_name) {
        match field.as_str() {
            Some(text) if text.len() > maximum_bytes => issues.push(format!(
                "{id} {field_name} exceeds the {maximum_bytes}-byte bound"
            )),
            Some(_) => {}
            None => issues.push(format!("{id} {field_name} must be a string")),
        }
    }
}

fn validate_optional_bounded_text(
    value: &Value,
    field_name: &str,
    id: &str,
    maximum_bytes: usize,
    issues: &mut Issues,
) {
    if value.get(field_name).is_some_and(|field| !field.is_null()) {
        validate_bounded_text(value, field_name, id, maximum_bytes, issues);
    }
}

fn validate_bounded_string_array(
    value: &Value,
    field_name: &str,
    id: &str,
    maximum_items: usize,
    maximum_item_bytes: usize,
    issues: &mut Issues,
) {
    let Some(field) = value.get(field_name) else {
        return;
    };
    let Some(items) = field.as_array() else {
        issues.push(format!("{id} {field_name} must be an array"));
        return;
    };
    if items.len() > maximum_items {
        issues.push(format!(
            "{id} {field_name} count {} exceeds the {maximum_items}-item bound",
            items.len()
        ));
    }
    for item in items {
        match item.as_str() {
            Some(text) if text.len() > maximum_item_bytes => issues.push(format!(
                "{id} {field_name} item exceeds the {maximum_item_bytes}-byte bound"
            )),
            Some(_) => {}
            None => issues.push(format!("{id} {field_name} items must be strings")),
        }
    }
}

fn validate_claim_relations(
    research: &Value,
    claims: &[Value],
    ids: &BTreeSet<&str>,
    issues: &mut Issues,
) {
    for claim in claims {
        let id = string_field(claim, "claimId").unwrap_or_default();
        let mut next = string_field(claim, "supersedes");
        let mut visited = BTreeSet::from([id]);
        while let Some(previous) = next {
            if !ids.contains(previous) {
                issues.push(format!("{id} supersedes unknown claim {previous}"));
                break;
            }
            if !visited.insert(previous) {
                issues.push(format!("{id} has a supersession cycle"));
                break;
            }
            next = claims
                .iter()
                .find(|item| string_field(item, "claimId") == Some(previous))
                .and_then(|item| string_field(item, "supersedes"));
        }
    }
    let Some(conflicts) = research.get("conflicts") else {
        return;
    };
    let Some(conflicts) = conflicts.as_array() else {
        issues.push("research conflicts must be an array".into());
        return;
    };
    for conflict in conflicts.iter().take(MAX_RESEARCH_CONFLICTS) {
        validate_bounded_string_array(
            conflict,
            "claimIds",
            "research conflict",
            MAX_RESEARCH_REFERENCES,
            MAX_RESEARCH_ID_BYTES,
            issues,
        );
        let references = string_array(conflict, "claimIds");
        let distinct: BTreeSet<&str> = references.iter().map(String::as_str).collect();
        if distinct.len() < 2 || distinct.iter().any(|id| !ids.contains(id)) {
            issues.push("research conflict requires at least two distinct known claimIds".into());
        }
        if string_field(conflict, "status") != Some("resolved") {
            issues.push("research has an unresolved claim conflict".into());
            continue;
        }
        required_field(conflict, "rationale", "research conflict", issues);
        validate_optional_bounded_text(
            conflict,
            "rationale",
            "research conflict",
            MAX_RESEARCH_RATIONALE_BYTES,
            issues,
        );
        let selected = required_field(conflict, "resolvedByClaimId", "research conflict", issues);
        if !selected.is_some_and(|id| {
            ids.contains(id)
                && claims.iter().any(|claim| {
                    string_field(claim, "claimId") == Some(id)
                        && string_field(claim, "classification") == Some("verified")
                })
        }) {
            issues.push("research conflict resolution requires a verified claim".into());
        }
    }
}

fn validate_source<'a>(
    source: &'a Value,
    uses: &Uses<'_>,
    policy: ResearchPolicy,
    now: DateTime<Utc>,
    source_ids: &mut BTreeSet<&'a str>,
    issues: &mut Issues,
) {
    let id = string_field(source, "sourceId").unwrap_or("source without id");
    if id == "source without id" {
        issues.push("research source has no sourceId".to_string());
    } else if !source_ids.insert(id) {
        issues.push(format!("research source ID {id} is duplicated"));
    }

    let source_type = required_field(source, "sourceType", id, issues);
    let source_url = required_field(source, "sourceUrl", id, issues);
    required_field(source, "support", id, issues);
    let freshness = required_field(source, "freshness", id, issues);
    validate_bounded_text(source, "sourceId", id, MAX_RESEARCH_ID_BYTES, issues);
    validate_bounded_text(source, "sourceType", id, MAX_RESEARCH_ID_BYTES, issues);
    validate_bounded_text(source, "sourceUrl", id, MAX_RESEARCH_URL_BYTES, issues);
    validate_bounded_text(source, "support", id, MAX_RESEARCH_EVIDENCE_BYTES, issues);
    validate_optional_bounded_text(source, "sourceVersion", id, MAX_RESEARCH_ID_BYTES, issues);
    validate_optional_bounded_text(source, "requiredVersion", id, MAX_RESEARCH_ID_BYTES, issues);
    validate_optional_bounded_text(source, "cacheId", id, MAX_RESEARCH_ID_BYTES, issues);
    validate_bounded_string_array(
        source,
        "usedBy",
        id,
        MAX_RESEARCH_USED_BY,
        MAX_RESEARCH_ID_BYTES,
        issues,
    );
    validate_used_by(source, id, uses, issues);
    validate_publication_date(source, id, issues);

    let retrieved_at = required_field(source, "retrievedAt", id, issues)
        .and_then(|value| parse_retrieved_at(value, id, issues));
    if retrieved_at.is_some_and(|retrieved| retrieved > now + chrono::Duration::minutes(5)) {
        issues.push(format!("{id} retrievedAt is in the future"));
    }
    if let (Some(kind), Some(url)) = (source_type, source_url) {
        validate_source_location(kind, url, id, issues);
    }
    if let (Some(kind), Some(class)) = (source_type, freshness) {
        validate_freshness_class(
            FreshnessCheck {
                source_type: kind,
                freshness: class,
                source,
                retrieved_at,
                policy,
                now,
                id,
            },
            issues,
        );
    }
}

fn validate_claim<'a>(
    claim: &'a Value,
    uses: &Uses<'_>,
    source_ids: &BTreeSet<&str>,
    claim_ids: &mut BTreeSet<&'a str>,
    issues: &mut Issues,
) {
    let id = string_field(claim, "claimId").unwrap_or("claim without id");
    if id == "claim without id" {
        issues.push("research claim has no claimId".to_string());
    } else if !claim_ids.insert(id) {
        issues.push(format!("research claim ID {id} is duplicated"));
    }
    required_field(claim, "claim", id, issues);
    validate_bounded_text(claim, "claimId", id, MAX_RESEARCH_ID_BYTES, issues);
    validate_bounded_text(claim, "claim", id, MAX_RESEARCH_CLAIM_BYTES, issues);
    validate_optional_bounded_text(claim, "supersedes", id, MAX_RESEARCH_ID_BYTES, issues);
    validate_bounded_string_array(
        claim,
        "sourceIds",
        id,
        MAX_RESEARCH_REFERENCES,
        MAX_RESEARCH_ID_BYTES,
        issues,
    );
    validate_bounded_string_array(
        claim,
        "usedBy",
        id,
        MAX_RESEARCH_USED_BY,
        MAX_RESEARCH_ID_BYTES,
        issues,
    );
    let classification = required_field(claim, "classification", id, issues).unwrap_or_default();
    if !matches!(classification, "verified" | "assumption" | "derived") {
        issues.push(format!(
            "{id} is unclassified; expected verified, assumption, or derived"
        ));
    }
    let referenced_sources = string_array(claim, "sourceIds");
    if classification == "verified" && referenced_sources.is_empty() {
        issues.push(format!("{id} is verified but has no source IDs"));
    }
    for source_id in referenced_sources {
        if !source_ids.contains(source_id.as_str()) {
            issues.push(format!("{id} references unknown source ID {source_id}"));
        }
    }
    validate_used_by(claim, id, uses, issues);
}

fn validate_used_by(value: &Value, id: &str, uses: &Uses<'_>, issues: &mut Issues) {
    let used_by = string_array(value, "usedBy");
    if used_by.is_empty() {
        issues.push(format!("{id} has no usedBy IDs"));
    }
    for usage_id in used_by {
        if !uses.contains(usage_id.as_str()) {
            issues.push(format!("{id} references unknown usedBy ID {usage_id}"));
        }
    }
}

fn validate_source_location(source_type: &str, source_url: &str, id: &str, issues: &mut Issues) {
    let known_type = matches!(
        source_type,
        "official-doc"
            | "paper"
            | "standard"
            | "repository"
            | "issue"
            | "local-code"
            | "user-request"
    );
    if !known_type {
        issues.push(format!("{id} has unsupported sourceType {source_type:?}"));
        return;
    }
    let valid_location = match source_type {
        "local-code" => source_url.starts_with("local-code://"),
        "user-request" => source_url.starts_with("request://"),
        _ => source_url.starts_with("https://") || source_url.starts_with("http://"),
    };
    if !valid_location {
        issues.push(format!(
            "{id} sourceUrl does not match sourceType {source_type}"
        ));
    }
}

fn validate_freshness_class(check: FreshnessCheck<'_>, issues: &mut Issues) {
    match check.freshness {
        "local-only" => {
            if !matches!(check.source_type, "local-code" | "user-request") {
                issues.push(format!(
                    "{} is local-only but sourceType is {}",
                    check.id, check.source_type
                ));
            }
        }
        "historical" => {
            if !matches!(check.source_type, "paper" | "standard") {
                issues.push(format!(
                    "{} historical evidence must use sourceType paper or standard",
                    check.id
                ));
            }
            if publication_date(check.source).is_none() {
                issues.push(format!(
                    "{} historical paper or standard requires publicationDate",
                    check.id
                ));
            }
        }
        "fresh" => {
            if matches!(
                check.source_type,
                "local-code" | "user-request" | "paper" | "standard"
            ) {
                issues.push(format!(
                    "{} sourceType {} cannot claim externally fresh product evidence",
                    check.id, check.source_type
                ));
            }
            if let Some(retrieved_at) = check.retrieved_at {
                let age = check.now.signed_duration_since(retrieved_at);
                let window_days = check.policy.window_days_for(check.source_type);
                if age > chrono::Duration::days(window_days) {
                    issues.push(format!(
                        "{} is stale for sourceType {} (older than {} days); re-search required",
                        check.id, check.source_type, window_days
                    ));
                }
            }
        }
        "version-bound" => {
            if !matches!(
                check.source_type,
                "official-doc" | "standard" | "repository"
            ) {
                issues.push(format!(
                    "{} version-bound evidence requires documentation, standard, or repository",
                    check.id
                ));
            }
            let version = required_field(check.source, "sourceVersion", check.id, issues);
            let required = required_field(check.source, "requiredVersion", check.id, issues);
            if version != required {
                issues.push(format!(
                    "{} sourceVersion does not match requiredVersion",
                    check.id
                ));
            }
        }
        _ => issues.push(format!(
            "{} has unsupported freshness {:?}; expected fresh, historical, version-bound, or local-only",
            check.id, check.freshness
        )),
    }
}

fn validate_publication_date(source: &Value, id: &str, issues: &mut Issues) {
    let Some(value) = source.get("publicationDate") else {
        return;
    };
    if value.is_null() {
        return;
    }
    let Some(date) = value.as_str().filter(|date| !date.trim().is_empty()) else {
        issues.push(format!(
            "{id} publicationDate must be a date string or null"
        ));
        return;
    };
    if NaiveDate::parse_from_str(date, "%Y-%m-%d").is_err()
        && DateTime::parse_from_rfc3339(date).is_err()
    {
        issues.push(format!("{id} publicationDate is not RFC3339-compatible"));
    }
}

fn parse_retrieved_at(value: &str, id: &str, issues: &mut Issues) -> Option<DateTime<Utc>> {
    match DateTime::parse_from_rfc3339(value) {
        Ok(timestamp) => Some(timestamp.with_timezone(&Utc)),
        Err(_) => {
            issues.push(format!("{id} retrievedAt is not a valid RFC3339 timestamp"));
            None
        }
    }
}

fn required_field<'a>(
    value: &'a Value,
    field_name: &str,
    id: &str,
    issues: &mut Issues,
) -> Option<&'a str> {
    let field = string_field(value, field_name).filter(|field| !field.trim().is_empty());
    if field.is_none() {
        issues.push(format!("{id} has no {field_name}"));
    }
    field
}

fn publication_date(source: &Value) -> Option<&str> {
    string_field(source, "publicationDate").filter(|date| !date.trim().is_empty())
}

fn string_array(value: &Value, field_name: &str) -> Vec<String> {
    value_array(value, field_name)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

fn value_array<'a>(value: &'a Value, field_name: &str) -> Option<&'a [Value]> {
    value.get(field_name)?.as_array().map(Vec::as_slice)
}

fn string_field<'a>(value: &'a Value, field_name: &str) -> Option<&'a str> {
    value.get(field_name)?.as_str()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn valid_artifact(origin: &str, query: &str, source: Value) -> Value {
        json!({
            "status": "complete",
            "query": query,
            "truncated": false,
            "researchSource": origin,
            "sources": [source],
            "claims": [{
                "claimId": "CLM-001",
                "claim": "The source supports the requested behavior.",
                "classification": "verified",
                "sourceIds": ["SRC-001"],
                "usedBy": ["REQ-001"]
            }]
        })
    }

    fn valid_source(source_type: &str, freshness: &str) -> Value {
        json!({
            "sourceId": "SRC-001",
            "sourceUrl": if source_type == "local-code" {
                "local-code://README.md#L1-L2"
            } else {
                "https://example.invalid/evidence"
            },
            "sourceType": source_type,
            "retrievedAt": "2026-09-11T00:00:00Z",
            "support": "A bounded evidence snippet.",
            "freshness": freshness,
            "usedBy": ["REQ-001"]
        })
    }

    #[test]
    fn research_requirement_uses_exact_tokens_and_prioritizes_external_facts() {
        assert_eq!(
            classify_research_requirement("Check current MCP protocol behavior"),
            ResearchRequirement::ExternalCurrent
        );
        assert_eq!(
            classify_research_requirement("Inspect current repository code"),
            ResearchRequirement::LocalCurrent
        );
        assert_eq!(
            classify_research_requirement("Add a sample JSON command"),
            ResearchRequirement::Stable
        );
        assert_eq!(
            classify_research_requirement("Fix apiary documentation typo"),
            ResearchRequirement::Stable
        );
    }

    #[test]
    fn local_index_cannot_satisfy_current_external_research() {
        let research = valid_artifact(
            "local-index",
            "Check current MCP protocol behavior",
            valid_source("local-code", "local-only"),
        );
        let uses = BTreeSet::from(["REQ-001"]);
        let issues = validate_research_artifact(
            &research,
            &uses,
            ResearchPolicy::default(),
            DateTime::parse_from_rfc3339("2026-09-12T00:00:00Z")
                .expect("now")
                .with_timezone(&Utc),
        );
        assert!(
            issues
                .iter()
                .any(|issue| issue.contains("cannot use local-index")),
            "external-current local fallback must fail closed: {issues:?}"
        );
    }

    #[test]
    fn cache_origin_requires_cache_identity_and_external_evidence() {
        let research = valid_artifact(
            "cache",
            "Check current API behavior",
            valid_source("local-code", "local-only"),
        );
        let uses = BTreeSet::from(["REQ-001"]);
        let issues =
            validate_research_artifact(&research, &uses, ResearchPolicy::default(), Utc::now());
        assert!(
            issues.iter().any(|issue| issue.contains("cacheId")),
            "{issues:?}"
        );
        assert!(
            issues.iter().any(|issue| issue.contains("external source")),
            "cache evidence must remain external for current API facts: {issues:?}"
        );
    }

    #[test]
    fn oversized_research_projection_is_rejected_without_trimming() {
        let mut source = valid_source("official-doc", "fresh");
        source["support"] = Value::String("x".repeat(MAX_RESEARCH_EVIDENCE_BYTES + 1));
        let research = valid_artifact("host", "Check current API behavior", source);
        let uses = BTreeSet::from(["REQ-001"]);
        let issues =
            validate_research_artifact(&research, &uses, ResearchPolicy::default(), Utc::now());
        assert!(
            issues
                .iter()
                .any(|issue| issue.contains("support") && issue.contains("bound")),
            "oversized support must be rejected rather than silently clipped: {issues:?}"
        );
    }

    #[test]
    fn research_conflicts_require_a_verified_resolution_and_preserve_supersession() {
        let mut research =
            serde_json::json!({"conflicts":[{"claimIds":["A", "B"], "status":"open"}]});
        let claims = vec![
            serde_json::json!({"claimId":"A", "classification":"verified"}),
            serde_json::json!({"claimId":"B", "classification":"verified", "supersedes":"A"}),
        ];
        let ids = BTreeSet::from(["A", "B"]);
        let mut issues = Vec::new();
        validate_claim_relations(&research, &claims, &ids, &mut issues);
        assert!(issues.iter().any(|issue| issue.contains("unresolved")));
        research["conflicts"][0] = serde_json::json!({"claimIds":["A", "B"], "status":"resolved", "resolvedByClaimId":"B", "rationale":"Current official source supersedes historical evidence"});
        issues.clear();
        validate_claim_relations(&research, &claims, &ids, &mut issues);
        assert!(issues.is_empty(), "{issues:?}");
        let cyclic = vec![
            serde_json::json!({"claimId":"A", "supersedes":"B"}),
            claims[1].clone(),
        ];
        validate_claim_relations(&research, &cyclic, &ids, &mut issues);
        assert!(issues
            .iter()
            .any(|issue| issue.contains("supersession cycle")));
    }

    #[test]
    fn version_bound_sources_require_the_exact_requested_version() {
        let source = serde_json::json!({"sourceVersion":"1", "requiredVersion":"2"});
        let mut issues = Vec::new();
        validate_freshness_class(
            FreshnessCheck {
                source_type: "standard",
                freshness: "version-bound",
                source: &source,
                retrieved_at: None,
                policy: ResearchPolicy::default(),
                now: Utc::now(),
                id: "SRC-1",
            },
            &mut issues,
        );
        assert!(issues.iter().any(|issue| issue.contains("does not match")));
    }

    #[test]
    fn project_freshness_defers_to_the_per_source_type_window() {
        let workspace = crate::test_support::unique_temp_dir("research-policy-default");
        let policy = ResearchPolicy::load(&workspace).expect("load default policy");
        assert_eq!(policy.max_age_days, None);

        // The plan forbids one universal window: fast-moving evidence is
        // re-checked sooner than version-bound documentation.
        assert_eq!(policy.window_days_for("issue"), 7);
        assert_eq!(policy.window_days_for("repository"), 30);
        assert_eq!(policy.window_days_for("official-doc"), 90);
        assert!(
            policy.window_days_for("issue") < policy.window_days_for("repository"),
            "issue state changes faster than a repository snapshot"
        );
        assert!(
            policy.window_days_for("repository") < policy.window_days_for("official-doc"),
            "a live repository moves faster than released documentation"
        );
    }

    #[test]
    fn project_freshness_is_configurable_but_cannot_be_disabled() {
        let workspace = crate::test_support::unique_temp_dir("research-policy-config");
        fs::write(
            workspace.join("keel.filters.toml"),
            "[research]\nmax_age_days = 30\n",
        )
        .expect("write project research policy");
        let configured = ResearchPolicy::load(&workspace).expect("load configured policy");
        assert_eq!(configured.max_age_days, Some(30));
        // An explicit override applies to every externally fresh source type.
        for source_type in ["issue", "repository", "official-doc"] {
            assert_eq!(configured.window_days_for(source_type), 30);
        }

        fs::write(
            workspace.join("keel.filters.toml"),
            "[research]\nmax_age_days = 0\n",
        )
        .expect("disable project research policy");
        assert!(ResearchPolicy::load(&workspace)
            .expect_err("zero-day policy must fail")
            .contains("cannot be disabled"));
    }

    /// The window is enforced per source type, so evidence whose subject moves
    /// fast is rejected sooner than version-bound documentation.
    #[test]
    fn per_source_type_windows_reject_fast_moving_evidence_sooner() {
        let now = DateTime::parse_from_rfc3339("2026-09-11T00:00:00Z")
            .expect("now")
            .with_timezone(&Utc);
        let retrieved_text = "2026-08-20T00:00:00Z";
        let retrieved = DateTime::parse_from_rfc3339(retrieved_text)
            .expect("retrieved")
            .with_timezone(&Utc);
        let source = |source_type: &str| {
            serde_json::json!({
                "sourceId": "SRC-001",
                "sourceUrl": "https://example.invalid/spec",
                "sourceType": source_type,
                "retrievedAt": retrieved_text,
                "support": "Cited for the freshness window test.",
                "freshness": "fresh",
                "usedBy": ["REQ-001"],
            })
        };
        let issue_source = source("issue");
        let doc_source = source("official-doc");
        let repo_source = source("repository");

        // 22 days old: past the 7-day issue window, inside the 90-day doc window.
        let mut issue_issues = Vec::new();
        validate_freshness_class(
            FreshnessCheck {
                source_type: "issue",
                freshness: "fresh",
                source: &issue_source,
                retrieved_at: Some(retrieved),
                policy: ResearchPolicy::default(),
                now,
                id: "SRC-001",
            },
            &mut issue_issues,
        );
        assert!(
            issue_issues.iter().any(|issue| issue.contains("stale")),
            "an issue older than its 7-day window must be stale: {issue_issues:?}"
        );

        let mut doc_issues = Vec::new();
        validate_freshness_class(
            FreshnessCheck {
                source_type: "official-doc",
                freshness: "fresh",
                source: &doc_source,
                retrieved_at: Some(retrieved),
                policy: ResearchPolicy::default(),
                now,
                id: "SRC-002",
            },
            &mut doc_issues,
        );
        assert!(
            doc_issues.is_empty(),
            "documentation inside its 90-day window stays fresh: {doc_issues:?}"
        );

        let mut repo_issues = Vec::new();
        validate_freshness_class(
            FreshnessCheck {
                source_type: "repository",
                freshness: "fresh",
                source: &repo_source,
                retrieved_at: Some(retrieved),
                policy: ResearchPolicy::default(),
                now,
                id: "SRC-003",
            },
            &mut repo_issues,
        );
        assert!(
            repo_issues.is_empty(),
            "a repository snapshot inside its 30-day window stays fresh: {repo_issues:?}"
        );
    }
}
