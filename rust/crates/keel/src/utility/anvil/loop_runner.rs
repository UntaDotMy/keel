use std::io::Write;
use std::time::{Duration, Instant};

use crate::args::FlagSet;
use crate::utility::anvil::cache;
use crate::utility::anvil::cast;
use crate::utility::anvil::job;
use crate::utility::anvil::prefix;
use crate::utility::anvil::report;
use crate::utility::anvil::sieve;
use crate::utility::anvil::supervisor;

fn fail_with_error(standard_error: &mut dyn Write, error: &str) -> u8 {
    let _ = writeln!(standard_error, "anvil loop: {error}");
    1
}

pub struct LoopConfig {
    pub max_iterations: usize,
    pub min_improvement: f64,
    pub wall_timeout: Duration,
}

impl Default for LoopConfig {
    fn default() -> Self {
        Self {
            max_iterations: 20,
            min_improvement: 0.05,
            wall_timeout: Duration::from_secs(300),
        }
    }
}

pub fn run_bounded_loop<F>(config: &LoopConfig, mut gate_pass: F) -> (usize, f64)
where
    F: FnMut() -> (bool, f64),
{
    let start = Instant::now();
    let mut iterations = 0usize;
    let mut prev: Option<f64> = None;
    let mut improvement = 0.0;
    while iterations < config.max_iterations {
        if start.elapsed() >= config.wall_timeout {
            break;
        }
        let (pass, score) = gate_pass();
        if pass {
            iterations += 1;
            improvement = score - prev.unwrap_or(0.0);
            break;
        }
        improvement = score - prev.unwrap_or(score);
        if prev.is_some() && improvement < config.min_improvement {
            iterations += 1;
            break;
        }
        prev = Some(score);
        iterations += 1;
    }
    (iterations, improvement)
}
fn refinement_gates(paths: &job::JobPaths, piece: &str) -> Result<Vec<String>, String> {
    let lock = job::load_lock(paths)?;
    let pieces = job::pieces_from_lock(&lock, piece)?;
    let gates: Vec<String> = pieces.into_iter().flat_map(|piece| piece.gates).collect();
    if gates.is_empty() {
        return Err("anvil loop: no refinement gates are configured".to_string());
    }
    Ok(gates)
}

pub fn run_loop(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flags = FlagSet::new("anvil loop");
    flags.string_flag("piece", "");
    flags.bool_flag("strict", false);
    flags.bool_flag("dry-run", false);
    flags.string_flag("workspace-root", "");
    flags.string_flag("claude-home", "");
    if let Err(error) = flags.parse(arguments) {
        let _ = writeln!(standard_error, "{}", error.message);
        return 1;
    }
    let strict = flags.bool_value("strict");
    let dry_run = flags.bool_value("dry-run");
    let mut cfg = LoopConfig::default();
    let paths = match job::JobPaths::resolve(
        flags.string_value("workspace-root"),
        flags.string_value("claude-home"),
    ) {
        Ok(value) => value,
        Err(error) => {
            let _ = writeln!(standard_error, "{error}");
            return 1;
        }
    };
    let piece = flags.string_value("piece").to_string();
    let lock = match job::load_lock(&paths) {
        Ok(lock) => lock,
        Err(error) => {
            return fail_with_error(standard_error, &error);
        }
    };
    let generation = match job::generation(&lock) {
        Ok(value) => value.to_string(),
        Err(error) => {
            return fail_with_error(standard_error, &error);
        }
    };
    let budget = match job::budget_from_lock(&lock) {
        Ok(value) => value,
        Err(error) => {
            return fail_with_error(standard_error, &error);
        }
    };
    cfg.max_iterations = budget.max_iterations;
    cfg.min_improvement = budget.min_improvement;
    cfg.wall_timeout = budget.wall_timeout;
    if dry_run {
        let pieces = match job::pieces_from_lock(&lock, &piece) {
            Ok(pieces) => pieces,
            Err(error) => {
                let _ = writeln!(standard_error, "anvil loop: dry-run: {error}");
                return 1;
            }
        };
        let _ = writeln!(
            standard_output,
            "anvil loop: dry-run plan pieces={} max_iterations={} wall_secs={} writes=0 executes=0",
            pieces.len(),
            cfg.max_iterations,
            cfg.wall_timeout.as_secs()
        );
        return 0;
    }
    let guard_signature = format!(
        "anvil-loop:{generation}:{}",
        if piece.is_empty() {
            "all"
        } else {
            piece.as_str()
        }
    );
    if crate::utility::memory_families::loop_guard_exhausted(&paths.home, &guard_signature, 2) {
        let _ = writeln!(
            standard_error,
            "anvil loop: loop-guard exhausted for {guard_signature}"
        );
        return 1;
    }
    let final_pass;
    let mut last_score = 0.0;
    let final_feedback;
    let mut loop_output_bytes = 0u64;
    let mut loop_output_token_estimate = 0u64;
    let mut loop_usage = cache::ProviderUsage::default();
    let mut loop_usage_measured = false;
    let mut loop_usage_source: Option<String> = None;
    let (iters, delta) = if paths.lock_path().is_file() {
        let workspace = match crate::utility::anvil::stamp::ensure_winner_workspace(&paths, strict)
        {
            Ok(path) => path,
            Err(error) => {
                let _ = writeln!(standard_error, "{error}");
                return 1;
            }
        };
        let gates = match refinement_gates(&paths, &piece) {
            Ok(gates) => gates,
            Err(error) => {
                let _ = writeln!(standard_error, "{error}");
                return 1;
            }
        };
        let builder_piece = if piece.is_empty() {
            "all".to_string()
        } else {
            piece.clone()
        };
        let prefix_text = match prefix::read_verified_prefix(&paths) {
            Ok(text) => text,
            Err(error) => {
                let _ = writeln!(standard_error, "{error}");
                return 1;
            }
        };
        let mut pass_state = false;
        let mut builder_error = None;
        let mut failure_feedback: Option<String> = None;
        let mut attempt = 0u64;
        let deadline = Instant::now() + cfg.wall_timeout;
        let result = run_bounded_loop(&cfg, || {
            attempt = attempt.saturating_add(1);
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                builder_error = Some("anvil loop: wall-clock budget exhausted".to_string());
                failure_feedback = Some(refinement_feedback(
                    attempt,
                    last_score,
                    "",
                    builder_error.as_deref(),
                ));
                return (false, last_score);
            }
            if let Err(error) = cast::write_builder_brief(
                &workspace,
                &builder_piece,
                &gates,
                &prefix_text,
                failure_feedback.as_deref(),
            ) {
                builder_error = Some(error.clone());
                failure_feedback = Some(refinement_feedback(
                    attempt,
                    last_score,
                    "",
                    builder_error.as_deref(),
                ));
                return (false, last_score);
            }
            let run_result = cast::run_builder_with_budget(
                &workspace,
                &builder_piece,
                &gates,
                remaining,
                budget.max_tool_chars,
                budget.max_tokens_loop,
            );
            let _ = std::fs::remove_file(workspace.join("BUILDER.md"));
            match run_result {
                Ok(run) => {
                    loop_output_bytes = loop_output_bytes.saturating_add(run.output_bytes);
                    loop_output_token_estimate =
                        loop_output_token_estimate.saturating_add(run.output_token_estimate);
                    if let Some(usage) = run.model_usage {
                        loop_usage.merge(&usage);
                        loop_usage_measured = true;
                        let source = format!("{}:json", run.provider);
                        report::merge_usage_source(&mut loop_usage_source, &source);
                    }
                    builder_error = None;
                    let scored = sieve::run_gates_scored_bounded(
                        &gates,
                        Some(&workspace),
                        budget.gate_timeout,
                        Some(deadline),
                    );
                    pass_state = scored.ok;
                    last_score = scored.rate();
                    failure_feedback = if scored.ok {
                        None
                    } else {
                        Some(refinement_feedback(
                            attempt,
                            scored.rate(),
                            &scored.logs,
                            None,
                        ))
                    };
                    (scored.ok, scored.rate())
                }
                Err(error) => {
                    builder_error = Some(error.clone());
                    pass_state = false;
                    failure_feedback = Some(refinement_feedback(attempt, 0.0, "", Some(&error)));
                    (false, last_score)
                }
            }
        });
        if let Some(error) = builder_error {
            let _ = writeln!(standard_error, "{error}");
        }
        final_pass = pass_state;
        final_feedback = failure_feedback;
        result
    } else {
        let _ = writeln!(
            standard_error,
            "anvil loop: missing lock at {}",
            paths.lock_path().display()
        );
        return 1;
    };
    let mut built = report::empty_report();
    built.loop_iterations = iters as u64;
    built.improvement_delta = delta;
    built.gate_pass_rate = if final_pass { 1.0 } else { last_score };
    built.refinement_feedback = final_feedback;
    match report::read_cast_metrics(&paths) {
        Ok(mut metrics) => {
            metrics.output_bytes = metrics.output_bytes.saturating_add(loop_output_bytes);
            metrics.output_token_estimate = metrics
                .output_token_estimate
                .saturating_add(loop_output_token_estimate);
            if loop_usage_measured {
                metrics.usage.merge(&loop_usage);
                metrics.measured_usage = true;
                if let Some(source) = loop_usage_source.as_deref() {
                    report::merge_usage_source(&mut metrics.usage_source, source);
                }
            }
            built.apply_cast_metrics(&metrics);
        }
        Err(error) => {
            let _ = writeln!(standard_error, "{error}");
            return 1;
        }
    }
    if !final_pass {
        match crate::utility::memory_families::bump_loop_guard(&paths.home, &guard_signature, 2) {
            Ok((_, true)) => {
                let _ = writeln!(
                    standard_error,
                    "anvil loop: loop-guard exhausted for {guard_signature}"
                );
            }
            // why: surface bump failures: a corrupt guard must be visible, not a silent fresh budget.
            Err(error) => {
                let _ = writeln!(
                    standard_error,
                    "anvil loop: loop-guard bump failed: {error}"
                );
            }
            Ok((_, false)) => {}
        }
    }
    if let Err(error) = report::write_report(&paths, &built) {
        let _ = writeln!(standard_error, "{error}");
        return 1;
    }
    let _ = writeln!(
        standard_output,
        "anvil loop: iters={iters} delta={delta:.3} pass={final_pass} strict={strict} {}",
        built.metrics_line()
    );
    if final_pass {
        0
    } else {
        1
    }
}

fn refinement_feedback(
    attempt: u64,
    gate_pass_rate: f64,
    gate_output: &str,
    builder_error: Option<&str>,
) -> String {
    serde_json::json!({
        "attempt": attempt,
        "gate_pass_rate": gate_pass_rate,
        "gate_output": supervisor::clip_output(gate_output, 2_000),
        "builder_error": builder_error.map(|error| supervisor::clip_output(error, 2_000)),
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loop_stops_when_gates_pass() {
        let cfg = LoopConfig {
            max_iterations: 20,
            min_improvement: 0.05,
            wall_timeout: Duration::from_secs(10),
        };
        let calls = std::cell::Cell::new(0);
        let (iters, _) = run_bounded_loop(&cfg, || {
            let count = calls.get();
            calls.set(count + 1);
            if count >= 2 {
                (true, 0.9)
            } else {
                (false, 0.3)
            }
        });
        assert!(iters <= 3);
    }

    #[test]
    fn loop_stops_on_min_improvement() {
        let cfg = LoopConfig {
            max_iterations: 20,
            min_improvement: 0.05,
            wall_timeout: Duration::from_secs(10),
        };
        let (iters, delta) = run_bounded_loop(&cfg, || (false, 0.5));
        assert!(iters >= 1);
        assert!(delta.abs() < 0.05 || iters == 1);
    }

    #[test]
    fn loop_stops_on_max_iterations() {
        let cfg = LoopConfig {
            max_iterations: 5,
            min_improvement: 0.001,
            wall_timeout: Duration::from_secs(10),
        };
        let (iters, _) = run_bounded_loop(&cfg, || (false, 0.1));
        assert!(iters <= 5);
    }

    #[test]
    fn loop_continues_while_gate_score_improves() {
        let cfg = LoopConfig {
            max_iterations: 10,
            min_improvement: 0.05,
            wall_timeout: Duration::from_secs(10),
        };
        let scores = [0.0, 0.25, 0.5, 0.8, 1.0];
        let index = std::cell::Cell::new(0);
        let (iters, _) = run_bounded_loop(&cfg, || {
            let i = index.get();
            let score = scores[i.min(scores.len() - 1)];
            index.set(i + 1);
            (score >= 1.0, score)
        });
        assert!(
            iters >= 4,
            "fractional improvement must keep iterating, iters={iters}"
        );
    }

    #[test]
    fn loop_stops_when_score_regresses() {
        let cfg = LoopConfig {
            max_iterations: 10,
            min_improvement: 0.05,
            wall_timeout: Duration::from_secs(10),
        };
        let scores = [0.8, 0.2, 0.9];
        let index = std::cell::Cell::new(0);
        let (iters, delta) = run_bounded_loop(&cfg, || {
            let i = index.get();
            index.set(i + 1);
            (false, scores[i.min(scores.len() - 1)])
        });
        assert_eq!(iters, 2);
        assert!(delta < 0.0);
    }
}
