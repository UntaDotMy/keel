//! Per-skill lexical experts learned from real rows, the evidence the router
//! lacks when a prompt does not match keel's own trigger vocabulary.
//!
//! Trained from weakly labelled rows (a community tag names the domain, the tag
//! maps to the skill), scored as a log-odds of the prompt's tokens against the
//! corpus background. The held-out split is fixed-seed and disjoint, so a
//! reported number is never a training number.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub const LEXICAL_SCHEMA: u32 = 1;
const SMOOTHING: f64 = 0.5;
const WEIGHT_CLIP: f64 = 6.0;
const DEFAULT_SEED: u64 = 42;
const REFUSE_BELOW_ROWS: usize = 100;
const REFUSE_BELOW_ACCURACY: f64 = 0.50;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LexicalExpert {
    pub name: String,
    pub terms: Vec<(String, f64)>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct HeldOut {
    pub rows: usize,
    pub correct: usize,
    pub accuracy: f64,
    pub brier: f64,
    pub scale: f64,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct LexicalModel {
    pub schema: u32,
    pub training_rows: usize,
    pub skills: usize,
    pub experts: Vec<LexicalExpert>,
    pub held_out: Option<HeldOut>,
    pub usable: bool,
}

/// Train one expert per skill. Returns `None` when the corpus cannot support a
/// model at all: a held-out score over a handful of rows would be a guess.
pub fn train(rows: &[(String, Option<String>)]) -> Option<LexicalModel> {
    let (train_rows, held_out_rows) = split(rows, DEFAULT_SEED);
    let model = fit(&train_rows)?;
    let held_out = score_held_out(&model, &held_out_rows);
    let usable = rows.len() >= REFUSE_BELOW_ROWS
        && held_out
            .as_ref()
            .is_some_and(|metrics| metrics.accuracy >= REFUSE_BELOW_ACCURACY);
    Some(LexicalModel {
        schema: LEXICAL_SCHEMA,
        training_rows: train_rows.len(),
        skills: model.len(),
        experts: model,
        held_out,
        usable,
    })
}

fn split(rows: &RawRows, seed: u64) -> (LabelledRows, LabelledRows) {
    let mut labelled: LabelledRows = rows
        .iter()
        .filter_map(|(prompt, skill)| Some((prompt.clone(), skill.clone()?)))
        .collect();
    let mut state = seed | 1;
    for index in (1..labelled.len()).rev() {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let swap = (state >> 33) as usize % (index + 1);
        labelled.swap(index, swap);
    }
    let held_out_count = labelled.len() / 5;
    let held_out = labelled.split_off(labelled.len() - held_out_count);
    (labelled, held_out)
}

/// A prompt with its weak label: a community tag mapped to a keel skill.
pub type LabelledRows = Vec<(String, String)>;

/// Raw rows as fetched, where an unlabelled row is allowed and then dropped.
pub type RawRows = [(String, Option<String>)];

type TermCounts = HashMap<String, HashMap<String, usize>>;

fn term_counts(rows: &[(String, String)]) -> TermCounts {
    let mut per_skill: TermCounts = HashMap::new();
    for (prompt, skill) in rows {
        let entry = per_skill.entry(skill.clone()).or_default();
        for token in crate::utility::skill_match::tokenize(prompt) {
            *entry.entry(token).or_default() += 1;
        }
    }
    per_skill
}

fn fit(rows: &[(String, String)]) -> Option<Vec<LexicalExpert>> {
    if rows.is_empty() {
        return None;
    }
    let per_skill = term_counts(rows);
    let mut background: HashMap<String, usize> = HashMap::new();
    for counts in per_skill.values() {
        for (token, count) in counts {
            *background.entry(token.clone()).or_default() += count;
        }
    }
    let background_total: usize = background.values().sum();
    let vocabulary = background.len().max(1) as f64;
    let experts: Vec<LexicalExpert> = per_skill
        .iter()
        .map(|(skill, counts)| {
            let skill_total: usize = counts.values().sum();
            let mut terms: Vec<(String, f64)> = counts
                .iter()
                .map(|(token, count)| {
                    let background_count = background.get(token).copied().unwrap_or(0) as f64;
                    let present =
                        (*count as f64 + SMOOTHING) / (skill_total as f64 + SMOOTHING * vocabulary);
                    let base = (background_count + SMOOTHING)
                        / (background_total as f64 + SMOOTHING * vocabulary);
                    let weight = (present / base).ln().clamp(-WEIGHT_CLIP, WEIGHT_CLIP);
                    (token.clone(), weight)
                })
                .collect();
            terms.sort_by(|left, right| {
                right
                    .1
                    .partial_cmp(&left.1)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            LexicalExpert {
                name: skill.clone(),
                terms,
            }
        })
        .collect();
    (!experts.is_empty()).then_some(experts)
}

fn score(experts: &[LexicalExpert], prompt: &str) -> Vec<(String, f64)> {
    let tokens = crate::utility::skill_match::tokenize(prompt);
    let mut scored: Vec<(String, f64)> = experts
        .iter()
        .map(|expert| {
            let mut sum = 0.0;
            let mut seen = 0usize;
            for (term, weight) in &expert.terms {
                if tokens.contains(term) {
                    sum += weight;
                    seen += 1;
                }
            }
            let mean = if seen == 0 { 0.0 } else { sum / seen as f64 };
            let coverage = seen as f64 / tokens.len().max(1) as f64;
            (expert.name.clone(), mean * coverage.sqrt())
        })
        .collect();
    scored.sort_by(|left, right| {
        right
            .1
            .partial_cmp(&left.1)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    scored
}

/// Best skill and its calibrated confidence. `None` when no expert has any
/// evidence for the prompt: silence is a valid answer, a coin flip is not.
pub fn predict(model: &LexicalModel, prompt: &str) -> Option<(String, f64)> {
    let scored = score(&model.experts, prompt);
    let (name, top) = scored.first()?;
    let second = scored.get(1).map(|(_, value)| *value).unwrap_or(0.0);
    let margin = top - second;
    if margin <= 0.0 {
        return None;
    }
    let scale = model
        .held_out
        .as_ref()
        .map(|metrics| metrics.scale)
        .unwrap_or(1.0);
    let confidence = 1.0 / (1.0 + (-scale * margin).exp());
    Some((name.clone(), confidence))
}

fn score_held_out(model: &[LexicalExpert], rows: &[(String, String)]) -> Option<HeldOut> {
    if rows.is_empty() {
        return None;
    }
    let mut best_scale = 0.25f64;
    let mut best_brier = f64::MAX;
    for step in 1..=40 {
        let scale = step as f64 * 0.25;
        let brier = brier_for(model, rows, scale);
        if brier < best_brier {
            best_scale = scale;
            best_brier = brier;
        }
    }
    let scale = best_scale;
    let mut correct = 0usize;
    for (prompt, skill) in rows {
        if predict_with_scale(model, prompt, scale).is_some_and(|(name, _)| name == *skill) {
            correct += 1;
        }
    }
    Some(HeldOut {
        rows: rows.len(),
        correct,
        accuracy: correct as f64 / rows.len() as f64,
        brier: brier_for(model, rows, scale),
        scale,
    })
}

fn brier_for(model: &[LexicalExpert], rows: &[(String, String)], scale: f64) -> f64 {
    let mut sum = 0.0;
    for (prompt, skill) in rows {
        let (name, confidence) = predict_with_scale(model, prompt, scale).unwrap_or_default();
        let correct = (!name.is_empty() && name == *skill) as u8 as f64;
        sum += (confidence - correct).powi(2);
    }
    sum / rows.len().max(1) as f64
}

fn predict_with_scale(model: &[LexicalExpert], prompt: &str, scale: f64) -> Option<(String, f64)> {
    let scored = score(model, prompt);
    let (name, top) = scored.first()?;
    let second = scored.get(1).map(|(_, value)| *value).unwrap_or(0.0);
    let margin = top - second;
    if margin <= 0.0 {
        return None;
    }
    Some((name.clone(), 1.0 / (1.0 + (-scale * margin).exp())))
}

pub fn artifact_path(claude_home: &Path) -> PathBuf {
    crate::runtime::state_directory(claude_home)
        .join("benchmarks")
        .join("lexical-experts.json")
}

pub fn save(path: &Path, model: &LexicalModel) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("create model dir: {error}"))?;
    }
    let text = serde_json::to_string(model).map_err(|error| format!("serialize model: {error}"))?;
    std::fs::write(path, text).map_err(|error| format!("write model: {error}"))
}

/// A model from another schema reads as untrained rather than as a wrong answer.
pub fn load(path: &Path) -> Option<LexicalModel> {
    let text = std::fs::read_to_string(path).ok()?; // why: absent model means untrained
    let model: LexicalModel = serde_json::from_str(&text).ok()?; // why: unreadable model means untrained
    (model.schema == LEXICAL_SCHEMA && model.usable).then_some(model)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn separable_rows() -> Vec<(String, Option<String>)> {
        let mut rows = Vec::new();
        for index in 0..120 {
            rows.push((
                format!("borrow checker lifetime error in cargo build {index}"),
                Some("rust".to_string()),
            ));
            rows.push((
                format!("postgres vacuum autovacuum bloat index {index}"),
                Some("postgres".to_string()),
            ));
        }
        rows
    }

    #[test]
    fn train_splits_disjointly_and_scores_held_out() {
        let rows = separable_rows();
        let model = train(&rows).expect("separable corpus trains");
        let held_out = model.held_out.as_ref().expect("held-out metrics");
        assert_eq!(model.training_rows + held_out.rows, rows.len());
        assert!(
            held_out.accuracy > 0.9,
            "separable corpus should separate: {}",
            held_out.accuracy
        );
        assert!(model.usable, "enough rows and accuracy make it usable");
    }

    #[test]
    fn predict_names_the_learned_skill_and_refuses_a_tie() {
        let rows = separable_rows();
        let model = train(&rows).expect("model");
        let (name, confidence) = predict(&model, "postgres autovacuum bloat").expect("predicts");
        assert_eq!(name, "postgres");
        assert!(confidence > 0.5, "confidence {confidence}");
    }

    #[test]
    fn small_corpus_is_refused() {
        let rows: Vec<(String, Option<String>)> = (0..8)
            .map(|index| (format!("tiny row {index}"), Some("rust".to_string())))
            .collect();
        let model = train(&rows).expect("still trains");
        assert!(!model.usable, "eight rows must not earn a usable model");
    }

    #[test]
    fn schema_mismatch_reads_as_untrained() {
        let dir = std::env::temp_dir().join(format!("keel-lexical-{}", std::process::id()));
        let path = dir.join("lexical-experts.json");
        let mut model = train(&separable_rows()).expect("model");
        model.schema = LEXICAL_SCHEMA + 1;
        save(&path, &model).unwrap();
        assert!(load(&path).is_none(), "another schema is not this model");
        model.schema = LEXICAL_SCHEMA;
        save(&path, &model).unwrap();
        assert!(load(&path).is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
