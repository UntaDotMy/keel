pub mod cache;
pub mod cast;
pub mod clarify;
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
             compile      goal -> lock + prefix + gates from --goal/--bar/--files\n\
             cast         run N builders in isolated workspaces (host CLI argv)\n\
             sieve        run gates only (0 LLM)\n\
             stamp        PPT over evidence strengths (Bradley-Terry ring; --strict fail-closed)\n\
             loop         bounded refinement if gates fail (max_iterations, delta)\n\
             run          compile->cast->sieve->stamp->loop orchestrator\n\
             prefix-check verify prefix SHA256 stability\n\
             \n\
             Common flags: --workspace-root <path> --claude-home <path> --json --dry-run --strict --clarify-required\n\
             Bank: <keel-home>/memories/workspaces/<slug>/anvil/ (never the user workspace)\n             ClarifyPacket: clarify.packet.json — when gated, compile refuses on missing/hard_block/drift"
        );
        return if action.is_empty() { 1 } else { 0 };
    }
    let dry_run = arguments[1..]
        .iter()
        .any(|argument| argument == "--dry-run");
    let compile_has_bar = arguments[1..]
        .windows(2)
        .find(|pair| pair[0] == "--bar")
        .is_some_and(|pair| !pair[1].trim().is_empty());
    let writes_state = ((action == "compile" && compile_has_bar)
        || matches!(action, "cast" | "stamp" | "loop" | "run"))
        && !(dry_run && action != "compile");
    let _lease = if writes_state {
        let value_after = |flag: &str| {
            arguments[1..]
                .windows(2)
                .find(|pair| pair[0] == flag)
                .map(|pair| pair[1].as_str())
                .unwrap_or("")
        };
        let paths = match job::JobPaths::resolve(
            value_after("--workspace-root"),
            value_after("--claude-home"),
        ) {
            Ok(paths) => paths,
            Err(error) => {
                let _ = writeln!(standard_error, "{error}");
                return 1;
            }
        };
        match job::JobLease::acquire(&paths) {
            Ok(lease) => Some(lease),
            Err(error) => {
                let _ = writeln!(standard_error, "{error}");
                return 1;
            }
        }
    } else {
        None
    };
    match action {
        "compile" => compile::run_compile(&arguments[1..], standard_output, standard_error),
        "cast" => cast::run_cast(&arguments[1..], standard_output, standard_error),
        "sieve" => sieve::run_sieve(&arguments[1..], standard_output, standard_error),
        "stamp" => stamp::run_stamp(&arguments[1..], standard_output, standard_error),
        "loop" => loop_runner::run_loop(&arguments[1..], standard_output, standard_error),
        "run" => {
            let code = run_orchestrator(&arguments[1..], standard_output, standard_error);
            if should_clear_edit_gate("run", &arguments[1..], code) {
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

fn should_clear_edit_gate(action: &str, arguments: &[String], code: u8) -> bool {
    code == 0 && action == "run" && arguments.iter().any(|argument| argument == "--dry-run")
}

fn fail_dry_run(standard_error: &mut dyn Write, error: &str) -> u8 {
    let _ = writeln!(standard_error, "anvil run: dry-run: {error}");
    1
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
    let goal = flags.string_value("goal").trim().to_string();
    let bar = flags.string_value("bar").trim().to_string();
    let files: Vec<String> = flags
        .string_value("files")
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect();
    if dry_run {
        let lock = if paths.lock_path().is_file()
            && goal.is_empty()
            && bar.is_empty()
            && files.is_empty()
        {
            match job::load_lock(&paths) {
                Ok(lock) => lock,
                Err(error) => {
                    return fail_dry_run(standard_error, &error);
                }
            }
        } else {
            if goal.is_empty() || bar.is_empty() || files.is_empty() {
                let _ = writeln!(
                    standard_error,
                    "anvil run: dry-run requires --goal, --bar, and --files when no current lock is selected"
                );
                return 1;
            }
            let lock = compile::build_lock_value(&goal, &bar, &files, "dry-run");
            let serialized = match serde_json::to_string(&lock) {
                Ok(value) => value,
                Err(error) => {
                    let _ = writeln!(standard_error, "anvil run: dry-run lock: {error}");
                    return 1;
                }
            };
            if let Err(error) = lock::validate_lock(&serialized) {
                let _ = writeln!(standard_error, "anvil run: dry-run lock: {error}");
                return 1;
            }
            lock
        };
        let pieces = match job::pieces_from_lock(&lock, "") {
            Ok(pieces) => pieces,
            Err(error) => {
                return fail_dry_run(standard_error, &error);
            }
        };
        for piece in &pieces {
            if let Err(error) = workspace::validate_workspace_files(&paths.workspace, &piece.files)
            {
                return fail_dry_run(standard_error, &error);
            }
        }
        let casts = job::n_casts(&lock);
        let _ = writeln!(
            standard_output,
            "anvil run: dry-run plan pieces={} casts={} gates={} writes=0 executes=0",
            pieces.len(),
            casts,
            pieces.iter().map(|piece| piece.gates.len()).sum::<usize>()
        );
        return 0;
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
        if let Err(error) = compile::write_lock(&paths, &goal, &bar, &files, "") {
            let _ = writeln!(standard_error, "anvil run: compile: {error}");
            return 1;
        }
    }
    let mut shared = Vec::new();
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
    let candidate_workspace = match stamp::ensure_winner_workspace(&paths, strict) {
        Ok(workspace) => workspace,
        Err(error) => {
            let _ = writeln!(standard_error, "{error}");
            return 1;
        }
    };
    let sieve_outcome = match sieve::sieve_lock_in_directory(&paths, "", &[], &candidate_workspace)
    {
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
    let mut loop_failed = false;
    let mut loop_report = None;
    if !sieve_ok {
        loop_failed = loop_runner::run_loop(&shared, standard_output, standard_error) != 0;
        loop_report = report::read_report(&paths).ok();
    }
    let stamp_winner = if stamp_used {
        read_stamp_winner(&paths)
    } else {
        None
    };
    let built = merge_pipeline_metrics(
        sieve_ok,
        sieve_outcome.pass_rate,
        stamp_used,
        stamp_winner,
        loop_report.as_ref(),
    );
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
    if loop_failed {
        1
    } else {
        0
    }
}

fn read_stamp_winner(paths: &job::JobPaths) -> Option<String> {
    let text = std::fs::read_to_string(paths.out_dir().join("winner.txt")).ok()?;
    let id = text.trim();
    if id.is_empty() {
        None
    } else {
        Some(id.to_string())
    }
}

fn merge_pipeline_metrics(
    sieve_ok: bool,
    sieve_pass_rate: f64,
    stamp_used: bool,
    stamp_winner: Option<String>,
    loop_report: Option<&report::Report>,
) -> report::Report {
    let mut built = report::empty_report();
    built.stamp_used = stamp_used;
    built.winner_id = stamp_winner.unwrap_or_else(|| "sieve".into());
    built.gate_pass_rate = if sieve_ok { 1.0 } else { sieve_pass_rate };
    if let Some(loop_report) = loop_report {
        built.loop_iterations = loop_report.loop_iterations;
        built.improvement_delta = loop_report.improvement_delta;
        built.gate_pass_rate = loop_report.gate_pass_rate;
        if built.winner_id == "sieve" && loop_report.winner_id != "none" {
            built.winner_id = loop_report.winner_id.clone();
        }
    }
    built
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
            for path in [&self.workspace, &self.home] {
                for _ in 0..5 {
                    match std::fs::remove_dir_all(path) {
                        Ok(()) => break,
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
                        Err(_) => std::thread::sleep(std::time::Duration::from_millis(10)),
                    }
                }
            }
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
        std::fs::write(workspace.join("input.txt"), "input\n").unwrap();
        let paths = job::JobPaths::from_resolved(workspace.clone(), home.clone());
        TempJob {
            workspace,
            home,
            paths,
        }
    }

    fn with_job(job: &TempJob, mut args: Vec<String>) -> Vec<String> {
        if args.first().map(String::as_str) == Some("compile")
            && !args.iter().any(|value| value == "--files")
        {
            args.push("--files".into());
            args.push("input.txt".into());
        }
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

    fn with_builder_argv<F: FnOnce() -> R, R>(builder: &str, run: F) -> R {
        let _guard = match BUILDER_ENV_LOCK.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let previous = std::env::var("KEEL_ANVIL_BUILDER_ARGV").ok();
        std::env::set_var("KEEL_ANVIL_BUILDER_ARGV", builder);
        let result = run();
        match previous {
            Some(value) => std::env::set_var("KEEL_ANVIL_BUILDER_ARGV", value),
            None => std::env::remove_var("KEEL_ANVIL_BUILDER_ARGV"),
        }
        result
    }

    fn with_builder<F: FnOnce() -> R, R>(run: F) -> R {
        let builder = if cfg!(windows) {
            r#"["cmd.exe","/C","echo built>built.txt"]"#
        } else {
            r#"["sh","-c","printf built > built.txt"]"#
        };
        with_builder_argv(builder, run)
    }

    fn looping_builder_argv() -> &'static str {
        if cfg!(windows) {
            r#"["cmd.exe","/C","if exist once.txt (echo built>built.txt) else (echo x>once.txt)"]"#
        } else {
            r#"["sh","-c","if [ -f once.txt ]; then printf built > built.txt; else printf x > once.txt; fi"]"#
        }
    }

    fn file_exists_bar() -> &'static str {
        if cfg!(windows) {
            "if (Test-Path -Path built.txt) { exit 0 } else { exit 1 }"
        } else {
            "test -f built.txt"
        }
    }

    #[test]
    fn empty_action_prints_usage_and_fails() {
        let (code, stdout, _) = run_cmd(&[]);
        assert_eq!(code, 1);
        assert!(stdout.contains("Usage: keel anvil"));
        assert!(
            stdout.contains("Bradley-Terry"),
            "help must describe local PPT, stdout={stdout}"
        );
        assert!(
            !stdout.contains("logprob"),
            "help must not claim a logprob API, stdout={stdout}"
        );
    }

    #[test]
    fn pipeline_report_uses_loop_and_stamp_outcome() {
        let mut loop_report = report::empty_report();
        loop_report.loop_iterations = 4;
        loop_report.improvement_delta = 0.2;
        loop_report.gate_pass_rate = 1.0;
        let built =
            merge_pipeline_metrics(false, 0.0, true, Some("cast_2".into()), Some(&loop_report));
        assert_eq!(built.winner_id, "cast_2");
        assert_eq!(built.loop_iterations, 4);
        assert!(built.stamp_used);
        assert!((built.gate_pass_rate - 1.0).abs() < 1e-9);
        assert!((built.improvement_delta - 0.2).abs() < 1e-9);
        assert_ne!(built.winner_id, "cast_0");
        assert_ne!(built.loop_iterations, 1);
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
    fn compile_refuses_when_clarify_required_and_packet_missing() {
        let job = temp_job("clarify-missing");
        let (code, _, stderr) = run_cmd(&with_job(
            &job,
            vec![
                "compile".into(),
                "--goal".into(),
                "vague feature".into(),
                "--bar".into(),
                "echo ok".into(),
                "--clarify-required".into(),
            ],
        ));
        assert_eq!(code, 1, "stderr={stderr}");
        assert!(stderr.contains("CLARIFY_BLOCKED"), "stderr={stderr}");
        assert!(stderr.contains("clarify.packet.json"), "stderr={stderr}");
        assert!(!job.paths.lock_path().is_file());
        assert_eq!(
            job.paths.clarify_packet_path(),
            job.paths.dir.join("clarify.packet.json")
        );
        assert_eq!(
            job.paths.clarify_required_path(),
            job.paths.dir.join("clarify.required")
        );
    }

    #[test]
    fn compile_refuses_on_clarify_hard_block_and_drift() {
        let job = temp_job("clarify-hard");
        std::fs::create_dir_all(&job.paths.dir).unwrap();
        let goal = "ship clarify gate";
        let hash = crate::utility::anvil::clarify::goal_hash(goal);
        // unanswered => hard_block
        let unanswered = format!(
            r#"{{"version":1,"trigger":"ambiguous_req","questions":[{{"id":"scope","header":"Scope","question":"Which?","type":"choice","options":["cli","docs"],"required":true}}],"answers":[],"locked_brief":{{"goal":"{goal}","non_goals":[],"constraints":[],"acceptance":[],"open_risks":[]}},"unanswered_policy":"hard_block","drift_check":{{"original_goal_hash":"{hash}","allowed_delta_fields":["constraints","acceptance","non_goals","open_risks"]}},"hard_block":false}}"#
        );
        std::fs::write(job.paths.clarify_packet_path(), unanswered).unwrap();
        let (code, _, stderr) = run_cmd(&with_job(
            &job,
            vec![
                "compile".into(),
                "--goal".into(),
                goal.into(),
                "--bar".into(),
                "echo ok".into(),
            ],
        ));
        assert_eq!(code, 1, "stderr={stderr}");
        assert!(stderr.contains("hard_block"), "stderr={stderr}");

        // answered but drifted goal hash
        let drifted = format!(
            r#"{{"version":1,"trigger":"ambiguous_req","questions":[{{"id":"scope","header":"Scope","question":"Which?","type":"choice","options":["cli","docs"],"required":true}}],"answers":[{{"id":"scope","value":"cli"}}],"locked_brief":{{"goal":"{goal}","non_goals":[],"constraints":[],"acceptance":[],"open_risks":[]}},"unanswered_policy":"hard_block","drift_check":{{"original_goal_hash":"deadbeefdeadbeef","allowed_delta_fields":["constraints"]}},"hard_block":false}}"#
        );
        std::fs::write(job.paths.clarify_packet_path(), drifted).unwrap();
        let (code, _, stderr) = run_cmd(&with_job(
            &job,
            vec![
                "compile".into(),
                "--goal".into(),
                goal.into(),
                "--bar".into(),
                "echo ok".into(),
            ],
        ));
        assert_eq!(code, 1, "stderr={stderr}");
        assert!(stderr.contains("drift_check"), "stderr={stderr}");
    }

    #[test]
    fn compile_accepts_complete_clarify_packet_and_preserves_it() {
        let job = temp_job("clarify-ok");
        std::fs::create_dir_all(&job.paths.dir).unwrap();
        let goal = "pretty json";
        let hash = crate::utility::anvil::clarify::goal_hash(goal);
        let packet = format!(
            r#"{{"version":1,"trigger":"ambiguous_req","questions":[{{"id":"scope","header":"Scope","question":"Which?","type":"choice","options":["cli","docs"],"required":true}}],"answers":[{{"id":"scope","value":"cli"}}],"locked_brief":{{"goal":"{goal}","non_goals":["P2"],"constraints":["MIT"],"acceptance":["gate works"],"open_risks":[]}},"unanswered_policy":"hard_block","drift_check":{{"original_goal_hash":"{hash}","allowed_delta_fields":["constraints","acceptance","non_goals","open_risks"]}},"hard_block":false,"ownership":{{"orchestrator":"owns AskUser adapters","subagents":"escalate only"}}}}"#
        );
        std::fs::write(job.paths.clarify_packet_path(), packet).unwrap();
        let (code, stdout, stderr) = run_cmd(&with_job(
            &job,
            vec![
                "compile".into(),
                "--goal".into(),
                goal.into(),
                "--bar".into(),
                "echo ok".into(),
            ],
        ));
        assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
        assert!(
            job.paths.clarify_packet_path().is_file(),
            "clarify packet must survive compile generation swap"
        );
        let lock = job::load_lock(&job.paths).expect("lock");
        assert_eq!(lock["goal"], goal);
    }

    #[test]
    fn compile_requires_at_least_one_owned_file() {
        let job = temp_job("compile-files");
        let (code, _, stderr) = run_cmd(&[
            "compile".into(),
            "--goal".into(),
            "pretty json".into(),
            "--bar".into(),
            "echo ok".into(),
            "--workspace-root".into(),
            job.workspace.display().to_string(),
            "--claude-home".into(),
            job.home.display().to_string(),
        ]);
        assert_eq!(code, 1);
        assert!(stderr.contains("--files is required"), "stderr={stderr}");
    }

    #[test]
    fn compile_starts_a_new_generation_and_removes_stale_outputs() {
        let job = temp_job("compile-generation");
        let args = with_job(
            &job,
            vec![
                "compile".into(),
                "--goal".into(),
                "first".into(),
                "--bar".into(),
                "echo ok".into(),
            ],
        );
        assert_eq!(run_cmd(&args).0, 0);
        let first = job::load_lock(&job.paths).expect("first lock")["generation"]
            .as_str()
            .expect("generation")
            .to_string();
        std::fs::create_dir_all(job.paths.dir.join("cast_stale")).unwrap();
        std::fs::create_dir_all(job.paths.out_dir()).unwrap();
        std::fs::write(job.paths.report_path(), "{}").unwrap();
        assert_eq!(run_cmd(&args).0, 0);
        let second = job::load_lock(&job.paths).expect("second lock")["generation"]
            .as_str()
            .expect("generation")
            .to_string();
        assert_ne!(first, second);
        assert!(!job.paths.dir.join("cast_stale").exists());
        assert!(!job.paths.out_dir().exists());
        assert!(!job.paths.report_path().exists());
    }

    #[test]
    fn failed_compile_preserves_the_current_generation() {
        let job = temp_job("compile-rollback");
        let args = with_job(
            &job,
            vec![
                "compile".into(),
                "--goal".into(),
                "first".into(),
                "--bar".into(),
                "echo ok".into(),
            ],
        );
        assert_eq!(run_cmd(&args).0, 0);
        let before = std::fs::read_to_string(job.paths.lock_path()).unwrap();
        std::fs::write(job.paths.report_path(), "preserve").unwrap();
        let result = compile::write_lock(
            &job.paths,
            "bad replacement",
            "echo ok",
            &["missing.txt".into()],
            "",
        );
        assert!(result.is_err());
        assert_eq!(
            std::fs::read_to_string(job.paths.lock_path()).unwrap(),
            before
        );
        assert_eq!(
            std::fs::read_to_string(job.paths.report_path()).unwrap(),
            "preserve"
        );
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
    fn dry_run_loop_is_a_read_only_plan() {
        let job = temp_job("dry-loop");
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
        let (code, stdout, stderr) =
            run_cmd(&with_job(&job, vec!["loop".into(), "--dry-run".into()]));
        assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
        assert!(stdout.contains("dry-run plan"), "stdout={stdout}");
        assert!(!job.paths.report_path().is_file());
    }

    #[test]
    fn live_loop_promotes_cast_and_iterates_until_gates_pass() {
        let job = temp_job("live-loop");
        let (compile_code, _, compile_stderr) = run_cmd(&with_job(
            &job,
            vec![
                "compile".into(),
                "--goal".into(),
                "file gate".into(),
                "--bar".into(),
                file_exists_bar().into(),
            ],
        ));
        assert_eq!(compile_code, 0, "compile stderr={compile_stderr}");
        let (cast_code, _, cast_stderr) = with_builder_argv(looping_builder_argv(), || {
            run_cmd(&with_job(&job, vec!["cast".into()]))
        });
        assert_eq!(cast_code, 0, "cast stderr={cast_stderr}");
        assert!(!job.paths.out_dir().join("workspace").is_dir());

        let (code, stdout, stderr) = with_builder_argv(looping_builder_argv(), || {
            run_cmd(&with_job(&job, vec!["loop".into()]))
        });
        assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
        assert!(stdout.contains("pass=true"), "stdout={stdout}");
        assert!(
            stdout.contains("iters=1") || stdout.contains("iters=2"),
            "live loop must iterate then pass, stdout={stdout}"
        );
        assert!(job
            .paths
            .out_dir()
            .join("workspace")
            .join("built.txt")
            .is_file());
        let built = report::read_report(&job.paths).expect("report");
        assert!(
            built.loop_iterations >= 1,
            "loop_iterations={}",
            built.loop_iterations
        );
        assert!((built.gate_pass_rate - 1.0).abs() < 1e-9);
    }

    #[test]
    fn live_cast_denies_git_commit_builder() {
        let job = temp_job("cast-deny");
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
        let (code, _, stderr) = with_builder_argv(r#"["git","commit","-am","x"]"#, || {
            run_cmd(&with_job(&job, vec!["cast".into()]))
        });
        assert_eq!(code, 1, "stderr={stderr}");
        assert!(
            stderr.contains("denied command"),
            "builder denylist must fire, stderr={stderr}"
        );
    }

    #[test]
    fn failed_recast_preserves_previous_candidates() {
        let job = temp_job("cast-rollback");
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
        assert_eq!(
            with_builder(|| run_cmd(&with_job(&job, vec!["cast".into()]))).0,
            0
        );
        let prior = std::fs::read_to_string(job.paths.dir.join("cast_0/result.json")).unwrap();
        let failing = if cfg!(windows) {
            r#"["cmd.exe","/C","exit 7"]"#
        } else {
            r#"["sh","-c","exit 7"]"#
        };
        let (code, _, _) =
            with_builder_argv(failing, || run_cmd(&with_job(&job, vec!["cast".into()])));
        assert_eq!(code, 1);
        assert_eq!(
            std::fs::read_to_string(job.paths.dir.join("cast_0/result.json")).unwrap(),
            prior
        );
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
    fn dry_run_cast_writes_no_result_json() {
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
        assert!(!job.paths.dir.join("cast_0").join("result.json").is_file());
        assert!(stdout.contains("writes=0"), "stdout={stdout}");
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
        let (cast_code, _, cast_stderr) =
            with_builder(|| run_cmd(&with_job(&job, vec!["cast".into()])));
        assert_eq!(cast_code, 0, "stderr={cast_stderr}");
        let (code, stdout, stderr) = run_cmd(&with_job(&job, vec!["stamp".into()]));
        assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
        assert!(stdout.contains("mode=ppt-evidence"));
        assert!(job.paths.out_dir().join("winner.txt").is_file());
        assert!(!job.workspace.join("anvil").exists());
    }

    #[test]
    fn dry_run_stamp_uses_evidence() {
        let (code, stdout, _) = run_cmd(&["stamp".into(), "--dry-run".into()]);
        assert_eq!(code, 0);
        assert!(stdout.contains("mode=ppt-evidence"));
        assert!(!stdout.contains("stub"));
        assert!(stdout.contains("strict=false"));
    }

    #[test]
    fn dry_run_stamp_strict_reports_strict_mode() {
        let (code, stdout, _) = run_cmd(&["stamp".into(), "--dry-run".into(), "--strict".into()]);
        assert_eq!(code, 0);
        assert!(stdout.contains("mode=ppt-evidence"));
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
                    "--files".into(),
                    "input.txt".into(),
                ],
            ))
        });
        assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
        assert!(job.paths.report_path().is_file());
        assert!(job
            .paths
            .out_dir()
            .join("workspace")
            .join("built.txt")
            .is_file());
        assert!(!job.workspace.join("built.txt").exists());
        assert!(!job.workspace.join("anvil").exists());
    }

    #[test]
    fn multi_piece_casts_are_unique_and_stamp_merges_winners() {
        let job = temp_job("multi-piece");
        std::fs::write(job.workspace.join("second.txt"), "second\n").unwrap();
        let args = with_job(
            &job,
            vec![
                "compile".into(),
                "--goal".into(),
                "two pieces".into(),
                "--bar".into(),
                "echo ok".into(),
            ],
        );
        assert_eq!(run_cmd(&args).0, 0);
        let mut lock = job::load_lock(&job.paths).expect("lock");
        lock["pieces"] = serde_json::json!([
            {"id":"first","files":["input.txt"],"gates":["echo ok"],"critic":"none"},
            {"id":"second","files":["second.txt"],"gates":["echo ok"],"critic":"none"}
        ]);
        std::fs::write(
            job.paths.lock_path(),
            serde_json::to_string_pretty(&lock).unwrap(),
        )
        .unwrap();
        let (cast_code, _, cast_stderr) =
            with_builder(|| run_cmd(&with_job(&job, vec!["cast".into()])));
        assert_eq!(cast_code, 0, "stderr={cast_stderr}");
        let result_count = std::fs::read_dir(&job.paths.dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry.file_name().to_string_lossy().starts_with("cast_")
                    && entry.path().join("result.json").is_file()
            })
            .count();
        assert_eq!(result_count, 6);
        let (stamp_code, _, stamp_stderr) = run_cmd(&with_job(&job, vec!["stamp".into()]));
        assert_eq!(stamp_code, 0, "stderr={stamp_stderr}");
        let promoted = job.paths.out_dir().join("workspace");
        assert!(promoted.join("input.txt").is_file());
        assert!(promoted.join("second.txt").is_file());
    }

    #[test]
    fn selective_recast_does_not_collide_with_preserved_candidate_ids() {
        let job = temp_job("selective-recast");
        std::fs::write(job.workspace.join("second.txt"), "second\n").unwrap();
        let args = with_job(
            &job,
            vec![
                "compile".into(),
                "--goal".into(),
                "two pieces".into(),
                "--bar".into(),
                "echo ok".into(),
            ],
        );
        assert_eq!(run_cmd(&args).0, 0);
        let mut lock = job::load_lock(&job.paths).expect("lock");
        lock["pieces"] = serde_json::json!([
            {"id":"first","files":["input.txt"],"gates":["echo ok"],"critic":"none"},
            {"id":"second","files":["second.txt"],"gates":["echo ok"],"critic":"none"}
        ]);
        std::fs::write(
            job.paths.lock_path(),
            serde_json::to_string_pretty(&lock).unwrap(),
        )
        .unwrap();
        assert_eq!(
            with_builder(|| run_cmd(&with_job(&job, vec!["cast".into()]))).0,
            0
        );

        let (code, _, stderr) = with_builder(|| {
            run_cmd(&with_job(
                &job,
                vec!["cast".into(), "--piece".into(), "second".into()],
            ))
        });
        assert_eq!(code, 0, "selective recast stderr={stderr}");
        let candidates: Vec<serde_json::Value> = std::fs::read_dir(&job.paths.dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter_map(|entry| std::fs::read_to_string(entry.path().join("result.json")).ok())
            .filter_map(|text| serde_json::from_str(&text).ok())
            .collect();
        assert_eq!(candidates.len(), 6);
        assert_eq!(
            candidates
                .iter()
                .filter(|value| value["piece"] == "first")
                .count(),
            3
        );
        assert_eq!(
            candidates
                .iter()
                .filter(|value| value["piece"] == "second")
                .count(),
            3
        );
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
        let sentinel = job.workspace.join("dry-run-sentinel.txt");
        let (code, stdout, stderr) = run_cmd(&with_job(
            &job,
            vec![
                "run".into(),
                "--dry-run".into(),
                "--goal".into(),
                "dry run".into(),
                "--bar".into(),
                "echo touched > dry-run-sentinel.txt".into(),
                "--files".into(),
                "input.txt".into(),
            ],
        ));
        assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
        assert!(!stdout.contains("stub"), "stdout={stdout}");
        assert!(stdout.contains("plan"), "stdout={stdout}");
        assert!(!sentinel.exists(), "dry-run executed a gate command");
        assert!(
            !job.paths.dir.exists(),
            "dry-run wrote persistent job state"
        );
        assert!(!job.workspace.join("anvil").exists());
    }

    #[test]
    fn edit_gate_clear_requires_successful_run_dry_run() {
        assert!(!should_clear_edit_gate("compile", &[], 0));
        assert!(!should_clear_edit_gate("cast", &["--dry-run".into()], 0));
        assert!(!should_clear_edit_gate("run", &[], 0));
        assert!(!should_clear_edit_gate("run", &["--dry-run".into()], 1));
        assert!(should_clear_edit_gate("run", &["--dry-run".into()], 0));
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
        let (code, _, stderr) = with_builder(|| run_cmd(&with_job(&job, vec!["cast".into()])));
        assert_eq!(code, 0, "stderr={stderr}");
        let lock = job::load_lock(&job.paths).expect("lock survives");
        assert_eq!(lock["goal"], "resume goal");
        assert!(job.paths.dir.join("cast_0").join("result.json").is_file());
        assert!(!job.workspace.join("anvil").exists());
    }
}
