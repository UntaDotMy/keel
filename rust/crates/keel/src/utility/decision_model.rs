//! Purpose: Offline-fit calibration experts over the labeled decision samples, sealed against tampering.
//! Caller: the decision `train` and `model` actions; the compute-backend profile names where this math may run.
//! Dependencies: serde, std::fs, utility::decision_samples, utility::hashing.
//! Main Functions: fit_surface_experts, predict_surface, train_decision_model, load_decision_model.
//! Side Effects: Reads the sample corpus and writes the model, its per-home key, and its seal under <keel-home>/state/.
//!
//! Fitting follows the post-hoc calibration literature: a two-parameter Platt
//! fit over the signal logit, temperature scaling only while the corpus is too
//! small to pay for a second parameter, k-fold scoring so every reported number
//! is held out rather than in sample, and counts-based sufficient statistics so
//! the gradient costs the same on ten rows or a hundred thousand.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Stored model file name under the keel state directory.
pub const DECISION_MODEL_FILE: &str = "decision-model.json";
/// Schema version of the stored model; a mismatch reads as untrained.
const DECISION_MODEL_SCHEMA: u32 = 2;
/// The seal bound to this home's key; a model without it never loads.
const DECISION_MODEL_SEAL_FILE: &str = "decision-model.seal";
/// The per-home key the seal is bound to.
const DECISION_MODEL_KEY_FILE: &str = "decision-model.key";

/// Below this many samples a surface fits temperature only: one parameter.
const TEMPERATURE_ONLY_BELOW: usize = 40;
/// Fold target; a small corpus takes fewer folds so each keeps MIN_FOLD_SAMPLES.
const TARGET_FOLDS: usize = 10;
const MIN_FOLD_SAMPLES: usize = 4;
/// Optimization budget; the loop exits early once the loss stops moving.
const MAX_ITERATIONS: usize = 600;
const LEARNING_RATE: f64 = 0.3;
const RIDGE: f64 = 0.01;
const CONVERGENCE_EPSILON: f64 = 1e-12;
/// Signals are aggregated to 1/1000 before fitting: gradients need counts per
/// distinct value, which is what makes the fit independent of corpus size.
const SIGNAL_QUANTUM: f64 = 1000.0;
/// Reliability bins for the expected calibration error.
const ECE_BINS: usize = 10;
/// Rows an expert needs before it may drive a live decision, together with a
/// held-out fold estimate that did not lose to the raw signal.
const MIN_TRUSTED_SAMPLES: usize = 100;

/// One surface's expert: Platt scaling over the signal's logit, so the raw
/// signal is the starting point and the fit learns a monotone correction.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SurfaceExpert {
    pub scale: f64,
    pub bias: f64,
    pub samples: usize,
    pub correct: usize,
    /// Held-out folds the reported scores average over. Zero means the corpus
    /// was too small to split, so every score is in sample.
    pub folds: usize,
    pub brier_raw: f64,
    pub brier_fitted: f64,
    pub ece_raw: f64,
    pub ece_fitted: f64,
    pub log_loss_fitted: f64,
}

/// The trained mixture: one expert per decision surface, keyed by surface name.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DecisionModel {
    pub schema: u32,
    pub trained_at_ms: u64,
    pub samples: usize,
    pub surfaces: BTreeMap<String, SurfaceExpert>,
}

#[derive(Debug, Clone, Copy)]
struct FoldMetrics {
    brier_raw: f64,
    brier_fitted: f64,
    ece_raw: f64,
    ece_fitted: f64,
    log_loss: f64,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|delta| delta.as_millis() as u64)
        .unwrap_or(0)
}

fn logit(signal: f64) -> f64 {
    let clamped = signal.clamp(0.001, 0.999);
    (clamped / (1.0 - clamped)).ln()
}

fn sigmoid(value: f64) -> f64 {
    1.0 / (1.0 + (-value).exp())
}

/// The expert's calibrated probability for one raw signal.
pub fn predict_surface(expert: &SurfaceExpert, signal: f64) -> f64 {
    sigmoid(expert.scale * logit(signal) + expert.bias)
}

/// Whether an expert's held-out evidence justifies letting it drive a live
/// decision: enough rows, a real fold estimate, and no loss against the raw
/// signal. Consumers must gate on this instead of trusting a fresh fit.
pub fn expert_is_usable(expert: &SurfaceExpert) -> bool {
    expert.samples >= MIN_TRUSTED_SAMPLES
        && expert.folds >= 2
        && expert.brier_fitted <= expert.brier_raw
}

/// Signals collapsed to their distinct values with positive counts: the
/// gradient's sufficient statistics. Returns (logit, positives, count).
fn buckets(pairs: &[(f64, bool)]) -> Vec<(f64, f64, f64)> {
    let mut counts: BTreeMap<i64, (f64, f64, f64)> = BTreeMap::new();
    for (signal, label) in pairs {
        let key = (signal * SIGNAL_QUANTUM).round() as i64;
        let entry = counts.entry(key).or_insert((*signal, 0.0, 0.0));
        entry.1 += if *label { 1.0 } else { 0.0 };
        entry.2 += 1.0;
    }
    counts
        .into_values()
        .map(|(signal, positives, count)| (logit(signal), positives, count))
        .collect()
}

/// Fit `scale` (and `bias` when allowed) by gradient descent on the mean
/// log-loss with a light ridge toward the identity calibration. Deterministic:
/// fixed budget, no sampling, early exit once the loss stops moving.
fn fit_params(pairs: &[(f64, bool)], allow_bias: bool) -> (f64, f64) {
    let buckets = buckets(pairs);
    let total: f64 = buckets.iter().map(|(_, _, count)| *count).sum();
    if total <= 0.0 {
        return (1.0, 0.0);
    }
    let (mut scale, mut bias) = (1.0f64, 0.0f64);
    let mut previous_loss = f64::INFINITY;
    for _ in 0..MAX_ITERATIONS {
        let mut gradient_scale = 0.0f64;
        let mut gradient_bias = 0.0f64;
        let mut loss = 0.0f64;
        for (x, positives, count) in &buckets {
            let predicted = sigmoid(scale * x + bias);
            let error = predicted * count - positives;
            gradient_scale += error * x;
            gradient_bias += error;
            loss -= positives * predicted.max(1e-12).ln()
                + (count - positives) * (1.0 - predicted).max(1e-12).ln();
        }
        gradient_scale = gradient_scale / total + RIDGE * (scale - 1.0);
        if allow_bias {
            gradient_bias = gradient_bias / total + RIDGE * bias;
        } else {
            gradient_bias = 0.0;
        }
        scale -= LEARNING_RATE * gradient_scale;
        bias -= LEARNING_RATE * gradient_bias;
        let mean_loss = loss / total;
        if (previous_loss - mean_loss).abs() < CONVERGENCE_EPSILON {
            break;
        }
        previous_loss = mean_loss;
    }
    (scale, if allow_bias { bias } else { 0.0 })
}

/// Expected calibration error over fixed reliability bins. Brier stays the
/// primary score because it needs no binning; ECE reads the shape of the gap.
pub(crate) fn expected_calibration_error(probabilities: &[(f64, bool)]) -> f64 {
    if probabilities.is_empty() {
        return 0.0;
    }
    let mut bins = [(0.0f64, 0.0f64, 0usize); ECE_BINS];
    for (probability, label) in probabilities {
        let scaled = (probability.clamp(0.0, 1.0) * ECE_BINS as f64) as usize;
        let bin = &mut bins[scaled.min(ECE_BINS - 1)];
        bin.0 += probability;
        bin.1 += if *label { 1.0 } else { 0.0 };
        bin.2 += 1;
    }
    let total = probabilities.len() as f64;
    bins.iter()
        .filter(|bin| bin.2 > 0)
        .map(|(sum_probability, sum_label, count)| {
            let mean_probability = sum_probability / *count as f64;
            let accuracy = sum_label / *count as f64;
            (mean_probability - accuracy).abs() * (*count as f64 / total)
        })
        .sum()
}

fn pair_metrics(pairs: &[(f64, bool)], params: (f64, f64)) -> FoldMetrics {
    let mut raw_squared = 0.0;
    let mut fitted_squared = 0.0;
    let mut log_loss = 0.0;
    let mut raw_probabilities = Vec::with_capacity(pairs.len());
    let mut fitted_probabilities = Vec::with_capacity(pairs.len());
    for (signal, label) in pairs {
        let target = if *label { 1.0 } else { 0.0 };
        let fitted = sigmoid(params.0 * logit(*signal) + params.1);
        raw_squared += (signal - target).powi(2);
        fitted_squared += (fitted - target).powi(2);
        log_loss -=
            target * fitted.max(1e-12).ln() + (1.0 - target) * (1.0 - fitted).max(1e-12).ln();
        raw_probabilities.push((*signal, *label));
        fitted_probabilities.push((fitted, *label));
    }
    let count = pairs.len().max(1) as f64;
    FoldMetrics {
        brier_raw: raw_squared / count,
        brier_fitted: fitted_squared / count,
        ece_raw: expected_calibration_error(&raw_probabilities),
        ece_fitted: expected_calibration_error(&fitted_probabilities),
        log_loss: log_loss / count,
    }
}

fn fold_count(samples: usize) -> usize {
    if samples < MIN_FOLD_SAMPLES * 2 {
        return 0;
    }
    TARGET_FOLDS.min(samples / MIN_FOLD_SAMPLES).max(2)
}

/// SplitMix64: the one random-looking step this module needs, seeded from the
/// row count so the shuffle is deterministic and reproducible.
fn splitmix64(mut state: u64) -> u64 {
    state = state.wrapping_add(0x9e3779b97f4a7c15);
    let mut z = state;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
    z ^ (z >> 31)
}

/// Deterministic Fisher-Yates over row indices. A stride split reads well on
/// time-ordered rows but aliases with periodic labels (every tenth row the same
/// class), which made a fold label-pure; a fixed-seed shuffle keeps every fold
/// representative and the run reproducible.
fn shuffled_indices(count: usize) -> Vec<usize> {
    let mut indices: Vec<usize> = (0..count).collect();
    let mut state = count as u64;
    for position in (1..count).rev() {
        state = splitmix64(state);
        let swap = (state % (position as u64 + 1)) as usize;
        indices.swap(position, swap);
    }
    indices
}

/// Held-out scores across `folds` disjoint test sets. Parameters still come
/// from the full corpus; these numbers are the estimate that generalizes.
fn cross_validated_metrics(pairs: &[(f64, bool)], folds: usize, allow_bias: bool) -> FoldMetrics {
    let mut sums = FoldMetrics {
        brier_raw: 0.0,
        brier_fitted: 0.0,
        ece_raw: 0.0,
        ece_fitted: 0.0,
        log_loss: 0.0,
    };
    let mut weight = 0.0;
    let order = shuffled_indices(pairs.len());
    for fold in 0..folds {
        let mut train = Vec::with_capacity(pairs.len());
        let mut test = Vec::new();
        for (position, index) in order.iter().enumerate() {
            if position % folds == fold {
                test.push(pairs[*index]);
            } else {
                train.push(pairs[*index]);
            }
        }
        if train.is_empty() || test.is_empty() {
            continue;
        }
        let metrics = pair_metrics(&test, fit_params(&train, allow_bias));
        let fold_weight = test.len() as f64;
        sums.brier_raw += metrics.brier_raw * fold_weight;
        sums.brier_fitted += metrics.brier_fitted * fold_weight;
        sums.ece_raw += metrics.ece_raw * fold_weight;
        sums.ece_fitted += metrics.ece_fitted * fold_weight;
        sums.log_loss += metrics.log_loss * fold_weight;
        weight += fold_weight;
    }
    let divisor = if weight > 0.0 { weight } else { 1.0 };
    FoldMetrics {
        brier_raw: sums.brier_raw / divisor,
        brier_fitted: sums.brier_fitted / divisor,
        ece_raw: sums.ece_raw / divisor,
        ece_fitted: sums.ece_fitted / divisor,
        log_loss: sums.log_loss / divisor,
    }
}

fn fit_surface(pairs: &[(f64, bool)]) -> SurfaceExpert {
    let allow_bias = pairs.len() >= TEMPERATURE_ONLY_BELOW;
    let (scale, bias) = fit_params(pairs, allow_bias);
    let folds = fold_count(pairs.len());
    let metrics = if folds >= 2 {
        cross_validated_metrics(pairs, folds, allow_bias)
    } else {
        pair_metrics(pairs, (scale, bias))
    };
    SurfaceExpert {
        scale,
        bias,
        samples: pairs.len(),
        correct: pairs.iter().filter(|(_, label)| *label).count(),
        folds,
        brier_raw: metrics.brier_raw,
        brier_fitted: metrics.brier_fitted,
        ece_raw: metrics.ece_raw,
        ece_fitted: metrics.ece_fitted,
        log_loss_fitted: metrics.log_loss,
    }
}

/// Fit one expert per surface. Every surface keeps its own parameter pair, and
/// a surface with no samples never produces an expert at all.
pub fn fit_surface_experts(
    samples: &[crate::utility::decision_samples::DecisionSample],
) -> BTreeMap<String, SurfaceExpert> {
    let mut grouped: BTreeMap<String, Vec<(f64, bool)>> = BTreeMap::new();
    for sample in samples {
        grouped
            .entry(sample.surface.clone())
            .or_default()
            .push((sample.signal, sample.label));
    }
    grouped
        .into_iter()
        .map(|(surface, pairs)| (surface, fit_surface(&pairs)))
        .collect()
}

/// Fit every surface from the recorded corpus. Pure: the caller persists the
/// result, so a write failure surfaces where the caller can report it.
pub fn train_decision_model(claude_home: &Path, days: u64) -> DecisionModel {
    let samples = crate::utility::decision_samples::iter_recent_samples(claude_home, days);
    DecisionModel {
        schema: DECISION_MODEL_SCHEMA,
        trained_at_ms: now_ms(),
        samples: samples.len(),
        surfaces: fit_surface_experts(&samples),
    }
}

/// The JSON summary the `train` and `model` actions both report: one row per
/// surface with its fit quality, so the two surfaces cannot drift apart.
pub fn summary_value(model: &DecisionModel) -> serde_json::Value {
    let surfaces: Vec<serde_json::Value> = model
        .surfaces
        .iter()
        .map(|(surface, expert)| {
            serde_json::json!({
                "surface": surface,
                "samples": expert.samples,
                "correct": expert.correct,
                "folds": expert.folds,
                "brier_raw": expert.brier_raw,
                "brier_fitted": expert.brier_fitted,
                "ece_raw": expert.ece_raw,
                "ece_fitted": expert.ece_fitted,
                "log_loss_fitted": expert.log_loss_fitted,
                "usable": expert_is_usable(expert),
                "scale": expert.scale,
                "bias": expert.bias,
            })
        })
        .collect();
    serde_json::json!({
        "schema": model.schema,
        "trained_at_ms": model.trained_at_ms,
        "samples": model.samples,
        "surfaces": surfaces,
    })
}

fn decision_model_file(claude_home: &Path) -> PathBuf {
    crate::runtime::state_directory(claude_home).join(DECISION_MODEL_FILE)
}

pub fn save_decision_model(claude_home: &Path, model: &DecisionModel) -> Result<(), String> {
    let path = decision_model_file(claude_home);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let text = serde_json::to_string_pretty(model)
        .map_err(|e| format!("serialize decision model: {e}"))?;
    fs::write(&path, &text).map_err(|e| format!("write decision model: {e}"))?;
    let key = decision_model_key(claude_home)?;
    fs::write(
        decision_model_seal_file(claude_home),
        model_seal(&key, &text),
    )
    .map_err(|e| format!("write decision model seal: {e}"))
}

/// The persisted model, or `None` when it is missing, unreadable, from another
/// schema, or failing its seal. Sealing is tamper evidence plus a home binding,
/// not secrecy: the key lives beside the model, so it stops drift and copying,
/// not a local reader.
pub fn load_decision_model(claude_home: &Path) -> Option<DecisionModel> {
    let Ok(text) = fs::read_to_string(decision_model_file(claude_home)) else {
        return None;
    };
    let Ok(seal) = fs::read_to_string(decision_model_seal_file(claude_home)) else {
        return None;
    };
    let Ok(key) = fs::read_to_string(decision_model_key_file(claude_home)) else {
        return None;
    };
    if model_seal(key.trim(), &text) != seal.trim() {
        return None;
    }
    let Ok(model) = serde_json::from_str::<DecisionModel>(&text) else {
        return None;
    };
    if model.schema != DECISION_MODEL_SCHEMA {
        return None;
    }
    Some(model)
}

fn decision_model_seal_file(claude_home: &Path) -> PathBuf {
    crate::runtime::state_directory(claude_home).join(DECISION_MODEL_SEAL_FILE)
}

fn decision_model_key_file(claude_home: &Path) -> PathBuf {
    crate::runtime::state_directory(claude_home).join(DECISION_MODEL_KEY_FILE)
}

/// The per-home key: created on the first save, so a model file copied to
/// another home has no matching key and never verifies there.
fn decision_model_key(claude_home: &Path) -> Result<String, String> {
    let path = decision_model_key_file(claude_home);
    if let Ok(existing) = fs::read_to_string(&path) {
        if !existing.trim().is_empty() {
            return Ok(existing.trim().to_string());
        }
    }
    let entropy = format!(
        "{}-{}-{}",
        now_ms(),
        std::process::id(),
        claude_home.display()
    );
    let key = crate::utility::hashing::sha256_hex(entropy.as_bytes());
    fs::write(&path, &key).map_err(|e| format!("write decision model key: {e}"))?;
    Ok(key)
}

/// Keyed digest over the exact stored bytes plus a version tag, so a seal can
/// never be replayed from another artifact shape.
fn model_seal(key: &str, text: &str) -> String {
    crate::utility::hashing::sha256_hex(format!("keel-decision-model-v1\n{key}\n{text}").as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utility::decision_samples::DecisionSample;

    fn sample(surface: &str, signal: f64, label: bool) -> DecisionSample {
        DecisionSample {
            at_ms: 1,
            surface: surface.to_string(),
            detail: "probe".to_string(),
            signal,
            label,
        }
    }

    /// The surface every calibration test exercises, so the literal and any
    /// rename have one owner.
    const ROUTING: &str = "routing";

    fn routing_expert(experts: &BTreeMap<String, SurfaceExpert>) -> &SurfaceExpert {
        experts.get(ROUTING).expect("one routing expert")
    }

    fn miscalibrated(count: usize) -> Vec<DecisionSample> {
        (0..count)
            .map(|index| sample(ROUTING, 0.9, index % 2 == 0))
            .collect()
    }

    #[test]
    fn identity_expert_reproduces_the_signal() {
        let expert = SurfaceExpert {
            scale: 1.0,
            bias: 0.0,
            samples: 0,
            correct: 0,
            folds: 0,
            brier_raw: 0.0,
            brier_fitted: 0.0,
            ece_raw: 0.0,
            ece_fitted: 0.0,
            log_loss_fitted: 0.0,
        };
        assert!((predict_surface(&expert, 0.8) - 0.8).abs() < 1e-9);
    }

    #[test]
    fn the_fit_improves_a_miscalibrated_surface() {
        // Declared 0.9 but only half right: the fit must pull the estimate down.
        let experts = fit_surface_experts(&miscalibrated(100));
        let expert = routing_expert(&experts);
        assert!(
            expert.brier_fitted < expert.brier_raw,
            "fitted {} must beat raw {}",
            expert.brier_fitted,
            expert.brier_raw
        );
        assert!(
            expert.ece_fitted < expert.ece_raw,
            "fitted ece {} must beat raw {}",
            expert.ece_fitted,
            expert.ece_raw
        );
        assert!(
            predict_surface(expert, 0.9) < 0.7,
            "a half-right 0.9 signal must calibrate down"
        );
        assert_eq!(expert.samples, 100);
        assert_eq!(expert.correct, 50);
        assert_eq!(expert.folds, TARGET_FOLDS);
        assert!(expert.log_loss_fitted > 0.0);
    }

    #[test]
    fn a_small_corpus_fits_temperature_only() {
        let experts = fit_surface_experts(&miscalibrated(16));
        let expert = routing_expert(&experts);
        assert_eq!(expert.bias, 0.0, "one parameter below the threshold");
        assert_eq!(expert.folds, 4, "16 rows keep four per fold");
    }

    #[test]
    fn a_tiny_corpus_reports_in_sample_scores() {
        let experts = fit_surface_experts(&miscalibrated(6));
        let expert = routing_expert(&experts);
        assert_eq!(expert.folds, 0, "too small to split, and the flag says so");
        assert!(expert.brier_fitted.is_finite());
    }

    #[test]
    fn duplicated_rows_do_not_change_the_fit() {
        let single = fit_surface_experts(&miscalibrated(40));
        let mut doubled: Vec<DecisionSample> = Vec::new();
        for entry in miscalibrated(40) {
            doubled.push(entry.clone());
            doubled.push(entry);
        }
        let repeated = fit_surface_experts(&doubled);
        let left = routing_expert(&single);
        let right = routing_expert(&repeated);
        assert!((left.scale - right.scale).abs() < 1e-9);
        assert!((left.bias - right.bias).abs() < 1e-9);
    }

    #[test]
    fn calibration_error_reads_a_known_gap() {
        let honest: Vec<(f64, bool)> = (0..100).map(|index| (0.5, index % 2 == 0)).collect();
        assert!(expected_calibration_error(&honest) < 1e-9);
        let overconfident: Vec<(f64, bool)> = (0..100).map(|index| (0.9, index % 2 == 0)).collect();
        assert!((expected_calibration_error(&overconfident) - 0.4).abs() < 1e-9);
    }

    #[test]
    fn model_round_trips_and_missing_reads_none() {
        let home = std::env::temp_dir().join(format!("keel-decision-model-{}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        assert!(load_decision_model(&home).is_none());
        let model = train_decision_model(&home, 30);
        assert_eq!(model.samples, 0);
        assert_eq!(model.schema, DECISION_MODEL_SCHEMA);
        save_decision_model(&home, &model).expect("save model");
        let loaded = load_decision_model(&home).expect("model file");
        assert_eq!(loaded.trained_at_ms, model.trained_at_ms);
        let _ = fs::remove_dir_all(&home);
    }

    /// Red team: a tampered model, a wrong schema, and a model copied to a home
    /// that does not hold its key must all fail closed.
    #[test]
    fn a_tampered_or_copied_model_fails_closed() {
        let home = std::env::temp_dir().join(format!("keel-decision-seal-{}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        let model = train_decision_model(&home, 30);
        save_decision_model(&home, &model).expect("save model");
        assert!(load_decision_model(&home).is_some());

        let path = decision_model_file(&home);
        let text = fs::read_to_string(&path).expect("read model");
        fs::write(&path, format!("{text} ")).expect("tamper");
        assert!(
            load_decision_model(&home).is_none(),
            "a tampered model must not load"
        );
        save_decision_model(&home, &model).expect("re-save");
        assert!(load_decision_model(&home).is_some());

        let future = DecisionModel {
            schema: DECISION_MODEL_SCHEMA + 1,
            ..model.clone()
        };
        save_decision_model(&home, &future).expect("save future schema");
        assert!(
            load_decision_model(&home).is_none(),
            "a model from another schema must not load"
        );
        save_decision_model(&home, &model).expect("re-save");

        let copied =
            std::env::temp_dir().join(format!("keel-decision-copy-{}", std::process::id()));
        let _ = fs::remove_dir_all(&copied);
        fs::create_dir_all(crate::runtime::state_directory(&copied)).expect("copy home");
        fs::copy(&path, decision_model_file(&copied)).expect("copy model");
        fs::copy(
            decision_model_seal_file(&home),
            decision_model_seal_file(&copied),
        )
        .expect("copy seal");
        assert!(
            load_decision_model(&copied).is_none(),
            "a copied model must not verify without its key"
        );
        let _ = fs::remove_dir_all(&home);
        let _ = fs::remove_dir_all(&copied);
    }

    /// Generated-corpus benchmark, not a gate. It builds a labeled corpus from a
    /// known miscalibration curve, trains it from disk through the real store, and
    /// prints what each expert recovered. Run with:
    /// `cargo test -p keel --lib decision_model_bench -- --ignored --nocapture`.
    #[test]
    #[ignore = "benchmark, run explicitly with --ignored --nocapture"]
    fn decision_model_bench() {
        // True reliability per surface is P(right) = signal^k: k above one
        // over-declares, and k of one is the control the fit should leave alone.
        let curves = [
            (ROUTING, 2.5),
            ("gate", 1.8),
            ("shell", 3.0),
            ("conformal", 1.0),
        ];
        let rows_per_surface = 5_000;
        let mut samples = Vec::with_capacity(curves.len() * rows_per_surface);
        let mut seed = 0x5eed_u64;
        for (surface, exponent) in curves {
            for _ in 0..rows_per_surface {
                seed = splitmix64(seed);
                let signal = 0.5 + 0.49 * ((seed % 1000) as f64 / 1000.0);
                seed = splitmix64(seed);
                let draw = (seed % 1000) as f64 / 1000.0;
                samples.push(sample(surface, signal, draw < signal.powf(exponent)));
            }
        }

        let home = std::env::temp_dir().join(format!("keel-model-bench-{}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        let samples_dir = crate::runtime::state_directory(&home).join("decision-samples");
        fs::create_dir_all(&samples_dir).expect("samples dir");
        let mut body = String::new();
        for entry in &samples {
            body.push_str(&serde_json::to_string(entry).expect("serialize sample"));
            body.push('\n');
        }
        fs::write(samples_dir.join("bench.jsonl"), &body).expect("write corpus");

        let start = std::time::Instant::now();
        let model = train_decision_model(&home, 30);
        let train_ms = start.elapsed().as_secs_f64() * 1000.0;
        println!(
            "decision model bench: {} rows generated, {} grouped, train from disk {:.1} ms",
            samples.len(),
            model.samples,
            train_ms
        );
        println!(
            "{:<12} {:>6} {:>6} {:>10} {:>10} {:>10} {:>10} {:>7}",
            "surface", "rows", "folds", "brier_raw", "brier_fit", "ece_raw", "ece_fit", "usable"
        );
        for (surface, expert) in &model.surfaces {
            println!(
                "{:<12} {:>6} {:>6} {:>10.5} {:>10.5} {:>10.5} {:>10.5} {:>7}",
                surface,
                expert.samples,
                expert.folds,
                expert.brier_raw,
                expert.brier_fitted,
                expert.ece_raw,
                expert.ece_fitted,
                expert_is_usable(expert)
            );
        }

        let expert = model.surfaces.get(ROUTING).expect("routing expert");
        let start = std::time::Instant::now();
        let mut checksum = 0.0;
        for index in 0..100_000 {
            checksum += predict_surface(expert, 0.5 + (index % 500) as f64 / 1000.0);
        }
        let predict_ms = start.elapsed().as_secs_f64() * 1000.0;
        println!(
            "predict: 100k calls {:.2} ms ({:.1} ns per call), checksum {:.1}",
            predict_ms,
            predict_ms * 1.0e6 / 100_000.0,
            checksum
        );

        for surface in [ROUTING, "gate", "shell"] {
            let miscalibrated = model.surfaces.get(surface).expect("surface expert");
            assert!(
                miscalibrated.brier_fitted < miscalibrated.brier_raw,
                "{surface}: fitted {} must beat raw {}",
                miscalibrated.brier_fitted,
                miscalibrated.brier_raw
            );
            assert!(
                miscalibrated.ece_fitted < miscalibrated.ece_raw,
                "{surface}: ece {} must beat raw {}",
                miscalibrated.ece_fitted,
                miscalibrated.ece_raw
            );
            assert!(
                expert_is_usable(miscalibrated),
                "{surface}: should be usable"
            );
        }
        let control = model.surfaces.get("conformal").expect("control expert");
        assert!(
            control.brier_fitted <= control.brier_raw + 0.01,
            "a calibrated control must not degrade: {} vs {}",
            control.brier_fitted,
            control.brier_raw
        );
        assert!(
            train_ms < 5_000.0,
            "the fit must stay interactive: {train_ms} ms"
        );
        assert!(
            predict_ms < 1_000.0,
            "predictions must stay trivial: {predict_ms} ms"
        );
        let _ = fs::remove_dir_all(&home);
    }
}
