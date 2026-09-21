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
    /// Rows where the local column made a decision at all. Silence is not a
    /// wrong answer, and reporting them together would hide which one happened.
    pub local_decided: usize,
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
    score_cases(
        home,
        &build_cases(),
        CONTROL_PROMPTS.len(),
        SOURCE_FIXTURES,
        remote,
    )
}

/// Score a corpus fetched from outside keel through the same path as the
/// fixtures, so the two numbers stay comparable and the source label keeps them
/// from being quoted as one result.
pub fn run_external(
    home: Option<&std::path::Path>,
    cases: &[(String, Option<String>)],
    remote: bool,
) -> BenchmarkReport {
    score_cases(home, cases, 0, SOURCE_EXTERNAL, remote)
}

fn score_cases(
    home: Option<&std::path::Path>,
    cases: &[(String, Option<String>)],
    controls: usize,
    source: &str,
    remote: bool,
) -> BenchmarkReport {
    let prompts: Vec<String> = cases.iter().map(|(prompt, _)| prompt.clone()).collect();
    let remote_batch = if remote {
        run_remote(&prompts, &expected_labels(cases), REMOTE_ENDPOINT)
    } else {
        None
    };
    let remote_items = remote_batch.as_ref().map(|batch| batch.items.as_slice());

    let mut rows = Vec::with_capacity(cases.len());
    let mut local_correct = 0;
    let mut remote_correct = 0;
    for (index, (prompt, expected)) in cases.iter().enumerate() {
        // With a home, the local column IS the live router, so silence is scored
        // as silence and an improvement in routing shows up in the number.
        let (local, local_confidence) = match home {
            Some(home) => {
                match crate::utility::skill_match::match_skill_for_prompt_with_details(home, prompt)
                {
                    Some(decision) => (Some(decision.name), Some(decision.confidence)),
                    None => (None, None),
                }
            }
            None => (curated_skill_for_prompt(prompt).map(str::to_string), None),
        };
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

    let local_decided = rows.iter().filter(|row| row.local.is_some()).count();

    BenchmarkReport {
        cases: cases.len(),
        controls,
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
        local_decided,
        source: source.to_string(),
        rows,
    }
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

/// Where a fetched corpus came from, kept distinct from the fixtures so a
/// self-referential run can never be quoted as a real-world result.
pub const SOURCE_EXTERNAL: &str = "stackoverflow tags (external ground truth)";

const EXTERNAL_ENDPOINT: &str = "https://api.stackexchange.com/2.3/questions";
const EXTERNAL_TIMEOUT_SECS: &str = "20";

/// Community tags mapped to the keel skill that owns that work. The mapping is
/// mechanical: the tag names the domain and the skill owns the domain, so the
/// labels are the community's, not keel's own vocabulary.
pub const EXTERNAL_TAGS: &[(&str, &str)] = &[
    ("rust", "rust"),
    ("unit-testing", "test-driven-development"),
    ("debugging", "systematic-debugging"),
    ("security", "adversarial-security-review"),
    ("postgresql", "postgres-migration-safety"),
    ("websocket", "websocket-realtime-design"),
    (
        "internationalization",
        "internationalization-and-localization",
    ),
    ("kubernetes", "cloud-and-devops-expert"),
];

/// Ops-domain corpus: the same API on another site, so a result here is not the
/// developer corpus wearing new labels.
const OPS_SKILL: &str = "cloud-and-devops-expert";

pub const OPS_TAGS: &[(&str, &str)] = &[
    ("nginx", OPS_SKILL),
    ("docker", OPS_SKILL),
    ("kubernetes", OPS_SKILL),
    ("postgresql", "postgres-migration-safety"),
    ("security", "adversarial-security-review"),
    ("monitoring", "observability-and-incident-response"),
];

/// Design-domain corpus for the third benchmark.
pub const DESIGN_TAGS: &[(&str, &str)] = &[
    ("architecture", "backend-and-data-architecture"),
    ("api", "api-contract-design"),
    ("testing", "test-driven-development"),
    ("security", "adversarial-security-review"),
    ("dependencies", "dependency-and-supply-chain"),
];

/// Desktop power-user corpus: OS-layer troubleshooting and infra.
pub const DESKTOP_TAGS: &[(&str, &str)] = &[
    ("security", "adversarial-security-review"),
    ("docker", OPS_SKILL),
    ("nginx", OPS_SKILL),
    ("postgresql", "postgres-migration-safety"),
    ("troubleshooting", "systematic-debugging"),
    ("monitoring", "observability-and-incident-response"),
];

/// Ubuntu desktop-and-server corpus.
pub const UBUNTU_TAGS: &[(&str, &str)] = &[
    ("docker", OPS_SKILL),
    ("nginx", OPS_SKILL),
    ("postgresql", "postgres-migration-safety"),
    ("security", "adversarial-security-review"),
    ("server", OPS_SKILL),
    ("monitoring", "observability-and-incident-response"),
];

/// Every corpus a model trains on, so no skill is left with zero evidence and
/// therefore unpredictable no matter how confident the gate is.
pub const EXTERNAL_SITES: &[&str] = &[
    "stackoverflow",
    "serverfault",
    "softwareengineering",
    "superuser",
    "askubuntu",
];

/// Site plus tag map for one corpus. An unknown name falls back to the developer
/// corpus rather than inventing a fourth.
pub fn corpus_for(site: &str) -> (&'static str, &'static [(&'static str, &'static str)]) {
    match site {
        "serverfault" => ("serverfault", OPS_TAGS),
        "softwareengineering" => ("softwareengineering", DESIGN_TAGS),
        "superuser" => ("superuser", DESKTOP_TAGS),
        "askubuntu" => ("askubuntu", UBUNTU_TAGS),
        _ => ("stackoverflow", EXTERNAL_TAGS),
    }
}

/// Fetch recent question titles per tag from the public Stack Exchange API. The
/// keyless quota is 300 requests a day and each tag costs one request per page,
/// so the caller caches the corpus instead of refetching it. Pages let training
/// rows stay disjoint from the evaluation rows they are scored against.
pub fn fetch_external_site(
    site_hint: &str,
    per_tag: usize,
    first_page: usize,
    pages: usize,
) -> Result<Vec<(String, Option<String>)>, String> {
    let (site, tags) = corpus_for(site_hint);
    let mut cases = Vec::new();
    for page in first_page..first_page + pages {
        for (tag, skill) in tags {
            let url = format!(
                "{EXTERNAL_ENDPOINT}?order=desc&sort=activity&site={site}&pagesize={per_tag}&page={page}&tagged={tag}"
            );
            let output = Command::new("curl")
                .args([
                    "-s",
                    "--compressed",
                    "--max-time",
                    EXTERNAL_TIMEOUT_SECS,
                    &url,
                ])
                .output()
                .map_err(|error| format!("curl failed for {tag}: {error}"))?;
            let parsed: serde_json::Value = serde_json::from_slice(&output.stdout)
                .map_err(|error| format!("stackexchange {tag}: unreadable response: {error}"))?;
            if let Some(message) = parsed
                .get("error_message")
                .and_then(serde_json::Value::as_str)
            {
                return Err(format!("stackexchange {tag}: {message}"));
            }
            // The API requires a caller to wait when it sets backoff, and a spent
            // quota would fail every later tag too.
            if let Some(seconds) = parsed.get("backoff").and_then(serde_json::Value::as_u64) {
                std::thread::sleep(std::time::Duration::from_secs(seconds));
            }
            if parsed
                .get("quota_remaining")
                .and_then(serde_json::Value::as_u64)
                == Some(0)
            {
                return Err(format!("stackexchange quota spent while reading {tag}"));
            }
            let items = parsed
                .get("items")
                .and_then(serde_json::Value::as_array)
                .ok_or_else(|| format!("stackexchange {tag}: no items array"))?;
            for item in items {
                let title = item
                    .get("title")
                    .and_then(serde_json::Value::as_str)
                    .map(str::trim)
                    .unwrap_or_default();
                if !title.is_empty() {
                    cases.push((title.to_string(), Some((*skill).to_string())));
                }
            }
        }
    }
    if cases.is_empty() {
        return Err("stackexchange returned no question titles".to_string());
    }
    Ok(cases)
}

const CRATES_ENDPOINT: &str = "https://crates.io/api/v1/crates";
const CRATES_USER_AGENT: &str = "keel-benchmark/0.1 (evaluation harness)";

/// crates.io categories are a controlled vocabulary as well, on a second provider
/// with a different noise profile: one-line crate descriptions instead of
/// question titles, so a result here is not the same corpus twice.
pub const CRATE_CATEGORIES: &[(&str, &str)] = &[
    ("database", "backend-and-data-architecture"),
    ("web-programming::websocket", "websocket-realtime-design"),
    (
        "internationalization",
        "internationalization-and-localization",
    ),
    ("cryptography", "adversarial-security-review"),
    ("development-tools::testing", "test-driven-development"),
    ("development-tools::debugging", "systematic-debugging"),
    ("api-bindings", "api-contract-design"),
    (
        "development-tools::cargo-plugins",
        "dependency-and-supply-chain",
    ),
];

/// Fetch crate descriptions per category. crates.io asks callers to identify
/// themselves, so the request carries a user agent.
pub fn fetch_crates_categories(
    per_category: usize,
    page: usize,
) -> Result<Vec<(String, Option<String>)>, String> {
    let mut rows = Vec::new();
    for (category, skill) in CRATE_CATEGORIES {
        let url =
            format!("{CRATES_ENDPOINT}?category={category}&per_page={per_category}&page={page}");
        let output = Command::new("curl")
            .args([
                "-s",
                "-A",
                CRATES_USER_AGENT,
                "--max-time",
                EXTERNAL_TIMEOUT_SECS,
                &url,
            ])
            .output()
            .map_err(|error| format!("crates {category}: {error}"))?;
        if !output.status.success() {
            return Err(format!("crates {category}: curl failed"));
        }
        let body = String::from_utf8_lossy(&output.stdout);
        let parsed: serde_json::Value =
            serde_json::from_str(&body).map_err(|error| format!("crates {category}: {error}"))?;
        for entry in parsed["crates"].as_array().into_iter().flatten() {
            if let Some(description) = entry["description"].as_str() {
                rows.push((description.to_string(), Some((*skill).to_string())));
            }
        }
    }
    Ok(rows)
}

/// Cache for the crates corpus, kept beside the Stack Exchange one.
pub fn crates_cache_path(claude_home: &std::path::Path) -> std::path::PathBuf {
    external_cache_path(claude_home).with_file_name("crates-corpus.json")
}

/// Every corpus trains. One provider failing must not stop training: the rows
/// that did arrive are kept and only an empty result is an error.
pub fn fetch_all_corpora(
    per_tag: usize,
    pages: usize,
) -> Result<Vec<(String, Option<String>)>, String> {
    let mut rows = Vec::new();
    let mut failures: Vec<String> = Vec::new();
    for site in EXTERNAL_SITES {
        match fetch_external_site(site, per_tag, 2, pages) {
            Ok(fetched) => rows.extend(fetched),
            Err(error) => failures.push(format!("{site}: {error}")),
        }
    }
    match fetch_crates_categories(per_tag, 1) {
        Ok(fetched) => rows.extend(fetched),
        Err(error) => failures.push(format!("crates: {error}")),
    }
    if rows.is_empty() {
        return Err(failures.join("; "));
    }
    Ok(rows)
}

/// Developer-corpus fetch, the corpus the models are trained on.
pub fn fetch_external(
    per_tag: usize,
    first_page: usize,
    pages: usize,
) -> Result<Vec<(String, Option<String>)>, String> {
    fetch_external_site("stackoverflow", per_tag, first_page, pages)
}

/// Cache location for a fetched corpus: refetching would spend the keyless quota
/// again and would make one run incomparable with the next.
pub fn external_cache_path(claude_home: &std::path::Path) -> std::path::PathBuf {
    crate::runtime::state_directory(claude_home)
        .join("benchmarks")
        .join("stackoverflow-corpus.json")
}

/// Training rows live apart from the evaluation rows: a model scored on the rows
/// it learned from is a training number wearing an evaluation label.
pub fn training_cache_path(claude_home: &std::path::Path) -> std::path::PathBuf {
    crate::runtime::state_directory(claude_home)
        .join("benchmarks")
        .join("stackoverflow-train.json")
}

pub fn read_external_cache(path: &std::path::Path) -> Option<Vec<(String, Option<String>)>> {
    let text = std::fs::read_to_string(path).ok()?; // why: an absent cache means fetch
    let cases: Vec<(String, Option<String>)> = serde_json::from_str(&text).ok()?; // why: an unreadable cache means fetch
    (!cases.is_empty()).then_some(cases)
}

pub fn write_external_cache(
    path: &std::path::Path,
    cases: &[(String, Option<String>)],
) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("create cache dir: {error}"))?;
    }
    let text =
        serde_json::to_string(cases).map_err(|error| format!("serialize corpus: {error}"))?;
    std::fs::write(path, text).map_err(|error| format!("write corpus cache: {error}"))
}

struct RemoteBatch {
    model: Option<String>,
    items: Vec<(String, Option<f64>, u64)>,
}

/// One request carries every case, so the label set is the corpus's own expected
/// skills plus the none-of-the-above option the service documents for that case.
fn expected_labels(cases: &[(String, Option<String>)]) -> Vec<String> {
    let labels: Vec<String> = cases
        .iter()
        .filter_map(|(_, skill)| skill.clone())
        .chain(std::iter::once(REMOTE_NONE_LABEL.to_string()))
        .collect();
    unique_labels(&labels)
}

fn run_remote(prompts: &[String], labels: &[String], endpoint: &str) -> Option<RemoteBatch> {
    let body = json!({ "inputs": prompts, "labels": labels }).to_string();
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

/// Per-class confusion, computed from the rows themselves so the table can never
/// disagree with the headline counts above it.
pub struct ClassScore {
    pub expected: String,
    pub rows: usize,
    pub decided: usize,
    pub correct: usize,
    pub precision: f64,
}

pub fn class_scores(report: &BenchmarkReport) -> Vec<ClassScore> {
    let mut scores: Vec<ClassScore> = Vec::new();
    for row in &report.rows {
        let Some(expected) = row.expected.as_deref() else {
            continue;
        };
        let index = match scores.iter().position(|score| score.expected == expected) {
            Some(index) => index,
            None => {
                scores.push(ClassScore {
                    expected: expected.to_string(),
                    rows: 0,
                    decided: 0,
                    correct: 0,
                    precision: 0.0,
                });
                scores.len() - 1
            }
        };
        scores[index].rows += 1;
        if row.local.is_some() {
            scores[index].decided += 1;
        }
        if row.local_correct {
            scores[index].correct += 1;
        }
    }
    for score in scores.iter_mut() {
        score.precision = if score.decided > 0 {
            score.correct as f64 / score.decided as f64
        } else {
            0.0
        };
    }
    scores.sort_by_key(|score| std::cmp::Reverse(score.rows));
    scores
}

/// Reliability data: one bucket per tenth of confidence, so the stated confidence
/// can be checked against what actually happened inside each band.
pub fn reliability_bins(report: &BenchmarkReport) -> Vec<(f64, f64, usize, f64, f64)> {
    let mut bins: Vec<(usize, f64, usize)> = vec![(0, 0.0, 0); 10];
    for row in &report.rows {
        let Some(confidence) = row.local_confidence else {
            continue;
        };
        let index = ((confidence * 10.0) as usize).min(9);
        bins[index].0 += 1;
        bins[index].1 += confidence;
        if row.local_correct {
            bins[index].2 += 1;
        }
    }
    bins.iter()
        .enumerate()
        .filter(|(_, (count, _, _))| *count > 0)
        .map(|(index, (count, sum, correct))| {
            (
                index as f64 / 10.0,
                (index + 1) as f64 / 10.0,
                *count,
                sum / *count as f64,
                *correct as f64 / *count as f64,
            )
        })
        .collect()
}

pub fn render(report: &BenchmarkReport) -> String {
    let mut out = String::new();
    out.push_str(&format!("KEEL DECISION BENCHMARK   {}\n", report.source));
    out.push_str(&format!(
        "cases: {} ({} controls)\n",
        report.cases, report.controls
    ));
    out.push_str(&format!(
        "keel local       correct {}/{}   decided {}   silent {}\n",
        report.local_correct,
        report.cases,
        report.local_decided,
        report.cases.saturating_sub(report.local_decided)
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
    for score in class_scores(report) {
        out.push_str(&format!(
            "class {:38} rows {:3}  decided {:3}  correct {:3}  precision {:3.0}%\n",
            score.expected,
            score.rows,
            score.decided,
            score.correct,
            score.precision * 100.0
        ));
    }
    for (low, high, rows, stated, actual) in reliability_bins(report) {
        out.push_str(&format!(
            "confidence {low:.1}-{high:.1}  rows {rows:3}  stated {stated:.3}  actual {actual:.3}\n"
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
        "local_decided": report.local_decided,
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
    fn external_tag_map_is_mechanical_and_unique() {
        let mut tags: Vec<&str> = Vec::new();
        for (tag, skill) in EXTERNAL_TAGS {
            assert!(
                !tag.is_empty() && !skill.is_empty(),
                "{tag} maps to nothing"
            );
            assert!(!tags.contains(tag), "{tag} appears twice");
            tags.push(tag);
        }
        assert!(
            tags.len() >= 8,
            "a real benchmark needs breadth, not one tag"
        );
        assert_ne!(
            SOURCE_EXTERNAL, SOURCE_FIXTURES,
            "an external run must not read as a self-referential one"
        );
    }

    #[test]
    fn external_corpus_scores_through_the_same_path() {
        let cases = vec![
            (
                "fix the borrow checker error".to_string(),
                Some("rust".to_string()),
            ),
            (
                "why is my unit test flaky".to_string(),
                Some("test-driven-development".to_string()),
            ),
        ];
        let report = run_external(None, &cases, false);
        assert_eq!(report.cases, 2);
        assert_eq!(report.controls, 0);
        assert_eq!(report.source, SOURCE_EXTERNAL);
        assert!(!report.remote_available);
        assert_eq!(
            report.local_confidence_rows, 0,
            "no home means no calibrated column, and the count must say so"
        );
        assert_eq!(
            report.local_decided, 0,
            "question-shaped titles route to nothing, and silence must be counted as silence"
        );
        assert_eq!(report.rows.len(), 2);
    }

    #[test]
    fn external_cache_round_trips_and_rejects_empty() {
        let dir = std::env::temp_dir().join(format!("keel-external-cache-{}", std::process::id()));
        let path = dir.join("corpus.json");
        let cases = vec![("title".to_string(), Some("rust".to_string()))];
        write_external_cache(&path, &cases).unwrap();
        assert_eq!(read_external_cache(&path), Some(cases));
        write_external_cache(&path, &[]).unwrap();
        assert_eq!(
            read_external_cache(&path),
            None,
            "an empty cache is not a corpus"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn curated_cases_route_locally() {
        let report = run(None, false);
        assert_eq!(report.cases, build_cases().len());
        assert_eq!(report.local_correct, report.cases);
        assert_eq!(
            report.local_decided,
            report.cases - report.controls,
            "the control prompts are correctly silent, everything else decides"
        );
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
        let labels = expected_labels(&build_cases());
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
