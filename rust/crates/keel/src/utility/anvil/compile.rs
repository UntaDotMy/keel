use std::io::Write;
use std::path::Path;

use crate::args::FlagSet;
use crate::utility::anvil::job;
use crate::utility::anvil::lock::validate_lock;
use crate::utility::anvil::prefix;

pub fn run_compile(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flags = FlagSet::new("anvil compile");
    flags.string_flag("goal", "");
    flags.string_flag("bar", "");
    flags.string_flag("files", "");
    flags.string_flag("out", "");
    flags.string_flag("workspace-root", "");
    flags.string_flag("claude-home", "");
    flags.bool_flag("json", false);
    if let Err(error) = flags.parse(arguments) {
        let _ = writeln!(standard_error, "{}", error.message);
        return 1;
    }
    let goal = flags.string_value("goal").trim().to_string();
    if goal.is_empty() {
        let _ = writeln!(standard_error, "anvil compile: --goal is required");
        return 1;
    }
    let bar = flags.string_value("bar").trim().to_string();
    if bar.is_empty() {
        let _ = writeln!(
            standard_output,
            "{{\"bars\":[{{\"name\":\"jq 1.7\",\"fetch\":\"cmd:jq --version\",\"compare\":\"stdout+exit\"}},{{\"name\":\"python 3.12 json\",\"fetch\":\"cmd:python --version\",\"compare\":\"stdout+exit\"}},{{\"name\":\"echo ok\",\"fetch\":\"cmd:echo ok\",\"compare\":\"stdout+exit\"}}]}}"
        );
        return 0;
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
    let files: Vec<String> = flags
        .string_value("files")
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect();
    match write_lock(&paths, &goal, &bar, &files, flags.string_value("out")) {
        Ok(hash) => {
            if flags.bool_value("json") {
                let _ = writeln!(
                    standard_output,
                    "{{\"ok\":true,\"prefix_sha256\":\"{hash}\",\"lock\":\"{}\"}}",
                    paths.lock_path().display().to_string().replace('\\', "/")
                );
            } else {
                let _ = writeln!(
                    standard_output,
                    "anvil compile: prefix hash {hash} — lock valid"
                );
            }
            0
        }
        Err(error) => {
            let _ = writeln!(standard_error, "anvil compile: {error}");
            1
        }
    }
}

pub fn write_lock(
    paths: &job::JobPaths,
    goal: &str,
    bar: &str,
    files: &[String],
    out_flag: &str,
) -> Result<String, String> {
    let quality_bar = bar.trim();
    let prefix = prefix::build_static_prefix(goal, quality_bar);
    let hash = prefix::write_prefix_files(paths, &prefix)?;
    let fetch = format!("cmd:{quality_bar}");
    let lock = serde_json::json!({
        "version": 1,
        "goal": goal,
        "bar": {"name": quality_bar, "fetch": fetch, "compare": "stdout+exit"},
        "budget": {
            "n_casts": 3,
            "k_pivots": 1,
            "critic_k": 1,
            "granularity": 20,
            "builder_retries": 2,
            "max_tokens_cast": 80000,
            "max_tokens_stamp": 40000,
            "max_tokens_loop": 100000,
            "max_tool_chars": 4000,
            "max_iterations": 20,
            "min_improvement_threshold": 0.05
        },
        "models": {
            "compile": job::env_model("ANVIL_COMPILE_MODEL", "host-cli"),
            "cast": job::env_model("ANVIL_CAST_MODEL", "host-cli"),
            "stamp": job::env_model("ANVIL_STAMP_MODEL", "host-cli"),
            "loop": job::env_model("ANVIL_LOOP_MODEL", "host-cli"),
            "allow_training_data": std::env::var("ANVIL_ALLOW_TRAINING_DATA")
                .map(|value| value == "true")
                .unwrap_or(false)
        },
        "criteria": ["specification", "output", "errors"],
        "pieces": [{
            "id": "main",
            "files": files,
            "gates": [quality_bar],
            "critic": "none"
        }]
    });
    let lock_text = serde_json::to_string_pretty(&lock).map_err(|error| error.to_string())?;
    validate_lock(&lock_text)?;
    paths.ensure_dir()?;
    let path = if out_flag.trim().is_empty() {
        paths.lock_path()
    } else {
        Path::new(out_flag).to_path_buf()
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    std::fs::write(&path, lock_text).map_err(|error| format!("lock write: {error}"))?;
    if path != paths.lock_path() {
        std::fs::copy(&path, paths.lock_path()).map_err(|error| error.to_string())?;
    }
    let gates_dir = paths.gates_dir();
    std::fs::create_dir_all(&gates_dir).map_err(|error| error.to_string())?;
    std::fs::write(gates_dir.join("main"), format!("{quality_bar}\n"))
        .map_err(|error| error.to_string())?;
    Ok(hash)
}
