pub mod cache;
pub mod cast;
pub mod compile;
pub mod filter;
pub mod job;
pub mod lock;
pub mod loop_runner;
pub mod prefix;
pub mod report;
pub mod sieve;
pub mod stamp;
pub mod supervisor;
pub mod workspace;

use std::io::Write;

use crate::args::FlagSet;

pub fn run_anvil_command(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let action = arguments.first().map(String::as_str).unwrap_or("");
    if action.is_empty() || matches!(action, "help" | "--help" | "-h") {
        let _ = writeln!(
            standard_output,
            "Usage: keel anvil <compile|cast|sieve|stamp|loop|run|prefix-check> [flags]\n\
             \n\
             compile      goal -> lock + prefix + gates (1 frontier call)\n\
             cast         run N builders in isolated workspaces\n\
             sieve        run gates only (0 LLM)\n\
             stamp        PPT over survivors (logprob EV)\n\
             loop         bounded refinement if gates fail (max_iterations, delta)\n\
             run          compile->cast->sieve->stamp->loop orchestrator\n\
             prefix-check verify prefix SHA256 stability\n\
             \n\
             Common flags: --workspace-root <path> --claude-home <path> --json --dry-run --strict\n\
             Bank: <keel-home>/memories/workspaces/<slug>/anvil/ (never the user workspace)"
        );
        return if action.is_empty() { 1 } else { 0 };
    }
    match action {
        "compile" => {
            let code = compile::run_compile(&arguments[1..], standard_output, standard_error);
            if code == 0 {
                crate::runner::hook_lifecycle::record_anvil_gate_clear();
            }
            code
        }
        "cast" => {
            let code = cast::run_cast(&arguments[1..], standard_output, standard_error);
            if code == 0 {
                crate::runner::hook_lifecycle::record_anvil_gate_clear();
            }
            code
        }
        "sieve" => sieve::run_sieve(&arguments[1..], standard_output, standard_error),
        "stamp" => stamp::run_stamp(&arguments[1..], standard_output, standard_error),
        "loop" => loop_runner::run_loop(&arguments[1..], standard_output, standard_error),
        "run" => {
            let code = run_orchestrator(&arguments[1..], standard_output, standard_error);
            if code == 0 {
                crate::runner::hook_lifecycle::record_anvil_gate_clear();
            }
            code
        }
        "prefix-check" => {
            prefix::run_prefix_check(&arguments[1..], standard_output, standard_error)
        }
        other => {
            let _ = writeln!(standard_error, "anvil: unknown subcommand: {other}");
            1
        }
    }
}

fn run_orchestrator(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flags = FlagSet::new("anvil run");
    flags.bool_flag("dry-run", false);
    flags.bool_flag("strict", false);
    flags.bool_flag("json", false);
    flags.string_flag("goal", "");
    flags.string_flag("bar", "");
    flags.string_flag("files", "");
    flags.string_flag("workspace-root", "");
    flags.string_flag("claude-home", "");
    if let Err(error) = flags.parse(arguments) {
        let _ = writeln!(standard_error, "{}", error.message);
        return 1;
    }
    let dry_run = flags.bool_value("dry-run");
    let strict = flags.bool_value("strict");
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
    let mut goal = flags.string_value("goal").trim().to_string();
    let mut bar = flags.string_value("bar").trim().to_string();
    if dry_run && goal.is_empty() {
        goal = "offline anvil demo".into();
    }
    if dry_run && bar.is_empty() {
        bar = "echo ok".into();
    }
    if !paths.lock_path().is_file() {
        if goal.is_empty() {
            let _ = writeln!(
                standard_error,
                "anvil run: --goal is required when no lock exists"
            );
            return 1;
        }
        if bar.is_empty() {
            let _ = writeln!(
                standard_error,
                "anvil run: --bar is required when no lock exists"
            );
            return 1;
        }
        let files: Vec<String> = flags
            .string_value("files")
            .split(',')
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(str::to_string)
            .collect();
        if let Err(error) = compile::write_lock(&paths, &goal, &bar, &files, "") {
            let _ = writeln!(standard_error, "anvil run: compile: {error}");
            return 1;
        }
    }
    let mut shared = Vec::new();
    if dry_run {
        shared.push("--dry-run".into());
    }
    if strict {
        shared.push("--strict".into());
    }
    shared.push("--workspace-root".into());
    shared.push(paths.workspace.display().to_string());
    shared.push("--claude-home".into());
    shared.push(paths.home.display().to_string());

    if cast::run_cast(&shared, standard_output, standard_error) != 0 {
        return 1;
    }
    let sieve_outcome = match sieve::sieve_lock(&paths, "", &[]) {
        Ok(outcome) => outcome,
        Err(error) => {
            let _ = writeln!(standard_error, "{error}");
            return 1;
        }
    };
    if sieve_outcome.ok {
        let _ = writeln!(
            standard_output,
            "anvil sieve: PASS greens={} critic={}",
            sieve_outcome.greens, sieve_outcome.critic
        );
    } else {
        let _ = writeln!(
            standard_error,
            "anvil sieve: FAIL greens={} critic={}\n{}",
            sieve_outcome.greens, sieve_outcome.critic, sieve_outcome.logs
        );
    }
    let sieve_ok = sieve_outcome.ok;
    let skip_stamp = sieve_outcome.skip_stamp;
    let mut stamp_used = false;
    if !skip_stamp {
        if stamp::run_stamp(&shared, standard_output, standard_error) != 0 {
            return 1;
        }
        stamp_used = true;
    }
    let mut loop_iterations = 0;
    if !sieve_ok {
        if loop_runner::run_loop(&shared, standard_output, standard_error) != 0 {
            return 1;
        }
        loop_iterations = 1;
    }
    let mut built = report::empty_report();
    built.stamp_used = stamp_used;
    built.winner_id = if stamp_used {
        "cast_0".into()
    } else {
        "sieve".into()
    };
    built.gate_pass_rate = if sieve_ok { 1.0 } else { 0.0 };
    built.loop_iterations = loop_iterations;
    if let Err(error) = report::write_report(&paths, &built) {
        let _ = writeln!(standard_error, "{error}");
        return 1;
    }
    let _ = writeln!(
        standard_output,
        "{} json={}",
        built.metrics_line(),
        built.to_json()
    );
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TempJob {
        workspace: PathBuf,
        home: PathBuf,
        paths: job::JobPaths,
    }

    impl Drop for TempJob {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.workspace);
            let _ = std::fs::remove_dir_all(&self.home);
        }
    }

    fn temp_job(label: &str) -> TempJob {
        let stamp = format!(
            "{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let workspace = std::env::temp_dir().join(format!("anvil-{label}-ws-{stamp}"));
        let home = std::env::temp_dir().join(format!("anvil-{label}-home-{stamp}"));
        let _ = std::fs::remove_dir_all(&workspace);
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        let paths = job::JobPaths::from_resolved(workspace.clone(), home.clone());
        TempJob {
            workspace,
            home,
            paths,
        }
    }

    fn with_job(job: &TempJob, mut args: Vec<String>) -> Vec<String> {
        args.push("--workspace-root".into());
        args.push(job.workspace.display().to_string());
        args.push("--claude-home".into());
        args.push(job.home.display().to_string());
        args
    }

    fn run_cmd(args: &[String]) -> (u8, String, String) {
        let mut out = Cursor::new(Vec::new());
        let mut err = Cursor::new(Vec::new());
        let code = run_anvil_command(args, &mut out, &mut err);
        (
            code,
            String::from_utf8_lossy(out.get_ref()).into_owned(),
            String::from_utf8_lossy(err.get_ref()).into_owned(),
        )
    }
    static BUILDER_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn with_builder<F: FnOnce() -> R, R>(run: F) -> R {
        let _guard = match BUILDER_ENV_LOCK.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let previous = std::env::var("KEEL_ANVIL_BUILDER_ARGV").ok();
        let builder = if cfg!(windows) {
            r#"["cmd.exe","/C","echo built>built.txt"]"#
        } else {
            r#"["sh","-c","printf built > built.txt"]"#
        };
        std::env::set_var("KEEL_ANVIL_BUILDER_ARGV", builder);
        let result = run();
        match previous {
            Some(value) => std::env::set_var("KEEL_ANVIL_BUILDER_ARGV", value),
            None => std::env::remove_var("KEEL_ANVIL_BUILDER_ARGV"),
        }
        result
    }

    #[test]
    fn empty_action_prints_usage_and_fails() {
        let (code, stdout, _) = run_cmd(&[]);
        assert_eq!(code, 1);
        assert!(stdout.contains("Usage: keel anvil"));
    }

    #[test]
    fn unknown_subcommand_fails() {
        let (code, _, stderr) = run_cmd(&["nope".into()]);
        assert_eq!(code, 1);
        assert!(stderr.contains("unknown subcommand"));
    }

    #[test]
    fn compile_requires_goal() {
        let (code, _, stderr) = run_cmd(&["compile".into()]);
        assert_eq!(code, 1);
        assert!(stderr.contains("--goal is required"));
    }

    #[test]
    fn compile_without_bar_proposes_named_bars() {
        let (code, stdout, _) = run_cmd(&["compile".into(), "--goal".into(), "pretty json".into()]);
        assert_eq!(code, 0);
        assert!(stdout.contains("\"bars\""));
        assert!(stdout.contains("jq 1.7"));
        assert!(stdout.contains("echo ok"));
    }

    #[test]
    fn compile_writes_lock_prefix_and_gates() {
        let job = temp_job("compile");
        let (code, stdout, stderr) = run_cmd(&with_job(
            &job,
            vec![
                "compile".into(),
                "--goal".into(),
                "pretty json".into(),
                "--bar".into(),
                "echo ok".into(),
            ],
        ));
        assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
        assert!(job.paths.lock_path().is_file());
        assert!(job.paths.prefix_path().is_file());
        assert!(job.paths.prefix_hash_path().is_file());
        assert!(job.paths.gates_dir().join("main").is_file());
        assert!(!job.workspace.join("anvil").exists());
        let lock = job::load_lock(&job.paths).expect("lock");
        assert_eq!(lock["goal"], "pretty json");
    }

    #[test]
    fn compile_bar_drives_failing_sieve_and_loop() {
        let job = temp_job("failing-bar");
        let (compile_code, _, compile_stderr) = run_cmd(&with_job(
            &job,
            vec![
                "compile".into(),
                "--goal".into(),
                "failing quality bar".into(),
                "--bar".into(),
                "exit 7".into(),
            ],
        ));
        assert_eq!(compile_code, 0, "compile stderr={compile_stderr}");
        let lock = job::load_lock(&job.paths).expect("lock");
        assert_eq!(lock["pieces"][0]["gates"][0], "exit 7");
        assert_eq!(
            std::fs::read_to_string(job.paths.gates_dir().join("main")).expect("gate"),
            "exit 7\n"
        );

        let (sieve_code, _, sieve_stderr) = run_cmd(&with_job(&job, vec!["sieve".into()]));
        assert_eq!(sieve_code, 1, "sieve stderr={sieve_stderr}");
        assert!(sieve_stderr.contains("FAIL"));

        let (loop_code, loop_stdout, loop_stderr) = run_cmd(&with_job(&job, vec!["loop".into()]));
        assert_eq!(loop_code, 1, "stdout={loop_stdout} stderr={loop_stderr}");
        assert!(loop_stdout.is_empty());
        assert!(loop_stderr.contains("no promoted winner workspace"));
        assert!(!job.paths.report_path().is_file());
    }

    #[test]
    fn compile_rejects_missing_workspace() {
        let (code, _, stderr) = run_cmd(&[
            "compile".into(),
            "--goal".into(),
            "x".into(),
            "--bar".into(),
            "echo ok".into(),
            "--workspace-root".into(),
            "D:/this-anvil-path-does-not-exist".into(),
        ]);
        assert_eq!(code, 1);
        assert!(stderr.contains("workspace-root not a directory"));
    }

    #[test]
    fn live_cast_uses_configured_host_builder() {
        let job = temp_job("cast-host");
        let _ = run_cmd(&with_job(
            &job,
            vec![
                "compile".into(),
                "--goal".into(),
                "g".into(),
                "--bar".into(),
                "echo ok".into(),
            ],
        ));
        let (code, stdout, stderr) = with_builder(|| run_cmd(&with_job(&job, vec!["cast".into()])));
        assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
        assert!(job.paths.dir.join("cast_0").join("result.json").is_file());
        assert!(!job.workspace.join("anvil").exists());
        let result = std::fs::read_to_string(job.paths.dir.join("cast_0").join("result.json"))
            .expect("result");
        let value: serde_json::Value = serde_json::from_str(&result).expect("json");
        let isolated = value["workspace"].as_str().unwrap_or("");
        assert!(isolated.contains("cast_0"));
        assert!(std::path::Path::new(isolated).starts_with(&job.paths.dir));
        assert!(std::path::Path::new(isolated).join("built.txt").is_file());
    }

    #[test]
    fn dry_run_cast_without_lock_fails() {
        let job = temp_job("cast-nolock");
        let (code, _, stderr) = run_cmd(&with_job(&job, vec!["cast".into(), "--dry-run".into()]));
        assert_eq!(code, 1);
        assert!(stderr.contains("missing lock"));
        assert!(!job.workspace.join("anvil").exists());
    }

    #[test]
    fn dry_run_cast_writes_result_json() {
        let job = temp_job("cast");
        let _ = run_cmd(&with_job(
            &job,
            vec![
                "compile".into(),
                "--goal".into(),
                "g".into(),
                "--bar".into(),
                "echo ok".into(),
            ],
        ));
        let (code, stdout, stderr) =
            run_cmd(&with_job(&job, vec!["cast".into(), "--dry-run".into()]));
        assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
        assert!(job.paths.dir.join("cast_0").join("result.json").is_file());
        assert!(!job.workspace.join("anvil").exists());
    }

    #[test]
    fn dry_run_cast_unknown_piece_fails() {
        let job = temp_job("cast-piece");
        let _ = run_cmd(&with_job(
            &job,
            vec![
                "compile".into(),
                "--goal".into(),
                "g".into(),
                "--bar".into(),
                "echo ok".into(),
            ],
        ));
        let (code, _, stderr) = run_cmd(&with_job(
            &job,
            vec![
                "cast".into(),
                "--dry-run".into(),
                "--piece".into(),
                "missing".into(),
            ],
        ));
        assert_eq!(code, 1);
        assert!(stderr.contains("not in lock"));
    }

    #[test]
    fn sieve_without_lock_or_gates_fails() {
        let job = temp_job("sieve-nolock");
        let (code, _, stderr) = run_cmd(&with_job(&job, vec!["sieve".into()]));
        assert_eq!(code, 1);
        assert!(stderr.contains("missing lock"));
        assert!(!job.workspace.join("anvil").exists());
    }

    #[test]
    fn sieve_override_failing_gate_fails() {
        let (code, _, stderr) = run_cmd(&[
            "sieve".into(),
            "--gates".into(),
            "anvil-gate-command-that-does-not-exist".into(),
        ]);
        assert_eq!(code, 1);
        assert!(stderr.contains("FAIL"));
    }

    #[test]
    fn sieve_echo_ok_passes() {
        let (code, stdout, _) = run_cmd(&["sieve".into(), "--gates".into(), "echo ok".into()]);
        assert_eq!(code, 0);
        assert!(stdout.contains("PASS"));
    }

    #[test]
    fn stamp_uses_evidence_winner() {
        let job = temp_job("stamp-host");
        let _ = run_cmd(&with_job(
            &job,
            vec![
                "compile".into(),
                "--goal".into(),
                "g".into(),
                "--bar".into(),
                "echo ok".into(),
            ],
        ));
        let _ = run_cmd(&with_job(&job, vec!["cast".into(), "--dry-run".into()]));
        let (code, stdout, stderr) = run_cmd(&with_job(&job, vec!["stamp".into()]));
        assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
        assert!(stdout.contains("mode=evidence"));
        assert!(job.paths.out_dir().join("winner.txt").is_file());
        assert!(!job.workspace.join("anvil").exists());
    }

    #[test]
    fn dry_run_stamp_uses_evidence() {
        let (code, stdout, _) = run_cmd(&["stamp".into(), "--dry-run".into()]);
        assert_eq!(code, 0);
        assert!(stdout.contains("mode=evidence"));
        assert!(!stdout.contains("stub"));
        assert!(stdout.contains("strict=false"));
    }

    #[test]
    fn dry_run_stamp_strict_reports_strict_mode() {
        let (code, stdout, _) = run_cmd(&["stamp".into(), "--dry-run".into(), "--strict".into()]);
        assert_eq!(code, 0);
        assert!(stdout.contains("mode=evidence"));
        assert!(stdout.contains("strict=true"));
    }

    #[test]
    fn live_run_uses_configured_host_builder() {
        let job = temp_job("run-host");
        let (code, stdout, stderr) = with_builder(|| {
            run_cmd(&with_job(
                &job,
                vec![
                    "run".into(),
                    "--goal".into(),
                    "offline anvil demo".into(),
                    "--bar".into(),
                    "echo ok".into(),
                ],
            ))
        });
        assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
        assert!(job.paths.report_path().is_file());
        assert!(!job.workspace.join("anvil").exists());
    }

    #[test]
    fn run_without_lock_or_goal_fails() {
        let job = temp_job("run-nogoal");
        let (code, _, stderr) = run_cmd(&with_job(&job, vec!["run".into()]));
        assert_eq!(code, 1);
        assert!(stderr.contains("--goal is required"));
        assert!(!job.workspace.join("anvil").exists());
    }

    #[test]
    fn prefix_check_without_prefix_or_file_fails() {
        let job = temp_job("prefix-empty");
        let (code, _, stderr) = run_cmd(&with_job(&job, vec!["prefix-check".into()]));
        assert_eq!(code, 1);
        assert!(stderr.contains("provide --prefix or compile first"));
    }

    #[test]
    fn prefix_check_after_compile_is_stable() {
        let job = temp_job("prefix-ok");
        let _ = run_cmd(&with_job(
            &job,
            vec![
                "compile".into(),
                "--goal".into(),
                "g".into(),
                "--bar".into(),
                "echo ok".into(),
            ],
        ));
        let (code, stdout, stderr) = run_cmd(&with_job(&job, vec!["prefix-check".into()]));
        assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
        assert!(stdout.contains("prefix sha256:"));
    }

    #[test]
    fn dry_run_pipeline_writes_lock_and_report() {
        let job = temp_job("run");
        let (code, stdout, stderr) =
            run_cmd(&with_job(&job, vec!["run".into(), "--dry-run".into()]));
        assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
        assert!(!stdout.contains("stub"), "stdout={stdout}");
        assert!(job.paths.lock_path().is_file());
        assert!(job.paths.report_path().is_file());
        assert!(!job.workspace.join("anvil").exists());
    }

    #[test]
    fn later_command_resumes_same_global_bank() {
        let job = temp_job("resume");
        let (code, _, stderr) = run_cmd(&with_job(
            &job,
            vec![
                "compile".into(),
                "--goal".into(),
                "resume goal".into(),
                "--bar".into(),
                "echo ok".into(),
            ],
        ));
        assert_eq!(code, 0, "stderr={stderr}");
        let (code, _, stderr) = run_cmd(&with_job(&job, vec!["cast".into(), "--dry-run".into()]));
        assert_eq!(code, 0, "stderr={stderr}");
        let lock = job::load_lock(&job.paths).expect("lock survives");
        assert_eq!(lock["goal"], "resume goal");
        assert!(job.paths.dir.join("cast_0").join("result.json").is_file());
        assert!(!job.workspace.join("anvil").exists());
    }
}
