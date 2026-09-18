//! Purpose: Jev-inspired typed decisions, calibration, structured scoring,
//!   shell risk noul evaluation, and outcome learning (J03-J11).
//! Caller: `runner::hook_lifecycle`, `runner::bridge`, `utility::skill_match`,
//!   `review`, `mcp::tools`.
//! Dependencies: std, serde_json, crate::runtime.
//! Main Functions: ConfidenceCalibrator, ReviewScore, PlanScore, ShellNoul,
//!   ReviewOutcome, SkillComposition.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::runner::hook_lifecycle::PreToolGateDecision;
use crate::runtime::state_directory;
// ============================================================================
// J03: Calibrated Confidence for Skill Routing
// ============================================================================

/// Number of bins for confidence calibration: [0.0..0.1), [0.1..0.2), ..., [0.9..1.0].
pub const CALIBRATION_BINS: usize = 10;

/// Minimum samples in a bucket before empirical accuracy is blended.
pub const MIN_CALIBRATION_SAMPLES: usize = 3;

/// Single calibration bin tracking predictions and actual outcomes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalibrationBucket {
    pub total: usize,
    pub correct: usize,
}

impl CalibrationBucket {
    pub fn empirical_accuracy(&self) -> f64 {
        if self.total == 0 {
            0.5
        } else {
            (self.correct as f64 + 1.0) / (self.total as f64 + 2.0)
        }
    }
}

/// Per-skill calibration record stored under `<claude_home>/state/skill-calibration/<skill>.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillCalibrationRecord {
    pub skill_name: String,
    pub buckets: [CalibrationBucket; CALIBRATION_BINS],
    pub updated_at_ms: u64,
}

impl SkillCalibrationRecord {
    pub fn new(skill_name: &str) -> Self {
        Self {
            skill_name: skill_name.to_string(),
            buckets: [CalibrationBucket::default(); CALIBRATION_BINS],
            updated_at_ms: current_time_ms(),
        }
    }

    pub fn record(&mut self, computed_confidence: f64, was_correct: bool) {
        let bin = confidence_to_bin(computed_confidence);
        self.buckets[bin].total = self.buckets[bin].total.saturating_add(1);
        if was_correct {
            self.buckets[bin].correct = self.buckets[bin].correct.saturating_add(1);
        }
        self.updated_at_ms = current_time_ms();
    }

    pub fn calibrated_confidence(&self, computed_confidence: f64) -> f64 {
        let bin = confidence_to_bin(computed_confidence);
        let bucket = self.buckets[bin];
        if bucket.total < MIN_CALIBRATION_SAMPLES {
            return computed_confidence.clamp(0.0, 1.0);
        }
        let empirical = bucket.empirical_accuracy();
        let weight = (bucket.total as f64 / (bucket.total as f64 + 10.0)).clamp(0.0, 0.85);
        (weight * empirical + (1.0 - weight) * computed_confidence).clamp(0.0, 1.0)
    }
}

pub fn confidence_to_bin(confidence: f64) -> usize {
    let clamped = confidence.clamp(0.0, 1.0);
    let bin = (clamped * CALIBRATION_BINS as f64).floor() as usize;
    bin.min(CALIBRATION_BINS - 1)
}

fn calibration_dir(claude_home: &Path) -> PathBuf {
    state_directory(claude_home).join("skill-calibration")
}

fn calibration_file(claude_home: &Path, skill_name: &str) -> PathBuf {
    let safe_name = sanitize_key(skill_name);
    calibration_dir(claude_home).join(format!("{safe_name}.json"))
}

pub fn load_skill_calibration(claude_home: &Path, skill_name: &str) -> SkillCalibrationRecord {
    let path = calibration_file(claude_home, skill_name);
    if let Ok(text) = fs::read_to_string(&path) {
        if let Ok(record) = serde_json::from_str::<SkillCalibrationRecord>(&text) {
            return record;
        }
    }
    SkillCalibrationRecord::new(skill_name)
}

pub fn save_skill_calibration(
    claude_home: &Path,
    record: &SkillCalibrationRecord,
) -> Result<(), String> {
    let path = calibration_file(claude_home, &record.skill_name);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let text =
        serde_json::to_string_pretty(record).map_err(|e| format!("serialize calibration: {e}"))?;
    fs::write(&path, text).map_err(|e| format!("write calibration: {e}"))
}

pub fn record_and_save_skill_calibration(
    claude_home: &Path,
    skill_name: &str,
    computed_confidence: f64,
    was_correct: bool,
) -> Result<f64, String> {
    let mut record = load_skill_calibration(claude_home, skill_name);
    record.record(computed_confidence, was_correct);
    let calibrated = record.calibrated_confidence(computed_confidence);
    save_skill_calibration(claude_home, &record)?;
    Ok(calibrated)
}

pub fn get_calibrated_confidence(
    claude_home: &Path,
    skill_name: &str,
    computed_confidence: f64,
) -> f64 {
    let record = load_skill_calibration(claude_home, skill_name);
    record.calibrated_confidence(computed_confidence)
}

// ============================================================================
// J04: Review Gates as Typed Score Operations
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewCriterion {
    pub name: String,
    pub description: String,
    pub weight: f64,
}

pub fn default_review_rubric() -> Vec<ReviewCriterion> {
    vec![
        ReviewCriterion {
            name: "correctness".to_string(),
            description: "Logical soundness and adherence to requirements".to_string(),
            weight: 0.30,
        },
        ReviewCriterion {
            name: "security".to_string(),
            description: "Absence of injection, auth bypass, or secret leakage".to_string(),
            weight: 0.25,
        },
        ReviewCriterion {
            name: "readability".to_string(),
            description: "Idiomatic style, clean structure, no slop or dead code".to_string(),
            weight: 0.15,
        },
        ReviewCriterion {
            name: "test_coverage".to_string(),
            description: "Presence and adequacy of tests for changed paths".to_string(),
            weight: 0.15,
        },
        ReviewCriterion {
            name: "error_handling".to_string(),
            description: "Explicit error recovery, no unhandled panic or unwrap in production"
                .to_string(),
            weight: 0.15,
        },
    ]
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReviewScoreVerdict {
    Pass,
    Escalate,
    Block,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewScoreDecision {
    pub mean_score: f64,
    pub confidence: f64,
    pub verdict: ReviewScoreVerdict,
    pub scores: BTreeMap<String, f64>,
    pub flags: Vec<String>,
    pub summary: String,
}

impl ReviewScoreDecision {
    pub fn is_pass(&self) -> bool {
        self.verdict == ReviewScoreVerdict::Pass
    }

    pub fn to_gate_decision(&self) -> PreToolGateDecision {
        match self.verdict {
            ReviewScoreVerdict::Pass => PreToolGateDecision::allow(),
            ReviewScoreVerdict::Escalate => PreToolGateDecision::deny_with_confidence(
                "Review score below threshold with low confidence — escalation required",
                self.confidence,
                true,
                "review_score",
            ),
            ReviewScoreVerdict::Block => PreToolGateDecision::deny_with_confidence(
                "Review score below threshold — review findings must be addressed",
                self.confidence,
                false,
                "review_score",
            ),
        }
    }
}

pub fn evaluate_review_scores(
    scores: &BTreeMap<String, f64>,
    confidence: f64,
    rubric: &[ReviewCriterion],
    flags: Vec<String>,
    summary: String,
) -> ReviewScoreDecision {
    let mut total_weight = 0.0;
    let mut weighted_sum = 0.0;

    for criterion in rubric {
        let score = scores
            .get(&criterion.name)
            .copied()
            .unwrap_or(0.5)
            .clamp(0.0, 1.0);
        weighted_sum += score * criterion.weight;
        total_weight += criterion.weight;
    }

    let mean_score = if total_weight > 0.0 {
        weighted_sum / total_weight
    } else {
        0.5
    };

    let clamped_confidence = confidence.clamp(0.0, 1.0);

    let verdict = if mean_score >= 0.75 && clamped_confidence >= 0.70 {
        ReviewScoreVerdict::Pass
    } else if mean_score < 0.75 && clamped_confidence < 0.60 {
        ReviewScoreVerdict::Escalate
    } else {
        ReviewScoreVerdict::Block
    };

    ReviewScoreDecision {
        mean_score,
        confidence: clamped_confidence,
        verdict,
        scores: scores.clone(),
        flags,
        summary,
    }
}

pub fn parse_review_score_feedback(feedback_json: &Value) -> Result<ReviewScoreDecision, String> {
    let scores_obj = feedback_json
        .get("scores")
        .and_then(Value::as_object)
        .ok_or_else(|| "missing or invalid 'scores' object in review feedback".to_string())?;

    let mut scores = BTreeMap::new();
    for (k, v) in scores_obj {
        if let Some(num) = v.as_f64() {
            scores.insert(k.clone(), num.clamp(0.0, 1.0));
        }
    }

    let confidence = feedback_json
        .get("confidence")
        .and_then(Value::as_f64)
        .unwrap_or(0.7);

    let flags = feedback_json
        .get("flags")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(String::from)
                .collect()
        })
        .unwrap_or_default();

    let summary = feedback_json
        .get("summary")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    let rubric = default_review_rubric();
    Ok(evaluate_review_scores(
        &scores, confidence, &rubric, flags, summary,
    ))
}

// ============================================================================
// J05: Plan Evaluation as Typed Score Operations
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlanScoreVerdict {
    Ready,
    Escalate,
    NotReady,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanScoreDecision {
    pub mean_score: f64,
    pub confidence: f64,
    pub verdict: PlanScoreVerdict,
    pub scores: BTreeMap<String, f64>,
    pub missing: Vec<String>,
    pub suggestions: Vec<String>,
}

impl PlanScoreDecision {
    pub fn is_ready(&self) -> bool {
        self.verdict == PlanScoreVerdict::Ready
    }
}

pub const PLAN_CRITERIA: &[&str] = &[
    "has_acceptance_criteria",
    "has_validation_plan",
    "scope_defined",
    "risks_identified",
    "dependencies_clear",
];

pub fn evaluate_plan_scores(
    scores: &BTreeMap<String, f64>,
    confidence: f64,
    missing: Vec<String>,
    suggestions: Vec<String>,
) -> PlanScoreDecision {
    let mut sum = 0.0;
    let mut count = 0usize;
    for criterion in PLAN_CRITERIA {
        let score = scores
            .get(*criterion)
            .copied()
            .unwrap_or(0.0)
            .clamp(0.0, 1.0);
        sum += score;
        count += 1;
    }
    let mean_score = if count > 0 { sum / count as f64 } else { 0.0 };
    let clamped_confidence = confidence.clamp(0.0, 1.0);

    let verdict = if mean_score >= 0.80 && clamped_confidence >= 0.70 {
        PlanScoreVerdict::Ready
    } else if clamped_confidence < 0.60 {
        PlanScoreVerdict::Escalate
    } else {
        PlanScoreVerdict::NotReady
    };

    PlanScoreDecision {
        mean_score,
        confidence: clamped_confidence,
        verdict,
        scores: scores.clone(),
        missing,
        suggestions,
    }
}

pub fn parse_plan_score_feedback(feedback_json: &Value) -> Result<PlanScoreDecision, String> {
    let scores_obj = feedback_json
        .get("scores")
        .and_then(Value::as_object)
        .ok_or_else(|| "missing or invalid 'scores' object in plan feedback".to_string())?;

    let mut scores = BTreeMap::new();
    for (k, v) in scores_obj {
        if let Some(num) = v.as_f64() {
            scores.insert(k.clone(), num.clamp(0.0, 1.0));
        }
    }

    let confidence = feedback_json
        .get("confidence")
        .and_then(Value::as_f64)
        .unwrap_or(0.7);

    let missing = feedback_json
        .get("missing")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(String::from)
                .collect()
        })
        .unwrap_or_default();

    let suggestions = feedback_json
        .get("suggestions")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(String::from)
                .collect()
        })
        .unwrap_or_default();

    Ok(evaluate_plan_scores(
        &scores,
        confidence,
        missing,
        suggestions,
    ))
}

// ============================================================================
// J06: Shell Destructive as Typed Noul Operations
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShellRiskCategory {
    Destructive,
    Risky,
    Safe,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShellRiskAction {
    Allow,
    Warn,
    Block,
    Escalate,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShellNoulDecision {
    pub command: String,
    pub probability: f64,
    pub confidence: f64,
    pub category: ShellRiskCategory,
    pub action: ShellRiskAction,
    pub reason: String,
}

impl ShellNoulDecision {
    pub fn is_allowed(&self) -> bool {
        matches!(self.action, ShellRiskAction::Allow | ShellRiskAction::Warn)
    }

    pub fn to_gate_decision(&self) -> PreToolGateDecision {
        match self.action {
            ShellRiskAction::Allow => PreToolGateDecision::allow(),
            ShellRiskAction::Warn => PreToolGateDecision::warn(
                "Command carries potential operational risk",
                self.confidence,
                true,
            ),
            ShellRiskAction::Block => PreToolGateDecision::deny_with_confidence(
                "Shell command classified as destructive and blocked by safety policy",
                self.confidence,
                false,
                "shell_noul",
            ),
            ShellRiskAction::Escalate => PreToolGateDecision::deny_with_confidence(
                "Shell command safety status uncertain — human escalation required",
                self.confidence,
                true,
                "shell_noul",
            ),
        }
    }
}

pub fn evaluate_shell_command_noul(command: &str) -> ShellNoulDecision {
    let trimmed = command.trim();
    let lower = trimmed.to_ascii_lowercase();

    // Known catastrophic destructive commands: probability >= 0.95, confidence = 0.95 -> Block
    let catastrophic = [
        "rm -rf /",
        "rm -rf /*",
        "rm -rf ~",
        "mkfs",
        ":(){ :|:& };:",
        "dd if=/dev/zero of=/dev/sd",
        "chmod -R 777 /",
        "drop database",
    ];
    for pat in catastrophic {
        if lower.contains(pat) {
            return ShellNoulDecision {
                command: trimmed.to_string(),
                probability: 0.95,
                confidence: 0.95,
                category: ShellRiskCategory::Destructive,
                action: ShellRiskAction::Block,
                reason: format!("Matches catastrophic destructive pattern: '{pat}'"),
            };
        }
    }

    // High risk destructive patterns: rm -rf with paths, git push --force to main, drop table
    let high_risk = [
        "rm -rf",
        "rm -r -f",
        "git push --force",
        "git push -f",
        "git reset --hard",
        "drop table",
        "truncate table",
        "kill -9 -1",
    ];
    for pat in high_risk {
        if lower.contains(pat) {
            return ShellNoulDecision {
                command: trimmed.to_string(),
                probability: 0.80,
                confidence: 0.85,
                category: ShellRiskCategory::Destructive,
                action: ShellRiskAction::Block,
                reason: format!("Matches destructive pattern: '{pat}'"),
            };
        }
    }

    // Risky modifications: package uninstalls, kill commands, git stash drop, broad chown
    let risky = [
        "apt-get remove",
        "npm un",
        "npm uninstall",
        "pip uninstall",
        "git stash drop",
        "pkill",
        "killall",
    ];
    for pat in risky {
        if lower.contains(pat) {
            return ShellNoulDecision {
                command: trimmed.to_string(),
                probability: 0.45,
                confidence: 0.80,
                category: ShellRiskCategory::Risky,
                action: ShellRiskAction::Warn,
                reason: format!("Potentially disruptive operation: '{pat}'"),
            };
        }
    }

    // Safe read-only / standard developer commands: probability < 0.15, confidence >= 0.9
    let safe_prefixes = [
        "git status",
        "git log",
        "git diff",
        "git branch",
        "cargo test",
        "cargo check",
        "cargo build",
        "cargo clippy",
        "cargo fmt",
        "npm test",
        "npm run build",
        "ls",
        "dir",
        "cat",
        "echo",
        "pwd",
        "grep",
        "keel",
    ];
    for prefix in safe_prefixes {
        if lower.starts_with(prefix) {
            return ShellNoulDecision {
                command: trimmed.to_string(),
                probability: 0.05,
                confidence: 0.95,
                category: ShellRiskCategory::Safe,
                action: ShellRiskAction::Allow,
                reason: "Standard safe developer command".to_string(),
            };
        }
    }

    // Default: low probability, moderate confidence
    ShellNoulDecision {
        command: trimmed.to_string(),
        probability: 0.20,
        confidence: 0.75,
        category: ShellRiskCategory::Safe,
        action: ShellRiskAction::Allow,
        reason: "No destructive markers detected".to_string(),
    }
}

// ============================================================================
// J07: Escalation Formatting
// ============================================================================

pub fn format_gate_escalation(gate_name: &str, reason: &str, confidence: f64) -> String {
    format!(
        "KEEL_GATE_ESCALATE\ngate: {gate_name}\nconfidence: {confidence:.2}\nreason: {reason}\n\
         note: This gate denial has confidence below the autonomous threshold (0.60). \
         Human verification or explicit override is required."
    )
}

// ============================================================================
// J08: Closed-Loop Learning for Skill Routing
// ============================================================================

pub fn record_skill_session_outcome(
    claude_home: &Path,
    skill_name: &str,
    was_helpful: bool,
    predicted_confidence: f64,
) -> Result<(), String> {
    crate::utility::skill_usage::record_skill_outcome(claude_home, skill_name, was_helpful);
    record_and_save_skill_calibration(claude_home, skill_name, predicted_confidence, was_helpful)?;
    Ok(())
}

// ============================================================================
// J09: Review Accuracy Feedback Loop
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewOutcome {
    pub session_id: String,
    pub issues_flagged: usize,
    pub issues_confirmed: usize,
    pub issues_false_positive: usize,
    pub confidence: f64,
    pub recorded_at_ms: u64,
}

impl ReviewOutcome {
    pub fn new(
        session_id: &str,
        issues_flagged: usize,
        issues_confirmed: usize,
        issues_false_positive: usize,
        confidence: f64,
    ) -> Self {
        Self {
            session_id: session_id.to_string(),
            issues_flagged,
            issues_confirmed,
            issues_false_positive,
            confidence: confidence.clamp(0.0, 1.0),
            recorded_at_ms: current_time_ms(),
        }
    }

    pub fn precision(&self) -> f64 {
        if self.issues_flagged == 0 {
            1.0
        } else {
            self.issues_confirmed as f64 / self.issues_flagged as f64
        }
    }

    pub fn brier_calibration_error(&self) -> f64 {
        let p = self.precision();
        (self.confidence - p).powi(2)
    }
}

fn review_outcome_dir(claude_home: &Path) -> PathBuf {
    state_directory(claude_home).join("review-outcomes")
}

pub fn save_review_outcome(claude_home: &Path, outcome: &ReviewOutcome) -> Result<(), String> {
    let dir = review_outcome_dir(claude_home);
    let _ = fs::create_dir_all(&dir);
    let key = sanitize_key(&outcome.session_id);
    let file = dir.join(format!("{key}.json"));
    let text = serde_json::to_string_pretty(outcome)
        .map_err(|e| format!("serialize review outcome: {e}"))?;
    fs::write(&file, text).map_err(|e| format!("write review outcome: {e}"))
}

pub fn aggregate_review_outcomes(claude_home: &Path) -> (f64, f64, usize) {
    let dir = review_outcome_dir(claude_home);
    let Ok(entries) = fs::read_dir(dir) else {
        return (1.0, 0.0, 0);
    };

    let mut count = 0usize;
    let mut total_precision = 0.0;
    let mut total_brier = 0.0;

    for entry in entries.flatten() {
        if entry.path().extension().and_then(|ext| ext.to_str()) == Some("json") {
            if let Ok(text) = fs::read_to_string(entry.path()) {
                if let Ok(outcome) = serde_json::from_str::<ReviewOutcome>(&text) {
                    total_precision += outcome.precision();
                    total_brier += outcome.brier_calibration_error();
                    count += 1;
                }
            }
        }
    }

    if count == 0 {
        (1.0, 0.0, 0)
    } else {
        (
            total_precision / count as f64,
            total_brier / count as f64,
            count,
        )
    }
}

// ============================================================================
// J11: Skill Composition as Typed Choice Decision
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SkillCompositionChoice {
    Single(String),
    Compose(Vec<String>),
    Generic,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillCompositionDecision {
    pub choice: SkillCompositionChoice,
    pub skills: Vec<String>,
    pub confidence: f64,
    pub reasoning: String,
}

pub fn evaluate_skill_composition(top_candidates: &[(String, f64)]) -> SkillCompositionDecision {
    if top_candidates.is_empty() {
        return SkillCompositionDecision {
            choice: SkillCompositionChoice::Generic,
            skills: Vec::new(),
            confidence: 0.8,
            reasoning: "No distinctive skill matched the prompt".to_string(),
        };
    }

    if top_candidates.len() == 1 {
        let (skill, score) = &top_candidates[0];
        return SkillCompositionDecision {
            choice: SkillCompositionChoice::Single(skill.clone()),
            skills: vec![skill.clone()],
            confidence: (*score).clamp(0.0, 1.0),
            reasoning: format!("Unambiguous match for single skill '{skill}'"),
        };
    }

    let (first_skill, first_score) = &top_candidates[0];
    let (second_skill, second_score) = &top_candidates[1];

    // Check for composition synergy: both scores strong (> 0.65) and distinct
    let are_distinct_domains = !is_same_skill_domain(first_skill, second_skill);
    if *first_score >= 0.65 && *second_score >= 0.60 && are_distinct_domains {
        let composed_confidence = ((first_score + second_score) / 2.0).clamp(0.0, 0.95);
        return SkillCompositionDecision {
            choice: SkillCompositionChoice::Compose(vec![first_skill.clone(), second_skill.clone()]),
            skills: vec![first_skill.clone(), second_skill.clone()],
            confidence: composed_confidence,
            reasoning: format!(
                "Prompt spans complementary domains: '{first_skill}' ({first_score:.2}) and '{second_skill}' ({second_score:.2})"
            ),
        };
    }

    // Default to winner
    SkillCompositionDecision {
        choice: SkillCompositionChoice::Single(first_skill.clone()),
        skills: vec![first_skill.clone()],
        confidence: (*first_score).clamp(0.0, 1.0),
        reasoning: format!("Primary skill '{first_skill}' won over runner-up '{second_skill}'"),
    }
}
fn skill_domain(name: &str) -> &str {
    name.split('-').next().unwrap_or(name)
}

fn is_same_skill_domain(a: &str, b: &str) -> bool {
    skill_domain(a) == skill_domain(b)
}
fn sanitize_key(value: &str) -> String {
    let mut key = String::new();
    let mut previous_dash = false;
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            key.push(ch.to_ascii_lowercase());
            previous_dash = false;
        } else if !previous_dash && !key.is_empty() {
            key.push('-');
            previous_dash = true;
        }
    }
    while key.ends_with('-') {
        key.pop();
    }
    if key.is_empty() {
        "default".to_string()
    } else {
        key
    }
}

fn current_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ============================================================================
// CLI and MCP Command Handlers
// ============================================================================

pub fn handle_decision_tool(arguments: &Value) -> Result<String, String> {
    let action = arguments
        .get("action")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            "decision: 'action' is required (score, noul, choice, calibrate, review-feedback)"
                .to_string()
        })?;

    match action {
        "score" => {
            let op = arguments.get("operation").and_then(Value::as_str).unwrap_or("review");
            if op == "plan" {
                let decision = parse_plan_score_feedback(arguments)?;
                serde_json::to_string_pretty(&decision)
                    .map_err(|e| format!("serialize plan decision: {e}"))
            } else {
                let decision = parse_review_score_feedback(arguments)?;
                serde_json::to_string_pretty(&decision)
                    .map_err(|e| format!("serialize review decision: {e}"))
            }
        }
        "noul" => {
            let cmd = arguments
                .get("command")
                .and_then(Value::as_str)
                .ok_or_else(|| "decision noul: 'command' argument required".to_string())?;
            let decision = evaluate_shell_command_noul(cmd);
            serde_json::to_string_pretty(&decision)
                .map_err(|e| format!("serialize noul decision: {e}"))
        }
        "choice" => {
            let mut candidates = Vec::new();
            if let Some(arr) = arguments.get("candidates").and_then(Value::as_array) {
                for item in arr {
                    if let Some(pair) = item.as_array() {
                        if pair.len() >= 2 {
                            if let (Some(name), Some(score)) = (pair[0].as_str(), pair[1].as_f64()) {
                                candidates.push((name.to_string(), score));
                            }
                        }
                    } else if let Some(obj) = item.as_object() {
                        if let (Some(name), Some(score)) = (
                            obj.get("name").and_then(Value::as_str),
                            obj.get("score").and_then(Value::as_f64),
                        ) {
                            candidates.push((name.to_string(), score));
                        }
                    }
                }
            }
            let decision = evaluate_skill_composition(&candidates);
            serde_json::to_string_pretty(&decision)
                .map_err(|e| format!("serialize choice decision: {e}"))
        }
        "calibrate" => {
            let skill = arguments
                .get("skill")
                .and_then(Value::as_str)
                .ok_or_else(|| "decision calibrate: 'skill' argument required".to_string())?;
            let confidence = arguments
                .get("confidence")
                .and_then(Value::as_f64)
                .unwrap_or(0.7);
            let home = crate::runtime::resolve_claude_home("")
                .map_err(|e| format!("resolve home: {e}"))?;

            let (calibrated, recorded) = if let Some(was_correct) = arguments.get("was_correct").and_then(Value::as_bool) {
                let cal = record_and_save_skill_calibration(&home, skill, confidence, was_correct)?;
                (cal, true)
            } else {
                let cal = get_calibrated_confidence(&home, skill, confidence);
                (cal, false)
            };

            let out = serde_json::json!({
                "skill": skill,
                "computed_confidence": confidence,
                "calibrated_confidence": calibrated,
                "was_recorded": recorded,
            });
            serde_json::to_string_pretty(&out)
                .map_err(|e| format!("serialize calibrate result: {e}"))
        }
        "review-feedback" => {
            let session_id = arguments
                .get("session_id")
                .and_then(Value::as_str)
                .unwrap_or("default");
            let flagged = arguments
                .get("issues_flagged")
                .and_then(Value::as_u64)
                .unwrap_or(0) as usize;
            let confirmed = arguments
                .get("issues_confirmed")
                .and_then(Value::as_u64)
                .unwrap_or(0) as usize;
            let false_pos = arguments
                .get("issues_false_positive")
                .and_then(Value::as_u64)
                .unwrap_or(0) as usize;
            let confidence = arguments
                .get("confidence")
                .and_then(Value::as_f64)
                .unwrap_or(0.75);

            let outcome = ReviewOutcome::new(session_id, flagged, confirmed, false_pos, confidence);
            let home = crate::runtime::resolve_claude_home("")
                .map_err(|e| format!("resolve home: {e}"))?;
            save_review_outcome(&home, &outcome)?;
            let (avg_p, avg_b, total_sessions) = aggregate_review_outcomes(&home);

            let out = serde_json::json!({
                "session_id": session_id,
                "precision": outcome.precision(),
                "brier_error": outcome.brier_calibration_error(),
                "aggregate_precision": avg_p,
                "aggregate_brier_error": avg_b,
                "total_sessions_tracked": total_sessions,
            });
            serde_json::to_string_pretty(&out)
                .map_err(|e| format!("serialize review feedback result: {e}"))
        }
        unknown => Err(format!("Unknown decision action: '{unknown}'. Supported: score, noul, choice, calibrate, review-feedback")),
    }
}

pub fn run_decision_command(
    arguments: &[String],
    standard_output: &mut dyn std::io::Write,
    standard_error: &mut dyn std::io::Write,
) -> u8 {
    if arguments.is_empty() || arguments[0] == "--help" || arguments[0] == "-h" {
        let _ = writeln!(
            standard_output,
            "Usage: keel decision <action> [options]\n\n\
             Jev-inspired typed decision operations:\n  \
               score            Score review findings or plan readiness against rubrics\n  \
               noul             Evaluate shell command danger probability and risk action\n  \
               choice           Evaluate skill composition choice for multi-domain prompt\n  \
               calibrate        Get or update calibrated confidence for skill routing\n  \
               review-feedback  Record review accuracy outcome and inspect calibration error"
        );
        return 0;
    }

    let action = arguments[0].as_str();
    let mut flag_set = crate::args::FlagSet::new(action);
    match action {
        "score" => {
            flag_set.string_flag("operation", "review");
            flag_set.string_flag("scores", "{}");
            flag_set.string_flag("confidence", "0.7");
            flag_set.string_flag("summary", "");
            flag_set.bool_flag("plan", false);
        }
        "noul" => {
            flag_set.string_flag("command", "");
        }
        "choice" => {
            flag_set.string_flag("candidates", "");
        }
        "calibrate" => {
            flag_set.string_flag("skill", "");
            flag_set.string_flag("confidence", "0.7");
            flag_set.bool_flag("was-correct", false);
        }
        "review-feedback" => {
            flag_set.string_flag("session", "default");
            flag_set.string_flag("flagged", "0");
            flag_set.string_flag("confirmed", "0");
            flag_set.string_flag("false-positive", "0");
            flag_set.string_flag("confidence", "0.75");
        }
        other => {
            let _ = writeln!(
                standard_error,
                "Unknown decision action: '{other}'. Use --help for available actions."
            );
            return 2;
        }
    }

    if let Err(e) = flag_set.parse(&arguments[1..]) {
        let _ = writeln!(standard_error, "error: {}", e.message);
        return 1;
    }
    let json_arg = match action {
        "score" => {
            let op = if flag_set.bool_value("plan") || flag_set.string_value("operation") == "plan"
            {
                "plan"
            } else {
                "review"
            };
            let scores_str = flag_set.string_value("scores");
            let scores: Value =
                serde_json::from_str(scores_str).unwrap_or_else(|_| serde_json::json!({}));
            let confidence = flag_set
                .string_value("confidence")
                .parse::<f64>()
                .unwrap_or(0.7);
            serde_json::json!({
                "action": "score",
                "operation": op,
                "scores": scores,
                "confidence": confidence,
                "summary": flag_set.string_value("summary"),
            })
        }
        "noul" => {
            let cmd = flag_set.string_value("command");
            serde_json::json!({
                "action": "noul",
                "command": cmd,
            })
        }
        "choice" => {
            let cand_str = flag_set.string_value("candidates");
            let mut list = Vec::new();
            for item in cand_str.split(',') {
                let parts: Vec<&str> = item.split(':').collect();
                if parts.len() == 2 {
                    if let Ok(s) = parts[1].trim().parse::<f64>() {
                        list.push(serde_json::json!([parts[0].trim(), s]));
                    }
                }
            }
            serde_json::json!({
                "action": "choice",
                "candidates": list,
            })
        }
        "calibrate" => {
            let skill = flag_set.string_value("skill");
            let conf = flag_set
                .string_value("confidence")
                .parse::<f64>()
                .unwrap_or(0.7);
            let mut payload = serde_json::json!({
                "action": "calibrate",
                "skill": skill,
                "confidence": conf,
            });
            if flag_set.bool_value("was-correct") {
                payload["was_correct"] = serde_json::json!(true);
            }
            payload
        }
        "review-feedback" => {
            let session = flag_set.string_value("session");
            let flagged = flag_set.string_value("flagged").parse::<u64>().unwrap_or(0);
            let confirmed = flag_set
                .string_value("confirmed")
                .parse::<u64>()
                .unwrap_or(0);
            let fp = flag_set
                .string_value("false-positive")
                .parse::<u64>()
                .unwrap_or(0);
            let conf = flag_set
                .string_value("confidence")
                .parse::<f64>()
                .unwrap_or(0.75);
            serde_json::json!({
                "action": "review-feedback",
                "session_id": session,
                "issues_flagged": flagged,
                "issues_confirmed": confirmed,
                "issues_false_positive": fp,
                "confidence": conf,
            })
        }
        other => {
            let _ = writeln!(
                standard_error,
                "Unknown decision action: '{other}'. Use --help for available actions."
            );
            return 2;
        }
    };

    match handle_decision_tool(&json_arg) {
        Ok(output) => {
            let _ = writeln!(standard_output, "{output}");
            0
        }
        Err(err) => {
            let _ = writeln!(standard_error, "error: {err}");
            1
        }
    }
}

// ============================================================================
// Unit Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confidence_binning_correctness() {
        assert_eq!(confidence_to_bin(0.0), 0);
        assert_eq!(confidence_to_bin(0.05), 0);
        assert_eq!(confidence_to_bin(0.15), 1);
        assert_eq!(confidence_to_bin(0.85), 8);
        assert_eq!(confidence_to_bin(0.95), 9);
        assert_eq!(confidence_to_bin(1.0), 9);
    }

    #[test]
    fn confidence_calibration_record_and_calibrate() {
        let mut record = SkillCalibrationRecord::new("reviewer");
        assert_eq!(record.calibrated_confidence(0.85), 0.85);

        // Record 5 outcomes in the 0.8-0.9 bin (bin 8): 4 correct, 1 incorrect (80% empirical)
        for _ in 0..4 {
            record.record(0.85, true);
        }
        record.record(0.85, false);

        let calibrated = record.calibrated_confidence(0.85);
        // Should blend empirical accuracy with computed confidence
        assert!((0.70..=0.85).contains(&calibrated), "got {calibrated}");
    }

    #[test]
    fn review_score_evaluation_pass() {
        let mut scores = BTreeMap::new();
        scores.insert("correctness".to_string(), 0.9);
        scores.insert("security".to_string(), 0.95);
        scores.insert("readability".to_string(), 0.8);
        scores.insert("test_coverage".to_string(), 0.85);
        scores.insert("error_handling".to_string(), 0.85);

        let rubric = default_review_rubric();
        let decision = evaluate_review_scores(
            &scores,
            0.85,
            &rubric,
            vec![],
            "Clean implementation".to_string(),
        );

        assert_eq!(decision.verdict, ReviewScoreVerdict::Pass);
        assert!(decision.mean_score >= 0.85);
        assert!(decision.is_pass());
    }

    #[test]
    fn review_score_evaluation_escalate() {
        let mut scores = BTreeMap::new();
        scores.insert("correctness".to_string(), 0.5);
        scores.insert("security".to_string(), 0.5);
        scores.insert("readability".to_string(), 0.5);
        scores.insert("test_coverage".to_string(), 0.5);
        scores.insert("error_handling".to_string(), 0.5);

        let rubric = default_review_rubric();
        let decision = evaluate_review_scores(
            &scores,
            0.50, // low confidence
            &rubric,
            vec!["low_confidence_finding".to_string()],
            "Unclear issues".to_string(),
        );

        assert_eq!(decision.verdict, ReviewScoreVerdict::Escalate);
        let gate_dec = decision.to_gate_decision();
        assert!(gate_dec.needs_escalation());
    }

    #[test]
    fn plan_score_evaluation_ready() {
        let mut scores = BTreeMap::new();
        for k in PLAN_CRITERIA {
            scores.insert((*k).to_string(), 0.9);
        }
        let decision = evaluate_plan_scores(&scores, 0.85, vec![], vec![]);
        assert_eq!(decision.verdict, PlanScoreVerdict::Ready);
        assert!(decision.is_ready());
    }

    #[test]
    fn shell_noul_evaluates_catastrophic_commands() {
        let decision = evaluate_shell_command_noul("rm -rf /");
        assert_eq!(decision.category, ShellRiskCategory::Destructive);
        assert_eq!(decision.action, ShellRiskAction::Block);
        assert!(decision.probability >= 0.90);
    }

    #[test]
    fn shell_noul_evaluates_safe_commands() {
        let decision = evaluate_shell_command_noul("cargo test --workspace");
        assert_eq!(decision.category, ShellRiskCategory::Safe);
        assert_eq!(decision.action, ShellRiskAction::Allow);
        assert!(decision.probability <= 0.10);
    }

    #[test]
    fn review_outcome_precision_and_brier() {
        let outcome = ReviewOutcome::new("sess-1", 10, 8, 2, 0.85);
        assert_eq!(outcome.precision(), 0.8);
        assert!((outcome.brier_calibration_error() - 0.0025).abs() < 1e-6);
    }

    #[test]
    fn skill_composition_choice_evaluation() {
        let candidates = vec![
            ("reviewer".to_string(), 0.85),
            ("qa-and-automation-engineer".to_string(), 0.75),
        ];
        let decision = evaluate_skill_composition(&candidates);
        match decision.choice {
            SkillCompositionChoice::Compose(skills) => {
                assert_eq!(skills.len(), 2);
                assert_eq!(skills[0], "reviewer");
                assert_eq!(skills[1], "qa-and-automation-engineer");
            }
            _ => panic!("expected Compose choice, got {:?}", decision.choice),
        }
    }

    #[test]
    fn decision_cli_subcommands_dispatch() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        assert_eq!(
            run_decision_command(&["--help".to_string()], &mut stdout, &mut stderr),
            0
        );
        let help_out = String::from_utf8_lossy(&stdout);
        assert!(help_out.contains("Usage: keel decision"));
        stdout.clear();
        stderr.clear();
        assert_eq!(
            run_decision_command(
                &[
                    "noul".to_string(),
                    "--command".to_string(),
                    "echo hi".to_string()
                ],
                &mut stdout,
                &mut stderr
            ),
            0
        );
        let noul_out = String::from_utf8_lossy(&stdout);
        assert!(noul_out.contains("Safe"));
    }
}
