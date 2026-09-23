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
const REMOTE_TIMEOUT_SECS: &str = "60";

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
    /// Set when `--remote` was asked and the service did not return a batch.
    pub remote_error: Option<String>,
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

/// Where the host-prompt benchmark came from. It is not a tag shell and not
/// the curated-trigger lookup.
pub const SOURCE_HOST: &str = "host prompts (disjoint from lexical training)";

/// Host sentences a person would type at an agent, each an installed skill or
/// silence. None of these strings are lexical training rows.
pub const HOST_BENCHMARK: &[(&str, Option<&str>)] = &[
    (
        "Our nightly checkout job is waiting on a row lock in the payments ledger. Show the statement that holds it and the migration that should stop taking that lock.",
        Some("postgres-migration-safety"),
    ),
    (
        "The expand step added a column and the backfill is rewriting the whole orders table. I need the safe sequence before we add the constraint.",
        Some("postgres-migration-safety"),
    ),
    (
        "The worker dies only after the third retry of the same invoice message, and the stack I have is truncated. Find what state survives between retries.",
        Some("systematic-debugging"),
    ),
    (
        "This failure shows up only when two people save the same draft a second apart. I need the race that causes it.",
        Some("systematic-debugging"),
    ),
    (
        "Before I change the refund calculator, I want a failing example for a partial refund that rounds half up.",
        Some("test-driven-development"),
    ),
    (
        "Pin an example for an empty cart total before the implementation of that total moves.",
        Some("test-driven-development"),
    ),
    (
        "A partner sent a webhook with a signature header we have never checked. Walk the verification and what we must reject.",
        Some("adversarial-security-review"),
    ),
    (
        "The admin export includes raw session tokens in the CSV. Close that exposure before customers can download it.",
        Some("adversarial-security-review"),
    ),
    (
        "Clients send both the old and new field names for the same address. Decide the compatibility rule for one more release.",
        Some("api-contract-design"),
    ),
    (
        "The list endpoint returns a bare array and the mobile client cannot tell when a page ends. Specify the pagination contract.",
        Some("api-contract-design"),
    ),
    (
        "The order service calls inventory, then payment, then email, and a failure in email leaves money captured. Where should that orchestration live?",
        Some("backend-and-data-architecture"),
    ),
    (
        "We are splitting the billing module out of the monolith. Name the boundary so inventory does not start owning invoices.",
        Some("backend-and-data-architecture"),
    ),
    (
        "The lockfile resolved two copies of the http stack and the build pulls the vulnerable one. I want a single resolved version.",
        Some("dependency-and-supply-chain"),
    ),
    (
        "The live cursor channel drops clients whenever we deploy, and reconnects replay the whole history. Design the resume behavior.",
        Some("websocket-realtime-design"),
    ),
    (
        "The deploy rolls every pod at once and the health check still hits the old path. Keep one healthy replica during the rollout.",
        Some("cloud-and-devops-expert"),
    ),
    (
        "Latency jumped after the last release but the dashboard only has a five-minute average. Which signals show the bad shard?",
        Some("observability-and-incident-response"),
    ),
    (
        "The invoice date is showing in the server locale for customers in Japan. Format it for their locale without changing the stored instant.",
        Some("internationalization-and-localization"),
    ),
    (
        "Please review this diff that changes how we persist drafts and tell me if that patch is safe to land.",
        Some("reviewer"),
    ),
    (
        "I committed the migration on the wrong branch and I have not pushed. Move that one commit onto the release branch without dragging the other two.",
        Some("git-expert"),
    ),
    (
        "What is the boiling point of water at sea level if I am only asking for the number?",
        None,
    ),
    (
        "Remind me of the capital of Portugal, nothing about this repository.",
        None,
    ),
];

pub fn host_benchmark_cases() -> Vec<(String, Option<String>)> {
    HOST_BENCHMARK
        .iter()
        .map(|(prompt, skill)| ((*prompt).to_string(), skill.map(str::to_string)))
        .collect()
}

pub fn host_benchmark_controls() -> usize {
    HOST_BENCHMARK
        .iter()
        .filter(|(_, skill)| skill.is_none())
        .count()
}

/// Score the fixed host prompts. This path does not read or write an eval cache.
pub fn run_host(home: Option<&std::path::Path>, remote: bool) -> BenchmarkReport {
    score_cases(
        home,
        &host_benchmark_cases(),
        host_benchmark_controls(),
        SOURCE_HOST,
        remote,
    )
}

/// Drop labelled rows whose text is a host-benchmark prompt. Unlabelled rows stay.
pub fn omit_host_benchmark_labels(
    rows: &[crate::utility::lexical_experts::SourcedRow],
) -> (Vec<crate::utility::lexical_experts::SourcedRow>, usize) {
    let reserved: std::collections::HashSet<&str> =
        HOST_BENCHMARK.iter().map(|(prompt, _)| *prompt).collect();
    let mut dropped = 0usize;
    let kept = rows
        .iter()
        .filter(|row| {
            let blocked = row.skill.is_some() && reserved.contains(row.text.trim());
            if blocked {
                dropped += 1;
            }
            !blocked
        })
        .cloned()
        .collect();
    (kept, dropped)
}

/// The row list `train-lexical` fits: ambiguous titles out, uninstalled skills
/// out, host-benchmark prompts out of the labelled set.
pub fn lexical_rows_to_fit(
    rows: &[crate::utility::lexical_experts::SourcedRow],
    installed: &dyn Fn(&str) -> bool,
) -> (
    Vec<crate::utility::lexical_experts::SourcedRow>,
    usize,
    usize,
    usize,
) {
    let (rows, dropped_ambiguous) = crate::utility::lexical_experts::drop_ambiguous_sourced(rows);
    let (rows, dropped_uninstalled) =
        crate::utility::lexical_experts::drop_uninstalled_skills(&rows, installed);
    let (rows, dropped_benchmark) = omit_host_benchmark_labels(&rows);
    (
        rows,
        dropped_ambiguous,
        dropped_uninstalled,
        dropped_benchmark,
    )
}

fn score_cases(
    home: Option<&std::path::Path>,
    cases: &[(String, Option<String>)],
    controls: usize,
    source: &str,
    remote: bool,
) -> BenchmarkReport {
    let prompts: Vec<String> = cases.iter().map(|(prompt, _)| prompt.clone()).collect();
    let (remote_batch, remote_error) = if remote {
        match run_remote(&prompts, &expected_labels(cases), REMOTE_ENDPOINT) {
            Ok(batch) => (Some(batch), None),
            Err(error) => (None, Some(error)),
        }
    } else {
        (None, None)
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
        remote_error,
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

/// Community tag → the installed skill that owns that work.
/// A tag with no installed skill is not collected. `rust` was removed for that reason.
pub const EXTERNAL_TAGS: &[(&str, &str)] = &[
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
/// Pages fetched for training. Page one is reserved as the evaluation shell.
const CRATES_TRAINING_PAGES: usize = 10;

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

/// Cache for the crates corpus, kept beside the Stack Exchange ones.
pub fn crates_cache_path(claude_home: &std::path::Path) -> std::path::PathBuf {
    crate::runtime::state_directory(claude_home)
        .join("benchmarks")
        .join("crates-corpus.json")
}

/// Every corpus trains. One provider failing must not stop training: the rows
/// that did arrive are kept and only an empty result is an error.
pub fn fetch_all_corpora(
    per_tag: usize,
    pages: usize,
    with_prose: bool,
) -> Result<Vec<crate::utility::lexical_experts::SourcedRow>, String> {
    let mut rows = Vec::new();
    let mut failures: Vec<String> = Vec::new();
    for site in EXTERNAL_SITES {
        match fetch_external_site(site, per_tag, 2, pages) {
            Ok(fetched) => {
                let provider = format!("stackexchange:{site}");
                rows.extend(fetched.into_iter().map(|(text, skill)| {
                    crate::utility::lexical_experts::SourcedRow {
                        text,
                        skill,
                        provider: provider.clone(),
                    }
                }));
            }
            Err(error) => failures.push(format!("{site}: {error}")),
        }
    }
    // why: pages two and up train while page one stays the evaluation shell.
    for page in 2..=CRATES_TRAINING_PAGES {
        match fetch_crates_categories(per_tag, page) {
            Ok(fetched) => rows.extend(fetched.into_iter().map(|(text, skill)| {
                crate::utility::lexical_experts::SourcedRow {
                    text,
                    skill,
                    provider: crate::utility::lexical_experts::CRATES_PROVIDER.to_string(),
                }
            })),
            Err(error) => failures.push(format!("crates page {page}: {error}")),
        }
    }
    // why: the only provider whose rows read like host prompts, and opt-in
    // because it lost on both product corpora when it trained by default.
    if with_prose {
        for (topic, skill) in OPENALEX_TOPICS {
            match fetch_openalex_topic(topic, OPENALEX_PAGES) {
                Ok(fetched) => {
                    let provider = format!("{OPENALEX_PROVIDER_PREFIX}{topic}");
                    rows.extend(fetched.into_iter().map(|(text, _)| {
                        crate::utility::lexical_experts::SourcedRow {
                            text,
                            skill: Some((*skill).to_string()),
                            provider: provider.clone(),
                        }
                    }));
                }
                Err(error) => failures.push(format!("openalex {topic}: {error}")),
            }
        }
    }
    if rows.is_empty() {
        return Err(failures.join("; "));
    }
    Ok(rows)
}

/// OpenAlex topic id to the installed skill that owns that research domain.
/// The rule is the same as the tag maps: a public vocabulary names the domain
/// and the skill owns the domain, so the label is the community's, not keel's.
/// Only names that state one domain are listed; a topic naming two skills at
/// once would teach the head a decision keel does not have.
const OA_SECURITY: &str = "adversarial-security-review";
const OA_AUTH: &str = "authentication-and-identity";
const OA_BACKEND: &str = "backend-and-data-architecture";
const OA_CLOUD: &str = "cloud-and-devops-expert";
const OA_ML: &str = "data-and-ml-engineering";
pub const OPENALEX_TOPICS: &[(&str, &str)] = &[
    ("T10237", OA_SECURITY),
    ("T10734", OA_SECURITY),
    ("T10400", OA_SECURITY),
    ("T11800", OA_AUTH),
    ("T11504", OA_AUTH),
    ("T10317", OA_BACKEND),
    ("T11106", OA_BACKEND),
    ("T10772", OA_CLOUD),
    ("T10101", OA_CLOUD),
    ("T12127", "observability-and-incident-response"),
    ("T13373", OA_ML),
    ("T11689", OA_ML),
    ("T10470", "ui-design-systems-and-responsive-interfaces"),
    ("T10743", "systematic-debugging"),
];

pub const OPENALEX_PROVIDER_PREFIX: &str = "openalex:";
/// Pages per topic at two hundred works a page. Enough prose to change the
/// feature vocabulary without burying the title and blurb providers.
const OPENALEX_PAGES: usize = 5;
const OPENALEX_ENDPOINT: &str = "https://api.openalex.org/works";
const OPENALEX_PAGE_SIZE: usize = 200;
/// Abstract words kept per row. A whole abstract would drown the title.
const OPENALEX_TEXT_CHARS: usize = 600;

/// One topic's works, newest first, as `title plus abstract` rows. OpenAlex is
/// open, needs no key, and answers a cursor page at a time; the polite pool
/// header identifies the caller.
pub fn fetch_openalex_topic(
    topic: &str,
    pages: usize,
) -> Result<Vec<(String, Option<String>)>, String> {
    let mut rows = Vec::new();
    let mut cursor = "*".to_string();
    for _ in 0..pages {
        let url = format!(
            "{OPENALEX_ENDPOINT}?filter=primary_topic.id:{topic}&per-page={OPENALEX_PAGE_SIZE}&cursor={cursor}\
             &select=title,abstract_inverted_index,publication_year"
        );
        let output = Command::new("curl")
            .args([
                "-s",
                "--compressed",
                "--max-time",
                EXTERNAL_TIMEOUT_SECS,
                "-A",
                OPENALEX_USER_AGENT,
                &url,
            ])
            .output()
            .map_err(|error| format!("curl failed: {error}"))?;
        if !output.status.success() {
            return Err("curl failed".to_string());
        }
        let parsed: serde_json::Value = serde_json::from_slice(&output.stdout)
            .map_err(|error| format!("unreadable response: {error}"))?;
        let results = parsed
            .get("results")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| "no results array".to_string())?;
        for work in results {
            let title = work
                .get("title")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .unwrap_or_default();
            let abstract_text = work
                .get("abstract_inverted_index")
                .map(abstract_from_inverted_index)
                .unwrap_or_default();
            let composed = compose_abstract_row(title, &abstract_text);
            if !composed.is_empty() {
                rows.push((composed, None));
            }
        }
        let next = parsed
            .get("meta")
            .and_then(|meta| meta.get("next_cursor"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        if next.is_empty() {
            break;
        }
        cursor = next;
    }
    if rows.is_empty() {
        return Err("returned no works".to_string());
    }
    Ok(rows)
}

pub const OPENALEX_USER_AGENT: &str = "keel-benchmark/0.1 (evaluation harness)";

/// Rows per installed skill, built from that skill's own description. A prompt
/// can only route to a class the head has, and most installed skills had none:
/// the head emitted eleven classes for fifty-three installed skills, so a
/// reviewer or git prompt could not be named by the trained tier at all. The
/// seeds are keel's own text, tagged so a run shows how much of the corpus they
/// are, and they never include the benchmark prompts.
pub const SKILL_SEED_PROVIDER: &str = "keel-skill-seed";
const SKILL_SEED_ROWS: usize = 40;

pub fn skill_seed_rows(
    catalog: &[crate::utility::skill_match::SkillCatalogEntry],
) -> Vec<crate::utility::lexical_experts::SourcedRow> {
    let mut rows = Vec::new();
    for entry in catalog {
        let mut texts: Vec<String> = Vec::new();
        // why: the name is the strongest cue a skill owns, and it is two words
        // joined by hyphens, which the tokenizer would otherwise see as one.
        texts.push(entry.name.replace('-', " "));
        for source in [&entry.description, &entry.when_to_use] {
            for sentence in source.split(['.', '\n']) {
                let sentence = sentence.trim();
                if sentence.len() >= 24 {
                    texts.push(sentence.to_string());
                }
            }
        }
        texts.truncate(SKILL_SEED_ROWS);
        for text in texts {
            rows.push(crate::utility::lexical_experts::SourcedRow {
                text,
                skill: Some(entry.name.clone()),
                provider: SKILL_SEED_PROVIDER.to_string(),
            });
        }
    }
    rows
}

/// Rows a person labelled for keel's own skills: host-shaped prompts with the
/// skill they should load. Kept outside the fetched corpora so a provider
/// refresh cannot drop them, and trusted like the seeds because the label is
/// deliberate rather than a community tag.
pub const USER_ROW_PROVIDER: &str = "user-labelled";

/// Where those rows live. A missing file means none were written, not an error.
pub fn user_rows_path(claude_home: &std::path::Path) -> std::path::PathBuf {
    crate::runtime::state_directory(claude_home)
        .join("benchmarks")
        .join("user-rows.json")
}

/// Reads the labelled rows and drops any that name a skill this install does not
/// have, or that repeat a host benchmark prompt: an evaluation row in training is
/// a training number wearing an evaluation label.
pub fn read_user_rows(
    claude_home: &std::path::Path,
) -> (
    Vec<crate::utility::lexical_experts::SourcedRow>,
    usize,
    usize,
) {
    let Ok(text) = std::fs::read_to_string(user_rows_path(claude_home)) else {
        return (Vec::new(), 0, 0);
    };
    // fallback: a malformed file means no labelled rows, never a failed run.
    let entries: Vec<serde_json::Value> = serde_json::from_str(&text).unwrap_or_default();
    let reserved: std::collections::HashSet<String> = HOST_BENCHMARK
        .iter()
        .map(|(prompt, _)| normalize_text(prompt))
        .collect();
    let mut rows = Vec::new();
    let mut uninstalled = 0usize;
    let mut benchmark = 0usize;
    for entry in entries {
        let (text, skill) = if let serde_json::Value::Array(pair) = &entry {
            match (pair.first(), pair.get(1)) {
                (Some(text), Some(skill)) => (
                    text.as_str().unwrap_or_default().to_string(),
                    skill.as_str().unwrap_or_default().to_string(),
                ),
                _ => continue,
            }
        } else {
            match (entry.get("text"), entry.get("skill")) {
                (Some(text), Some(skill)) => (
                    text.as_str().unwrap_or_default().to_string(),
                    skill.as_str().unwrap_or_default().to_string(),
                ),
                _ => continue,
            }
        };
        if text.trim().is_empty() || skill.is_empty() {
            continue;
        }
        if reserved.contains(&normalize_text(&text)) {
            benchmark += 1;
            continue;
        }
        if crate::utility::skill_match::installed_skill_path(claude_home, &skill).is_none() {
            uninstalled += 1;
            continue;
        }
        rows.push(crate::utility::lexical_experts::SourcedRow {
            text,
            skill: Some(skill),
            provider: USER_ROW_PROVIDER.to_string(),
        });
    }
    (rows, uninstalled, benchmark)
}

/// OpenAlex stores an abstract as word to positions. Rebuild it in position
/// order, which is the only order that reproduces the sentence.
fn abstract_from_inverted_index(index: &serde_json::Value) -> String {
    let mut positioned: Vec<(u64, &str)> = Vec::new();
    for (word, positions) in index.as_object().into_iter().flatten() {
        for position in positions.as_array().into_iter().flatten() {
            if let Some(position) = position.as_u64() {
                positioned.push((position, word.as_str()));
            }
        }
    }
    positioned.sort_by_key(|(position, _)| *position);
    let text: Vec<&str> = positioned.into_iter().map(|(_, word)| word).collect();
    text.join(" ")
}

/// A row is the title plus as much abstract as the cap allows, cut at a word.
fn compose_abstract_row(title: &str, abstract_text: &str) -> String {
    let mut composed = String::with_capacity(title.len() + OPENALEX_TEXT_CHARS);
    composed.push_str(title.trim());
    if abstract_text.is_empty() {
        return composed;
    }
    composed.push(' ');
    let remaining = OPENALEX_TEXT_CHARS.saturating_sub(composed.len());
    if abstract_text.len() <= remaining {
        composed.push_str(abstract_text);
        return composed;
    }
    // why: cutting mid-word teaches the tokenizer a word that does not exist.
    let cut = abstract_text
        .char_indices()
        .take_while(|(index, _)| *index < remaining)
        .filter(|(_, character)| character.is_whitespace())
        .map(|(index, _)| index)
        .last()
        .unwrap_or(0);
    composed.push_str(&abstract_text[..cut]);
    composed
}

/// Descriptions from the crates.io training pages, used to attribute an old
/// cache that stored titles without a provider.
pub fn fetch_crates_training_descriptions(per_category: usize) -> Result<Vec<String>, String> {
    let mut texts = Vec::new();
    let mut failures: Vec<String> = Vec::new();
    for page in 2..=CRATES_TRAINING_PAGES {
        match fetch_crates_categories(per_category, page) {
            Ok(fetched) => texts.extend(fetched.into_iter().map(|(text, _)| text)),
            Err(error) => failures.push(format!("crates page {page}: {error}")),
        }
    }
    if texts.is_empty() {
        return Err(failures.join("; "));
    }
    Ok(texts)
}

/// Tag rows whose text is a known crate description. Everything else in an
/// untagged cache is Stack Exchange. Returns how many rows were crates.io.
pub fn assign_providers_from_crates(
    rows: &mut [crate::utility::lexical_experts::SourcedRow],
    descriptions: &[String],
) -> usize {
    let known: std::collections::HashSet<String> = descriptions
        .iter()
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
        .collect();
    if known.is_empty() {
        return 0;
    }
    let mut tagged = 0usize;
    for row in rows.iter_mut() {
        if !row.provider.is_empty() {
            continue;
        }
        if known.contains(row.text.trim()) {
            row.provider = crate::utility::lexical_experts::CRATES_PROVIDER.to_string();
            tagged += 1;
        } else {
            row.provider = "stackexchange".to_string();
        }
    }
    tagged
}

pub fn merge_sourced_rows(rows: &mut Vec<crate::utility::lexical_experts::SourcedRow>) {
    rows.sort_by(|left, right| left.text.cmp(&right.text));
    let mut merged: Vec<crate::utility::lexical_experts::SourcedRow> = Vec::new();
    for row in rows.drain(..) {
        if let Some(last) = merged.last_mut() {
            if last.text == row.text {
                if last.provider.is_empty() {
                    last.provider = row.provider;
                }
                if last.skill.is_none() {
                    last.skill = row.skill;
                }
                continue;
            }
        }
        merged.push(row);
    }
    *rows = merged;
}

/// Developer-corpus fetch, the corpus the models are trained on.
pub fn fetch_external(
    per_tag: usize,
    first_page: usize,
    pages: usize,
) -> Result<Vec<(String, Option<String>)>, String> {
    fetch_external_site("stackoverflow", per_tag, first_page, pages)
}

/// Cache location for a fetched corpus, named by the site it came from. One
/// shared file let a five-site leaderboard overwrite its own corpora, and the
/// keyless quota meant they could not be fetched back the same day. The file
/// name cannot be reconstructed from its rows, so there is deliberately no
/// fallback to the pre-site file: a corpus without its site is a mislabeled
/// corpus.
pub fn external_cache_path(claude_home: &std::path::Path, site: &str) -> std::path::PathBuf {
    let safe_site: String = site
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    crate::runtime::state_directory(claude_home)
        .join("benchmarks")
        .join(format!("external-corpus-{safe_site}.json"))
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

/// Every held-out evaluation text under this home: one file per site, plus the
/// crates shell. A later fetch can pull the same text into the training cache,
/// and a benchmark that scores a memorized row reports the memory, not the model.
pub fn eval_corpus_texts(claude_home: &std::path::Path) -> Vec<String> {
    let directory = crate::runtime::state_directory(claude_home).join("benchmarks");
    let mut texts: Vec<String> = Vec::new();
    let Ok(entries) = std::fs::read_dir(&directory) else {
        return texts;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let is_external = name.starts_with("external-corpus-") || name == "crates-corpus.json";
        if !is_external || !name.ends_with(".json") {
            continue;
        }
        if let Some(cases) = read_external_cache(&entry.path()) {
            texts.extend(cases.into_iter().map(|(text, _)| text));
        }
    }
    texts
}

/// Drops training rows whose text is an evaluation row: the two are fetched from
/// the same pages, so an overlap is a fetch artefact, and scoring a row the model
/// was fitted on reports memory instead of generalization.
pub fn drop_eval_rows(
    rows: &[crate::utility::lexical_experts::SourcedRow],
    claude_home: &std::path::Path,
) -> (Vec<crate::utility::lexical_experts::SourcedRow>, usize) {
    let held_out: std::collections::HashSet<String> = eval_corpus_texts(claude_home)
        .into_iter()
        .map(|text| normalize_text(&text))
        .collect();
    if held_out.is_empty() {
        return (rows.to_vec(), 0);
    }
    let mut dropped = 0usize;
    let kept = rows
        .iter()
        .filter(|row| {
            if held_out.contains(&normalize_text(&row.text)) {
                dropped += 1;
                false
            } else {
                true
            }
        })
        .cloned()
        .collect();
    (kept, dropped)
}

/// Case and punctuation collapse, so a title that differs only in formatting
/// still reads as the same row.
pub fn normalize_text(text: &str) -> String {
    text.chars()
        .filter(|character| character.is_alphanumeric())
        .flat_map(|character| character.to_lowercase())
        .collect()
}

pub fn read_sourced_cache(
    path: &std::path::Path,
) -> Option<Vec<crate::utility::lexical_experts::SourcedRow>> {
    let text = std::fs::read_to_string(path).ok()?; // why: an absent cache means fetch
    if let Ok(rows) =
        serde_json::from_str::<Vec<crate::utility::lexical_experts::SourcedRow>>(&text)
    {
        return (!rows.is_empty()).then_some(rows);
    }
    let pairs: Vec<(String, Option<String>)> = serde_json::from_str(&text).ok()?; // why: an unreadable cache means fetch
    let rows: Vec<crate::utility::lexical_experts::SourcedRow> = pairs
        .into_iter()
        .map(
            |(text, skill)| crate::utility::lexical_experts::SourcedRow {
                text,
                skill,
                provider: String::new(),
            },
        )
        .collect();
    (!rows.is_empty()).then_some(rows)
}

pub fn write_sourced_cache(
    path: &std::path::Path,
    rows: &[crate::utility::lexical_experts::SourcedRow],
) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("create cache dir: {error}"))?;
    }
    let text = serde_json::to_string(rows).map_err(|error| format!("serialize corpus: {error}"))?;
    std::fs::write(path, text).map_err(|error| format!("write corpus cache: {error}"))
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

fn run_remote(
    prompts: &[String],
    labels: &[String],
    endpoint: &str,
) -> Result<RemoteBatch, String> {
    // The service currently refuses a body above 20 inputs.
    const CHUNK: usize = 20;
    if prompts.is_empty() {
        return Err("remote classify: no prompts".to_string());
    }
    let mut items = Vec::with_capacity(prompts.len());
    let mut model = None;
    for (index, chunk) in prompts.chunks(CHUNK).enumerate() {
        let batch = run_remote_chunk(chunk, labels, endpoint, index)?;
        if model.is_none() {
            model = batch.model;
        }
        items.extend(batch.items);
    }
    if items.len() != prompts.len() {
        return Err(format!(
            "remote classify: expected {} results, got {}",
            prompts.len(),
            items.len()
        ));
    }
    Ok(RemoteBatch { model, items })
}

fn run_remote_chunk(
    prompts: &[String],
    labels: &[String],
    endpoint: &str,
    index: usize,
) -> Result<RemoteBatch, String> {
    let body = json!({ "inputs": prompts, "labels": labels }).to_string();
    let path =
        std::env::temp_dir().join(format!("keel-classify-{}-{index}.json", std::process::id()));
    std::fs::write(&path, &body).map_err(|error| format!("write classify body: {error}"))?;
    let file_arg = format!("@{}", path.display());
    // why: a JSON body on the Windows command line is reparsed and the service rejects it.
    let output = Command::new("curl")
        .args([
            "-sS",
            "--max-time",
            REMOTE_TIMEOUT_SECS,
            "-H",
            "content-type: application/json",
            "--data-binary",
            &file_arg,
            endpoint,
        ])
        .output();
    let _ = std::fs::remove_file(&path); // why: temp-body cleanup is best-effort
    let output = output.map_err(|error| format!("curl: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("curl status {}: {}", output.status, stderr.trim()));
    }
    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("remote body was not JSON: {error}"))?;
    parse_remote_response(&parsed).ok_or_else(|| {
        let text = String::from_utf8_lossy(&output.stdout);
        format!(
            "remote response had no results: {}",
            text.chars().take(240).collect::<String>()
        )
    })
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
    } else if let Some(error) = &report.remote_error {
        out.push_str(&format!("classifier.dev   failed: {error}\n"));
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
    fn skill_seeds_label_every_row_with_its_own_skill() {
        let home = crate::runtime::resolve_claude_home("").expect("home");
        let catalog = crate::utility::skill_match::load_skill_catalog_for_home(&home);
        assert!(!catalog.is_empty(), "an installed corpus exists");
        let rows = skill_seed_rows(&catalog);
        assert!(!rows.is_empty(), "seeds exist");
        let labelled: std::collections::HashSet<&str> =
            rows.iter().filter_map(|row| row.skill.as_deref()).collect();
        assert!(
            labelled.len() >= catalog.len() * 3 / 4,
            "seeds cover the catalogue: {} of {}",
            labelled.len(),
            catalog.len()
        );
        for row in &rows {
            assert!(
                row.skill.is_some(),
                "a seed without a label teaches nothing: {}",
                row.text
            );
            assert_eq!(row.provider, SKILL_SEED_PROVIDER);
            assert!(!row.text.trim().is_empty(), "an empty seed teaches nothing");
            assert!(
                !HOST_BENCHMARK
                    .iter()
                    .any(|(prompt, _)| row.text.trim() == prompt.trim()),
                "a benchmark prompt must never be a training row"
            );
        }
        let biggest = rows
            .iter()
            .filter(|row| row.skill.as_deref() == Some(catalog[0].name.as_str()))
            .count();
        assert!(biggest <= SKILL_SEED_ROWS, "the per-skill cap holds");
        assert!(
            rows.iter().any(|row| row.text.len() >= 24),
            "descriptions contribute sentence rows, not only names"
        );
    }

    #[test]
    fn abstract_inverted_index_rebuilds_in_position_order() {
        let index = serde_json::json!({
            "consistency": [3],
            "Distributed": [0],
            "systems": [1],
            "trade": [2],
            "off": [4],
            "and": [5],
            "latency.": [6],
        });
        assert_eq!(
            abstract_from_inverted_index(&index),
            "Distributed systems trade consistency off and latency."
        );
        assert_eq!(
            abstract_from_inverted_index(&serde_json::json!({})),
            "",
            "an abstract the record does not carry is empty, never guessed"
        );
    }

    #[test]
    fn abstract_rows_keep_the_title_and_cut_on_a_word() {
        let abstract_text = "word ".repeat(400);
        let composed = compose_abstract_row("A title", &abstract_text);
        assert!(composed.starts_with("A title "), "{composed}");
        assert!(
            composed.len() <= OPENALEX_TEXT_CHARS,
            "row stays inside the cap: {}",
            composed.len()
        );
        assert!(
            !composed.ends_with("wor"),
            "the cut lands on a word boundary: {composed}"
        );
        assert_eq!(
            compose_abstract_row("Only a title", ""),
            "Only a title",
            "a work with no abstract keeps its title"
        );
    }

    #[test]
    fn openalex_topic_map_names_one_domain_each() {
        let mut topics: Vec<&str> = Vec::new();
        for (topic, skill) in OPENALEX_TOPICS {
            assert!(
                topic.starts_with('T') && topic.len() > 1,
                "{topic} is not a topic id"
            );
            assert!(!skill.is_empty(), "{topic} maps to nothing");
            assert!(!topics.contains(topic), "{topic} appears twice");
            topics.push(topic);
        }
        assert!(
            topics.len() >= 10,
            "breadth is the point of a second prose provider"
        );
        assert!(
            OPENALEX_TOPICS
                .iter()
                .all(|(_, skill)| *skill != "rust" && !skill.is_empty()),
            "every label is an installed skill, never a phantom one"
        );
    }

    #[test]
    fn host_benchmark_is_disjoint_from_the_training_cache() {
        let home = crate::runtime::resolve_claude_home("").expect("home");
        // why: the cache belongs to the developer's machine, so its absence is
        // a skip: panicking on it says nothing about the invariant checked here.
        let Some(cached) = read_sourced_cache(&training_cache_path(&home)) else {
            return;
        };
        let texts: std::collections::HashSet<String> = cached
            .iter()
            .map(|row| row.text.trim().to_string())
            .collect();
        assert!(HOST_BENCHMARK.iter().any(|(_, skill)| skill.is_none()));
        assert!(HOST_BENCHMARK.iter().any(|(_, skill)| skill.is_some()));
        for (prompt, skill) in HOST_BENCHMARK {
            match skill {
                Some("rust") => panic!("rust is not a host-benchmark label"),
                Some(name) => {
                    assert!(
                        crate::utility::skill_match::installed_skill_path(&home, name).is_some(),
                        "{name} is not installed"
                    );
                }
                None => {}
            }
            assert!(
                !texts.contains((*prompt).trim()),
                "host prompt is already a training row"
            );
        }
    }

    #[test]
    fn lexical_fit_rows_omit_host_benchmark_prompts() {
        let home = crate::runtime::resolve_claude_home("").expect("home");
        let Some(cached) = read_sourced_cache(&training_cache_path(&home)) else {
            return;
        };
        let reserved: std::collections::HashSet<&str> =
            HOST_BENCHMARK.iter().map(|(prompt, _)| *prompt).collect();
        let (fitted, _, _, _) = lexical_rows_to_fit(&cached, &|skill| {
            crate::utility::skill_match::installed_skill_path(&home, skill).is_some()
        });
        assert!(fitted
            .iter()
            .all(|row| { row.skill.is_none() || !reserved.contains(row.text.trim()) }));
        let mut injected = cached;
        injected.push(crate::utility::lexical_experts::SourcedRow {
            text: HOST_BENCHMARK[0].0.to_string(),
            skill: Some("postgres-migration-safety".to_string()),
            provider: "stackexchange".to_string(),
        });
        let (fitted, _, _, dropped_benchmark) = lexical_rows_to_fit(&injected, &|skill| {
            crate::utility::skill_match::installed_skill_path(&home, skill).is_some()
        });
        assert!(dropped_benchmark >= 1);
        assert!(fitted
            .iter()
            .all(|row| { row.text.trim() != HOST_BENCHMARK[0].0 || row.skill.is_none() }));
    }

    #[test]
    fn training_rows_that_are_eval_rows_are_dropped() {
        const DEBUGGING: &str = "systematic-debugging";
        let home = std::env::temp_dir().join(format!("keel-eval-leak-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let benchmarks = crate::runtime::state_directory(&home).join("benchmarks");
        std::fs::create_dir_all(&benchmarks).expect("benchmark dir");
        let eval_rows = vec![
            (
                "How do I stop a long migration from locking the ledger?".to_string(),
                Some("postgres-migration-safety".to_string()),
            ),
            (
                "The retry handler never fires twice".to_string(),
                Some(DEBUGGING.to_string()),
            ),
        ];
        write_external_cache(
            &benchmarks.join("external-corpus-softwareengineering.json"),
            &eval_rows,
        )
        .expect("write eval cache");
        let training_row = |text: &str, skill: &str| crate::utility::lexical_experts::SourcedRow {
            text: text.to_string(),
            skill: Some(skill.to_string()),
            provider: "stackexchange".to_string(),
        };
        let rows = vec![
            training_row(
                "How do I stop a long migration from locking the ledger?",
                "postgres-migration-safety",
            ),
            // Same row, different punctuation and case: still the eval row.
            training_row("the retry handler never fires twice.", DEBUGGING),
            training_row("a genuinely independent training row", DEBUGGING),
        ];
        let (kept, dropped) = drop_eval_rows(&rows, &home);
        assert_eq!(dropped, 2, "both overlapping rows leave the corpus");
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].text, "a genuinely independent training row");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn eval_corpus_cache_is_named_for_its_site() {
        let home = std::path::PathBuf::from(r"C:\keel-home");
        let overflow = external_cache_path(&home, "stackoverflow");
        let design = external_cache_path(&home, "softwareengineering");
        assert_ne!(
            overflow, design,
            "two sites must not share one corpus file, or the second fetch erases the first"
        );
        assert_eq!(
            external_cache_path(&home, "softwareengineering"),
            design,
            "the same site resolves to the same file"
        );
        assert!(
            overflow
                .to_string_lossy()
                .contains("external-corpus-stackoverflow"),
            "the file name carries the site: {}",
            overflow.display()
        );
        assert!(
            !external_cache_path(&home, "../escape")
                .to_string_lossy()
                .contains(".."),
            "a site name cannot walk out of the cache directory"
        );
    }

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
            tags.len() >= 7,
            "a real benchmark needs breadth, not one tag"
        );
        assert!(
            EXTERNAL_TAGS.iter().all(|(_, skill)| *skill != "rust"),
            "rust is not an installed skill, so it is not a training label"
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
    fn sourced_cache_attributes_an_old_pair_file() {
        let dir = std::env::temp_dir().join(format!("keel-sourced-cache-{}", std::process::id()));
        let path = dir.join("corpus.json");
        let pairs = vec![
            (
                "  A rust library for vacuuming. ".to_string(),
                Some("postgres-migration-safety".to_string()),
            ),
            (
                "How do I borrow in Rust?".to_string(),
                Some("rust".to_string()),
            ),
        ];
        write_external_cache(&path, &pairs).unwrap();
        let mut rows = read_sourced_cache(&path).expect("pairs still load");
        assert!(rows.iter().all(|row| row.provider.is_empty()));
        let tagged =
            assign_providers_from_crates(&mut rows, &["A rust library for vacuuming.".to_string()]);
        assert_eq!(tagged, 1);
        assert_eq!(rows[0].provider, "crates.io");
        assert_eq!(rows[1].provider, "stackexchange");
        write_sourced_cache(&path, &rows).unwrap();
        let again = read_sourced_cache(&path).expect("objects load");
        assert_eq!(again[0].provider, "crates.io");
        assert_eq!(again[1].skill.as_deref(), Some("rust"));
        let _ = std::fs::remove_dir_all(&dir);
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
