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
}

/// A prompt with its weak label: a community tag mapped to a keel skill.
pub type LabelledRows = Vec<(String, String)>;

/// Raw rows as fetched, where an unlabelled row is allowed and then dropped.
pub type RawRows = [(String, Option<String>)];

type Weights = Vec<HashMap<String, f64>>;
type Accumulators = Vec<HashMap<String, f64>>;

struct Fitted {
    experts: Vec<LexicalExpert>,
    idf: Vec<(String, f64)>,
}

/// Train the experts. Returns `None` when the corpus cannot support a model at
/// all: a held-out score over a handful of rows would be a guess.
pub fn train(rows: &RawRows) -> Option<LexicalModel> {
    let (train_rows, validation_rows, test_rows) = split(rows, DEFAULT_SEED);
    // why: community tags carry label noise, and pruning the rows the fitted
    // model confidently contradicts lifted held-out accuracy in measurement.
    let (train_rows, _) = prune_label_noise(&train_rows);
    let fitted = fit(&train_rows, &validation_rows)?;
    let held_out = score_held_out(&fitted, &validation_rows, &test_rows);
    let usable = rows.len() >= REFUSE_BELOW_ROWS
        && held_out
            .as_ref()
            .is_some_and(|metrics| metrics.accuracy >= REFUSE_BELOW_ACCURACY);
    Some(LexicalModel {
        schema: LEXICAL_SCHEMA,
        training_rows: train_rows.len(),
        skills: fitted.experts.len(),
        experts: fitted.experts,
        idf: fitted.idf,
        held_out,
        usable,
    })
}

/// A question carrying two mapped tags appears under both classes, which teaches
/// the model that one row owns two skills. Keep single-tag rows only, and report
/// how many were dropped so the loss is visible rather than silent.
pub fn drop_ambiguous_tags(rows: &RawRows) -> (Vec<(String, Option<String>)>, usize) {
    let mut owners: HashMap<String, Vec<&str>> = HashMap::new();
    for (prompt, skill) in rows {
        if let Some(skill) = skill {
            let entry = owners.entry(prompt.clone()).or_default();
            if !entry.contains(&skill.as_str()) {
                entry.push(skill.as_str());
            }
        }
    }
    let ambiguous: Vec<String> = owners
        .into_iter()
        .filter(|(_, skills)| skills.len() > 1)
        .map(|(title, _)| title)
        .collect();
    let kept: Vec<(String, Option<String>)> = rows
        .iter()
        .filter(|(prompt, _)| !ambiguous.iter().any(|title| title == prompt))
        .cloned()
        .collect();
    let dropped = rows.len().saturating_sub(kept.len());
    (kept, dropped)
}

fn split(rows: &RawRows, seed: u64) -> (LabelledRows, LabelledRows, LabelledRows) {
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
    let test_count = labelled.len() / 5;
    // why: the accept point is fitted here, and a tenth of the corpus was thin
    // enough that the fitted floor moved on noise alone.
    let validation_count = labelled.len() * 3 / 20;
    let test = labelled.split_off(labelled.len() - test_count);
    let validation = labelled.split_off(labelled.len() - validation_count);
    (labelled, validation, test)
}

/// Unigrams plus adjacent bigrams: a bigram is what separates `borrow checker`
/// from a page that merely mentions both words.
fn features(prompt: &str) -> Vec<String> {
    let words: Vec<String> = prompt
        .to_ascii_lowercase()
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|word| word.len() >= MIN_WORD && !STOPWORDS.contains(word))
        .map(str::to_string)
        .collect();
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

/// Sublinear term frequency against the fitted document frequencies, L2
/// normalized so a long title cannot outvote a short one.
fn vectorize(prompt: &str, idf: &HashMap<String, f64>) -> HashMap<String, f64> {
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

fn fit(rows: &LabelledRows, validation_rows: &LabelledRows) -> Option<Fitted> {
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
        .map(|(prompt, skill)| (class_index[skill], vectorize(prompt, &idf)))
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
        .filter_map(|(prompt, skill)| Some((*class_index.get(skill)?, vectorize(prompt, &idf))))
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
}

impl Scorer {
    fn new(model: &LexicalModel) -> Self {
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
        }
    }

    fn vectorize(&self, prompt: &str) -> HashMap<String, f64> {
        vectorize(prompt, &self.idf)
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

    fn predict(&self, prompt: &str, temperature: f64) -> Option<(String, f64)> {
        let document = self.vectorize(prompt);
        if document.is_empty() {
            return None;
        }
        let (best, probability) = best_of(&self.probabilities(&document, temperature));
        Some((self.names[best].clone(), probability))
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

/// Best skill and its temperature-calibrated confidence. `None` when the prompt
/// shares no feature with the training rows: silence beats a coin flip.
pub fn predict(model: &LexicalModel, prompt: &str) -> Option<(String, f64)> {
    let scale = model
        .held_out
        .as_ref()
        .map(|metrics| metrics.scale)
        .unwrap_or(1.0);
    Scorer::new(model).predict(prompt, scale)
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
fn prune_label_noise(rows: &LabelledRows) -> (LabelledRows, usize) {
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
        let Some(fitted) = fit(&training, &LabelledRows::new()) else {
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
        };
        let scorer = Scorer::new(&model);
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
) -> Option<HeldOut> {
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
    };
    let scorer = Scorer::new(&model);
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
            let brier = brier_for_documents(&scorer, &calibration, scale);
            if brier < best_brier {
                best_scale = scale;
                best_brier = brier;
            }
        }
    }
    let mut correct = 0usize;
    for (skill, document) in &documents {
        if document.is_empty() {
            continue;
        }
        let (best, _) = best_of(&scorer.probabilities(document, best_scale));
        if &scorer.names[best] == skill {
            correct += 1;
        }
    }
    // Fitted, never assumed: the lowest confidence whose precision clears the
    // floor, because a hand-picked 0.80 threw most correct answers away.
    const PRECISION_FLOOR: f64 = 0.75;
    let decided = documents
        .iter()
        .filter(|(_, document)| !document.is_empty())
        .count();
    let mut accept = 1.0f64;
    let mut operating_points = Vec::new();
    for step in 1..=19 {
        let threshold = step as f64 * 0.05;
        let mut answered = 0usize;
        let mut right = 0usize;
        for (skill, document) in &calibration {
            if document.is_empty() {
                continue;
            }
            let (best, confidence) = best_of(&scorer.probabilities(document, best_scale));
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
            for (skill, document) in &calibration {
                if document.is_empty() {
                    continue;
                }
                let (best, confidence) = best_of(&scorer.probabilities(document, best_scale));
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
    Some(HeldOut {
        rows: test_rows.len(),
        decided,
        correct,
        accuracy: correct as f64 / test_rows.len() as f64,
        brier: brier_for_documents(&scorer, &documents, best_scale),
        scale: best_scale,
        accept,
        accept_per_skill,
        operating_points,
    })
}

/// Brier over the rows the model actually answered: a silent row has no stated
/// probability to score, so it is reported through accuracy and coverage instead.
fn brier_for_documents(
    scorer: &Scorer,
    documents: &[(String, HashMap<String, f64>)],
    scale: f64,
) -> f64 {
    let mut sum = 0.0;
    let mut scored = 0usize;
    for (skill, document) in documents {
        if document.is_empty() {
            continue;
        }
        let (best, confidence) = best_of(&scorer.probabilities(document, scale));
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
        let rows: Vec<(String, Option<String>)> = (0..40)
            .map(|index| {
                (
                    format!("title {index}"),
                    Some(format!("skill {}", index % 4)),
                )
            })
            .collect();
        let (train_rows, validation_rows, test_rows) = split(&rows, DEFAULT_SEED);
        assert_eq!(
            train_rows.len() + validation_rows.len() + test_rows.len(),
            rows.len(),
            "every row lands in exactly one slice"
        );
        assert!(!validation_rows.is_empty(), "validation fits the threshold");
        assert!(!test_rows.is_empty(), "test is what gets reported");
        let mut seen: Vec<&String> = Vec::new();
        for (prompt, _) in train_rows.iter().chain(&validation_rows).chain(&test_rows) {
            assert!(!seen.contains(&prompt), "title appears twice: {prompt}");
            seen.push(prompt);
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
