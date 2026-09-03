use crate::runtime::{display_path, safe_path_segment, write_text};
use crate::utility::hashing::fnv1a64_hex;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::{Component, Path, PathBuf};

pub const REVIEW_LEDGER_SCHEMA: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReviewFindingStatus {
    Open,
    Closed,
    Stale,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReviewSeverity {
    Blocker,
    Major,
    Minor,
    Nit,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReviewFinding {
    pub id: String,
    pub rule: String,
    pub severity: ReviewSeverity,
    pub status: ReviewFindingStatus,
    pub file: String,
    pub line: Option<usize>,
    pub message: String,
    pub evidence: String,
    pub first_seen_head: String,
    pub last_seen_head: String,
    pub closed_head: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReviewRequirement {
    pub id: String,
    pub text: String,
    pub status: ReviewFindingStatus,
    pub evidence: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReviewGateSnapshot {
    pub name: String,
    pub status: String,
    pub blocking: bool,
    pub details: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReviewLedger {
    pub schema_version: u32,
    pub id: String,
    pub head_sha: String,
    pub base_ref: String,
    pub repo_root: String,
    pub scope_fingerprint: String,
    pub created_at: String,
    pub updated_at: String,
    pub requirements: Vec<ReviewRequirement>,
    pub findings: Vec<ReviewFinding>,
    pub gates: Vec<ReviewGateSnapshot>,
}

pub const REVIEW_BASELINE_SCHEMA: u32 = 1;
pub const DEFAULT_REVIEW_BASELINE_FILE: &str = "review-closeout-baseline.json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReviewBaseline {
    pub schema_version: u32,
    pub generated_from_head: String,
    pub generated_at: String,
    pub expires_at: String,
    pub reviewed_by: String,
    pub reason: String,
    pub finding_ids: Vec<String>,
}

/// Return a deterministic, path-safe identifier for a review head.
pub fn ledger_id(head_sha: &str) -> String {
    let short_sha: String = head_sha.trim().chars().take(12).collect();
    if short_sha.is_empty() {
        "review-unknown".to_string()
    } else {
        format!("review-{short_sha}")
    }
}

pub fn ledger_path(claude_home: &Path, id: &str) -> Result<PathBuf, String> {
    let safe_id = safe_path_segment(id).ok_or_else(|| {
        format!("review ledger id must be a single safe path segment, got {id:?}")
    })?;
    Ok(claude_home
        .join("state")
        .join("review-closeout")
        .join(format!("{safe_id}.json")))
}

pub fn load_ledger(claude_home: &Path, id: &str) -> Result<Option<ReviewLedger>, String> {
    let path = ledger_path(claude_home, id)?;
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("read {}: {error}", display_path(&path))),
    };
    let ledger: ReviewLedger = serde_json::from_str(&text)
        .map_err(|error| format!("parse {}: {error}", display_path(&path)))?;
    if ledger.schema_version != REVIEW_LEDGER_SCHEMA {
        return Err(format!(
            "unsupported review ledger schema {} in {} (expected {})",
            ledger.schema_version,
            display_path(&path),
            REVIEW_LEDGER_SCHEMA
        ));
    }
    Ok(Some(ledger))
}

pub fn save_ledger(claude_home: &Path, ledger: &ReviewLedger) -> Result<PathBuf, String> {
    if ledger.schema_version != REVIEW_LEDGER_SCHEMA {
        return Err(format!(
            "unsupported review ledger schema {} (expected {})",
            ledger.schema_version, REVIEW_LEDGER_SCHEMA
        ));
    }
    let path = ledger_path(claude_home, &ledger.id)?;
    let rendered = serde_json::to_string_pretty(ledger)
        .map_err(|error| format!("serialize {}: {error}", display_path(&path)))?;
    write_text(&path, &format!("{rendered}\n"))?;
    Ok(path)
}

pub fn review_baseline_path(
    repository_root: &Path,
    requested_path: &str,
) -> Result<PathBuf, String> {
    let candidate = if requested_path.trim().is_empty() {
        repository_root.join(DEFAULT_REVIEW_BASELINE_FILE)
    } else {
        let requested = PathBuf::from(requested_path.trim());
        if requested.is_absolute() {
            requested
        } else {
            repository_root.join(requested)
        }
    };
    if candidate
        .components()
        .any(|component| component == Component::ParentDir)
    {
        return Err(format!(
            "review baseline path must stay inside the repository: {}",
            display_path(&candidate)
        ));
    }
    let comparable_path = |path: &Path| {
        let displayed = display_path(path);
        let displayed = displayed.strip_prefix("\\\\?\\").unwrap_or(&displayed);
        displayed.trim_end_matches(['\\', '/']).to_ascii_lowercase()
    };
    let candidate_display = display_path(&candidate);
    let root_lower = comparable_path(repository_root);
    let candidate_lower = comparable_path(&candidate);
    let root_prefix = format!("{}{}", root_lower, std::path::MAIN_SEPARATOR);
    if candidate_lower != root_lower && !candidate_lower.starts_with(&root_prefix) {
        return Err(format!(
            "review baseline path must stay inside the repository: {}",
            candidate_display
        ));
    }
    if candidate.exists() {
        let canonical_candidate = fs::canonicalize(&candidate).map_err(|error| {
            format!(
                "canonicalize review baseline {}: {error}",
                candidate_display
            )
        })?;
        let canonical_lower = comparable_path(&canonical_candidate);
        if canonical_lower != root_lower && !canonical_lower.starts_with(&root_prefix) {
            return Err(format!(
                "review baseline path must stay inside the repository: {}",
                candidate_display
            ));
        }
    }
    Ok(candidate)
}

pub fn load_review_baseline(path: &Path, now: &str) -> Result<Option<ReviewBaseline>, String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("read {}: {error}", display_path(path))),
    };
    let baseline: ReviewBaseline = serde_json::from_str(&text)
        .map_err(|error| format!("parse {}: {error}", display_path(path)))?;
    validate_review_baseline(&baseline, now)?;
    Ok(Some(baseline))
}

pub fn save_review_baseline(path: &Path, baseline: &ReviewBaseline) -> Result<(), String> {
    validate_review_baseline(baseline, &baseline.generated_at)?;
    let rendered = serde_json::to_string_pretty(baseline)
        .map_err(|error| format!("serialize {}: {error}", display_path(path)))?;
    write_text(path, &format!("{rendered}\n"))
}

fn validate_review_baseline(baseline: &ReviewBaseline, now: &str) -> Result<(), String> {
    if baseline.schema_version != REVIEW_BASELINE_SCHEMA {
        return Err(format!(
            "unsupported review baseline schema {} (expected {})",
            baseline.schema_version, REVIEW_BASELINE_SCHEMA
        ));
    }
    if baseline.generated_from_head.trim().is_empty()
        || baseline.reviewed_by.trim().is_empty()
        || baseline.reason.trim().is_empty()
    {
        return Err(
            "review baseline requires generated_from_head, reviewed_by, and reason".to_string(),
        );
    }
    let generated_at = DateTime::parse_from_rfc3339(&baseline.generated_at)
        .map_err(|error| format!("review baseline generated_at is invalid: {error}"))?;
    let expires_at = DateTime::parse_from_rfc3339(&baseline.expires_at)
        .map_err(|error| format!("review baseline expires_at is invalid: {error}"))?;
    let current_time = DateTime::parse_from_rfc3339(now)
        .map_err(|error| format!("review baseline comparison time is invalid: {error}"))?;
    if expires_at <= current_time {
        return Err(format!(
            "review baseline expired at {}",
            baseline.expires_at
        ));
    }
    if generated_at > expires_at {
        return Err("review baseline generated_at is after expires_at".to_string());
    }
    let mut unique_ids = HashSet::new();
    for finding_id in &baseline.finding_ids {
        if finding_id.trim().is_empty() || !unique_ids.insert(finding_id) {
            return Err("review baseline finding_ids must be non-empty and unique".to_string());
        }
    }
    Ok(())
}

fn baseline_eligible(finding: &ReviewFinding) -> bool {
    finding.rule.starts_with("comment:")
        || finding.rule.starts_with("prose:")
        || finding.rule.starts_with("slop:")
        || matches!(
            finding.rule.as_str(),
            "gate:comment_style" | "gate:prose_style"
        )
}

fn build_review_baseline(
    findings: &[ReviewFinding],
    head: &str,
    reviewed_by: &str,
    reason: &str,
    expires_at: &str,
) -> Result<ReviewBaseline, String> {
    let mut finding_ids: Vec<String> = findings
        .iter()
        .filter(|finding| baseline_eligible(finding))
        .map(|finding| finding.id.clone())
        .collect();
    finding_ids.sort();
    finding_ids.dedup();
    let baseline = ReviewBaseline {
        schema_version: REVIEW_BASELINE_SCHEMA,
        generated_from_head: head.to_string(),
        generated_at: Utc::now().to_rfc3339(),
        expires_at: expires_at.to_string(),
        reviewed_by: reviewed_by.to_string(),
        reason: reason.to_string(),
        finding_ids,
    };
    validate_review_baseline(&baseline, &baseline.generated_at)?;
    Ok(baseline)
}

fn apply_review_baseline(
    findings: Vec<ReviewFinding>,
    baseline: Option<&ReviewBaseline>,
    head: &str,
) -> (Vec<ReviewFinding>, usize) {
    let Some(baseline) = baseline else {
        return (findings, 0);
    };
    let baseline_ids: HashSet<&str> = baseline.finding_ids.iter().map(String::as_str).collect();
    let mut suppressed = 0usize;
    let findings = findings
        .into_iter()
        .map(|mut finding| {
            if baseline_ids.contains(finding.id.as_str()) {
                finding.status = ReviewFindingStatus::Closed;
                finding.closed_head = Some(head.to_string());
                suppressed += 1;
            }
            finding
        })
        .collect();
    (findings, suppressed)
}

/// Build an ID from semantic finding content. Include a location when available
/// so two identical rules at different lines remain distinct ledger findings.
pub fn stable_finding_id(rule: &str, file: &str, line: Option<usize>, message: &str) -> String {
    let normalized_rule = normalize_identifier(rule);
    let normalized_file = normalize_identifier(file).replace('\\', "/");
    let normalized_message = normalize_identifier(message);
    let location = line
        .map(|value| value.to_string())
        .unwrap_or_else(|| "*".to_string());
    let hash_input = format!(
        "rule={normalized_rule}\nfile={normalized_file}\nline={location}\nmessage={normalized_message}"
    );
    let readable_rule = readable_rule_prefix(&normalized_rule);
    format!("{readable_rule}-{}", fnv1a64_hex(&hash_input))
}
pub fn stable_requirement_id(text: &str) -> String {
    format!("requirement-{}", fnv1a64_hex(&normalize_identifier(text)))
}
fn requirement_proof(proof: &str, requirement_id: &str) -> Option<String> {
    proof.lines().find_map(|line| {
        let (id, evidence) = line.split_once('=')?;
        if id.trim() != requirement_id {
            return None;
        }
        let evidence = evidence.trim();
        (!evidence.is_empty()).then(|| evidence.to_string())
    })
}

pub fn scope_fingerprint(head_sha: &str, changed_paths: &[String], dirty_status: &str) -> String {
    let mut sorted_paths = changed_paths.to_vec();
    sorted_paths.sort();
    let mut input = format!(
        "head={head_sha}\nstatus={dirty_status}\npaths={}\n",
        sorted_paths.len()
    );
    for path in sorted_paths {
        input.push_str(&format!("{}:{path}\n", path.len()));
    }
    fnv1a64_hex(&input)
}

pub fn reconcile_findings(
    previous: &[ReviewFinding],
    current: &[ReviewFinding],
    head_sha: &str,
) -> Vec<ReviewFinding> {
    let mut reconciled = Vec::with_capacity(previous.len() + current.len());
    let mut emitted_current = HashSet::new();

    for old in previous {
        if let Some(new) = current.iter().find(|finding| {
            stable_finding_id(&finding.rule, &finding.file, finding.line, &finding.message)
                == old.id
        }) {
            let mut finding = new.clone();
            finding.id =
                stable_finding_id(&finding.rule, &finding.file, finding.line, &finding.message);
            finding.status = ReviewFindingStatus::Open;
            finding.first_seen_head = old.first_seen_head.clone();
            finding.last_seen_head = head_sha.to_string();
            finding.closed_head = None;
            emitted_current.insert(finding.id.clone());
            reconciled.push(finding);
        } else {
            let mut finding = old.clone();
            finding.status = ReviewFindingStatus::Closed;
            finding.closed_head = Some(head_sha.to_string());
            reconciled.push(finding);
        }
    }

    for finding in current {
        let stable_id =
            stable_finding_id(&finding.rule, &finding.file, finding.line, &finding.message);
        if emitted_current.contains(&stable_id) {
            continue;
        }
        let mut finding = finding.clone();
        finding.id = stable_id.clone();
        finding.status = ReviewFindingStatus::Open;
        finding.first_seen_head = head_sha.to_string();
        finding.last_seen_head = head_sha.to_string();
        finding.closed_head = None;
        emitted_current.insert(stable_id);
        reconciled.push(finding);
    }
    reconciled
}

pub fn reconcile_requirements(
    previous: &[ReviewRequirement],
    current: &[ReviewRequirement],
) -> Vec<ReviewRequirement> {
    let mut reconciled = Vec::with_capacity(previous.len() + current.len());
    let mut emitted_current = HashSet::new();

    for old in previous {
        let old_id = stable_requirement_id(&old.text);
        if let Some(new) = current
            .iter()
            .find(|requirement| stable_requirement_id(&requirement.text) == old_id)
        {
            let mut requirement = new.clone();
            requirement.id = old_id.clone();
            emitted_current.insert(old_id);
            reconciled.push(requirement);
        } else {
            let mut requirement = old.clone();
            requirement.id = old_id;
            requirement.status = ReviewFindingStatus::Stale;
            reconciled.push(requirement);
        }
    }

    for requirement in current {
        let id = stable_requirement_id(&requirement.text);
        if emitted_current.contains(&id) {
            continue;
        }
        let mut requirement = requirement.clone();
        requirement.id = id.clone();
        emitted_current.insert(id);
        reconciled.push(requirement);
    }
    reconciled
}

pub fn unresolved_findings(ledger: &ReviewLedger) -> Vec<&ReviewFinding> {
    ledger
        .findings
        .iter()
        .filter(|finding| finding.status != ReviewFindingStatus::Closed)
        .collect()
}

pub fn unresolved_requirements(ledger: &ReviewLedger) -> Vec<&ReviewRequirement> {
    ledger
        .requirements
        .iter()
        .filter(|requirement| requirement.status != ReviewFindingStatus::Closed)
        .collect()
}

fn normalize_identifier(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn readable_rule_prefix(rule: &str) -> String {
    let mut prefix = String::new();
    for character in rule.chars() {
        if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
            prefix.push(character);
        } else if !prefix.ends_with('-') {
            prefix.push('-');
        }
    }
    let prefix = prefix.trim_matches('-');
    if prefix.is_empty() {
        "finding".to_string()
    } else {
        prefix.to_string()
    }
}

use super::{
    collect_review_gate_results, git_lines, git_text, resolve_provider, CiProvider, GateResult,
    GateStatus, ProviderResolution, FLOW_SOURCE_EXTENSIONS,
};
use crate::args::FlagSet;
use crate::runtime::{resolve_claude_home, resolve_repository_root, run_command};
use crate::utility::working_brief::read_brief;
use std::io::Write;

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn gate_status(status: GateStatus) -> &'static str {
    match status {
        GateStatus::Pass => "pass",
        GateStatus::Fail => "fail",
        GateStatus::Warn => "warn",
        GateStatus::Blocked => "blocked",
    }
}

fn severity_for_text(severity: &str) -> ReviewSeverity {
    match severity.to_ascii_lowercase().as_str() {
        "blocker" => ReviewSeverity::Blocker,
        "major" | "error" | "high" | "blocking" => ReviewSeverity::Major,
        "nit" | "info" | "low" => ReviewSeverity::Nit,
        _ => ReviewSeverity::Minor,
    }
}

fn finding(
    rule: impl Into<String>,
    severity: ReviewSeverity,
    file: impl Into<String>,
    line: Option<usize>,
    message: impl Into<String>,
    evidence: impl Into<String>,
    head: &str,
) -> ReviewFinding {
    let rule = rule.into();
    let file = file.into();
    let message = message.into();
    ReviewFinding {
        id: stable_finding_id(&rule, &file, line, &message),
        rule,
        severity,
        status: ReviewFindingStatus::Open,
        file,
        line,
        message,
        evidence: evidence.into(),
        first_seen_head: head.to_string(),
        last_seen_head: head.to_string(),
        closed_head: None,
    }
}

fn porcelain_paths(status: &str) -> Vec<String> {
    status
        .lines()
        .filter_map(|line| {
            if line.len() < 3 {
                return None;
            }
            let path = line[3..].trim();
            if path.is_empty() {
                return None;
            }
            Some(
                path.rsplit_once(" -> ")
                    .map(|(_, new_path)| new_path.trim())
                    .unwrap_or(path)
                    .replace('\\', "/"),
            )
        })
        .collect()
}

fn add_gate_findings(
    gates: &[GateResult],
    findings: &mut Vec<ReviewFinding>,
    snapshots: &mut Vec<ReviewGateSnapshot>,
    head: &str,
) {
    for gate in gates {
        let status = gate_status(gate.status).to_string();
        let details = gate
            .details
            .clone()
            .unwrap_or_else(|| format!("gate status: {status}"));
        snapshots.push(ReviewGateSnapshot {
            name: gate.name.clone(),
            status: status.clone(),
            blocking: gate.blocking,
            details: Some(details.clone()),
        });
        if gate.status != GateStatus::Pass {
            findings.push(finding(
                format!("gate:{}", gate.name),
                if gate.blocking && gate.status == GateStatus::Fail {
                    ReviewSeverity::Major
                } else {
                    ReviewSeverity::Minor
                },
                format!("review/{}", gate.name),
                None,
                format!("review gate {} is {status}", gate.name),
                details,
                head,
            ));
        }
    }
}

fn add_wiring_findings(repository_root: &Path, findings: &mut Vec<ReviewFinding>, head: &str) {
    let mut advertised = HashSet::new();
    for relative in [
        "rust/crates/keel/src/help_operator.txt",
        "rust/crates/keel/src/help_advanced.txt",
    ] {
        let path = repository_root.join(relative);
        match fs::read_to_string(&path) {
            Ok(text) => {
                for token in text
                    .lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty() && !line.starts_with('#'))
                    .filter_map(|line| line.split_whitespace().next())
                {
                    advertised.insert(token.to_string());
                }
            }
            Err(error) => findings.push(finding(
                format!("wiring:help:{relative}"),
                ReviewSeverity::Major,
                relative,
                None,
                "advertised help file is unreadable",
                error.to_string(),
                head,
            )),
        }
    }
    for command in crate::commands::TOP_LEVEL_COMMANDS {
        if !advertised.contains(*command) {
            findings.push(finding(
                format!("wiring:help-missing:{command}"),
                ReviewSeverity::Major,
                "rust/crates/keel/src/help_operator.txt",
                None,
                format!("top-level command {command} is not advertised by help"),
                "TOP_LEVEL_COMMANDS entry has no help first token",
                head,
            ));
        }
    }
    for token in advertised {
        if !crate::commands::TOP_LEVEL_COMMANDS.contains(&token.as_str()) {
            findings.push(finding(
                format!("wiring:help-extra:{token}"),
                ReviewSeverity::Major,
                "rust/crates/keel/src/help_operator.txt",
                None,
                format!("help advertises unknown top-level command {token}"),
                "help first token is absent from TOP_LEVEL_COMMANDS",
                head,
            ));
        }
    }

    let commands_path = repository_root.join("rust/crates/keel/src/commands.rs");
    match fs::read_to_string(&commands_path) {
        Ok(source) => {
            for command in crate::commands::TOP_LEVEL_COMMANDS {
                let arm = format!("\"{command}\"");
                if !source
                    .lines()
                    .any(|line| line.contains(&arm) && line.contains("=>"))
                {
                    findings.push(finding(
                        format!("wiring:dispatch:{command}"),
                        ReviewSeverity::Major,
                        "rust/crates/keel/src/commands.rs",
                        None,
                        format!("top-level command {command} has no literal dispatcher arm"),
                        "expected a source line containing the command literal and `=>`",
                        head,
                    ));
                }
            }
        }
        Err(error) => findings.push(finding(
            "wiring:dispatch-source",
            ReviewSeverity::Major,
            "rust/crates/keel/src/commands.rs",
            None,
            "commands source is unreadable",
            error.to_string(),
            head,
        )),
    }

    for relative in [
        "_shared/ts/bridge-core.ts",
        "pi/keel-pi.ts",
        "opencode/keel.ts",
        "codex/keel-codex.ts",
        "commandcode/keel-cmdc.ts",
        "antigravity/keel-antigravity.js",
        "cursor/hooks/keel-cursor.sh",
        "statusline/statusline-keel.sh",
        "statusline/statusline-keel.ps1",
    ] {
        if !repository_root.join(relative).is_file() {
            findings.push(finding(
                format!("wiring:adapter:{relative}"),
                ReviewSeverity::Major,
                relative,
                None,
                format!("required shipped adapter source is missing: {relative}"),
                "required adapter path does not exist",
                head,
            ));
        }
    }
    let release_path = repository_root.join(".github/workflows/release.yml");
    match fs::read_to_string(&release_path) {
        Ok(source) => {
            let adapter_staging = source
                .split("# Stage cross-agent adapter sources")
                .nth(1)
                .and_then(|tail| tail.split("# Fail closed").next())
                .unwrap_or("");
            let shared_staging = source
                .split("# Stage cross-skill resource directories")
                .nth(1)
                .and_then(|tail| tail.split("# Stage custom slash commands").next())
                .unwrap_or("");
            for relative in [
                "_shared/ts/bridge-core.ts",
                "pi/keel-pi.ts",
                "opencode/keel.ts",
                "codex/keel-codex.ts",
                "commandcode/keel-cmdc.ts",
                "antigravity/keel-antigravity.js",
                "cursor/hooks/keel-cursor.sh",
                "statusline/statusline-keel.sh",
                "statusline/statusline-keel.ps1",
            ] {
                let marker = relative.split('/').next().unwrap_or(relative);
                let staged = if marker == "_shared" {
                    shared_staging.contains(marker)
                } else {
                    adapter_staging.contains(marker)
                };
                if !staged {
                    findings.push(finding(
                        format!("wiring:release:{relative}"),
                        ReviewSeverity::Major,
                        ".github/workflows/release.yml",
                        None,
                        format!("release staging omits {relative}"),
                        "required adapter/statusline source is not copied into the release archive",
                        head,
                    ));
                }
            }
        }
        Err(error) => findings.push(finding(
            "wiring:release-workflow",
            ReviewSeverity::Major,
            ".github/workflows/release.yml",
            None,
            "release workflow is unreadable",
            error.to_string(),
            head,
        )),
    }
}

fn refresh_evidence(
    repository_root: &Path,
    changed_paths: &[String],
    head: &str,
    findings: &mut Vec<ReviewFinding>,
    snapshots: &mut Vec<ReviewGateSnapshot>,
) {
    let root_display = display_path(repository_root);
    let sibling_arguments = vec![
        "siblings".to_string(),
        "--workspace-root".to_string(),
        root_display.clone(),
        "--query".to_string(),
        "review closeout sibling implementation".to_string(),
        "--json".to_string(),
    ];
    let mut sibling_stdout = Vec::new();
    let mut sibling_stderr = Vec::new();
    let sibling_code = crate::utility::run_code_search_command(
        &sibling_arguments,
        &mut sibling_stdout,
        &mut sibling_stderr,
    );
    let sibling_output = String::from_utf8_lossy(&sibling_stdout);
    let sibling_error = String::from_utf8_lossy(&sibling_stderr);
    if sibling_code == 0 {
        snapshots.push(ReviewGateSnapshot {
            name: "evidence:siblings".to_string(),
            status: "pass".to_string(),
            blocking: true,
            details: Some(format!("stdout={sibling_output} stderr={sibling_error}")),
        });
    } else {
        let evidence =
            format!("exit={sibling_code} stdout={sibling_output} stderr={sibling_error}");
        findings.push(finding(
            "evidence:siblings",
            ReviewSeverity::Major,
            "review/evidence",
            None,
            "code-search sibling evidence refresh failed",
            evidence.clone(),
            head,
        ));
        snapshots.push(ReviewGateSnapshot {
            name: "evidence:siblings".to_string(),
            status: "blocked".to_string(),
            blocking: true,
            details: Some(evidence),
        });
    }

    let target = changed_paths
        .iter()
        .find(|path| {
            Path::new(path)
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| FLOW_SOURCE_EXTENSIONS.contains(&extension))
        })
        .cloned()
        .unwrap_or_else(|| "rust/crates/keel/src/review.rs".to_string());
    let flow_arguments = vec![
        "flow".to_string(),
        "start".to_string(),
        "--target-file".to_string(),
        target.clone(),
        "--target-function".to_string(),
        "run_review_closeout_command".to_string(),
        "--current-behavior".to_string(),
        "Runs the persistent review closeout scan and reconciles its ledger".to_string(),
        "--entry-point".to_string(),
        "review closeout CLI command".to_string(),
        "--producer".to_string(),
        "review.rs dispatcher".to_string(),
        "--source-of-truth".to_string(),
        "closeout ledger and review gate collector".to_string(),
        "--storage-state-queue-owner".to_string(),
        "review closeout ledger and command registry".to_string(),
        "--side-effect-owner".to_string(),
        "review scans, flow refresh, and ledger writes".to_string(),
        "--cleanup-recovery-path".to_string(),
        "ledger history and command termination".to_string(),
        "--consumers".to_string(),
        "review command and closeout ledger".to_string(),
        "--edit-boundary".to_string(),
        "closeout evidence refresh and gate scan".to_string(),
        "--validation-needed".to_string(),
        "rerun review closeout after fixes".to_string(),
        "--validation-evidence".to_string(),
        "automatic closeout evidence refresh".to_string(),
        "--repo-root".to_string(),
        root_display,
    ];
    let mut flow_stdout = Vec::new();
    let mut flow_stderr = Vec::new();
    let flow_code = crate::commands::Application::new(env!("CARGO_PKG_VERSION")).run(
        &flow_arguments,
        &mut flow_stdout,
        &mut flow_stderr,
    );
    let flow_output = String::from_utf8_lossy(&flow_stdout);
    let flow_error = String::from_utf8_lossy(&flow_stderr);
    if flow_code == 0 {
        snapshots.push(ReviewGateSnapshot {
            name: "evidence:flow".to_string(),
            status: "pass".to_string(),
            blocking: true,
            details: Some(format!(
                "target={target} stdout={flow_output} stderr={flow_error}"
            )),
        });
    } else {
        let evidence =
            format!("target={target} exit={flow_code} stdout={flow_output} stderr={flow_error}");
        findings.push(finding(
            "evidence:flow",
            ReviewSeverity::Major,
            "review/evidence",
            None,
            "flow evidence refresh failed",
            evidence.clone(),
            head,
        ));
        snapshots.push(ReviewGateSnapshot {
            name: "evidence:flow".to_string(),
            status: "blocked".to_string(),
            blocking: true,
            details: Some(evidence),
        });
    }
}

fn render_closeout(
    format: &str,
    ledger: &ReviewLedger,
    ledger_path: &Path,
    status: &str,
    unresolved_findings_count: usize,
    unresolved_requirements_count: usize,
    standard_output: &mut dyn Write,
) {
    let closed_count = ledger
        .findings
        .iter()
        .filter(|finding| finding.status == ReviewFindingStatus::Closed)
        .count();
    match format {
        "json" => {
            let mut payload =
                serde_json::to_value(ledger).unwrap_or_else(|_| serde_json::json!({}));
            if let serde_json::Value::Object(object) = &mut payload {
                object.insert("status".into(), serde_json::Value::String(status.into()));
                object.insert(
                    "unresolvedFindingCount".into(),
                    serde_json::Value::Number(unresolved_findings_count.into()),
                );
                object.insert(
                    "unresolvedRequirementCount".into(),
                    serde_json::Value::Number(unresolved_requirements_count.into()),
                );
                object.insert(
                    "ledgerPath".into(),
                    serde_json::Value::String(crate::runtime::display_path(ledger_path)),
                );
            }
            if let Ok(text) = serde_json::to_string_pretty(&payload) {
                let _ = writeln!(standard_output, "{text}");
            }
        }
        "markdown" => {
            let _ = writeln!(standard_output, "# Review Closeout");
            let _ = writeln!(standard_output);
            let _ = writeln!(standard_output, "- ledger: {}", ledger.id);
            let _ = writeln!(standard_output, "- head: {}", ledger.head_sha);
            let _ = writeln!(standard_output, "- status: {status}");
            let _ = writeln!(
                standard_output,
                "- unresolved_findings: {unresolved_findings_count}"
            );
            let _ = writeln!(
                standard_output,
                "- unresolved_requirements: {unresolved_requirements_count}"
            );
            let _ = writeln!(standard_output, "- closed_findings: {closed_count}");
            let _ = writeln!(standard_output);
            for item in unresolved_findings(ledger) {
                let location = item
                    .line
                    .map(|line| format!("{}:{line}", item.file))
                    .unwrap_or_else(|| item.file.clone());
                let _ = writeln!(
                    standard_output,
                    "- [{}] {} — {} — {}",
                    item.rule, location, item.message, item.evidence
                );
            }
            for item in unresolved_requirements(ledger) {
                let _ = writeln!(
                    standard_output,
                    "- [requirement] {} — {}",
                    item.text,
                    item.evidence.join("; ")
                );
            }
        }
        _ => {
            let _ = writeln!(
                standard_output,
                "ledger={} head={} status={} unresolved_findings={} unresolved_requirements={} closed_findings={}",
                ledger.id,
                ledger.head_sha,
                status,
                unresolved_findings_count,
                unresolved_requirements_count,
                closed_count
            );
            for item in unresolved_findings(ledger) {
                let location = item
                    .line
                    .map(|line| format!("{}:{line}", item.file))
                    .unwrap_or_else(|| item.file.clone());
                let _ = writeln!(
                    standard_output,
                    "finding={} file={} message={} evidence={}",
                    item.id, location, item.message, item.evidence
                );
            }
            for item in unresolved_requirements(ledger) {
                let _ = writeln!(
                    standard_output,
                    "requirement={} status={:?} evidence={}",
                    item.id,
                    item.status,
                    item.evidence.join("; ")
                );
            }
        }
    }
}

fn exact_ci_evidence(repository_root: &Path, head_sha: &str) -> (bool, String) {
    let provider = match resolve_provider("auto", Some(repository_root)) {
        ProviderResolution::Found(provider) => provider,
        ProviderResolution::NoneDetected => {
            return (
                false,
                format!("no supported CI provider detected for HEAD {head_sha}"),
            )
        }
        ProviderResolution::ExplicitUnavailable(provider) => {
            return (
                false,
                format!("requested CI provider {provider:?} is unavailable for HEAD {head_sha}"),
            )
        }
    };

    if provider != CiProvider::Gh {
        return (
            false,
            format!(
                "{} exact-head verification is not implemented; refusing to trust stale CI",
                provider.label()
            ),
        );
    }

    let run_list = match run_command(
        "gh",
        &[
            "run".to_string(),
            "list".to_string(),
            "--workflow".to_string(),
            "validate.yml".to_string(),
            "--commit".to_string(),
            head_sha.to_string(),
            "--limit".to_string(),
            "20".to_string(),
            "--json".to_string(),
            "databaseId,headSha,status,conclusion".to_string(),
        ],
        Some(repository_root),
    ) {
        Ok(result) if result.code == 0 => result,
        Ok(result) => {
            return (
                false,
                format!(
                    "gh run list failed with exit {}: {}",
                    result.code,
                    String::from_utf8_lossy(&result.stderr)
                ),
            )
        }
        Err(error) => return (false, format!("gh run list failed: {error}")),
    };
    let parsed: serde_json::Value = match serde_json::from_slice(&run_list.stdout) {
        Ok(value) => value,
        Err(error) => return (false, format!("gh run list returned invalid JSON: {error}")),
    };
    let selected = parsed.as_array().and_then(|runs| {
        runs.iter()
            .filter(|run| {
                run.get("headSha")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|sha| sha.eq_ignore_ascii_case(head_sha))
            })
            .max_by_key(|run| {
                run.get("databaseId")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0)
            })
    });
    let Some(run) = selected else {
        return (
            false,
            format!("no Validate run found for exact HEAD {head_sha}"),
        );
    };
    let run_id = run
        .get("databaseId")
        .map(serde_json::Value::to_string)
        .unwrap_or_else(|| "unknown".to_string());
    let status = run
        .get("status")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    let conclusion = run
        .get("conclusion")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    if status != "completed" {
        return (
            false,
            format!("GitHub Validate run {run_id} for HEAD {head_sha} is {status}"),
        );
    }
    if conclusion == "success" {
        (
            true,
            format!("GitHub Validate run {run_id} green for HEAD {head_sha}"),
        )
    } else {
        (
            false,
            format!("GitHub Validate run {run_id} for HEAD {head_sha} concluded {conclusion}"),
        )
    }
}
pub(crate) fn run_review_closeout_command(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    if arguments.is_empty()
        || arguments
            .iter()
            .any(|argument| super::is_help_argument(argument))
    {
        let _ = writeln!(
            standard_output,
            "Usage: keel review closeout [--repo-root <path>] [--base-ref <ref>] [--brief-id <id>] [--proof <text>] [--baseline <path>] [--baseline-reviewer <name>] [--baseline-reason <text>] [--baseline-expires <rfc3339>] [--write-baseline] [--format json|markdown|compact] [--strict] [--require-ci]"
        );
        return if arguments.is_empty() { 1 } else { 0 };
    }
    let mut flags = FlagSet::new("review closeout");
    flags.string_flag("repo-root", "");
    flags.string_flag("base-ref", "origin/main");
    flags.string_flag("brief-id", "");

    flags.string_flag("review-id", "");
    flags.string_flag("proof", "");
    flags.string_flag("format", "json");
    flags.bool_flag("strict", false);
    flags.bool_flag("require-ci", false);
    flags.string_flag("baseline", "");
    flags.string_flag("baseline-reviewer", "");
    flags.string_flag("baseline-reason", "");
    flags.string_flag("baseline-expires", "");
    flags.bool_flag("write-baseline", false);
    if let Err(error) = flags.parse(arguments) {
        let _ = writeln!(standard_error, "{}", error.message);
        return 1;
    }
    let format = flags.string_value("format").trim();
    if !matches!(format, "json" | "markdown" | "compact") {
        let _ = writeln!(standard_error, "review closeout: unknown format {format:?}");
        return 1;
    }

    let repository_root = match resolve_repository_root(flags.string_value("repo-root")) {
        Ok(path) => path,
        Err(error) => {
            let _ = writeln!(standard_error, "{error}");
            return 1;
        }
    };
    let claude_home = match resolve_claude_home("") {
        Ok(path) => path,
        Err(error) => {
            let _ = writeln!(standard_error, "{error}");
            return 1;
        }
    };
    let head_sha = match git_text(Some(&repository_root), &["rev-parse", "HEAD"]) {
        Some(head) if !head.trim().is_empty() => head.trim().to_string(),
        _ => {
            let _ = writeln!(standard_error, "review closeout: unable to resolve HEAD");
            return 1;
        }
    };
    let dirty_status = git_text(
        Some(&repository_root),
        &["status", "--porcelain=v1", "--untracked-files=all"],
    )
    .unwrap_or_default();
    let mut changed_paths = porcelain_paths(&dirty_status);
    let base_ref = {
        let value = flags.string_value("base-ref").trim();
        if value.is_empty() {
            "origin/main"
        } else {
            value
        }
    };
    let base_range = format!("{base_ref}...HEAD");
    if let Some(paths) = git_lines(
        Some(&repository_root),
        &["diff", "--name-only", &base_range],
    ) {
        changed_paths.extend(paths.into_iter().map(|path| path.replace('\\', "/")));
    }
    changed_paths.sort();
    changed_paths.dedup();
    let scope = scope_fingerprint(&head_sha, &changed_paths, &dirty_status);
    let review_id = {
        let value = flags.string_value("review-id").trim();
        if value.is_empty() {
            ledger_id(&head_sha)
        } else {
            value.to_string()
        }
    };
    let previous = match load_ledger(&claude_home, &review_id) {
        Ok(value) => value,
        Err(error) => {
            let _ = writeln!(standard_error, "{error}");
            return 1;
        }
    };
    let prior_findings = previous
        .as_ref()
        .map(|ledger| ledger.findings.as_slice())
        .unwrap_or(&[]);
    let prior_requirements = previous
        .as_ref()
        .map(|ledger| ledger.requirements.as_slice())
        .unwrap_or(&[]);
    let mut current_findings = Vec::new();
    let brief_id = flags.string_value("brief-id").trim();
    let mut current_requirements = Vec::new();
    if brief_id.is_empty() {
        current_findings.push(finding(
            "requirement:brief",
            if flags.bool_value("strict") {
                ReviewSeverity::Major
            } else {
                ReviewSeverity::Minor
            },
            "working-brief",
            None,
            if flags.bool_value("strict") {
                "strict closeout requires --brief-id".to_string()
            } else {
                "closeout has no --brief-id; acceptance criteria were not checked".to_string()
            },
            "no working brief id was supplied",
            &head_sha,
        ));
    } else {
        match read_brief(&claude_home, brief_id) {
            Ok(Some(brief)) => {
                let proof = flags.string_value("proof").trim();
                current_requirements = brief
                    .acceptance_criteria
                    .iter()
                    .map(|text| {
                        let id = stable_requirement_id(text);
                        ReviewRequirement {
                            id,
                            text: text.clone(),
                            status: ReviewFindingStatus::Open,
                            evidence: requirement_proof(proof, &stable_requirement_id(text))
                                .into_iter()
                                .collect(),
                        }
                    })
                    .collect();
                for requirement in &current_requirements {
                    if requirement.evidence.is_empty() {
                        current_findings.push(finding(
                            "requirement:evidence",
                            ReviewSeverity::Major,
                            "working-brief",
                            None,
                            format!(
                                "acceptance criterion {} lacks criterion-specific proof",
                                requirement.id
                            ),
                            format!(
                                "provide `{}=<command or artifact evidence>` in --proof",
                                requirement.id
                            ),
                            &head_sha,
                        ));
                    }
                }
            }
            Ok(None) => current_findings.push(finding(
                "requirement:brief",
                ReviewSeverity::Major,
                "working-brief",
                None,
                format!("working brief {brief_id} was not found"),
                "read_brief returned no brief",
                &head_sha,
            )),
            Err(error) => current_findings.push(finding(
                "requirement:brief",
                ReviewSeverity::Major,
                "working-brief",
                None,
                format!("working brief {brief_id} could not be read"),
                error.to_string(),
                &head_sha,
            )),
        }
    }
    let mut snapshots = Vec::new();
    snapshots.push(ReviewGateSnapshot {
        name: "head".to_string(),
        status: "current".to_string(),
        blocking: false,
        details: Some(format!("head_sha={head_sha}")),
    });
    snapshots.push(ReviewGateSnapshot {
        name: "scope".to_string(),
        status: "current".to_string(),
        blocking: false,
        details: Some(format!(
            "scope_fingerprint={scope} changed_paths={} dirty_status={}",
            changed_paths.len(),
            dirty_status.trim()
        )),
    });
    refresh_evidence(
        &repository_root,
        &changed_paths,
        &head_sha,
        &mut current_findings,
        &mut snapshots,
    );
    let gates = collect_review_gate_results(&repository_root, base_ref, "pre-pr", true, true, true);
    add_gate_findings(&gates, &mut current_findings, &mut snapshots, &head_sha);
    for item in crate::comment_lint::lint_tracked_tree(&repository_root) {
        current_findings.push(finding(
            format!("comment:{}", item.id),
            severity_for_text(&item.severity),
            item.file.clone(),
            Some(item.line),
            item.message.clone(),
            format!("{} ({})", item.message, item.id),
            &head_sha,
        ));
    }
    for item in crate::comment_lint::lint_tracked_tree_prose(&repository_root) {
        current_findings.push(finding(
            format!("prose:{}", item.id),
            severity_for_text(&item.severity),
            item.file.clone(),
            Some(item.line),
            item.message.clone(),
            format!("{} ({})", item.message, item.id),
            &head_sha,
        ));
    }
    for item in crate::slop_detector::lint_tracked_tree_slop(&repository_root) {
        current_findings.push(finding(
            format!("slop:{}", item.pattern),
            severity_for_text(item.severity),
            item.file.clone(),
            Some(item.line),
            item.message.clone(),
            format!("{} ({})", item.message, item.pattern),
            &head_sha,
        ));
    }
    let wiring_findings_before = current_findings.len();
    add_wiring_findings(&repository_root, &mut current_findings, &head_sha);
    let wiring_findings = current_findings.len() - wiring_findings_before;
    snapshots.push(ReviewGateSnapshot {
        name: "wiring".to_string(),
        status: if wiring_findings == 0 { "pass" } else { "fail" }.to_string(),
        blocking: true,
        details: Some(format!("{wiring_findings} wiring mismatches")),
    });
    if flags.bool_value("require-ci") {
        let (ci_green, ci_evidence) = if dirty_status.trim().is_empty() {
            exact_ci_evidence(&repository_root, &head_sha)
        } else {
            (
                false,
                format!(
                    "working tree is dirty; CI can only prove committed HEAD {head_sha}: {}",
                    dirty_status.trim()
                ),
            )
        };
        if !ci_green {
            current_findings.push(finding(
                "ci:exact-head",
                ReviewSeverity::Major,
                "review/ci",
                None,
                "exact-head CI evidence is unavailable or not green",
                ci_evidence.clone(),
                &head_sha,
            ));
        }
        snapshots.push(ReviewGateSnapshot {
            name: "ci".to_string(),
            status: if ci_green { "pass" } else { "blocked" }.to_string(),
            blocking: true,
            details: Some(ci_evidence),
        });
    } else {
        snapshots.push(ReviewGateSnapshot {
            name: "ci".to_string(),
            status: "not_required".to_string(),
            blocking: false,
            details: Some("CI evidence was not required".to_string()),
        });
    }
    let baseline_argument = flags.string_value("baseline").trim();
    let baseline_requested = !baseline_argument.is_empty();
    let write_baseline = flags.bool_value("write-baseline");
    if write_baseline && !dirty_status.trim().is_empty() {
        let _ = writeln!(
            standard_error,
            "review closeout: --write-baseline requires a clean working tree"
        );
        return 1;
    }
    let baseline_path = match review_baseline_path(&repository_root, baseline_argument) {
        Ok(path) => path,
        Err(error) => {
            let _ = writeln!(standard_error, "{error}");
            return 1;
        }
    };
    let mut baseline = None;
    let mut baseline_error = None;
    let mut baseline_status = "not_configured";
    if !write_baseline {
        match load_review_baseline(&baseline_path, &now_rfc3339()) {
            Ok(Some(value)) => {
                baseline = Some(value);
                baseline_status = "pass";
            }
            Ok(None) if baseline_requested => {
                let error = format!(
                    "review baseline was not found at {}",
                    display_path(&baseline_path)
                );
                baseline_error = Some(error.clone());
                current_findings.push(finding(
                    "baseline:missing",
                    ReviewSeverity::Major,
                    display_path(&baseline_path),
                    None,
                    "review baseline is missing",
                    error,
                    &head_sha,
                ));
                baseline_status = "blocked";
            }
            Ok(None) => {}
            Err(error) => {
                baseline_error = Some(error.clone());
                current_findings.push(finding(
                    "baseline:invalid",
                    ReviewSeverity::Major,
                    display_path(&baseline_path),
                    None,
                    "review baseline is invalid",
                    error,
                    &head_sha,
                ));
                baseline_status = "blocked";
            }
        }
    } else {
        let reviewed_by = flags.string_value("baseline-reviewer").trim();
        let reason = flags.string_value("baseline-reason").trim();
        let expires_at = flags.string_value("baseline-expires").trim();
        if reviewed_by.is_empty() || reason.is_empty() || expires_at.is_empty() {
            let _ = writeln!(
                standard_error,
                "review closeout: --write-baseline requires --baseline-reviewer, --baseline-reason, and --baseline-expires"
            );
            return 1;
        }
        let generated = match build_review_baseline(
            &current_findings,
            &head_sha,
            reviewed_by,
            reason,
            expires_at,
        ) {
            Ok(value) => value,
            Err(error) => {
                let _ = writeln!(standard_error, "{error}");
                return 1;
            }
        };
        if let Err(error) = save_review_baseline(&baseline_path, &generated) {
            let _ = writeln!(standard_error, "{error}");
            return 1;
        }
        baseline = Some(generated);
        baseline_status = "written";
    }

    let reconciled_findings = reconcile_findings(prior_findings, &current_findings, &head_sha);
    let (reconciled_findings, baseline_suppressed) =
        apply_review_baseline(reconciled_findings, baseline.as_ref(), &head_sha);
    let baseline_details = baseline.as_ref().map(|value| {
        format!(
            "path={} reviewer={} expires_at={} suppressed_findings={}",
            display_path(&baseline_path),
            value.reviewed_by,
            value.expires_at,
            baseline_suppressed
        )
    });
    snapshots.push(ReviewGateSnapshot {
        name: "baseline".to_string(),
        status: baseline_status.to_string(),
        blocking: baseline_error.is_some(),
        details: baseline_details.or_else(|| baseline_error.clone()),
    });
    let all_requirements_proven = !current_requirements.is_empty()
        && current_requirements
            .iter()
            .all(|requirement| !requirement.evidence.is_empty());
    if all_requirements_proven
        && reconciled_findings
            .iter()
            .all(|finding| finding.status == ReviewFindingStatus::Closed)
    {
        for requirement in &mut current_requirements {
            requirement.status = ReviewFindingStatus::Closed;
        }
    }
    let reconciled_requirements = reconcile_requirements(prior_requirements, &current_requirements);
    let now = now_rfc3339();
    let ledger = ReviewLedger {
        schema_version: REVIEW_LEDGER_SCHEMA,
        id: review_id,
        head_sha,
        base_ref: base_ref.to_string(),
        repo_root: crate::runtime::display_path(&repository_root),
        scope_fingerprint: scope,
        created_at: previous
            .as_ref()
            .map(|ledger| ledger.created_at.clone())
            .unwrap_or_else(|| now.clone()),
        updated_at: now,
        requirements: reconciled_requirements,
        findings: reconciled_findings,
        gates: snapshots,
    };
    let path = match save_ledger(&claude_home, &ledger) {
        Ok(path) => path,
        Err(error) => {
            let _ = writeln!(standard_error, "{error}");
            return 1;
        }
    };
    let unresolved_finding_count = unresolved_findings(&ledger).len();
    let unresolved_requirement_count = unresolved_requirements(&ledger).len();
    let status = if unresolved_finding_count == 0 && unresolved_requirement_count == 0 {
        "passed"
    } else {
        "blocked"
    };
    render_closeout(
        format,
        &ledger,
        &path,
        status,
        unresolved_finding_count,
        unresolved_requirement_count,
        standard_output,
    );
    if status != "passed" {
        1
    } else {
        0
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_home(label: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "keel-review-closeout-{label}-{}-{stamp}",
            std::process::id()
        ))
    }

    fn finding(id: &str, status: ReviewFindingStatus) -> ReviewFinding {
        ReviewFinding {
            id: id.to_string(),
            rule: "R-1".to_string(),
            severity: ReviewSeverity::Major,
            status,
            file: "src/lib.rs".to_string(),
            line: Some(10),
            message: "must be fixed".to_string(),
            evidence: "evidence".to_string(),
            first_seen_head: "head-a".to_string(),
            last_seen_head: "head-a".to_string(),
            closed_head: None,
        }
    }

    fn ledger() -> ReviewLedger {
        ReviewLedger {
            schema_version: REVIEW_LEDGER_SCHEMA,
            id: ledger_id("0123456789abcdef"),
            head_sha: "0123456789abcdef".to_string(),
            base_ref: "origin/main".to_string(),
            repo_root: "D:/repo".to_string(),
            scope_fingerprint: "scope".to_string(),
            created_at: "2026-08-22T00:00:00Z".to_string(),
            updated_at: "2026-08-22T00:01:00Z".to_string(),
            requirements: vec![ReviewRequirement {
                id: stable_requirement_id("Requirement one"),
                text: "Requirement one".to_string(),
                status: ReviewFindingStatus::Closed,
                evidence: vec!["test output".to_string()],
            }],
            findings: vec![finding(
                &stable_finding_id("R-1", "src/lib.rs", Some(10), "must be fixed"),
                ReviewFindingStatus::Open,
            )],
            gates: vec![ReviewGateSnapshot {
                name: "tests".to_string(),
                status: "passed".to_string(),
                blocking: false,
                details: Some("ok".to_string()),
            }],
        }
    }

    #[test]
    fn ledger_path_rejects_traversal() {
        let home = Path::new("/tmp/claude");
        assert!(ledger_path(home, "../escape").is_err());
        assert!(ledger_path(home, r"nested\\escape").is_err());
    }

    #[test]
    fn missing_ledger_returns_none() {
        let home = test_home("missing");
        assert_eq!(load_ledger(&home, "review-head").expect("load"), None);
    }

    #[test]
    fn malformed_json_and_schema_return_errors() {
        let home = test_home("malformed");
        let path = ledger_path(&home, "review-head").expect("path");
        std::fs::create_dir_all(path.parent().expect("parent")).expect("parent");
        std::fs::write(&path, "not json").expect("write");
        assert!(load_ledger(&home, "review-head").is_err());

        std::fs::write(
            &path,
            serde_json::json!({
                "schema_version": 99,
                "id": "review-head",
                "head_sha": "head",
                "base_ref": "main",
                "repo_root": ".",
                "scope_fingerprint": "scope",
                "created_at": "now",
                "updated_at": "now",
                "requirements": [],
                "findings": [],
                "gates": []
            })
            .to_string(),
        )
        .expect("write schema");
        assert!(load_ledger(&home, "review-head").is_err());
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn stable_id_tracks_line_number_for_distinct_findings() {
        assert_ne!(
            stable_finding_id("R-1", "src/lib.rs", Some(10), "must be fixed"),
            stable_finding_id("R-1", "src/lib.rs", Some(42), "must be fixed")
        );
    }
    #[test]
    fn criterion_proof_requires_matching_requirement_id() {
        let id = stable_requirement_id("compile");
        assert_eq!(
            requirement_proof(&format!("{id}=cargo test passed"), &id),
            Some("cargo test passed".to_string())
        );
        assert_eq!(requirement_proof("generic proof", &id), None);
        assert_eq!(requirement_proof("other=proof", &id), None);
    }

    #[test]
    fn reconciliation_closes_absent_finding_and_preserves_history() {
        let old = finding(
            &stable_finding_id("R-1", "src/lib.rs", Some(10), "must be fixed"),
            ReviewFindingStatus::Open,
        );
        let result = reconcile_findings(&[old], &[], "head-b");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].status, ReviewFindingStatus::Closed);
        assert_eq!(result[0].closed_head.as_deref(), Some("head-b"));
        assert_eq!(result[0].first_seen_head, "head-a");
    }

    #[test]
    fn reconciliation_keeps_current_finding_open_and_updates_head() {
        let id = stable_finding_id("R-1", "src/lib.rs", Some(10), "must be fixed");
        let old = finding(&id, ReviewFindingStatus::Open);
        let mut current = finding(&id, ReviewFindingStatus::Closed);
        current.line = Some(10);
        let result = reconcile_findings(&[old], &[current], "head-b");
        assert_eq!(result[0].status, ReviewFindingStatus::Open);
        assert_eq!(result[0].first_seen_head, "head-a");
        assert_eq!(result[0].last_seen_head, "head-b");
        assert_eq!(result[0].line, Some(10));
        assert_eq!(result[0].closed_head, None);
    }

    #[test]
    fn requirement_proof_does_not_close_without_clean_scan_status() {
        let current = vec![ReviewRequirement {
            id: stable_requirement_id("compile"),
            text: "compile".to_string(),
            status: ReviewFindingStatus::Open,
            evidence: vec!["cargo test passed".to_string()],
        }];
        let result = reconcile_requirements(&[], &current);
        assert_eq!(result[0].status, ReviewFindingStatus::Open);
    }

    #[test]
    fn closeout_help_is_actionable() {
        let mut output = Vec::new();
        let mut error = Vec::new();
        let code = run_review_closeout_command(&["--help".to_string()], &mut output, &mut error);
        assert_eq!(code, 0);
        assert!(String::from_utf8_lossy(&output).contains("--brief-id"));
        assert!(error.is_empty());
    }

    #[test]
    fn scope_fingerprint_is_order_independent_but_tracks_dirty_status() {
        let first = scope_fingerprint("head", &["b.rs".to_string(), "a.rs".to_string()], "dirty");
        let second = scope_fingerprint("head", &["a.rs".to_string(), "b.rs".to_string()], "dirty");
        let clean = scope_fingerprint("head", &["a.rs".to_string(), "b.rs".to_string()], "clean");
        assert_eq!(first, second);
        assert_ne!(first, clean);
    }

    #[test]
    fn save_load_round_trip_preserves_all_fields() {
        let home = test_home("round-trip");
        let expected = ledger();
        let path = save_ledger(&home, &expected).expect("save");
        assert_eq!(path, ledger_path(&home, &expected.id).expect("path"));
        let actual = load_ledger(&home, &expected.id)
            .expect("load")
            .expect("ledger");
        assert_eq!(actual, expected);
        let _ = std::fs::remove_dir_all(home);
    }
    #[test]
    fn baseline_path_rejects_escape() {
        let root = test_home("baseline-path");
        assert!(review_baseline_path(&root, "../outside.json").is_err());
        assert!(review_baseline_path(&root, "review-closeout-baseline.json").is_ok());
    }

    #[test]
    fn baseline_validation_requires_review_and_future_expiry() {
        let valid = ReviewBaseline {
            schema_version: REVIEW_BASELINE_SCHEMA,
            generated_from_head: "head-a".to_string(),
            generated_at: "2026-08-24T00:00:00Z".to_string(),
            expires_at: "2027-08-24T00:00:00Z".to_string(),
            reviewed_by: "reviewer".to_string(),
            reason: "historical static findings".to_string(),
            finding_ids: vec!["comment-1".to_string()],
        };
        assert!(validate_review_baseline(&valid, "2026-08-24T00:00:01Z").is_ok());

        let mut expired = valid.clone();
        expired.expires_at = "2026-08-24T00:00:00Z".to_string();
        assert!(validate_review_baseline(&expired, "2026-08-24T00:00:01Z").is_err());

        let mut unreviewed = valid;
        unreviewed.reviewed_by.clear();
        assert!(validate_review_baseline(&unreviewed, "2026-08-24T00:00:01Z").is_err());
    }

    #[test]
    fn baseline_suppresses_only_exact_finding_ids() {
        let mut historical = finding(
            &stable_finding_id("comment:style", "docs/a.md", Some(4), "historical"),
            ReviewFindingStatus::Open,
        );
        historical.rule = "comment:style".to_string();
        historical.file = "docs/a.md".to_string();
        historical.line = Some(4);
        historical.message = "historical".to_string();
        let mut new_finding = historical.clone();
        new_finding.id = stable_finding_id("comment:style", "docs/a.md", Some(5), "new");
        new_finding.line = Some(5);
        new_finding.message = "new".to_string();
        let baseline = ReviewBaseline {
            schema_version: REVIEW_BASELINE_SCHEMA,
            generated_from_head: "head-a".to_string(),
            generated_at: "2026-08-24T00:00:00Z".to_string(),
            expires_at: "2027-08-24T00:00:00Z".to_string(),
            reviewed_by: "reviewer".to_string(),
            reason: "historical static findings".to_string(),
            finding_ids: vec![historical.id.clone()],
        };
        let (result, suppressed) =
            apply_review_baseline(vec![historical, new_finding], Some(&baseline), "head-b");
        assert_eq!(suppressed, 1);
        assert_eq!(result[0].status, ReviewFindingStatus::Closed);
        assert_eq!(result[0].closed_head.as_deref(), Some("head-b"));
        assert_eq!(result[1].status, ReviewFindingStatus::Open);
    }

    #[test]
    fn baseline_generation_excludes_dynamic_findings() {
        let mut static_finding = finding(
            &stable_finding_id("comment:style", "docs/a.md", Some(4), "historical"),
            ReviewFindingStatus::Open,
        );
        static_finding.rule = "comment:style".to_string();
        let dynamic_finding = finding(
            &stable_finding_id("ci:exact-head", "review/ci", None, "ci"),
            ReviewFindingStatus::Open,
        );
        let baseline = build_review_baseline(
            &[static_finding, dynamic_finding],
            "head-a",
            "reviewer",
            "historical static findings",
            "2027-08-24T00:00:00Z",
        )
        .expect("baseline");
        assert_eq!(baseline.finding_ids.len(), 1);
        assert!(baseline.finding_ids[0].starts_with("comment-style-"));
    }

    #[test]
    fn baseline_generation_includes_comment_and_prose_gate_summaries() {
        let mut aggregate = finding(
            &stable_finding_id("gate:comment_style", "review/comment_style", None, "gate"),
            ReviewFindingStatus::Open,
        );
        aggregate.rule = "gate:comment_style".to_string();
        let baseline = build_review_baseline(
            &[aggregate],
            "head-a",
            "reviewer",
            "historical static findings",
            "2027-08-24T00:00:00Z",
        )
        .expect("baseline");
        assert_eq!(baseline.finding_ids.len(), 1);
        assert!(baseline.finding_ids[0].starts_with("gate-comment_style-"));
    }

    #[test]
    fn closeout_help_advertises_baseline_controls() {
        let mut output = Vec::new();
        let mut error = Vec::new();
        let code = run_review_closeout_command(&["--help".to_string()], &mut output, &mut error);
        let text = String::from_utf8_lossy(&output);
        assert_eq!(code, 0);
        assert!(text.contains("--baseline"));
        assert!(text.contains("--write-baseline"));
        assert!(error.is_empty());
    }
}
