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
    if string_field(research, "status") != Some("complete") {
        issues.push("research.json status is not complete".to_string());
    }

    let mut source_ids = BTreeSet::new();
    match value_array(research, "sources") {
        Some(sources) if !sources.is_empty() => {
            for source in sources {
                validate_source(source, uses, policy, now, &mut source_ids, &mut issues);
            }
        }
        _ => issues.push("research.json has no source records".to_string()),
    }
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

    let Some(claims) = value_array(research, "claims") else {
        issues.push("research.json has no claims array".to_string());
        return issues;
    };
    if claims.is_empty() {
        issues.push("research.json has no factual claims".to_string());
    }
    let mut claim_ids = BTreeSet::new();
    for claim in claims {
        validate_claim(claim, uses, &source_ids, &mut claim_ids, &mut issues);
    }
    issues
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
    validate_used_by(source, id, uses, issues);
    validate_publication_date(source, id, issues);

    let retrieved_at = required_field(source, "retrievedAt", id, issues)
        .and_then(|value| parse_retrieved_at(value, id, issues));
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
                if age.num_minutes() < -5 {
                    issues.push(format!("{} retrievedAt is in the future", check.id));
                } else if age.num_days() > window_days {
                    issues.push(format!(
                        "{} is stale for sourceType {} (older than {} days); re-search required",
                        check.id, check.source_type, window_days
                    ));
                }
            }
        }
        _ => issues.push(format!(
            "{} has unsupported freshness {:?}; expected fresh, historical, or local-only",
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
