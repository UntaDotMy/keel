//! Shared calibration math for every decision surface.
//!
//! One implementation of Laplace-smoothed rates, N-weighted blending, and
//! calibration-error metrics, used by skill routing, review/plan scores,
//! and composition tracking alike. All functions are pure and dependency-free.

/// Reference sample count for blend weights: computed dominates until N=100+.
/// (Plan risk table: J03 cold-start, J08 conservative rate.)
pub const BLEND_REFERENCE_SAMPLES: f64 = 100.0;

/// Laplace-smoothed success rate: (correct + 1) / (total + 2).
/// Empty history yields the neutral 0.5 prior instead of 0 or NaN.
pub fn laplace_rate(total: usize, correct: usize) -> f64 {
    (correct as f64 + 1.0) / (total as f64 + 2.0)
}

/// Weight given to empirical evidence: total / (total + 100), capped at 0.95.
/// N=10 leans ~9% empirical, N=100 splits evenly, N=1000 leans ~91%.
pub fn blend_weight(total: usize) -> f64 {
    (total as f64 / (total as f64 + BLEND_REFERENCE_SAMPLES)).clamp(0.0, 0.95)
}

/// Blend an empirical rate toward a computed fallback by sample count.
pub fn blend(empirical: f64, computed: f64, total: usize) -> f64 {
    let weight = blend_weight(total);
    (weight * empirical + (1.0 - weight) * computed).clamp(0.0, 1.0)
}

/// Brier score for one probabilistic prediction: squared error against the
/// binary outcome. Lower is better; 0 is perfect, 1 is maximally wrong.
pub fn brier_score(confidence: f64, outcome: bool) -> f64 {
    let actual = if outcome { 1.0 } else { 0.0 };
    (confidence.clamp(0.0, 1.0) - actual).powi(2)
}

/// Mean absolute calibration error over (confidence, outcome) pairs:
/// how far stated confidence strays from reality on average.
pub fn mean_absolute_calibration_error(pairs: &[(f64, bool)]) -> f64 {
    if pairs.is_empty() {
        return 0.0;
    }

    let total: f64 = pairs
        .iter()
        .map(|(confidence, outcome)| {
            let actual = if *outcome { 1.0 } else { 0.0 };
            (confidence.clamp(0.0, 1.0) - actual).abs()
        })
        .sum();
    total / pairs.len() as f64
}

/// Result of hierarchical blending: the calibrated value plus how much data
/// stands behind it. `prior_dominated` is true while the computed fallback
/// still carries the estimate, so callers can report honest uncertainty.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TieredEstimate {
    pub calibrated: f64,
    pub total_samples: u64,
    pub prior_dominated: bool,
}

/// Three-tier shrinkage: the bin rate shrinks toward the parent rate by bin
/// count, then toward computed with N=100 reference weighting. When the
/// parent is empty the global rate feeds it; when everything is empty the
/// input returns unchanged (cold start trusts the computed value).
pub fn hierarchical_blend(
    bin_total: usize,
    bin_correct: usize,
    parent_total: usize,
    parent_correct: usize,
    global_total: usize,
    global_correct: usize,
    computed: f64,
) -> TieredEstimate {
    let bin_rate = laplace_rate(bin_total, bin_correct);
    let parent_rate = if parent_total > 0 {
        laplace_rate(parent_total, parent_correct)
    } else if global_total > 0 {
        laplace_rate(global_total, global_correct)
    } else {
        0.5
    };
    let bin_weight = (bin_total as f64 / (bin_total as f64 + 10.0)).clamp(0.0, 1.0);
    let shrunk = bin_weight * bin_rate + (1.0 - bin_weight) * parent_rate;
    let parent_weight =
        (parent_total as f64 / (parent_total as f64 + BLEND_REFERENCE_SAMPLES)).clamp(0.0, 0.95);
    let (base, base_weight) = if parent_total > 0 {
        (shrunk, parent_weight)
    } else if global_total > 0 {
        let global_weight = (global_total as f64 / (global_total as f64 + BLEND_REFERENCE_SAMPLES))
            .clamp(0.0, 0.95);
        (
            global_weight * parent_rate + (1.0 - global_weight) * computed,
            global_weight,
        )
    } else {
        (computed, 0.0)
    };
    TieredEstimate {
        calibrated: base.clamp(0.0, 1.0),
        total_samples: parent_total as u64,
        prior_dominated: base_weight < 0.5,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calibration_laplace_rate_vectors() {
        assert_eq!(laplace_rate(0, 0), 0.5);
        assert!((laplace_rate(8, 7) - (8.0 / 10.0)).abs() < 1e-12);
        assert!((laplace_rate(998, 698) - (699.0 / 1000.0)).abs() < 1e-12);
    }

    #[test]
    fn calibration_blend_weight_schedule() {
        assert_eq!(blend_weight(0), 0.0);
        assert!(blend_weight(10) < 0.10);
        assert!((blend_weight(100) - 0.5).abs() < 1e-12);
        assert!(blend_weight(1_000_000) <= 0.95);
    }

    #[test]
    fn calibration_blend_boundaries() {
        assert_eq!(blend(0.2, 0.8, 0), 0.8);
        // The 0.95 weight cap keeps a 5% computed floor even at huge N.
        assert!((blend(0.7, 0.9, 100_000) - (0.95 * 0.7 + 0.05 * 0.9)).abs() < 1e-12);
    }
    #[test]
    fn calibration_brier_vectors() {
        assert_eq!(brier_score(1.0, true), 0.0);
        assert_eq!(brier_score(0.0, false), 0.0);
        assert_eq!(brier_score(1.0, false), 1.0);
        assert!((brier_score(0.85, true) - 0.0225).abs() < 1e-12);
    }

    #[test]
    fn calibration_mean_error_vectors() {
        assert_eq!(mean_absolute_calibration_error(&[]), 0.0);
        assert_eq!(
            mean_absolute_calibration_error(&[(1.0, true), (0.0, false)]),
            0.0
        );
        assert!(
            (mean_absolute_calibration_error(&[(0.8, true), (0.8, false)]) - 0.5).abs() < 1e-12
        );
    }

    #[test]
    fn calibration_hierarchical_empty_returns_computed() {
        let est = hierarchical_blend(0, 0, 0, 0, 0, 0, 0.85);
        assert_eq!(est.calibrated, 0.85);
        assert_eq!(est.total_samples, 0);
        assert!(est.prior_dominated);
    }

    #[test]
    fn calibration_hierarchical_global_tempers_new_skill() {
        // No skill data, rich global at 70%: an overconfident 0.9 blends down.
        let est = hierarchical_blend(0, 0, 0, 0, 500, 350, 0.9);
        let expected = (500.0 / 600.0) * (351.0 / 502.0) + (100.0 / 600.0) * 0.9;
        assert!(
            (est.calibrated - expected).abs() < 1e-12,
            "got {}",
            est.calibrated
        );
        assert!(est.calibrated < 0.9 && est.calibrated > 0.69);
        // Rich global history carries the estimate even with no skill data.
        assert!(!est.prior_dominated);
    }

    #[test]
    fn calibration_hierarchical_converges_with_volume() {
        // 1000 samples at 70% in-bin: estimate lands on empirical accuracy.
        let est = hierarchical_blend(1000, 700, 1000, 700, 5000, 3500, 0.7);
        assert!(
            (est.calibrated - 0.6996).abs() < 0.01,
            "got {}",
            est.calibrated
        );
        assert_eq!(est.total_samples, 1000);
        assert!(!est.prior_dominated);
    }
}
