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

/// Outcome-semantics epoch. Records written under an older epoch measured a
/// different quantity (silence was recorded as a routing failure), so they are
/// ineligible for calibration rather than deleted: the audit trail stays, the
/// estimator ignores it until fresh evidence replaces it.
pub const OUTCOME_SEMANTICS_EPOCH: u32 = 2;

/// Prior strength for m-estimate shrinkage, in pseudo-observations. Small enough
/// that tens of real outcomes carry the estimate; the legacy N=100 reference left
/// calibration inert for roughly 100 outcomes per skill.
pub const SHRINKAGE_PRIOR_STRENGTH: f64 = 10.0;

/// Evidence half-life in days. Calibration drifts as routing behaviour changes,
/// so older evidence decays instead of dominating forever.
pub const EVIDENCE_HALF_LIFE_DAYS: f64 = 30.0;

/// m-estimate rate: the empirical rate shrunk toward `computed` by a fixed prior
/// strength. With no evidence it returns `computed` exactly, so cold start stays
/// honest rather than neutral.
pub fn shrinkage_rate(correct: f64, total: f64, computed: f64) -> f64 {
    let bounded_total = total.max(0.0);
    let bounded_correct = correct.clamp(0.0, bounded_total.max(0.0));
    ((bounded_correct + SHRINKAGE_PRIOR_STRENGTH * computed.clamp(0.0, 1.0))
        / (bounded_total + SHRINKAGE_PRIOR_STRENGTH))
        .clamp(0.0, 1.0)
}

/// Age weight for stored evidence: 1.0 fresh, 0.5 per half-life elapsed.
pub fn staleness_factor(updated_at_ms: u64, now_ms: u64) -> f64 {
    let age_days = now_ms.saturating_sub(updated_at_ms) as f64 / 86_400_000.0;
    0.5f64.powf(age_days / EVIDENCE_HALF_LIFE_DAYS)
}

/// Shrinkage over staleness-scaled evidence: the same m-estimate, but older
/// records carry proportionally less weight than fresh ones.
pub fn staleness_weighted_rate(correct: f64, total: f64, computed: f64, age_weight: f64) -> f64 {
    let weight = age_weight.clamp(0.0, 1.0);
    shrinkage_rate(correct * weight, total * weight, computed)
}

#[cfg(test)]
mod benchmark_tests {
    use super::*;

    /// Deterministic LCG so the published table is reproducible in CI.
    struct Lcg(u64);

    impl Lcg {
        fn next_f64(&mut self) -> f64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            ((self.0 >> 11) as f64) / ((1u64 << 53) as f64)
        }
    }

    /// The routed skill is right 75% of the time; evidence appears on 25% of
    /// decisions. Silence is the common case, which is the whole problem.
    const ROUTE_ACCURACY: f64 = 0.75;
    const EVIDENCE_RATE: f64 = 0.25;

    /// Estimates both pipelines from one stream and prints the comparison table.
    /// The legacy pipeline scores silence as failure; the new one scores only
    /// evidence-bearing decisions, so their estimands differ by construction.
    #[test]
    fn silence_poisoning_benchmark_table() {
        println!("\n| decisions | estimator | estimand | estimate | truth | bias | scorable |");
        println!("|---|---|---|---|---|---|---|");
        for decisions in [10usize, 50, 200, 2000] {
            let mut rng = Lcg(0x5eed);
            let (mut legacy_total, mut legacy_correct) = (0usize, 0usize);
            let (mut new_total, mut new_correct) = (0usize, 0usize);
            for _ in 0..decisions {
                let right = rng.next_f64() < ROUTE_ACCURACY;
                let evidenced = rng.next_f64() < EVIDENCE_RATE;
                // Legacy: every decision is scored, silence counts as a miss.
                legacy_total += 1;
                if right && evidenced {
                    legacy_correct += 1;
                }
                // New: only an evidenced decision carries a usable outcome.
                if evidenced {
                    new_total += 1;
                    if right {
                        new_correct += 1;
                    }
                }
            }
            let legacy = laplace_rate(legacy_total, legacy_correct);
            let new = laplace_rate(new_total, new_correct);
            let scorable = new_total as f64 / decisions as f64;
            println!(
                "| {decisions} | legacy | routing accuracy | {legacy:.3} | {ROUTE_ACCURACY:.3} | {:.3} | 1.000 |",
                legacy - ROUTE_ACCURACY
            );
            println!(
                "| {decisions} | new | P(right \\| evidence) | {new:.3} | {ROUTE_ACCURACY:.3} | {:.3} | {scorable:.3} |",
                new - ROUTE_ACCURACY
            );
            assert!(
                (legacy - ROUTE_ACCURACY).abs() > 0.30,
                "the legacy estimand must stay badly biased at n={decisions}"
            );
        }
        let mut rng = Lcg(0x5eed);
        let (mut total, mut correct) = (0usize, 0usize);
        for _ in 0..2000 {
            let right = rng.next_f64() < ROUTE_ACCURACY;
            let evidenced = rng.next_f64() < EVIDENCE_RATE;
            if evidenced {
                total += 1;
                if right {
                    correct += 1;
                }
            }
        }
        let converged = laplace_rate(total, correct) - ROUTE_ACCURACY;
        assert!(
            converged.abs() < 0.10,
            "the new estimand must converge on the truth, got bias {converged:.3}"
        );
    }

    /// The retired N-weighted blend, kept as the benchmark's legacy baseline so
    /// the table still contrasts the two estimators.
    fn legacy_blend(empirical: f64, computed: f64, total: usize) -> f64 {
        let weight = (total as f64 / (total as f64 + BLEND_REFERENCE_SAMPLES)).clamp(0.0, 0.95);
        (weight * empirical + (1.0 - weight) * computed).clamp(0.0, 1.0)
    }

    /// Evidence arrives in tens, not hundreds: the legacy reference weighting
    /// barely moves at n=20, while shrinkage has already taken the evidence.
    #[test]
    fn shrinkage_authority_table() {
        let computed = 0.60;
        let at_n20 = laplace_rate(20, 18);
        let legacy = legacy_blend(at_n20, computed, 20);
        let shrunk = shrinkage_rate(18.0, 20.0, computed);
        println!("\n| estimator | prior strength | estimate at n=20 | distance from computed |");
        println!("|---|---|---|---|");
        println!(
            "| legacy blend | {BLEND_REFERENCE_SAMPLES:.0} | {legacy:.3} | {:.3} |",
            legacy - computed
        );
        println!(
            "| shrinkage | {SHRINKAGE_PRIOR_STRENGTH:.0} | {shrunk:.3} | {:.3} |",
            shrunk - computed
        );
        assert!(
            shrunk - computed > (legacy - computed) * 2.0,
            "shrinkage must take the evidence faster: legacy {legacy:.3} vs shrunk {shrunk:.3}"
        );
        assert!(
            (shrinkage_rate(0.0, 0.0, computed) - computed).abs() < f64::EPSILON,
            "cold start must return the computed value unchanged"
        );
        // The retired blend's own boundaries: cold start returns computed, and
        // the 0.95 weight cap keeps a 5% computed floor even at huge N.
        assert_eq!(legacy_blend(0.2, 0.8, 0), 0.8);
        assert!((legacy_blend(0.7, 0.9, 100_000) - (0.95 * 0.7 + 0.05 * 0.9)).abs() < 1e-12);
    }

    /// Age decays evidence rather than letting stale outcomes dominate forever.
    #[test]
    fn staleness_decays_old_evidence() {
        let now = 1_800_000_000_000u64;
        let fresh = staleness_factor(now, now);
        let one_half_life =
            staleness_factor(now - (EVIDENCE_HALF_LIFE_DAYS as u64) * 86_400_000, now);
        assert!((fresh - 1.0).abs() < f64::EPSILON);
        assert!((one_half_life - 0.5).abs() < 0.01);
        let stale = staleness_weighted_rate(20.0, 20.0, 0.5, one_half_life);
        let fresh_rate = staleness_weighted_rate(20.0, 20.0, 0.5, fresh);
        assert!(
            (stale - 0.5).abs() < (fresh_rate - 0.5).abs(),
            "older evidence must pull the estimate less: stale {stale:.3} vs fresh {fresh_rate:.3}"
        );
    }
}

/// Default significance / error level for Conformal Risk Control (CRC): 5% miscoverage.
pub const DEFAULT_CONFORMAL_ALPHA: f64 = 0.05;

/// Nonconformity score for a binary prediction: s = 1 - p(y).
/// If outcome is true (e.g. action was safe / skill was helpful), s = 1.0 - confidence.
/// If outcome is false (e.g. action was harmful / skill was wrong), s = confidence.
pub fn nonconformity_score(confidence: f64, outcome: bool) -> f64 {
    let conf = confidence.clamp(0.0, 1.0);
    if outcome {
        1.0 - conf
    } else {
        conf
    }
}

/// Compute the empirical (1 - alpha) quantile from nonconformity scores per
/// Angelopoulos & Bates (2021/2024).
///
/// Mathematical guarantee: with n calibration samples, the probability of
/// miscoverage on a fresh test point is at most alpha:
///   P(s_test > q_hat) <= alpha.
///
/// When n is too small to provide the exact guarantee (ceil((n+1)*(1-alpha)) > n),
/// this returns 1.0 (conservative: escalate borderline predictions).
pub fn conformal_quantile(scores: &[f64], alpha: f64) -> f64 {
    if scores.is_empty() {
        return 1.0;
    }
    let alpha_clamped = alpha.clamp(0.001, 0.999);
    let mut sorted = scores.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let n = sorted.len();
    let rank = ((n as f64 + 1.0) * (1.0 - alpha_clamped)).ceil() as usize;
    if rank > n {
        // Sample size insufficient for strict distribution-free guarantee:
        // fall back to conservative ceiling.
        1.0
    } else if rank == 0 {
        sorted[0]
    } else {
        sorted[rank - 1]
    }
}

/// Check if a test prediction's nonconformity exceeds the conformal quantile threshold.
/// Returns true if the prediction is nonconformant (unsafe / anomalous / below guarantee).
pub fn is_nonconformant(nonconformity: f64, quantile_threshold: f64) -> bool {
    nonconformity > quantile_threshold
}

/// Conformal p-value for a test nonconformity score against historical scores.
/// p = (1 + sum(s_i >= s_test)) / (n + 1).
pub fn conformal_p_value(scores: &[f64], test_score: f64) -> f64 {
    if scores.is_empty() {
        return 1.0;
    }
    let count_greater_or_equal = scores.iter().filter(|&&s| s >= test_score).count();
    (1.0 + count_greater_or_equal as f64) / (scores.len() as f64 + 1.0)
}

/// Evaluation result from a Conformal Calibrator.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ConformalEvaluation {
    pub nonconformity_score: f64,
    pub quantile_threshold: f64,
    pub satisfies_guarantee: bool,
    pub p_value: f64,
}

/// Stateful Conformal Risk Control calibrator.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ConformalCalibrator {
    pub alpha: f64,
    pub nonconformity_scores: Vec<f64>,
}

impl ConformalCalibrator {
    pub fn new(alpha: f64) -> Self {
        Self {
            alpha: alpha.clamp(0.001, 0.999),
            nonconformity_scores: Vec::new(),
        }
    }

    pub fn record(&mut self, confidence: f64, outcome: bool) {
        let score = nonconformity_score(confidence, outcome);
        self.nonconformity_scores.push(score);
    }

    pub fn quantile_threshold(&self) -> f64 {
        conformal_quantile(&self.nonconformity_scores, self.alpha)
    }

    pub fn evaluate_prediction(&self, confidence: f64) -> ConformalEvaluation {
        // Hypothesizing positive outcome (e.g. action is safe / correct)
        let s_test = nonconformity_score(confidence, true);
        let q_hat = self.quantile_threshold();
        let satisfies = !is_nonconformant(s_test, q_hat);
        let p_val = conformal_p_value(&self.nonconformity_scores, s_test);
        ConformalEvaluation {
            nonconformity_score: s_test,
            quantile_threshold: q_hat,
            satisfies_guarantee: satisfies,
            p_value: p_val,
        }
    }

    pub fn sample_count(&self) -> usize {
        self.nonconformity_scores.len()
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

    #[test]
    fn conformal_nonconformity_score_vectors() {
        assert_eq!(nonconformity_score(1.0, true), 0.0);
        assert_eq!(nonconformity_score(0.0, false), 0.0);
        assert_eq!(nonconformity_score(1.0, false), 1.0);
        assert_eq!(nonconformity_score(0.0, true), 1.0);
        assert!((nonconformity_score(0.8, true) - 0.2).abs() < 1e-12);
        assert!((nonconformity_score(0.8, false) - 0.8).abs() < 1e-12);
    }

    #[test]
    fn conformal_quantile_boundary_and_coverage() {
        assert_eq!(conformal_quantile(&[], 0.05), 1.0);

        // Small sample (n=5): ceil((5+1)*0.95) = ceil(5.7) = 6 > 5 -> returns 1.0 (conservative fallback)
        let small = vec![0.1, 0.2, 0.3, 0.4, 0.5];
        assert_eq!(conformal_quantile(&small, 0.05), 1.0);

        // With 100 samples uniformly spaced in [0.01, 1.00]
        let mut scores = Vec::new();
        for i in 1..=100 {
            scores.push(i as f64 / 100.0);
        }
        // alpha = 0.10: rank = ceil(101 * 0.90) = 91. sorted[90] = 0.91
        let q_90 = conformal_quantile(&scores, 0.10);
        assert!((q_90 - 0.91).abs() < 1e-12, "got {q_90}");

        // alpha = 0.05: rank = ceil(101 * 0.95) = 96. sorted[95] = 0.96
        let q_95 = conformal_quantile(&scores, 0.05);
        assert!((q_95 - 0.96).abs() < 1e-12, "got {q_95}");
    }

    #[test]
    fn conformal_calibrator_stateful_evaluation() {
        let mut calibrator = ConformalCalibrator::new(0.05);
        assert_eq!(calibrator.sample_count(), 0);
        assert_eq!(calibrator.quantile_threshold(), 1.0);

        // Record 100 actions: 96 high-confidence safe actions (confidence 0.95, outcome true -> score 0.05)
        for _ in 0..96 {
            calibrator.record(0.95, true);
        }
        // Record 4 mistakes (confidence 0.95, outcome false -> score 0.95)
        for _ in 0..4 {
            calibrator.record(0.95, false);
        }

        assert_eq!(calibrator.sample_count(), 100);
        let q_hat = calibrator.quantile_threshold();
        // rank = ceil(101 * 0.95) = 96. sorted[95] is 0.05 because 96 items are 0.05.
        assert!((q_hat - 0.05).abs() < 1e-12, "q_hat was {q_hat}");

        // A high confidence test (0.96) has score 0.04 <= 0.05 -> satisfies guarantee
        let eval_good = calibrator.evaluate_prediction(0.96);
        assert!(eval_good.satisfies_guarantee);
        assert!(eval_good.nonconformity_score <= q_hat);

        // A lower confidence test (0.80) has score 0.20 > 0.05 -> violates 95% guarantee -> nonconformant
        let eval_borderline = calibrator.evaluate_prediction(0.80);
        assert!(!eval_borderline.satisfies_guarantee);
    }
}
