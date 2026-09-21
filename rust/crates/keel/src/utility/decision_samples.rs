//! Purpose: Append-only labeled decision samples for the offline trainer.
//! Caller: utility::decision outcome recorders (routing, gates, conformal, shell) and the decision `samples` surface.
//! Dependencies: std::fs, std::io::Write, std::path, serde.
//! Main Functions: record_decision_sample, iter_recent_samples, sample_counts.
//! Side Effects: Appends one JSON line per recorded outcome under <keel-home>/state/decision-samples/.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Surfaces that label samples: one owner for the vocabulary the trainer keys
/// on.
pub const SURFACE_ROUTING: &str = "routing";
pub const SURFACE_GATE: &str = "gate";
pub const SURFACE_CONFORMAL: &str = "conformal";
pub const SURFACE_SHELL: &str = "shell";

/// One labeled decision sample: the confidence a surface declared and whether
/// the decision proved correct. No prompt, command, or payload text is stored,
/// only the context tag.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DecisionSample {
    pub at_ms: u64,
    pub surface: String,
    pub detail: String,
    pub signal: f64,
    pub label: bool,
}

fn samples_dir(claude_home: &Path) -> PathBuf {
    claude_home.join("state").join("decision-samples")
}

fn samples_path_for_today(claude_home: &Path) -> PathBuf {
    let date = chrono::Local::now().format("%Y-%m-%d").to_string();
    samples_dir(claude_home).join(format!("{date}.jsonl"))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|delta| delta.as_millis() as u64)
        .unwrap_or(0)
}

/// Append one labeled sample. Best-effort by design: training data must never
/// break the outcome write that produced it, so every failure is dropped.
pub fn record_decision_sample(
    claude_home: &Path,
    surface: &str,
    detail: &str,
    signal: f64,
    label: bool,
) {
    let path = samples_path_for_today(claude_home);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let sample = DecisionSample {
        at_ms: now_ms(),
        surface: surface.to_string(),
        detail: detail.to_string(),
        signal: signal.clamp(0.0, 1.0),
        label,
    };
    let Ok(line) = serde_json::to_string(&sample) else {
        return;
    };
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(file, "{line}");
    }
}

/// Samples recorded over the requested day window, oldest day first. A
/// malformed line is skipped so one bad write cannot hide the rest of the
/// corpus.
pub fn iter_recent_samples(claude_home: &Path, days: u64) -> Vec<DecisionSample> {
    let mut samples = Vec::new();
    for path in daily_files(claude_home, days) {
        let Ok(body) = fs::read_to_string(&path) else {
            continue;
        };
        for line in body.lines() {
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(sample) = serde_json::from_str::<DecisionSample>(line) {
                samples.push(sample);
            }
        }
    }
    samples
}

/// Per-surface `(surface, samples, correct)` totals over the requested day
/// window, sorted by surface name.
pub fn sample_counts(claude_home: &Path, days: u64) -> Vec<(String, usize, usize)> {
    let mut counts: std::collections::BTreeMap<String, (usize, usize)> =
        std::collections::BTreeMap::new();
    for sample in iter_recent_samples(claude_home, days) {
        let entry = counts.entry(sample.surface).or_default();
        entry.0 += 1;
        if sample.label {
            entry.1 += 1;
        }
    }
    counts
        .into_iter()
        .map(|(surface, (samples, correct))| (surface, samples, correct))
        .collect()
}

/// Daily files under the samples directory, oldest first, limited to the last
/// `days` of the sorted corpus.
fn daily_files(claude_home: &Path, days: u64) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = Vec::new();
    let Ok(entries) = fs::read_dir(samples_dir(claude_home)) else {
        return files;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) == Some("jsonl") {
            files.push(path);
        }
    }
    files.sort();
    let keep = days.max(1).min(files.len() as u64) as usize;
    files.split_off(files.len() - keep)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The gate name the sample tests exercise, so the literal has one owner.
    const IRON_GATE: &str = "iron_law";

    fn sample_home(label: &str) -> PathBuf {
        let home =
            std::env::temp_dir().join(format!("keel-samples-{label}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        home
    }

    fn drop_sample_home(home: &Path) {
        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn appends_and_counts_by_surface() {
        let home = sample_home("counts");
        record_decision_sample(&home, SURFACE_ROUTING, "reviewer", 0.8, true);
        record_decision_sample(&home, SURFACE_ROUTING, "critic", 0.7, false);
        record_decision_sample(&home, SURFACE_GATE, IRON_GATE, 0.95, true);
        assert_eq!(
            sample_counts(&home, 30),
            vec![("gate".to_string(), 1, 1), ("routing".to_string(), 2, 1)]
        );
        let samples = iter_recent_samples(&home, 30);
        assert_eq!(samples.len(), 3);
        assert!(samples[0].at_ms > 0);
        assert_eq!(samples[2].detail, IRON_GATE);
        drop_sample_home(&home);
    }

    #[test]
    fn a_labeled_outcome_writes_one_sample_and_silence_writes_none() {
        let home = sample_home("outcome");
        crate::utility::decision::record_gate_outcome(
            &home,
            IRON_GATE,
            crate::utility::decision::GateOutcome::Upheld,
            0.95,
        )
        .expect("record upheld outcome");
        crate::utility::decision::record_gate_outcome(
            &home,
            IRON_GATE,
            crate::utility::decision::GateOutcome::Unknown,
            0.95,
        )
        .expect("record unknown outcome");
        let samples = iter_recent_samples(&home, 30);
        assert_eq!(samples.len(), 1, "samples: {samples:?}");
        assert_eq!(samples[0].surface, SURFACE_GATE);
        assert!(samples[0].label);
        drop_sample_home(&home);
    }
}
