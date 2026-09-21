//! Purpose: Offline-fit calibration experts over the labeled decision samples.
//! Caller: the decision `train` and `model` actions; the compute-backend profile names where this math may run.
//! Dependencies: serde, std::fs, utility::decision_samples.
//! Main Functions: fit_surface_experts, predict_surface, train_decision_model, load_decision_model.
//! Side Effects: Reads the sample corpus and writes <keel-home>/state/decision-model.json.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Stored model file name under the keel state directory.
pub const DECISION_MODEL_FILE: &str = "decision-model.json";

/// One surface's expert: Platt scaling over the signal's logit, so the raw
/// signal is the starting point and the fit learns a monotone correction.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SurfaceExpert {
    pub scale: f64,
    pub bias: f64,
    pub samples: usize,
    pub correct: usize,
    pub brier_raw: f64,
    pub brier_fitted: f64,
}

/// The trained mixture: one expert per decision surface, keyed by surface name.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DecisionModel {
    pub trained_at_ms: u64,
    pub samples: usize,
    pub surfaces: BTreeMap<String, SurfaceExpert>,
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

/// Fit one expert per surface by gradient descent on log-loss with a light
/// ridge toward the identity calibration, so a small corpus stays near the raw
/// signal instead of overfitting. Deterministic: fixed iterations, no sampling,
/// no randomness, and the CPU path the `ComputeProfile` currently selects.
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
    let mut experts = BTreeMap::new();
    for (surface, pairs) in grouped {
        let (mut scale, mut bias) = (1.0f64, 0.0f64);
        let iterations = 300;
        let learning_rate = 0.2;
        let ridge = 0.01;
        let count = pairs.len().max(1) as f64;
        for _ in 0..iterations {
            let mut gradient_scale = 0.0f64;
            let mut gradient_bias = 0.0f64;
            for (signal, label) in &pairs {
                let x = logit(*signal);
                let error = sigmoid(scale * x + bias) - if *label { 1.0 } else { 0.0 };
                gradient_scale += error * x;
                gradient_bias += error;
            }
            gradient_scale = gradient_scale / count + ridge * (scale - 1.0);
            gradient_bias = gradient_bias / count + ridge * bias;
            scale -= learning_rate * gradient_scale;
            bias -= learning_rate * gradient_bias;
        }
        let correct = pairs.iter().filter(|(_, label)| *label).count();
        let brier_raw = pairs
            .iter()
            .map(|(signal, label)| (signal - if *label { 1.0 } else { 0.0 }).powi(2))
            .sum::<f64>()
            / count;
        let expert = SurfaceExpert {
            scale,
            bias,
            samples: pairs.len(),
            correct,
            brier_raw,
            brier_fitted: 0.0,
        };
        let brier_fitted = pairs
            .iter()
            .map(|(signal, label)| {
                let predicted = predict_surface(&expert, *signal);
                (predicted - if *label { 1.0 } else { 0.0 }).powi(2)
            })
            .sum::<f64>()
            / count;
        experts.insert(
            surface,
            SurfaceExpert {
                brier_fitted,
                ..expert
            },
        );
    }
    experts
}

/// Fit every surface from the recorded corpus. Pure: the caller persists the
/// result, so a write failure surfaces where the caller can report it.
pub fn train_decision_model(claude_home: &Path, days: u64) -> DecisionModel {
    let samples = crate::utility::decision_samples::iter_recent_samples(claude_home, days);
    DecisionModel {
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
                "brier_raw": expert.brier_raw,
                "brier_fitted": expert.brier_fitted,
                "scale": expert.scale,
                "bias": expert.bias,
            })
        })
        .collect();
    serde_json::json!({
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
    fs::write(&path, text).map_err(|e| format!("write decision model: {e}"))
}

/// The persisted model, or `None` before the first train (or on a bad file).
pub fn load_decision_model(claude_home: &Path) -> Option<DecisionModel> {
    let path = decision_model_file(claude_home);
    let Ok(text) = fs::read_to_string(&path) else {
        return None;
    };
    let Ok(model) = serde_json::from_str::<DecisionModel>(&text) else {
        return None;
    };
    Some(model)
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

    #[test]
    fn identity_expert_reproduces_the_signal() {
        let expert = SurfaceExpert {
            scale: 1.0,
            bias: 0.0,
            samples: 0,
            correct: 0,
            brier_raw: 0.0,
            brier_fitted: 0.0,
        };
        assert!((predict_surface(&expert, 0.8) - 0.8).abs() < 1e-9);
    }

    #[test]
    fn the_fit_improves_a_miscalibrated_surface() {
        // Declared 0.9 but only half right: the fit must pull the estimate down.
        let samples: Vec<DecisionSample> = (0..16)
            .map(|index| sample("routing", 0.9, index % 2 == 0))
            .collect();
        let experts = fit_surface_experts(&samples);
        let expert = experts.get("routing").expect("one expert");
        assert!(
            expert.brier_fitted < expert.brier_raw,
            "fitted {} must beat raw {}",
            expert.brier_fitted,
            expert.brier_raw
        );
        assert!(
            predict_surface(expert, 0.9) < 0.7,
            "a half-right 0.9 signal must calibrate down"
        );
        assert_eq!(expert.samples, 16);
        assert_eq!(expert.correct, 8);
    }

    #[test]
    fn model_round_trips_and_missing_reads_none() {
        let home = std::env::temp_dir().join(format!("keel-decision-model-{}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        assert!(load_decision_model(&home).is_none());
        let model = train_decision_model(&home, 30);
        assert_eq!(model.samples, 0);
        save_decision_model(&home, &model).expect("save model");
        let loaded = load_decision_model(&home).expect("model file");
        assert_eq!(loaded.trained_at_ms, model.trained_at_ms);
        let _ = fs::remove_dir_all(&home);
    }
}
