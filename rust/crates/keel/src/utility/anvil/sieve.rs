use std::io::Write;

use crate::args::FlagSet;
use crate::utility::anvil::filter::compress_output;
use crate::utility::anvil::job;

pub struct SieveOutcome {
    pub ok: bool,
    pub greens: u64,
    pub pass_rate: f64,
    pub logs: String,
    pub skip_stamp: bool,
    pub critic: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GateScore {
    pub ok: bool,
    pub passed: u64,
    pub total: u64,
    pub logs: String,
}

impl GateScore {
    pub fn rate(&self) -> f64 {
        gate_pass_rate(self.passed, self.total)
    }
}

pub fn gate_pass_rate(passed: u64, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        passed as f64 / total as f64
    }
}

pub fn run_gates(gates: &[String]) -> (bool, String) {
    run_gates_in_directory(gates, None)
}

pub fn run_gates_in_directory(
    gates: &[String],
    working_directory: Option<&std::path::Path>,
) -> (bool, String) {
    let scored = run_gates_scored(gates, working_directory);
    (scored.ok, scored.logs)
}

pub fn run_gates_scored(
    gates: &[String],
    working_directory: Option<&std::path::Path>,
) -> GateScore {
    run_gates_scored_bounded(
        gates,
        working_directory,
        std::time::Duration::from_secs(300),
        None,
    )
}

pub fn run_gates_scored_bounded(
    gates: &[String],
    working_directory: Option<&std::path::Path>,
    gate_timeout: std::time::Duration,
    deadline: Option<std::time::Instant>,
) -> GateScore {
    let mut all_ok = true;
    let mut passed = 0u64;
    let mut total = 0u64;
    let mut logs = String::new();
    for gate in gates {
        let trimmed = gate.trim();
        if trimmed.is_empty() {
            continue;
        }
        total += 1;
        let timeout = deadline
            .map(|value| value.saturating_duration_since(std::time::Instant::now()))
            .map(|remaining| remaining.min(gate_timeout))
            .unwrap_or(gate_timeout);
        if timeout.is_zero() {
            all_ok = false;
            logs.push_str(&format!(
                "gate={trimmed} status=error error=wall-clock budget exhausted\n"
            ));
            continue;
        }
        let (program, arguments) = crate::runtime::platform_shell_command_parts(trimmed);
        match crate::runtime::run_command_with_timeout(
            &program,
            &arguments,
            working_directory,
            timeout,
        ) {
            Ok(result) => {
                let gate_passed = result.code == 0;
                if gate_passed {
                    passed += 1;
                } else {
                    all_ok = false;
                }
                logs.push_str(&format!(
                    "gate={trimmed} exit_code={} status={}\n",
                    result.code,
                    if gate_passed { "pass" } else { "fail" }
                ));
                logs.push_str(&String::from_utf8_lossy(&result.stdout));
                logs.push_str(&String::from_utf8_lossy(&result.stderr));
            }
            Err(error) => {
                all_ok = false;
                logs.push_str(&format!("gate={trimmed} status=error error={error}\n"));
            }
        }
    }
    GateScore {
        ok: all_ok && total > 0,
        passed,
        total,
        logs: compress_output(&logs),
    }
}

pub fn sieve_lock(
    paths: &job::JobPaths,
    piece: &str,
    override_gates: &[String],
) -> Result<SieveOutcome, String> {
    sieve_lock_in_directory(paths, piece, override_gates, &paths.workspace)
}

pub fn sieve_lock_in_directory(
    paths: &job::JobPaths,
    piece: &str,
    override_gates: &[String],
    working_directory: &std::path::Path,
) -> Result<SieveOutcome, String> {
    let lock = job::load_lock(paths)?;
    let budget = job::budget_from_lock(&lock)?;
    let pieces = job::pieces_from_lock(&lock, piece)?;
    let mut greens = 0u64;
    let mut pieces_total = 0u64;
    let mut ok = true;
    let mut logs = String::new();
    let mut has_blind = false;
    let deadline = std::time::Instant::now() + budget.wall_timeout;
    for spec in pieces {
        pieces_total += 1;
        if spec.critic == "blind_ab" {
            has_blind = true;
        }
        let gates = if override_gates.is_empty() {
            spec.gates.clone()
        } else {
            override_gates.to_vec()
        };
        if spec.critic == "none" && gates.is_empty() {
            return Err(format!(
                "anvil sieve: piece {} critic:none has no gates",
                spec.id
            ));
        }
        let scored = run_gates_scored_bounded(
            &gates,
            Some(working_directory),
            budget.gate_timeout,
            Some(deadline),
        );
        let pass = scored.ok;
        let piece_logs = scored.logs;
        logs.push_str(&piece_logs);
        if pass && !gates.is_empty() {
            greens += 1;
        } else {
            ok = false;
        }
    }
    if greens == 0 && override_gates.is_empty() {
        ok = false;
    }
    let skip_stamp = if has_blind { greens < 2 } else { greens >= 1 };
    Ok(SieveOutcome {
        ok,
        greens,
        pass_rate: gate_pass_rate(greens, pieces_total),
        logs,
        skip_stamp,
        critic: if has_blind {
            "blind_ab".into()
        } else {
            "none".into()
        },
    })
}

pub fn run_sieve(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flags = FlagSet::new("anvil sieve");
    flags.string_flag("piece", "");
    flags.string_flag("gates", "");
    flags.string_flag("workspace-root", "");
    flags.string_flag("claude-home", "");
    if let Err(error) = flags.parse(arguments) {
        let _ = writeln!(standard_error, "{}", error.message);
        return 1;
    }
    let gates_str = flags.string_value("gates").to_string();
    let override_gates: Vec<String> = if gates_str.trim().is_empty() {
        Vec::new()
    } else {
        vec![gates_str]
    };
    if !override_gates.is_empty() {
        let (ok, logs) = run_gates(&override_gates);
        return emit_sieve(ok, 0, &logs, standard_output, standard_error);
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
    match sieve_lock(&paths, flags.string_value("piece"), &override_gates) {
        Ok(outcome) => emit_sieve(
            outcome.ok,
            outcome.greens,
            &outcome.logs,
            standard_output,
            standard_error,
        ),
        Err(error) => {
            let _ = writeln!(standard_error, "{error}");
            1
        }
    }
}

fn emit_sieve(
    ok: bool,
    greens: u64,
    logs: &str,
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    if ok {
        let _ = writeln!(standard_output, "anvil sieve: PASS greens={greens}\n{logs}");
        0
    } else {
        let _ = writeln!(standard_error, "anvil sieve: FAIL greens={greens}\n{logs}");
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_gates_cannot_pass_without_evidence() {
        assert!(!run_gates(&[]).0);
        assert!(!run_gates(&[" \n".into()]).0);
    }

    #[test]
    fn gate_pass_rate_is_fractional() {
        assert!((gate_pass_rate(1, 4) - 0.25).abs() < 1e-9);
        assert_eq!(gate_pass_rate(0, 2), 0.0);
        assert_eq!(gate_pass_rate(2, 2), 1.0);
        assert_eq!(gate_pass_rate(0, 0), 0.0);
    }

    #[test]
    fn scored_gates_count_partial_passes() {
        let pass = "echo ok".to_string();
        let fail = if cfg!(windows) {
            "cmd /C exit 7".to_string()
        } else {
            "sh -c 'exit 7'".to_string()
        };
        let scored = run_gates_scored(&[pass, fail], None);
        assert!(!scored.ok);
        assert_eq!(scored.passed, 1);
        assert_eq!(scored.total, 2);
        assert!((scored.rate() - 0.5).abs() < 1e-9);
    }

    #[test]
    fn failed_gate_reports_command_and_exit_code() {
        let gate = if cfg!(windows) {
            "cmd /C exit 7"
        } else {
            "sh -c 'exit 7'"
        };
        let (ok, logs) = run_gates(&[gate.to_string()]);
        assert!(!ok);
        assert!(logs.contains("status=fail"), "logs: {logs}");
        assert!(
            logs.contains("exit_code=1") || logs.contains("exit_code=7"),
            "logs: {logs}"
        );
    }

    #[test]
    fn bounded_gate_times_out_instead_of_sticking() {
        let gate = if cfg!(windows) {
            "Start-Sleep -Seconds 5"
        } else {
            "sleep 5"
        };
        let started = std::time::Instant::now();
        let scored = run_gates_scored_bounded(
            &[gate.to_string()],
            None,
            std::time::Duration::from_millis(100),
            None,
        );
        assert!(!scored.ok);
        let elapsed = started.elapsed();
        assert!(
            elapsed < std::time::Duration::from_millis(4_500),
            "bounded gate took {elapsed:?}"
        );
        assert!(scored.logs.contains("timed out"), "logs={}", scored.logs);
    }
}
