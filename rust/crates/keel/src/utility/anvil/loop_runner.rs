use std::io::Write;
use std::time::{Duration, Instant};

use crate::args::FlagSet;
use crate::utility::anvil::cast;
use crate::utility::anvil::job;
use crate::utility::anvil::report;
use crate::utility::anvil::sieve;

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
        if prev.is_some() && improvement.abs() < config.min_improvement {
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
    if strict {
        cfg.min_improvement = 0.02;
        cfg.max_iterations = 30;
    }
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
    let final_pass;
    let (iters, delta) = if dry_run {
        let piece_ref = piece.clone();
        let mut pass_state = false;
        let result = run_bounded_loop(&cfg, || match sieve::sieve_lock(&paths, &piece_ref, &[]) {
            Ok(outcome) => {
                pass_state = outcome.ok;
                (outcome.ok, if outcome.ok { 1.0 } else { 0.0 })
            }
            Err(_) => {
                pass_state = false;
                (false, 0.0)
            }
        });
        final_pass = pass_state;
        result
    } else if paths.lock_path().is_file() {
        let workspace = paths.out_dir().join("workspace");
        if !workspace.is_dir() {
            let _ = writeln!(
                standard_error,
                "anvil loop: no promoted winner workspace at {}",
                workspace.display()
            );
            return 1;
        }
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
        let mut pass_state = false;
        let mut builder_error = None;
        let result = run_bounded_loop(&cfg, || {
            match cast::run_builder(&workspace, &builder_piece, &gates) {
                Ok(_) => {
                    let (pass, _) = sieve::run_gates_in_directory(&gates, Some(&workspace));
                    pass_state = pass;
                    (pass, if pass { 1.0 } else { 0.0 })
                }
                Err(error) => {
                    builder_error = Some(error);
                    pass_state = false;
                    (false, 0.0)
                }
            }
        });
        if let Some(error) = builder_error {
            let _ = writeln!(standard_error, "{error}");
        }
        final_pass = pass_state;
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
    built.gate_pass_rate = if final_pass { 1.0 } else { 0.0 };
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
}
