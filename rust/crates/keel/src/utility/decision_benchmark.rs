//! Head-to-head decision benchmark over keel's own curated routing vocabulary.
//!
//! The local column routes the curated prompts in-process. The remote column is
//! an opt-in shell-out to a zero-shot HTTP classifier, so CI stays offline.

use std::process::Command;

use serde::Serialize;
use serde_json::json;

use crate::utility::decision_model::expected_calibration_error;
use crate::utility::skill_match::{curated_skill_cases, curated_skill_for_prompt};

/// Public zero-shot classifier. Free, no key, POST with text plus labels.
const REMOTE_ENDPOINT: &str = "https://classifier.dev";
/// Label the remote model picks when no supplied label fits (its documented
/// way to model a none-of-the-above outcome).
const REMOTE_NONE_LABEL: &str = "none of these";
const REMOTE_TIMEOUT_SECS: &str = "20";

/// Prompts that must stay unrouted. The test asserts every one is silent, so a
/// prompt that starts tripping the curated tier fails here, not on the network.
const CONTROL_PROMPTS: &[&str] = &[
    "what is 2 + 2",
    "hi",
    "explain this codebase to me",
    "rename this variable to total",
    "summarize the changelog since the last release",
];

/// Where the scored prompts came from, so a self-referential run can never be
/// quoted as a real-world one.
pub const SOURCE_FIXTURES: &str = "keel curated vocabulary (self-referential)";

#[derive(Debug, Clone, Serialize)]
pub struct BenchmarkRow {
    pub prompt: String,
    pub expected: Option<String>,
    pub local: Option<String>,
    pub local_confidence: Option<f64>,
    pub remote: Option<String>,
    pub remote_confidence: Option<f64>,
    pub remote_ms: Option<u64>,
    pub local_correct: bool,
    pub remote_correct: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BenchmarkReport {
    pub cases: usize,
    pub controls: usize,
    pub local_correct: usize,
    pub remote_available: bool,
    pub remote_correct: usize,
    pub remote_model: Option<String>,
    pub remote_p50_ms: Option<u64>,
    pub remote_brier: Option<f64>,
    pub remote_ece: Option<f64>,
    /// Scored over the rows that carried a calibrated local confidence, which is
    /// why the count travels with the numbers instead of being implied by cases.
    pub local_brier: Option<f64>,
    pub local_ece: Option<f64>,
    pub local_confidence_rows: usize,
    /// Where the prompts came from. Keel's own fixtures and a fetched public
    /// corpus must never read as the same result.
    pub source: String,
    pub rows: Vec<BenchmarkRow>,
}

/// Case shape: the prompt, and the skill it must route to (`None` = stay silent).
pub fn build_cases() -> Vec<(String, Option<String>)> {
    let mut cases: Vec<(String, Option<String>)> = curated_skill_cases()
        .into_iter()
        .map(|(prompt, skill)| (prompt, Some(skill)))
        .collect();
    cases.extend(
        CONTROL_PROMPTS
            .iter()
            .map(|prompt| ((*prompt).to_string(), None)),
    );
    cases
}

pub fn run(home: Option<&std::path::Path>, remote: bool) -> BenchmarkReport {
    let cases = build_cases();
    let prompts: Vec<String> = cases.iter().map(|(prompt, _)| prompt.clone()).collect();
    let remote_batch = if remote {
        run_remote(&prompts, REMOTE_ENDPOINT)
    } else {
        None
    };
    let remote_items = remote_batch.as_ref().map(|batch| batch.items.as_slice());

    let mut rows = Vec::with_capacity(cases.len());
    let mut local_correct = 0;
    let mut remote_correct = 0;
    for (index, (prompt, expected)) in cases.iter().enumerate() {
        let local = curated_skill_for_prompt(prompt).map(str::to_string);
        let local_confidence = local
            .as_deref()
            .and_then(|skill| router_confidence(home, prompt, skill));
        let local_correct_case = local == *expected;
        if local_correct_case {
            local_correct += 1;
        }
        let item = remote_items.and_then(|items| items.get(index));
        let remote = item.map(|(label, _, _)| label.clone());
        let remote_correct_case = match (item, expected) {
            (Some((label, _, _)), Some(skill)) => Some(label == skill),
            (Some((label, _, _)), None) => Some(label == REMOTE_NONE_LABEL),
            (None, _) => None,
        };
        if remote_correct_case == Some(true) {
            remote_correct += 1;
        }
        rows.push(BenchmarkRow {
            prompt: prompt.clone(),
            expected: expected.clone(),
            local,
            local_confidence,
            remote,
            remote_confidence: item.and_then(|(_, confidence, _)| *confidence),
            remote_ms: item.map(|(_, _, ms)| *ms),
            local_correct: local_correct_case,
            remote_correct: remote_correct_case,
        });
    }

    // Both columns are scored the same way: confidence versus correctness, so
    // keel's own calibration stops reading as n/a next to the remote service.
    let remote_pairs = scored_pairs(
        &rows
            .iter()
            .map(|row| (row.remote_confidence, row.remote_correct))
            .collect::<Vec<_>>(),
    );
    let local_pairs = scored_pairs(
        &rows
            .iter()
            .map(|row| (row.local_confidence, Some(row.local_correct)))
            .collect::<Vec<_>>(),
    );
    let remote_brier = brier_score(&remote_pairs);
    let remote_ece = ece_score(&remote_pairs);
    let local_brier = brier_score(&local_pairs);
    let local_ece = ece_score(&local_pairs);

    BenchmarkReport {
        cases: cases.len(),
        controls: CONTROL_PROMPTS.len(),
        local_correct,
        remote_available: remote_batch.is_some(),
        remote_correct,
        remote_model: remote_batch.and_then(|batch| batch.model),
        remote_p50_ms: percentile_ms(&rows, 50),
        remote_brier,
        remote_ece,
        local_brier,
        local_ece,
        local_confidence_rows: local_pairs.len(),
        source: SOURCE_FIXTURES.to_string(),
        rows,
    }
}

/// Calibrated confidence the live router reports for `skill` on this prompt, or
/// `None` when it is silent or names a different skill: a confidence is never
/// attached to a decision the router did not make.
fn router_confidence(home: Option<&std::path::Path>, prompt: &str, skill: &str) -> Option<f64> {
    let decision = crate::utility::skill_match::match_skill_for_prompt_with_details(home?, prompt)?;
    (decision.name == skill).then_some(decision.confidence)
}

fn scored_pairs(pairs: &[(Option<f64>, Option<bool>)]) -> Vec<(f64, bool)> {
    pairs
        .iter()
        .filter_map(|(confidence, correct)| Some(((*confidence)?, (*correct)?)))
        .collect()
}

fn brier_score(pairs: &[(f64, bool)]) -> Option<f64> {
    if pairs.is_empty() {
        return None;
    }
    let sum: f64 = pairs
        .iter()
        .map(|(confidence, correct)| (confidence - if *correct { 1.0 } else { 0.0 }).powi(2))
        .sum();
    Some(sum / pairs.len() as f64)
}

fn ece_score(pairs: &[(f64, bool)]) -> Option<f64> {
    if pairs.is_empty() {
        return None;
    }
    Some(expected_calibration_error(pairs))
}

struct RemoteBatch {
    model: Option<String>,
    items: Vec<(String, Option<f64>, u64)>,
}

/// One request carries every case, so the label set is the deduped skill list
/// plus the none-of-the-above option the service documents for that case.
fn remote_labels() -> Vec<String> {
    let labels: Vec<String> = curated_skill_cases()
        .into_iter()
        .map(|(_, skill)| skill)
        .chain(std::iter::once(REMOTE_NONE_LABEL.to_string()))
        .collect();
    unique_labels(&labels)
}

fn run_remote(prompts: &[String], endpoint: &str) -> Option<RemoteBatch> {
    let body = json!({ "inputs": prompts, "labels": remote_labels() }).to_string();
    let output = Command::new("curl")
        .args([
            "-s",
            "--max-time",
            REMOTE_TIMEOUT_SECS,
            "-H",
            "content-type: application/json",
            "-d",
            &body,
            endpoint,
        ])
        // why: an unavailable curl reads as "no remote column", never an error.
        .output()
        .ok()?;
    // why: a non-JSON body is an outage page or a rate limit, not a parse bug.
    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    parse_remote_response(&parsed)
}

fn unique_labels(labels: &[String]) -> Vec<String> {
    let mut unique: Vec<String> = Vec::new();
    for label in labels {
        if !unique.contains(label) {
            unique.push(label.clone());
        }
    }
    unique
}

/// Wire keys of the remote response, shared by the parser and its tests.
const REMOTE_KEY_RESULTS: &str = "results";
const REMOTE_KEY_LABEL: &str = "label";
const REMOTE_KEY_CONFIDENCE: &str = "confidence";
const REMOTE_KEY_MS: &str = "ms";

fn parse_remote_response(value: &serde_json::Value) -> Option<RemoteBatch> {
    let results = value.get(REMOTE_KEY_RESULTS)?.as_array()?;
    let mut items = Vec::with_capacity(results.len());
    for result in results {
        let label = result.get(REMOTE_KEY_LABEL)?.as_str()?.to_string();
        let confidence = result
            .get(REMOTE_KEY_CONFIDENCE)
            .and_then(serde_json::Value::as_f64);
        let ms = result
            .get(REMOTE_KEY_MS)
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        items.push((label, confidence, ms));
    }
    if items.is_empty() {
        return None;
    }
    let model = value
        .get("model")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    Some(RemoteBatch { model, items })
}

fn percentile_ms(rows: &[BenchmarkRow], percentile: usize) -> Option<u64> {
    let mut values: Vec<u64> = rows.iter().filter_map(|row| row.remote_ms).collect();
    if values.is_empty() {
        return None;
    }
    values.sort_unstable();
    let index = (values.len() * percentile) / 100;
    values.get(index.min(values.len() - 1)).copied()
}

const PROMPT_WIDTH: usize = 38;
const CELL_WIDTH: usize = 34;

pub fn render(report: &BenchmarkReport) -> String {
    let mut out = String::new();
    out.push_str(&format!("KEEL DECISION BENCHMARK   {}\n", report.source));
    out.push_str(&format!(
        "cases: {} ({} curated prompts, {} controls)\n",
        report.cases,
        report.cases.saturating_sub(report.controls),
        report.controls
    ));
    out.push_str(&format!(
        "keel local       correct {}/{}\n",
        report.local_correct, report.cases
    ));
    if report.remote_available {
        out.push_str(&format!(
            "classifier.dev   correct {}/{}   p50 {} ms   model {}\n",
            report.remote_correct,
            report.cases,
            report.remote_p50_ms.unwrap_or(0),
            report.remote_model.as_deref().unwrap_or("unknown")
        ));
    } else {
        out.push_str("classifier.dev   not run (pass --remote to spend one live call)\n");
    }
    if let (Some(brier), Some(ece)) = (report.local_brier, report.local_ece) {
        out.push_str(&format!(
            "keel local       confidence vs correctness: brier {brier:.4}   ece {ece:.4}   over {} of {} rows\n",
            report.local_confidence_rows, report.cases
        ));
    }
    if let (Some(brier), Some(ece)) = (report.remote_brier, report.remote_ece) {
        out.push_str(&format!(
            "classifier.dev   confidence vs correctness: brier {brier:.4}   ece {ece:.4}\n"
        ));
    }
    out.push('\n');
    out.push_str(&format!(
        "{:<PROMPT_WIDTH$}  {:<CELL_WIDTH$}  {:<CELL_WIDTH$}  {}\n",
        "prompt", "expected", "keel local", "classifier.dev"
    ));
    for row in &report.rows {
        let expected = row.expected.as_deref().unwrap_or("(stay silent)");
        let local = match (&row.local, row.local_correct) {
            (Some(skill), true) => format!("{skill} ok"),
            (Some(skill), false) => format!("{skill} WRONG"),
            (None, true) => "silent ok".to_string(),
            (None, false) => "silent WRONG".to_string(),
        };
        let remote = match (&row.remote, row.remote_correct) {
            (Some(label), Some(true)) => match row.remote_confidence {
                Some(confidence) => format!("{label} {confidence:.2}"),
                None => label.clone(),
            },
            (Some(label), Some(false)) => format!("{label} WRONG"),
            (Some(label), None) => label.clone(),
            (None, _) => "-".to_string(),
        };
        out.push_str(&format!(
            "{:<PROMPT_WIDTH$}  {:<CELL_WIDTH$}  {:<CELL_WIDTH$}  {}\n",
            truncate(&row.prompt, PROMPT_WIDTH),
            truncate(expected, CELL_WIDTH),
            truncate(&local, CELL_WIDTH),
            remote
        ));
    }
    out
}

pub fn to_json(report: &BenchmarkReport) -> serde_json::Value {
    let rows: Vec<serde_json::Value> = report
        .rows
        .iter()
        .map(|row| {
            json!({
                "prompt": row.prompt,
                "expected": row.expected,
                "local": row.local,
                "local_confidence": row.local_confidence,
                "remote": row.remote,
                "remote_confidence": row.remote_confidence,
                "remote_ms": row.remote_ms,
                "local_correct": row.local_correct,
                "remote_correct": row.remote_correct,
            })
        })
        .collect();
    json!({
        "cases": report.cases,
        "controls": report.controls,
        "local_correct": report.local_correct,
        "local_brier": report.local_brier,
        "local_ece": report.local_ece,
        "local_confidence_rows": report.local_confidence_rows,
        "source": report.source,
        "remote_available": report.remote_available,
        "remote_correct": report.remote_correct,
        "remote_model": report.remote_model,
        "remote_p50_ms": report.remote_p50_ms,
        "remote_brier": report.remote_brier,
        "remote_ece": report.remote_ece,
        "endpoint": REMOTE_ENDPOINT,
        "rows": rows,
    })
}

fn truncate(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    let mut cut: String = text.chars().take(width.saturating_sub(1)).collect();
    cut.push('~');
    cut
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curated_cases_route_locally() {
        let report = run(None, false);
        assert_eq!(report.cases, build_cases().len());
        assert_eq!(report.local_correct, report.cases);
        assert!(!report.remote_available);
        assert!(report.remote_brier.is_none());
        assert!(report.remote_ece.is_none());
    }

    #[test]
    fn control_prompts_stay_silent() {
        for prompt in CONTROL_PROMPTS {
            assert_eq!(curated_skill_for_prompt(prompt), None, "{prompt}");
        }
    }

    #[test]
    fn local_miss_is_scored_wrong() {
        let expected = build_cases()
            .into_iter()
            .find_map(|(_, skill)| skill)
            .expect("curated cases carry an expected skill");
        let local = curated_skill_for_prompt("what is 2 + 2");
        assert_ne!(local.map(str::to_string), Some(expected));
    }

    #[test]
    fn remote_response_parses_and_rejects_garbage() {
        let good = json!({
            "tier": "fast",
            "model": "jev-1.13.0",
            (REMOTE_KEY_RESULTS): [
                {(REMOTE_KEY_LABEL): "reviewer", (REMOTE_KEY_CONFIDENCE): 0.98, (REMOTE_KEY_MS): 260},
                {(REMOTE_KEY_LABEL): REMOTE_NONE_LABEL, (REMOTE_KEY_CONFIDENCE): 0.71, (REMOTE_KEY_MS): 240}
            ]
        });
        let batch = parse_remote_response(&good).expect("valid response");
        assert_eq!(batch.model.as_deref(), Some("jev-1.13.0"));
        assert_eq!(batch.items.len(), 2);
        assert_eq!(batch.items[0].0, "reviewer");
        assert_eq!(batch.items[0].2, 260);
        assert!(parse_remote_response(&json!({ (REMOTE_KEY_RESULTS): [] })).is_none());
        assert!(parse_remote_response(&json!({"error": "rate limited"})).is_none());
    }

    #[test]
    fn labels_are_unique_and_cover_controls() {
        let labels = remote_labels();
        let mut sorted = labels.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), labels.len());
        assert!(labels.contains(&REMOTE_NONE_LABEL.to_string()));
        assert!(labels.len() <= 100);
        let duplicated = vec!["a".to_string(), "a".to_string(), "b".to_string()];
        assert_eq!(unique_labels(&duplicated).len(), 2);
    }

    #[test]
    fn render_names_both_columns_without_remote() {
        let report = run(None, false);
        let text = render(&report);
        assert!(text.contains("keel local"));
        assert!(text.contains("classifier.dev"));
        assert!(text.contains("not run"));
        assert!(to_json(&report)["rows"]
            .as_array()
            .is_some_and(|r| !r.is_empty()));
    }
}
