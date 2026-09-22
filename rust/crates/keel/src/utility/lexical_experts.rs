//! Per-skill lexical experts learned from real rows, the evidence the router
//! lacks when a prompt does not match keel's own trigger vocabulary.
//!
//! Multinomial logistic regression over TF-IDF unigram and bigram features,
//! trained by adagrad on weakly labelled rows (a community tag names the domain,
//! the tag maps to the skill) with class priors in the bias. TF-IDF plus a linear
//! model is the strong baseline for short multi-class text: it matches
//! transformers on standard benchmarks while staying a hash lookup to score.
//! The held-out split is fixed-seed and disjoint, and confidence is temperature
//! fitted on that split, so no reported number is a training number.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub const LEXICAL_SCHEMA: u32 = 3;
/// crates.io rows share this tag. Their descriptions name the language
/// ("a rust library") even when the skill is websocket or postgres.
pub const CRATES_PROVIDER: &str = "crates.io";
const DEFAULT_SEED: u64 = 42;
const EPOCHS: usize = 30;
const LEARNING_RATE: f64 = 0.5;
const WEIGHT_DECAY: f64 = 1e-4;
const ADAGRAD_EPSILON: f64 = 1e-8;
/// Epochs without a validation improvement before training stops.
const EARLY_STOP_PATIENCE: usize = 5;
const MIN_WORD: usize = 3;
const REFUSE_BELOW_ROWS: usize = 100;
const REFUSE_BELOW_ACCURACY: f64 = 0.50;

/// Question words carry no domain signal and would otherwise dominate bigrams.
const STOPWORDS: &[&str] = &[
    "the", "and", "for", "with", "how", "why", "what", "when", "where", "which", "does", "did",
    "not", "you", "your", "from", "this", "that", "these", "those", "are", "was", "were", "have",
    "has", "had", "can", "could", "should", "would", "about", "into", "over", "under", "between",
    "after", "before", "during", "there", "their", "them", "they", "its", "get", "got", "use",
    "using", "one", "two", "any", "all", "but", "out", "own", "same", "than", "then", "too",
    "very", "just", "also", "more", "most", "some", "such", "only", "other", "want", "need",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LexicalExpert {
    pub name: String,
    pub bias: f64,
    pub terms: Vec<(String, f64)>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct HeldOut {
    pub rows: usize,
    /// Rows the model had a verdict for, whatever the confidence: accuracy
    /// divides by `rows`, brier by `decided`, so neither reads against the
    /// wrong denominator.
    pub decided: usize,
    pub correct: usize,
    pub accuracy: f64,
    pub brier: f64,
    /// Temperature fitted on this split by lowest Brier.
    pub scale: f64,
    /// Lowest confidence that still held the precision floor on this split.
    pub accept: f64,
    /// Per-class accept points: one global cut punishes a strong class and a
    /// weak one the same way, which selective classification shows is avoidable.
    pub accept_per_skill: Vec<(String, f64)>,
    /// The trade the accept point was chosen from, so the choice is inspectable.
    pub operating_points: Vec<OperatingPoint>,
    /// Test ECE under the shipped mapping. Older artifacts predate the field.
    #[serde(default)]
    pub ece: f64,
    /// Entropy-bin upper edges with fitted temperatures, ascending. Empty
    /// means the global scale alone, so older artifacts behave as before.
    #[serde(default)]
    pub entropy_temperatures: Vec<(f64, f64)>,
}

/// Static-vs-adaptive fit report, printed by the training run so the keep
/// decision is inspectable. Selection reads validation; test only reports.
#[derive(Clone, Default, Serialize)]
pub struct CalibrationFit {
    pub static_validation_ece: f64,
    pub static_validation_coverage: f64,
    pub adaptive_validation_ece: f64,
    pub adaptive_validation_coverage: f64,
    pub static_test_brier: f64,
    pub static_test_ece: f64,
    pub adaptive_test_brier: f64,
    pub adaptive_test_ece: f64,
    pub kept_adaptive: bool,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct OperatingPoint {
    pub threshold: f64,
    pub coverage: f64,
    pub precision: f64,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct LexicalModel {
    pub schema: u32,
    pub training_rows: usize,
    pub skills: usize,
    pub experts: Vec<LexicalExpert>,
    /// Inverse document frequency per feature, so scoring the same features a
    /// training row used needs no corpus.
    pub idf: Vec<(String, f64)>,
    pub held_out: Option<HeldOut>,
    pub usable: bool,
    /// Words in the word-vector table this model was trained with. Zero means
    /// no embedding block, and a reader that cannot find the table for a model
    /// with rows refuses to score rather than scoring half the features.
    #[serde(default)]
    pub vector_rows: usize,
    /// Width of the sentence-encoder block this model was trained with. Zero
    /// means no encoder block, and like `vector_rows` it is checked at load so
    /// a model is never served with half its features missing.
    #[serde(default)]
    pub embedding_dim: usize,
    /// Mean encoder vector per class, when an encoder was present at training
    /// time. Empty means the encoder tier has nothing to say.
    #[serde(default)]
    pub centroids: Vec<ClassCentroid>,
}

/// A prompt with its weak label: a community tag mapped to a keel skill.
pub type LabelledRows = Vec<(String, String)>;

/// Raw rows as fetched, where an unlabelled row is allowed and then dropped.
pub type RawRows = [(String, Option<String>)];

/// One fetched row plus the provider it came from. An empty provider is an
/// old cache row that has not been attributed yet.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SourcedRow {
    pub text: String,
    pub skill: Option<String>,
    #[serde(default)]
    pub provider: String,
}

/// Words that mean "this crate is written in Rust", not which skill owns it.
const CRATES_BOILERPLATE: &[&str] = &["rust", "crate", "crates", "library", "libraries"];

type Weights = Vec<HashMap<String, f64>>;
type Accumulators = Vec<HashMap<String, f64>>;

struct Fitted {
    experts: Vec<LexicalExpert>,
    idf: Vec<(String, f64)>,
}

/// The optional blocks a document carries beyond its own words: the static
/// word-vector table and the sentence encoder. Both travel as one value so
/// training, pruning, held-out scoring and serving cannot disagree about which
/// blocks were used.
#[derive(Clone, Copy, Default)]
struct Priors<'a> {
    vectors: Option<&'a crate::utility::word_vectors::WordVectors>,
}

/// Label support per class, so a training run shows which skills have evidence
/// and which are running on fumes.
fn tally(labels: impl Iterator<Item = String>) -> Vec<(String, usize)> {
    let mut counts: Vec<(String, usize)> = Vec::new();
    for label in labels {
        match counts.iter().position(|(name, _)| name == &label) {
            Some(index) => counts[index].1 += 1,
            None => counts.push((label, 1)),
        }
    }
    counts.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
    counts
}

pub fn class_balance(rows: &RawRows) -> Vec<(String, usize)> {
    tally(rows.iter().filter_map(|(_, skill)| skill.clone()))
}

pub fn class_balance_sourced(rows: &[SourcedRow]) -> Vec<(String, usize)> {
    tally(rows.iter().filter_map(|row| row.skill.clone()))
}

/// Rows per provider, so a training run shows which source the evidence came from.
pub fn provider_counts(rows: &[SourcedRow]) -> Vec<(String, usize)> {
    tally(rows.iter().map(|row| {
        if row.provider.is_empty() {
            "untagged".to_string()
        } else {
            row.provider.clone()
        }
    }))
}

/// Train the experts. Returns `None` when the corpus cannot support a model at
/// all: a held-out score over a handful of rows would be a guess.
pub fn train(rows: &RawRows) -> Option<LexicalModel> {
    let sourced: Vec<SourcedRow> = rows
        .iter()
        .map(|(text, skill)| SourcedRow {
            text: text.clone(),
            skill: skill.clone(),
            provider: String::new(),
        })
        .collect();
    train_sourced(&sourced)
}

/// Train from rows that name their provider.
pub fn train_sourced(rows: &[SourcedRow]) -> Option<LexicalModel> {
    train_sourced_reported(rows, None, None).map(|(model, _)| model)
}

/// Train and report the calibration fit, so the run output shows the keep
/// decision instead of only the mapping that won.
pub fn train_sourced_reported(
    rows: &[SourcedRow],
    vectors: Option<&crate::utility::word_vectors::WordVectors>,
    encoder: Option<&crate::utility::embedding::Encoder>,
) -> Option<(LexicalModel, CalibrationFit)> {
    let priors = Priors { vectors };
    let (train_rows, validation_rows, test_rows) = split_sourced(rows, DEFAULT_SEED);
    // Rejected: masking crates boilerplate on this slice moved held-out
    // accuracy 0.7386 → 0.7285 and Brier 0.1380 → 0.1428 (accept 0.25 → 0.35).
    let train_pairs = pairs_of(&train_rows);
    let validation_pairs = pairs_of(&validation_rows);
    let test_pairs = pairs_of(&test_rows);
    // why: community tags carry label noise, and pruning the rows the fitted
    // model confidently contradicts lifted held-out accuracy in measurement.
    let (train_rows, _) = prune_label_noise(&train_pairs, priors);
    let fitted = fit(&train_rows, &validation_pairs, priors)?;
    let centroids = match encoder {
        Some(encoder) => build_centroids(&train_rows, encoder),
        None => Vec::new(),
    };
    let (held_out, report) = match score_held_out(
        &fitted,
        &validation_pairs,
        &test_pairs,
        vectors.map(|table| std::sync::Arc::new(table.clone())),
        encoder.map(|encoder| std::sync::Arc::new(encoder.clone())),
    ) {
        Some(scored) => (Some(scored.0), scored.1),
        None => (None, CalibrationFit::default()),
    };
    let usable = rows.len() >= REFUSE_BELOW_ROWS
        && held_out
            .as_ref()
            .is_some_and(|metrics| metrics.accuracy >= REFUSE_BELOW_ACCURACY);
    Some((
        LexicalModel {
            schema: LEXICAL_SCHEMA,
            training_rows: train_rows.len(),
            skills: fitted.experts.len(),
            experts: fitted.experts,
            idf: fitted.idf,
            held_out,
            usable,
            vector_rows: vectors.map(|table| table.rows()).unwrap_or(0),
            embedding_dim: if encoder.is_some() {
                crate::utility::embedding::EMBEDDING_DIM
            } else {
                0
            },
            centroids,
        },
        report,
    ))
}

/// A question carrying two mapped tags appears under both classes, which teaches
/// the model that one row owns two skills. Keep single-tag rows only, and report
/// how many were dropped so the loss is visible rather than silent.
pub fn drop_ambiguous_tags(rows: &RawRows) -> (Vec<(String, Option<String>)>, usize) {
    let sourced: Vec<SourcedRow> = rows
        .iter()
        .map(|(text, skill)| SourcedRow {
            text: text.clone(),
            skill: skill.clone(),
            provider: String::new(),
        })
        .collect();
    let (kept, dropped) = drop_ambiguous_sourced(&sourced);
    (
        kept.into_iter().map(|row| (row.text, row.skill)).collect(),
        dropped,
    )
}

/// Keep unlabelled rows. Drop a labelled row whose skill is not installed.
pub fn drop_uninstalled_skills(
    rows: &[SourcedRow],
    installed: &dyn Fn(&str) -> bool,
) -> (Vec<SourcedRow>, usize) {
    let mut dropped = 0usize;
    let kept = rows
        .iter()
        .filter(|row| match row.skill.as_deref() {
            Some(skill) if !installed(skill) => {
                dropped += 1;
                false
            }
            _ => true,
        })
        .cloned()
        .collect();
    (kept, dropped)
}

pub fn drop_ambiguous_sourced(rows: &[SourcedRow]) -> (Vec<SourcedRow>, usize) {
    let mut owners: HashMap<String, Vec<&str>> = HashMap::new();
    for row in rows {
        if let Some(skill) = row.skill.as_deref() {
            let entry = owners.entry(row.text.clone()).or_default();
            if !entry.contains(&skill) {
                entry.push(skill);
            }
        }
    }
    let ambiguous: Vec<String> = owners
        .into_iter()
        .filter(|(_, skills)| skills.len() > 1)
        .map(|(title, _)| title)
        .collect();
    let kept: Vec<SourcedRow> = rows
        .iter()
        .filter(|row| !ambiguous.iter().any(|title| title == &row.text))
        .cloned()
        .collect();
    let dropped = rows.len().saturating_sub(kept.len());
    (kept, dropped)
}

fn shuffle<T>(labelled: &mut [T], seed: u64) {
    let mut state = seed | 1;
    for index in (1..labelled.len()).rev() {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let swap = (state >> 33) as usize % (index + 1);
        labelled.swap(index, swap);
    }
}

fn split_counts(len: usize) -> (usize, usize) {
    let test_count = len / 5;
    // why: the accept point is fitted here, and a tenth of the corpus was thin
    // enough that the fitted floor moved on noise alone.
    let validation_count = len * 3 / 20;
    (test_count, validation_count)
}

fn split_three<T>(mut labelled: Vec<T>, seed: u64) -> (Vec<T>, Vec<T>, Vec<T>) {
    shuffle(&mut labelled, seed);
    let (test_count, validation_count) = split_counts(labelled.len());
    let test = labelled.split_off(labelled.len() - test_count);
    let validation = labelled.split_off(labelled.len() - validation_count);
    (labelled, validation, test)
}

fn split_sourced(
    rows: &[SourcedRow],
    seed: u64,
) -> (Vec<SourcedRow>, Vec<SourcedRow>, Vec<SourcedRow>) {
    let labelled: Vec<SourcedRow> = rows
        .iter()
        .filter(|row| row.skill.is_some())
        .cloned()
        .collect();
    split_three(labelled, seed)
}

fn pairs_of(rows: &[SourcedRow]) -> LabelledRows {
    rows.iter()
        .filter_map(|row| Some((row.text.clone(), row.skill.clone()?)))
        .collect()
}

/// Drop language boilerplate from a crates.io description. `cargo` stays: it is
/// the dependency skill's own word, not the language the crate is written in.
pub fn mask_crates_boilerplate(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for word in text.split_inclusive(|character: char| !character.is_ascii_alphanumeric()) {
        let core_len = word
            .trim_end_matches(|character: char| !character.is_ascii_alphanumeric())
            .len();
        let (core, tail) = word.split_at(core_len);
        if CRATES_BOILERPLATE.contains(&core.to_ascii_lowercase().as_str()) {
            out.push(' ');
            out.push_str(tail);
        } else {
            out.push_str(word);
        }
    }
    out
}

/// Words a row carries, before bigrams: the embedding block reads these.
fn words(prompt: &str) -> Vec<String> {
    prompt
        .to_ascii_lowercase()
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|word| word.len() >= MIN_WORD && !STOPWORDS.contains(word))
        .map(str::to_string)
        .collect()
}

/// Unigrams plus adjacent bigrams: a bigram is what separates `borrow checker`
/// from a page that merely mentions both words.
fn features(prompt: &str) -> Vec<String> {
    let words = words(prompt);
    let mut features: Vec<String> = words.clone();
    for pair in words.windows(2) {
        features.push(format!("{}_{}", pair[0], pair[1]));
    }
    features
}

fn inverse_document_frequency(rows: &LabelledRows) -> Vec<(String, f64)> {
    let mut document_frequency: HashMap<String, usize> = HashMap::new();
    for (prompt, _) in rows {
        let mut seen: Vec<String> = features(prompt);
        seen.sort();
        seen.dedup();
        for feature in seen {
            *document_frequency.entry(feature).or_default() += 1;
        }
    }
    let total = rows.len() as f64;
    let mut idf: Vec<(String, f64)> = document_frequency
        .into_iter()
        .map(|(feature, count)| (feature, ((1.0 + total) / (1.0 + count as f64)).ln() + 1.0))
        .collect();
    idf.sort_by(|left, right| left.0.cmp(&right.0));
    idf
}

/// Sublinear term frequency against the fitted document frequencies, plus the
/// averaged word-vector block and the encoder block, L2 normalized once over the
/// whole document so a long title cannot outvote a short one and no block
/// outweighs another.
fn vectorize(
    prompt: &str,
    idf: &HashMap<String, f64>,
    vectors: Option<&crate::utility::word_vectors::WordVectors>,
) -> HashMap<String, f64> {
    let mut counts: HashMap<String, f64> = HashMap::new();
    for feature in features(prompt) {
        if idf.contains_key(&feature) {
            *counts.entry(feature).or_default() += 1.0;
        }
    }
    let mut vector: HashMap<String, f64> = counts
        .into_iter()
        .map(|(feature, count)| (feature.clone(), (1.0 + count.ln()) * idf[&feature]))
        .collect();
    // why: the block carries words the idf never saw, which is the whole point
    // of a prior; without a table this is exactly the model it was before.
    if let Some(embedding) =
        vectors.and_then(|table| table.average(words(prompt).iter().map(String::as_str)))
    {
        for (index, value) in embedding.iter().enumerate() {
            vector.insert(
                format!(
                    "{}{index}",
                    crate::utility::word_vectors::VECTOR_FEATURE_PREFIX
                ),
                *value,
            );
        }
    }
    let norm = vector
        .values()
        .map(|value| value * value)
        .sum::<f64>()
        .sqrt();
    if norm > 0.0 {
        for value in vector.values_mut() {
            *value /= norm;
        }
    }
    vector
}

/// Validation Brier at the untempered scale, used only to choose the epoch. The
/// reported Brier is still the test split's.
fn validation_brier(
    validation: &[(usize, HashMap<String, f64>)],
    weights: &Weights,
    biases: &[f64],
    classes: usize,
) -> f64 {
    let mut total = 0.0;
    let mut counted = 0usize;
    for (label, document) in validation {
        if document.is_empty() {
            continue;
        }
        let logits: Vec<f64> = (0..classes)
            .map(|class| biases[class] + dot(&weights[class], document))
            .collect();
        let probabilities = softmax(&logits, 1.0);
        total += (1.0 - probabilities[*label]).powi(2);
        counted += 1;
    }
    if counted == 0 {
        f64::MAX
    } else {
        total / counted as f64
    }
}

fn fit(rows: &LabelledRows, validation_rows: &LabelledRows, priors: Priors<'_>) -> Option<Fitted> {
    if rows.is_empty() {
        return None;
    }
    let idf_pairs = inverse_document_frequency(rows);
    let idf: HashMap<String, f64> = idf_pairs.iter().cloned().collect();
    let mut classes: Vec<String> = rows.iter().map(|(_, skill)| skill.clone()).collect();
    classes.sort();
    classes.dedup();
    if classes.is_empty() {
        return None;
    }
    let class_index: HashMap<String, usize> = classes
        .iter()
        .enumerate()
        .map(|(index, name)| (name.clone(), index))
        .collect();
    let documents: Vec<(usize, HashMap<String, f64>)> = rows
        .iter()
        .map(|(prompt, skill)| (class_index[skill], vectorize(prompt, &idf, priors.vectors)))
        .collect();

    let mut weights: Weights = vec![HashMap::new(); classes.len()];
    let mut accumulators: Accumulators = vec![HashMap::new(); classes.len()];
    // Class priors start in the bias, so an over-sampled tag does not win by count.
    let mut biases: Vec<f64> = classes
        .iter()
        .map(|name| {
            let count = rows.iter().filter(|(_, skill)| skill == name).count() as f64;
            (count / rows.len() as f64).max(1e-6).ln()
        })
        .collect();

    let mut order: Vec<usize> = (0..documents.len()).collect();
    let mut state = DEFAULT_SEED | 1;
    let validation: Vec<(usize, HashMap<String, f64>)> = validation_rows
        .iter()
        .filter_map(|(prompt, skill)| {
            Some((
                *class_index.get(skill)?,
                vectorize(prompt, &idf, priors.vectors),
            ))
        })
        .collect();
    let mut best_brier = f64::MAX;
    let mut best_weights = weights.clone();
    let mut best_biases = biases.clone();
    let mut stale = 0usize;
    for _ in 0..EPOCHS {
        for index in (1..order.len()).rev() {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let swap = (state >> 33) as usize % (index + 1);
            order.swap(index, swap);
        }
        for position in &order {
            let (label, document) = &documents[*position];
            let logits: Vec<f64> = (0..classes.len())
                .map(|class| biases[class] + dot(&weights[class], document))
                .collect();
            let probabilities = softmax(&logits, 1.0);
            for class in 0..classes.len() {
                let observed = (class == *label) as u8 as f64;
                let gradient = probabilities[class] - observed;
                biases[class] -= LEARNING_RATE * gradient;
                for (feature, value) in document {
                    let step = LEARNING_RATE * gradient * value;
                    let slot = accumulators[class].entry(feature.clone()).or_default();
                    *slot += step * step;
                    let update = step / (slot.sqrt() + ADAGRAD_EPSILON);
                    let weight = weights[class].entry(feature.clone()).or_default();
                    *weight -= update;
                }
            }
        }
        for class_weights in weights.iter_mut() {
            for weight in class_weights.values_mut() {
                *weight *= 1.0 - LEARNING_RATE * WEIGHT_DECAY;
            }
        }
        // why: a fixed epoch count trains past the point where the validation
        // slice improves, so the best epoch is kept rather than the last one.
        if !validation.is_empty() {
            let brier = validation_brier(&validation, &weights, &biases, classes.len());
            if brier < best_brier - 1e-6 {
                best_brier = brier;
                best_weights = weights.clone();
                best_biases = biases.clone();
                stale = 0;
            } else {
                stale += 1;
                if stale >= EARLY_STOP_PATIENCE {
                    break;
                }
            }
        }
    }
    let (weights, biases) = if validation.is_empty() {
        (weights, biases)
    } else {
        (best_weights, best_biases)
    };
    // Logit adjustment for imbalance: how much the priors should count is fitted
    // on validation instead of assumed, since the thin classes are what it moves.
    let priors: Vec<f64> = classes
        .iter()
        .map(|name| {
            (rows.iter().filter(|(_, skill)| skill == name).count() as f64 / rows.len() as f64)
                .max(1e-6)
                .ln()
        })
        .collect();
    let mut best_alpha = 0.0f64;
    if !validation.is_empty() {
        let mut best = f64::MAX;
        for step in 0..=8 {
            let alpha = step as f64 * 0.125;
            let adjusted: Vec<f64> = (0..classes.len())
                .map(|class| biases[class] + alpha * priors[class])
                .collect();
            let brier = validation_brier(&validation, &weights, &adjusted, classes.len());
            if brier < best {
                best = brier;
                best_alpha = alpha;
            }
        }
    }
    let biases: Vec<f64> = if validation.is_empty() {
        biases
    } else {
        (0..classes.len())
            .map(|class| biases[class] + best_alpha * priors[class])
            .collect()
    };

    let experts = classes
        .iter()
        .enumerate()
        .map(|(class, name)| {
            let mut terms: Vec<(String, f64)> = weights[class]
                .iter()
                .map(|(feature, weight)| (feature.clone(), *weight))
                .collect();
            terms.sort_by(|left, right| {
                right
                    .1
                    .abs()
                    .partial_cmp(&left.1.abs())
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            // why: a weight for every feature a class ever saw is millions of
            // near-zero numbers the router would parse on every prompt.
            terms.retain(|(_, weight)| weight.abs() >= MIN_TERM_WEIGHT);
            terms.truncate(MAX_TERMS_PER_EXPERT);
            LexicalExpert {
                name: name.clone(),
                bias: biases[class],
                terms,
            }
        })
        .collect();
    Some(Fitted {
        experts,
        idf: idf_pairs,
    })
}

/// A term whose weight cannot move a decision past a thousandth is weight the
/// router pays to parse on every prompt.
pub const MIN_TERM_WEIGHT: f64 = 1e-3;
/// Cap per expert, so one huge vocabulary cannot inflate the artifact without
/// bound. The retained terms are the largest by absolute weight.
pub const MAX_TERMS_PER_EXPERT: usize = 20_000;

/// Shrink an already fitted model in place. Returns the number of terms kept,
/// which the training header reports next to the artifact size.
pub fn prune_terms(model: &mut LexicalModel) -> usize {
    let mut kept = 0usize;
    for expert in &mut model.experts {
        expert
            .terms
            .retain(|(_, weight)| weight.abs() >= MIN_TERM_WEIGHT);
        expert.terms.truncate(MAX_TERMS_PER_EXPERT);
        kept += expert.terms.len();
    }
    kept
}

fn dot(weights: &HashMap<String, f64>, document: &HashMap<String, f64>) -> f64 {
    document
        .iter()
        .map(|(feature, value)| weights.get(feature).copied().unwrap_or(0.0) * value)
        .sum()
}

fn softmax(logits: &[f64], temperature: f64) -> Vec<f64> {
    let scaled: Vec<f64> = logits.iter().map(|logit| logit / temperature).collect();
    let top = scaled.iter().copied().fold(f64::MIN, f64::max);
    let exponents: Vec<f64> = scaled.iter().map(|value| (value - top).exp()).collect();
    let total: f64 = exponents.iter().sum();
    exponents.into_iter().map(|value| value / total).collect()
}

/// Feature-major weights, built once per model. Scoring must never rebuild a
/// map per row: the temperature search alone scores every held-out row 40 times.
struct Scorer {
    names: Vec<String>,
    biases: Vec<f64>,
    weights: HashMap<String, Vec<f64>>,
    idf: HashMap<String, f64>,
    vectors: Option<std::sync::Arc<crate::utility::word_vectors::WordVectors>>,
}

impl Scorer {
    fn new(
        model: &LexicalModel,
        vectors: Option<std::sync::Arc<crate::utility::word_vectors::WordVectors>>,
    ) -> Self {
        let mut weights: HashMap<String, Vec<f64>> = HashMap::new();
        for (class, expert) in model.experts.iter().enumerate() {
            for (feature, weight) in &expert.terms {
                let row = weights
                    .entry(feature.clone())
                    .or_insert_with(|| vec![0.0; model.experts.len()]);
                row[class] = *weight;
            }
        }
        Self {
            names: model
                .experts
                .iter()
                .map(|expert| expert.name.clone())
                .collect(),
            biases: model.experts.iter().map(|expert| expert.bias).collect(),
            weights,
            idf: model.idf.iter().cloned().collect(),
            vectors,
        }
    }

    fn vectorize(&self, prompt: &str) -> HashMap<String, f64> {
        vectorize(prompt, &self.idf, self.vectors.as_deref())
    }

    fn probabilities(&self, document: &HashMap<String, f64>, temperature: f64) -> Vec<f64> {
        let logits: Vec<f64> = (0..self.names.len())
            .map(|class| {
                self.biases[class]
                    + document
                        .iter()
                        .map(|(feature, value)| {
                            self.weights
                                .get(feature)
                                .map(|row| row[class] * value)
                                .unwrap_or(0.0)
                        })
                        .sum::<f64>()
            })
            .collect();
        softmax(&logits, temperature)
    }
}

fn best_of(probabilities: &[f64]) -> (usize, f64) {
    probabilities
        .iter()
        .enumerate()
        .max_by(|left, right| {
            left.1
                .partial_cmp(right.1)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(index, probability)| (index, *probability))
        .unwrap_or((0, 0.0))
}

/// Shannon entropy in nats. High entropy means the head is spread thin.
fn entropy(probabilities: &[f64]) -> f64 {
    probabilities
        .iter()
        .filter(|probability| **probability > 0.0)
        .map(|probability| -probability * probability.ln())
        .sum()
}

/// Temperature applied after softmax: sharpen or soften without refitting.
/// A bad temperature reads as no change, never as a wrong probability.
fn rescale(probabilities: &[f64], temperature: f64) -> Vec<f64> {
    if !temperature.is_finite() || temperature <= 0.0 {
        return probabilities.to_vec();
    }
    let powered: Vec<f64> = probabilities
        .iter()
        .map(|probability| probability.powf(1.0 / temperature))
        .collect();
    let total: f64 = powered.iter().sum();
    if !total.is_finite() || total <= 0.0 {
        return probabilities.to_vec();
    }
    powered.into_iter().map(|value| value / total).collect()
}

/// The fitted temperature for this entropy, or 1.0 when no bin claims it.
fn entropy_temperature(entropy: f64, bins: &[(f64, f64)]) -> f64 {
    if !entropy.is_finite() {
        return 1.0;
    }
    bins.iter()
        .find(|(edge, _)| entropy <= *edge)
        .map(|(_, temperature)| *temperature)
        .unwrap_or(1.0)
}

/// Static probabilities when no bins are fitted, adaptive ones otherwise.
fn probabilities_for(
    scorer: &Scorer,
    document: &HashMap<String, f64>,
    scale: f64,
    bins: &[(f64, f64)],
) -> Vec<f64> {
    let probabilities = scorer.probabilities(document, scale);
    if bins.is_empty() {
        return probabilities;
    }
    let temperature = entropy_temperature(entropy(&probabilities), bins);
    rescale(&probabilities, temperature)
}

/// Best skill and its temperature-calibrated confidence. `None` when the prompt
/// shares no feature with the training rows: silence beats a coin flip.
pub fn predict(model: &LexicalModel, prompt: &str) -> Option<(String, f64)> {
    rank(model, prompt).and_then(|ranked| ranked.into_iter().next())
}

/// Every class probability, highest first. Empty when the prompt shares no
/// feature with the training rows.
pub fn rank(model: &LexicalModel, prompt: &str) -> Option<Vec<(String, f64)>> {
    Head::from_model(model, None).rank(prompt)
}

/// A model plus everything the router needs to score a prompt: the fitted
/// temperature, the entropy bins, the accept point, and the feature-major index
/// that scoring reads.
pub struct Head {
    scorer: Scorer,
    scale: f64,
    bins: Vec<(f64, f64)>,
    accept: f64,
}

impl Head {
    fn from_model(
        model: &LexicalModel,
        vectors: Option<std::sync::Arc<crate::utility::word_vectors::WordVectors>>,
    ) -> Self {
        let (scale, bins, accept) = match model.held_out.as_ref() {
            Some(metrics) => (
                metrics.scale,
                metrics.entropy_temperatures.clone(),
                metrics.accept,
            ),
            None => (1.0, Vec::new(), 1.0),
        };
        Self {
            scorer: Scorer::new(model, vectors),
            scale,
            bins,
            accept,
        }
    }

    /// Every class probability, highest first, or `None` when the prompt shares
    /// no feature with the training rows.
    pub fn rank(&self, prompt: &str) -> Option<Vec<(String, f64)>> {
        let document = self.scorer.vectorize(prompt);
        if document.is_empty() {
            return None;
        }
        let mut ranked: Vec<(String, f64)> = self
            .scorer
            .names
            .iter()
            .cloned()
            .zip(probabilities_for(
                &self.scorer,
                &document,
                self.scale,
                &self.bins,
            ))
            .collect();
        ranked.sort_by(|left, right| {
            right
                .1
                .partial_cmp(&left.1)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Some(ranked)
    }

    /// Lowest confidence that held the precision floor on the training split.
    pub fn accept(&self) -> f64 {
        self.accept
    }
}

/// The router consults the head on every prompt. Parsing the artifact and
/// building the feature-major index costs more than the scoring does, and the
/// file only changes when a training run writes it, so the built head is cached
/// under the file's own fingerprint: same file, same head.
type HeadCache = Option<(String, std::sync::Arc<Head>)>;
static HEAD_CACHE: std::sync::LazyLock<std::sync::Mutex<HeadCache>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(None));

/// The cached head for an artifact path, rebuilt when the file changes. `None`
/// means no usable artifact, which reads as "no learned evidence" upstream.
pub fn head(path: &Path) -> Option<std::sync::Arc<Head>> {
    let fingerprint = head_fingerprint(path)?;
    if let Ok(cache) = HEAD_CACHE.lock() {
        if let Some((cached, head)) = cache.as_ref() {
            if cached == &fingerprint {
                return Some(std::sync::Arc::clone(head));
            }
        }
    }
    let model = load(path)?;
    // why: a model trained with the embedding block needs the same table to
    // score; serving it without one would score half the features.
    let vectors = if model.vector_rows > 0 {
        Some(crate::utility::word_vectors::table_beside(path)?)
    } else {
        None
    };
    // why: the centroids need the same encoder the training rows were encoded
    // with, so a model carrying them refuses to serve without it.
    if model.embedding_dim > 0 && crate::utility::embedding::encoder_beside(path).is_none() {
        return None;
    }
    let head = std::sync::Arc::new(Head::from_model(&model, vectors));
    if let Ok(mut cache) = HEAD_CACHE.lock() {
        *cache = Some((fingerprint, std::sync::Arc::clone(&head)));
    }
    Some(head)
}

/// The file's identity: path, length, modification time. A fingerprint without
/// a timestamp is weaker but still usable, so an unavailable mtime is zero
/// rather than a reason to stop serving the head.
fn head_fingerprint(path: &Path) -> Option<String> {
    // why: an unreadable path is the absence of a head, not an error to report.
    let metadata = std::fs::metadata(path).ok()?;
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    Some(format!(
        "{}:{}:{}",
        path.display(),
        metadata.len(),
        modified
    ))
}

/// Highest class that is a real keel skill and still clears the accept point.
/// A phantom class can win the head and must not erase a skill that cleared it.
pub fn select_installed<'a>(
    ranked: &'a [(String, f64)],
    accept: f64,
    installed: &dyn Fn(&str) -> bool,
) -> Option<(&'a str, f64)> {
    ranked
        .iter()
        .find(|(name, probability)| *probability >= accept && installed(name))
        .map(|(name, probability)| (name.as_str(), *probability))
}

/// The confidence the held-out split supports. With no held-out split this is
/// 1.0, so an unfitted model refuses everything instead of guessing.
pub fn accept_threshold(model: &LexicalModel) -> f64 {
    model
        .held_out
        .as_ref()
        .map(|metrics| metrics.accept)
        .unwrap_or(1.0)
}

/// One class's mean sentence vector. The encoder speaks through these rather
/// than through the linear head: as 384 dense features per row inside the class
/// loop the block multiplied training cost by ten, and a centroid costs one
/// encode per training row and one per prompt.
#[derive(Clone, Serialize, Deserialize)]
pub struct ClassCentroid {
    pub name: String,
    pub vector: Vec<f32>,
}

/// Mean encoder vector per class, unit-normalized. Empty when no encoder was
/// available at training time.
pub fn build_centroids(
    rows: &LabelledRows,
    encoder: &crate::utility::embedding::Encoder,
) -> Vec<ClassCentroid> {
    let mut sums: HashMap<String, (Vec<f64>, usize)> = HashMap::new();
    for (prompt, skill) in rows {
        let Some(embedding) = encoder.encode(prompt) else {
            continue;
        };
        let entry = sums
            .entry(skill.clone())
            .or_insert_with(|| (vec![0.0; embedding.len()], 0));
        if entry.0.len() != embedding.len() {
            continue;
        }
        for (slot, value) in entry.0.iter_mut().zip(&embedding) {
            *slot += f64::from(*value);
        }
        entry.1 += 1;
    }
    let mut centroids: Vec<ClassCentroid> = sums
        .into_iter()
        .filter(|(_, (_, count))| *count > 0)
        .map(|(name, (sum, count))| {
            let mut vector: Vec<f32> = sum
                .into_iter()
                .map(|value| (value / count as f64) as f32)
                .collect();
            let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
            if norm > 0.0 {
                for value in &mut vector {
                    *value /= norm;
                }
            }
            ClassCentroid { name, vector }
        })
        .collect();
    centroids.sort_by(|left, right| left.name.cmp(&right.name));
    centroids
}

/// Classes ranked by cosine similarity to the prompt's encoder vector. `None`
/// when no centroid table or no encoder is available: silence, not a guess.
pub fn centroid_rank(
    centroids: &[ClassCentroid],
    encoder: &crate::utility::embedding::Encoder,
    prompt: &str,
) -> Option<Vec<(String, f64)>> {
    if centroids.is_empty() {
        return None;
    }
    let embedding = encoder.encode(prompt)?;
    let mut ranked: Vec<(String, f64)> = centroids
        .iter()
        .map(|centroid| {
            let dot: f64 = centroid
                .vector
                .iter()
                .zip(&embedding)
                .map(|(left, right)| f64::from(*left) * f64::from(*right))
                .sum();
            (centroid.name.clone(), dot.max(0.0))
        })
        .collect();
    ranked.sort_by(|left, right| {
        right
            .1
            .partial_cmp(&left.1)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Some(ranked)
}

/// The accept point for one class, falling back to the global one for a class
/// the held-out split could not score.
pub fn accept_threshold_for(model: &LexicalModel, skill: &str) -> f64 {
    let held_out = match model.held_out.as_ref() {
        Some(metrics) => metrics,
        None => return 1.0,
    };
    held_out
        .accept_per_skill
        .iter()
        .find(|(name, _)| name == skill)
        .map(|(_, accept)| *accept)
        .unwrap_or(held_out.accept)
}

/// One row's out-of-fold judgement: the class the fold model chose, how sure it
/// was, and what it gave the tag the row already claims.
struct OutOfFold {
    best: usize,
    confidence: f64,
    given: usize,
    given_probability: f64,
}

/// Confident Learning on weak labels: judge every row with a fold model that
/// never saw it, then drop rows whose given tag that model confidently
/// contradicts. In-sample judging finds nothing, because a fitted model agrees
/// with the rows it memorized, so the folds are what make this worth doing.
fn prune_label_noise(rows: &LabelledRows, priors: Priors<'_>) -> (LabelledRows, usize) {
    const FOLDS: usize = 5;
    if rows.len() < FOLDS {
        return (rows.clone(), 0);
    }
    let mut judgements: Vec<Option<OutOfFold>> = Vec::new();
    judgements.resize_with(rows.len(), || None);
    for fold in 0..FOLDS {
        let training: LabelledRows = rows
            .iter()
            .enumerate()
            .filter(|(index, _)| index % FOLDS != fold)
            .map(|(_, row)| row.clone())
            .collect();
        let Some(fitted) = fit(&training, &LabelledRows::new(), priors) else {
            return (rows.clone(), 0);
        };
        let model = LexicalModel {
            schema: LEXICAL_SCHEMA,
            training_rows: training.len(),
            skills: fitted.experts.len(),
            experts: fitted.experts,
            idf: fitted.idf,
            held_out: None,
            usable: true,
            vector_rows: 0,
            embedding_dim: 0,
            centroids: Vec::new(),
        };
        let scorer = Scorer::new(&model, None);
        for (index, (prompt, skill)) in rows.iter().enumerate() {
            if index % FOLDS != fold {
                continue;
            }
            let document = scorer.vectorize(prompt);
            if document.is_empty() {
                continue;
            }
            let Some(given) = scorer.names.iter().position(|name| name == skill) else {
                continue;
            };
            let probabilities = scorer.probabilities(&document, 1.0);
            let (best, confidence) = best_of(&probabilities);
            judgements[index] = Some(OutOfFold {
                best,
                confidence,
                given,
                given_probability: probabilities[given],
            });
        }
    }
    let mut self_confidence: HashMap<&str, (f64, usize)> = HashMap::new();
    for ((_, skill), judgement) in rows.iter().zip(&judgements) {
        if let Some(judgement) = judgement {
            let entry = self_confidence.entry(skill.as_str()).or_default();
            entry.0 += judgement.given_probability;
            entry.1 += 1;
        }
    }
    let mut kept = Vec::with_capacity(rows.len());
    let mut pruned = 0usize;
    for ((prompt, skill), judgement) in rows.iter().zip(&judgements) {
        let suspect = judgement.as_ref().is_some_and(|judgement| {
            let mean = self_confidence
                .get(skill.as_str())
                .map(|(sum, count)| sum / (*count).max(1) as f64)
                .unwrap_or(0.0);
            judgement.best != judgement.given && judgement.confidence > mean
        });
        if suspect {
            pruned += 1;
        } else {
            kept.push((prompt.clone(), skill.clone()));
        }
    }
    (kept, pruned)
}

/// Fit the operating point on the validation rows, then report on the test rows.
/// They must not be the same set: a threshold chosen on the rows it is scored on
/// picks itself, and the number that comes out is not a held-out score.
fn score_held_out(
    fitted: &Fitted,
    calibration_rows: &LabelledRows,
    test_rows: &LabelledRows,
    vectors: Option<std::sync::Arc<crate::utility::word_vectors::WordVectors>>,
    encoder: Option<std::sync::Arc<crate::utility::embedding::Encoder>>,
) -> Option<(HeldOut, CalibrationFit)> {
    if test_rows.is_empty() {
        return None;
    }
    let model = LexicalModel {
        schema: LEXICAL_SCHEMA,
        training_rows: 0,
        skills: fitted.experts.len(),
        experts: fitted.experts.clone(),
        idf: fitted.idf.clone(),
        held_out: None,
        usable: true,
        vector_rows: vectors.as_ref().map(|table| table.rows()).unwrap_or(0),
        embedding_dim: encoder
            .as_ref()
            .map(|_| crate::utility::embedding::EMBEDDING_DIM)
            .unwrap_or(0),
        centroids: Vec::new(),
    };
    let scorer = Scorer::new(&model, vectors);
    let vectorize = |rows: &LabelledRows| -> Vec<(String, HashMap<String, f64>)> {
        rows.iter()
            .map(|(prompt, skill)| (skill.clone(), scorer.vectorize(prompt)))
            .collect()
    };
    let calibration = vectorize(calibration_rows);
    let documents = vectorize(test_rows);
    let mut best_scale = 1.0f64;
    let mut best_brier = f64::MAX;
    if calibration.iter().any(|(_, document)| !document.is_empty()) {
        for step in 1..=40 {
            let scale = step as f64 * 0.05;
            let brier = brier_for_documents(&scorer, &calibration, scale, &[]);
            if brier < best_brier {
                best_scale = scale;
                best_brier = brier;
            }
        }
    }
    let candidate = fit_entropy_temperatures(&scorer, &calibration, best_scale);
    let static_fit = fit_accept(&scorer, &calibration, best_scale, &[]);
    let adaptive_fit = fit_accept(&scorer, &calibration, best_scale, &candidate);
    let static_ece = ece_for_documents(&scorer, &calibration, best_scale, &[]);
    let adaptive_ece = ece_for_documents(&scorer, &calibration, best_scale, &candidate);
    let static_coverage = coverage_for(&scorer, &calibration, best_scale, &[], static_fit.accept);
    let adaptive_coverage = coverage_for(
        &scorer,
        &calibration,
        best_scale,
        &candidate,
        adaptive_fit.accept,
    );
    let kept = !candidate.is_empty()
        && keep_adaptive(static_ece, static_coverage, adaptive_ece, adaptive_coverage);
    let adaptive_test_brier = brier_for_documents(&scorer, &documents, best_scale, &candidate);
    let adaptive_test_ece = ece_for_documents(&scorer, &documents, best_scale, &candidate);
    let bins: Vec<(f64, f64)> = if kept { candidate } else { Vec::new() };
    let fit = if kept { adaptive_fit } else { static_fit };
    let mut correct = 0usize;
    for (skill, document) in &documents {
        if document.is_empty() {
            continue;
        }
        let (best, _) = best_of(&probabilities_for(&scorer, document, best_scale, &bins));
        if &scorer.names[best] == skill {
            correct += 1;
        }
    }
    let decided = documents
        .iter()
        .filter(|(_, document)| !document.is_empty())
        .count();
    let held_out = HeldOut {
        rows: test_rows.len(),
        decided,
        correct,
        accuracy: correct as f64 / test_rows.len() as f64,
        brier: brier_for_documents(&scorer, &documents, best_scale, &bins),
        scale: best_scale,
        accept: fit.accept,
        accept_per_skill: fit.accept_per_skill,
        operating_points: fit.operating_points,
        ece: ece_for_documents(&scorer, &documents, best_scale, &bins),
        entropy_temperatures: bins,
    };
    let report = CalibrationFit {
        static_validation_ece: static_ece,
        static_validation_coverage: static_coverage,
        adaptive_validation_ece: adaptive_ece,
        adaptive_validation_coverage: adaptive_coverage,
        static_test_brier: brier_for_documents(&scorer, &documents, best_scale, &[]),
        static_test_ece: ece_for_documents(&scorer, &documents, best_scale, &[]),
        adaptive_test_brier,
        adaptive_test_ece,
        kept_adaptive: kept,
    };
    Some((held_out, report))
}

/// The accept point and its trade, fitted under one temperature mapping.
struct AcceptFit {
    accept: f64,
    operating_points: Vec<OperatingPoint>,
    accept_per_skill: Vec<(String, f64)>,
}

fn fit_accept(
    scorer: &Scorer,
    calibration: &[(String, HashMap<String, f64>)],
    scale: f64,
    bins: &[(f64, f64)],
) -> AcceptFit {
    // Fitted, never assumed: the lowest confidence whose precision clears the
    // floor, because a hand-picked 0.80 threw most correct answers away.
    const PRECISION_FLOOR: f64 = 0.75;
    let mut accept = 1.0f64;
    let mut operating_points = Vec::new();
    for step in 1..=19 {
        let threshold = step as f64 * 0.05;
        let mut answered = 0usize;
        let mut right = 0usize;
        for (skill, document) in calibration {
            if document.is_empty() {
                continue;
            }
            let (best, confidence) = best_of(&probabilities_for(scorer, document, scale, bins));
            if confidence < threshold {
                continue;
            }
            answered += 1;
            if &scorer.names[best] == skill {
                right += 1;
            }
        }
        let coverage = answered as f64 / calibration.len().max(1) as f64;
        let precision = if answered == 0 {
            0.0
        } else {
            right as f64 / answered as f64
        };
        operating_points.push(OperatingPoint {
            threshold,
            coverage,
            precision,
        });
        if precision >= PRECISION_FLOOR && accept == 1.0 {
            accept = threshold;
        }
    }
    let mut accept_per_skill: Vec<(String, f64)> = Vec::new();
    for (class, name) in scorer.names.iter().enumerate() {
        let mut accept_for_class = 1.0f64;
        for step in 1..=19 {
            let threshold = step as f64 * 0.05;
            let mut answered = 0usize;
            let mut right = 0usize;
            for (skill, document) in calibration {
                if document.is_empty() {
                    continue;
                }
                let (best, confidence) = best_of(&probabilities_for(scorer, document, scale, bins));
                if best != class || confidence < threshold {
                    continue;
                }
                answered += 1;
                if skill == name {
                    right += 1;
                }
            }
            let precision = if answered == 0 {
                0.0
            } else {
                right as f64 / answered as f64
            };
            if answered > 0 && precision >= PRECISION_FLOOR && accept_for_class == 1.0 {
                accept_for_class = threshold;
            }
        }
        accept_per_skill.push((name.clone(), accept_for_class));
    }
    AcceptFit {
        accept,
        operating_points,
        accept_per_skill,
    }
}

/// One temperature per entropy quartile, fitted by bin Brier on validation.
/// Thin bins keep 1.0: a temperature fitted on a handful of rows is noise.
fn fit_entropy_temperatures(
    scorer: &Scorer,
    calibration: &[(String, HashMap<String, f64>)],
    scale: f64,
) -> Vec<(f64, f64)> {
    const BINS: usize = 4;
    const MIN_BIN_ROWS: usize = 50;
    let rows: Vec<(String, Vec<f64>, f64)> = calibration
        .iter()
        .filter(|(_, document)| !document.is_empty())
        .map(|(skill, document)| {
            let probabilities = scorer.probabilities(document, scale);
            let value = entropy(&probabilities);
            (skill.clone(), probabilities, value)
        })
        .collect();
    if rows.len() < MIN_BIN_ROWS * BINS {
        return Vec::new();
    }
    let mut sorted: Vec<f64> = rows.iter().map(|(_, _, value)| *value).collect();
    sorted.sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
    let mut fitted = Vec::with_capacity(BINS);
    let mut lower = f64::MIN;
    for bin in 1..=BINS {
        let edge = sorted[(bin * sorted.len() / BINS).min(sorted.len() - 1)];
        let members: Vec<(&String, &Vec<f64>)> = rows
            .iter()
            .filter(|(_, _, value)| *value > lower && *value <= edge)
            .map(|(skill, probabilities, _)| (skill, probabilities))
            .collect();
        lower = edge;
        if members.len() < MIN_BIN_ROWS {
            fitted.push((edge, 1.0));
            continue;
        }
        let mut best_temperature = 1.0f64;
        let mut best = f64::MAX;
        for step in 1..=30 {
            let temperature = step as f64 * 0.1;
            let brier = bin_brier(&members, &scorer.names, temperature);
            if brier < best {
                best = brier;
                best_temperature = temperature;
            }
        }
        fitted.push((edge, best_temperature));
    }
    fitted
}

/// Top-1 Brier over bin members at one candidate temperature.
fn bin_brier(rows: &[(&String, &Vec<f64>)], names: &[String], temperature: f64) -> f64 {
    let mut sum = 0.0;
    for (skill, probabilities) in rows {
        let (best, confidence) = best_of(&rescale(probabilities, temperature));
        let correct = (&names[best] == *skill) as u8 as f64;
        sum += (confidence - correct).powi(2);
    }
    sum / rows.len() as f64
}

/// Pre-registered keep rule: adaptive ships only when validation ECE falls
/// without coverage collapsing. Selection reads validation; test only reports.
fn keep_adaptive(
    static_ece: f64,
    static_coverage: f64,
    adaptive_ece: f64,
    adaptive_coverage: f64,
) -> bool {
    const MAX_COVERAGE_LOSS: f64 = 0.05;
    adaptive_ece < static_ece && adaptive_coverage >= static_coverage - MAX_COVERAGE_LOSS
}

/// Fraction of rows speaking at or above the fitted accept point.
fn coverage_for(
    scorer: &Scorer,
    calibration: &[(String, HashMap<String, f64>)],
    scale: f64,
    bins: &[(f64, f64)],
    accept: f64,
) -> f64 {
    if calibration.is_empty() {
        return 0.0;
    }
    let answered = calibration
        .iter()
        .filter(|(_, document)| {
            !document.is_empty()
                && best_of(&probabilities_for(scorer, document, scale, bins)).1 >= accept
        })
        .count();
    answered as f64 / calibration.len() as f64
}

/// ECE over decided rows: stated confidence against what actually happened.
fn ece_for_documents(
    scorer: &Scorer,
    documents: &[(String, HashMap<String, f64>)],
    scale: f64,
    bins: &[(f64, f64)],
) -> f64 {
    let pairs: Vec<(f64, bool)> = documents
        .iter()
        .filter(|(_, document)| !document.is_empty())
        .map(|(skill, document)| {
            let (best, confidence) = best_of(&probabilities_for(scorer, document, scale, bins));
            (confidence, &scorer.names[best] == skill)
        })
        .collect();
    crate::utility::decision_model::expected_calibration_error(&pairs)
}

/// Brier over the rows the model actually answered: a silent row has no stated
/// probability to score, so it is reported through accuracy and coverage instead.
fn brier_for_documents(
    scorer: &Scorer,
    documents: &[(String, HashMap<String, f64>)],
    scale: f64,
    bins: &[(f64, f64)],
) -> f64 {
    let mut sum = 0.0;
    let mut scored = 0usize;
    for (skill, document) in documents {
        if document.is_empty() {
            continue;
        }
        let (best, confidence) = best_of(&probabilities_for(scorer, document, scale, bins));
        let correct = (&scorer.names[best] == skill) as u8 as f64;
        sum += (confidence - correct).powi(2);
        scored += 1;
    }
    if scored == 0 {
        return f64::MAX;
    }
    sum / scored as f64
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
    const PHANTOM_LABEL: &str = "rust";
    const POSTGRES_LABEL: &str = "postgres-migration-safety";
    const BIN_A: &str = "alpha";
    const BIN_B: &str = "beta";
    const BIN_FEATURE_A: &str = "aaa";
    const BIN_FEATURE_B: &str = "bbb";
    fn sourced_row(text: &str, skill: Option<&str>) -> SourcedRow {
        SourcedRow {
            text: text.to_string(),
            skill: skill.map(str::to_string),
            provider: String::new(),
        }
    }

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
    fn features_carry_unigrams_and_bigrams_without_question_words() {
        let produced = features("How do I fix the borrow checker error");
        assert!(produced.contains(&"borrow".to_string()));
        assert!(produced.contains(&"borrow_checker".to_string()));
        assert!(!produced.contains(&"the".to_string()));
    }

    #[test]
    fn ambiguous_titles_are_dropped_and_single_tag_rows_kept() {
        const SKILL: &str = "rust";
        let rows: Vec<(String, Option<String>)> = vec![
            ("shared title".to_string(), Some(SKILL.to_string())),
            ("shared title".to_string(), Some("postgres".to_string())),
            ("only rust".to_string(), Some(SKILL.to_string())),
        ];
        let (kept, dropped) = drop_ambiguous_tags(&rows);
        assert_eq!(dropped, 2, "both copies of the shared title go");
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].1.as_deref(), Some(SKILL));
    }

    #[test]
    fn train_splits_disjointly_and_scores_held_out() {
        let rows = separable_rows();
        let model = train(&rows).expect("separable corpus trains");
        let held_out = model.held_out.as_ref().expect("held-out metrics");
        assert!(
            model.training_rows + held_out.rows < rows.len(),
            "train {} plus test {} leave the validation slice apart from both, of {}",
            model.training_rows,
            held_out.rows,
            rows.len()
        );
        assert!(
            held_out.accuracy > 0.9,
            "separable corpus should separate: {}",
            held_out.accuracy
        );
        assert!(model.usable, "enough rows and accuracy make it usable");
    }

    #[test]
    fn split_keeps_three_way_disjoint_slices() {
        let rows: Vec<SourcedRow> = (0..40)
            .map(|index| SourcedRow {
                text: format!("title {index}"),
                skill: Some(format!("skill {}", index % 4)),
                provider: String::new(),
            })
            .collect();
        let (train_rows, validation_rows, test_rows) = split_sourced(&rows, DEFAULT_SEED);
        assert_eq!(
            train_rows.len() + validation_rows.len() + test_rows.len(),
            rows.len(),
            "every row lands in exactly one slice"
        );
        assert!(!validation_rows.is_empty(), "validation fits the threshold");
        assert!(!test_rows.is_empty(), "test is what gets reported");
        let mut seen: Vec<&String> = Vec::new();
        for row in train_rows.iter().chain(&validation_rows).chain(&test_rows) {
            assert!(
                !seen.contains(&&row.text),
                "title appears twice: {}",
                row.text
            );
            seen.push(&row.text);
        }
    }

    #[test]
    fn predict_names_the_learned_skill() {
        let rows = separable_rows();
        let model = train(&rows).expect("model");
        let (name, confidence) = predict(&model, "postgres autovacuum bloat").expect("predicts");
        assert_eq!(name, "postgres");
        assert!(confidence > 0.5, "confidence {confidence}");
    }

    #[test]
    fn unseen_vocabulary_stays_silent() {
        let rows = separable_rows();
        let model = train(&rows).expect("model");
        assert!(
            predict(&model, "zzz qqq").is_none(),
            "no shared feature means no answer, not a guess"
        );
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
    fn crates_boilerplate_drops_the_language_and_keeps_cargo() {
        let masked = mask_crates_boilerplate(&format!(
            "A {PHANTOM_LABEL} library and crate for cargo and postgres"
        ));
        let produced = features(&masked);
        assert!(!produced.iter().any(|feature| feature == PHANTOM_LABEL));
        assert!(!produced.iter().any(|feature| feature == "library"));
        assert!(!produced.iter().any(|feature| feature == "crate"));
        assert!(produced.iter().any(|feature| feature == "cargo"));
        assert!(produced.iter().any(|feature| feature == "postgres"));
        assert!(
            features("trust the borrow checker")
                .iter()
                .any(|feature| feature == "trust"),
            "a word that merely contains {PHANTOM_LABEL} stays"
        );
    }

    #[test]
    fn uninstalled_skill_rows_leave_the_training_set() {
        let rows = vec![
            sourced_row("borrow checker lifetime", Some(PHANTOM_LABEL)),
            sourced_row("postgres vacuum autovacuum", Some(POSTGRES_LABEL)),
            sourced_row("unlabelled blurb", None),
        ];
        let (kept, dropped) = drop_uninstalled_skills(&rows, &|name| name != PHANTOM_LABEL);
        assert_eq!(dropped, 1);
        assert_eq!(kept.len(), 2);
        assert!(kept
            .iter()
            .all(|row| row.skill.as_deref() != Some(PHANTOM_LABEL)));
    }

    #[test]
    fn select_installed_skips_a_phantom_class_above_the_floor() {
        let ranked = vec![
            (PHANTOM_LABEL.to_string(), 0.62),
            (POSTGRES_LABEL.to_string(), 0.31),
            ("websocket-realtime-design".to_string(), 0.07),
        ];
        let chosen = select_installed(&ranked, 0.25, &|name| name != PHANTOM_LABEL);
        assert_eq!(chosen.map(|(name, _)| name), Some(POSTGRES_LABEL));
        assert!(
            select_installed(&ranked, 0.40, &|name| name != PHANTOM_LABEL).is_none(),
            "the runner-up must clear the same floor on its own probability"
        );
    }

    #[test]
    fn sourced_training_keeps_the_provider_on_the_training_slice() {
        let mut rows = Vec::new();
        for index in 0..80 {
            rows.push(SourcedRow {
                text: format!("{PHANTOM_LABEL} library crate postgres vacuum autovacuum {index}"),
                skill: Some(POSTGRES_LABEL.to_string()),
                provider: CRATES_PROVIDER.to_string(),
            });
            rows.push(SourcedRow {
                text: format!("borrow checker lifetime error in cargo build {index}"),
                skill: Some(PHANTOM_LABEL.to_string()),
                provider: "stackexchange:stackoverflow".to_string(),
            });
        }
        let (train_rows, validation_rows, _) = split_sourced(&rows, DEFAULT_SEED);
        let crates_train = train_rows.iter().any(|row| row.provider == CRATES_PROVIDER);
        assert!(crates_train, "the training slice still knows the provider");
        assert!(
            validation_rows
                .iter()
                .filter(|row| row.provider == CRATES_PROVIDER)
                .any(|row| row.text.contains("library")),
            "validation text stays raw"
        );
        let model = train_sourced(&rows).expect("sourced corpus trains");
        assert!(model.held_out.is_some());
    }

    #[test]
    fn prune_terms_drops_weights_that_cannot_move_a_decision() {
        let mut model = train(&separable_rows()).expect("model");
        for (index, expert) in model.experts.iter_mut().enumerate() {
            expert
                .terms
                .push((format!("noise{index}"), MIN_TERM_WEIGHT / 100.0));
            expert
                .terms
                .push((format!("signal{index}"), MIN_TERM_WEIGHT * 100.0));
        }
        let before: usize = model.experts.iter().map(|expert| expert.terms.len()).sum();
        let kept = prune_terms(&mut model);
        assert_eq!(
            kept,
            before - model.experts.len(),
            "one noise term each goes"
        );
        assert!(
            model.experts.iter().all(|expert| expert
                .terms
                .iter()
                .all(|(term, _)| !term.starts_with("noise"))),
            "a weight below the floor does not survive"
        );
        assert!(
            model.experts.iter().all(|expert| expert
                .terms
                .iter()
                .any(|(term, _)| term.starts_with("signal"))),
            "a weight above the floor does"
        );
    }

    #[test]
    fn head_is_cached_per_artifact_version() {
        let dir = std::env::temp_dir().join(format!("keel-head-{}", std::process::id()));
        let path = dir.join("lexical-experts.json");
        let model = train(&separable_rows()).expect("model");
        save(&path, &model).unwrap();
        let first = head(&path).expect("head");
        let second = head(&path).expect("head");
        assert!(
            std::sync::Arc::ptr_eq(&first, &second),
            "an unchanged artifact is parsed once"
        );
        let (name, _) = first
            .rank("postgres autovacuum bloat")
            .expect("ranks")
            .into_iter()
            .next()
            .expect("a class");
        assert_eq!(name, "postgres", "the cached head still scores");
        let mut changed = model;
        changed.usable = false;
        save(&path, &changed).unwrap();
        assert!(
            head(&path).is_none(),
            "a rewritten artifact is rebuilt, and an unusable one reads as absent"
        );
        assert!(head(&dir.join("absent.json")).is_none());
        let _ = std::fs::remove_dir_all(&dir);
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

    #[test]
    fn entropy_spans_zero_to_uniform() {
        assert_eq!(entropy(&[1.0, 0.0, 0.0]), 0.0);
        assert!((entropy(&[0.25, 0.25, 0.25, 0.25]) - 4f64.ln()).abs() < 1e-12);
    }

    #[test]
    fn rescale_keeps_the_winner_and_renormalizes() {
        let sharp = vec![0.7, 0.2, 0.1];
        let identical = rescale(&sharp, 1.0);
        for (left, right) in identical.iter().zip(&sharp) {
            assert!((left - right).abs() < 1e-12);
        }
        let softened = rescale(&sharp, 2.0);
        assert!((softened.iter().sum::<f64>() - 1.0).abs() < 1e-12);
        assert!(softened[0] < sharp[0], "heat spreads the mass");
        assert_eq!(best_of(&softened).0, 0, "rescaling never flips the argmax");
        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert_eq!(rescale(&sharp, bad), sharp, "bad temperature {bad}");
        }
    }

    #[test]
    fn entropy_temperature_selects_the_first_covering_bin() {
        let bins = vec![(0.5, 0.5), (1.0, 2.0)];
        assert_eq!(entropy_temperature(0.3, &bins), 0.5);
        assert_eq!(entropy_temperature(0.7, &bins), 2.0);
        assert_eq!(entropy_temperature(5.0, &bins), 1.0);
        assert_eq!(entropy_temperature(0.3, &[]), 1.0);
        assert_eq!(entropy_temperature(f64::NAN, &bins), 1.0);
    }

    #[test]
    fn keep_rule_pins_ece_down_and_coverage_held() {
        assert!(keep_adaptive(0.20, 0.90, 0.15, 0.88));
        assert!(
            !keep_adaptive(0.20, 0.90, 0.15, 0.80),
            "coverage collapse vetoes"
        );
        assert!(!keep_adaptive(0.20, 0.90, 0.25, 0.90), "rising ECE vetoes");
        assert!(
            keep_adaptive(0.20, 0.90, 0.15, 0.85),
            "five points lost still ships"
        );
    }

    #[test]
    fn adaptive_rank_softens_through_fitted_bins() {
        let model = train(&separable_rows()).expect("model");
        // why: a one-sided prompt saturates to exactly 1.0, where heat is identity.
        let prompt = "postgres borrow";
        let static_winner = predict(&model, prompt).expect("predicts");
        assert!(
            static_winner.1 > 0.5 && static_winner.1 < 1.0,
            "mixed prompt stays unsaturated: {}",
            static_winner.1
        );
        let mut adaptive = model;
        let held_out = adaptive.held_out.as_mut().expect("held out");
        held_out.entropy_temperatures = vec![(f64::MAX, 2.0)];
        let adaptive_winner = predict(&adaptive, prompt).expect("still predicts");
        assert_eq!(static_winner.0, adaptive_winner.0);
        assert!(
            adaptive_winner.1 < static_winner.1,
            "heat lowers the stated confidence"
        );
    }

    #[test]
    fn old_artifact_without_calibration_fields_ranks_static() {
        let model = LexicalModel {
            schema: LEXICAL_SCHEMA,
            training_rows: 240,
            skills: 2,
            vector_rows: 0,
            embedding_dim: 0,
            centroids: Vec::new(),
            experts: vec![
                LexicalExpert {
                    name: BIN_A.to_string(),
                    bias: 0.0,
                    terms: vec![(BIN_FEATURE_A.to_string(), 2.0)],
                },
                LexicalExpert {
                    name: BIN_B.to_string(),
                    bias: 0.0,
                    terms: vec![(BIN_FEATURE_B.to_string(), 2.0)],
                },
            ],
            idf: vec![
                (BIN_FEATURE_A.to_string(), 1.0),
                (BIN_FEATURE_B.to_string(), 1.0),
            ],
            held_out: Some(HeldOut {
                rows: 10,
                decided: 10,
                correct: 9,
                accuracy: 0.9,
                brier: 0.1,
                scale: 1.5,
                accept: 0.25,
                accept_per_skill: Vec::new(),
                operating_points: Vec::new(),
                ece: 0.2,
                entropy_temperatures: vec![(f64::MAX, 2.0)],
            }),
            usable: true,
        };
        // why: an artifact written before the fields existed has neither key.
        let mut value = serde_json::to_value(&model).expect("serializes");
        let held_out = value
            .get_mut("held_out")
            .and_then(|held_out| held_out.as_object_mut())
            .expect("held out object");
        held_out.remove("ece");
        held_out.remove("entropy_temperatures");
        let old: LexicalModel = serde_json::from_value(value).expect("old shape loads");
        let held_out = old.held_out.as_ref().expect("held out");
        assert_eq!(held_out.ece, 0.0);
        assert!(held_out.entropy_temperatures.is_empty());
        let (name, _) = predict(&old, BIN_FEATURE_A).expect("ranks");
        assert_eq!(name, BIN_A);
    }

    #[test]
    fn entropy_temperatures_fit_deterministically() {
        let model = LexicalModel {
            schema: LEXICAL_SCHEMA,
            training_rows: 0,
            skills: 2,
            vector_rows: 0,
            embedding_dim: 0,
            centroids: Vec::new(),
            experts: vec![
                LexicalExpert {
                    name: BIN_A.to_string(),
                    bias: 0.0,
                    terms: vec![(BIN_FEATURE_A.to_string(), 2.0)],
                },
                LexicalExpert {
                    name: BIN_B.to_string(),
                    bias: 0.0,
                    terms: vec![(BIN_FEATURE_B.to_string(), 2.0)],
                },
            ],
            idf: vec![
                (BIN_FEATURE_A.to_string(), 1.0),
                (BIN_FEATURE_B.to_string(), 1.0),
            ],
            held_out: None,
            usable: true,
        };
        let scorer = Scorer::new(&model, None);
        let calibration: Vec<(String, HashMap<String, f64>)> = (0..400)
            .map(|index| {
                let weight = (index % 200) as f64 / 200.0;
                let mut document = HashMap::new();
                document.insert(BIN_FEATURE_A.to_string(), weight);
                document.insert(BIN_FEATURE_B.to_string(), 1.0 - weight);
                let label = if index % 2 == 0 { BIN_A } else { BIN_B };
                (label.to_string(), document)
            })
            .collect();
        let first = fit_entropy_temperatures(&scorer, &calibration, 1.0);
        let second = fit_entropy_temperatures(&scorer, &calibration, 1.0);
        assert_eq!(first, second, "the same rows fit the same bins");
        assert_eq!(first.len(), 4);
        for (edge, temperature) in &first {
            assert!(edge.is_finite());
            assert!(
                (0.1..=3.0).contains(temperature),
                "grid temperature {temperature}"
            );
        }
        assert!(
            first.windows(2).all(|pair| pair[0].0 <= pair[1].0),
            "edges ascend"
        );
        assert!(fit_entropy_temperatures(&scorer, &calibration[..100], 1.0).is_empty());
    }
}
