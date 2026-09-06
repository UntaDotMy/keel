use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::args::FlagSet;
use crate::runtime::write_text;
use crate::utility::anvil::job;
use crate::utility::anvil::lock::validate_lock;
use crate::utility::anvil::prefix;

static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);

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
    flags.bool_flag("clarify-required", false);
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
    if files.is_empty() {
        let _ = writeln!(standard_error, "anvil compile: --files is required");
        return 1;
    }
    let clarify_required = flags.bool_value("clarify-required");
    if let Err(error) = crate::utility::anvil::clarify::enforce_clarify_for_compile(
        &paths.dir,
        &goal,
        clarify_required,
    ) {
        let _ = writeln!(
            standard_error,
            "anvil compile: {error}\n  packet: {}\n  sentinel: {}",
            paths.clarify_packet_path().display(),
            paths.clarify_required_path().display()
        );
        let _ = writeln!(
            standard_error,
            "{}",
            crate::utility::anvil::clarify::ask_user_adapter_playbook()
        );
        return 1;
    }
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
    let generation = format!(
        "{}-{}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0),
        std::process::id(),
        NEXT_GENERATION.fetch_add(1, Ordering::Relaxed)
    );
    let lock = build_lock_value(goal, quality_bar, files, &generation);
    let lock_text = serde_json::to_string_pretty(&lock).map_err(|error| error.to_string())?;
    validate_lock(&lock_text)?;
    crate::utility::anvil::workspace::validate_workspace_files(&paths.workspace, files)?;

    let parent = paths
        .dir
        .parent()
        .ok_or_else(|| "anvil: job directory has no parent".to_string())?;
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let suffix = format!(
        "{}-{}",
        std::process::id(),
        NEXT_GENERATION.fetch_add(1, Ordering::Relaxed)
    );
    let staging = parent.join(format!("anvil.next-{suffix}"));
    let backup = parent.join(format!("anvil.previous-{suffix}"));
    if staging.exists() || backup.exists() {
        return Err("anvil: unique compile staging path already exists".to_string());
    }
    let mut staged_paths = paths.clone();
    staged_paths.dir = staging.clone();
    // Preserve ClarifyPacket artifacts across generation swap (same anvil bank path).
    preserve_clarify_artifacts(&paths.dir, &staging)?;
    let staged = (|| {
        let hash = prefix::write_prefix_files(&staged_paths, &prefix)?;
        write_text(&staged_paths.lock_path(), &lock_text)
            .map_err(|error| format!("lock write: {error}"))?;
        std::fs::create_dir_all(staged_paths.gates_dir()).map_err(|error| error.to_string())?;
        write_text(
            &staged_paths.gates_dir().join("main"),
            &format!("{quality_bar}\n"),
        )
        .map_err(|error| error.to_string())?;
        Ok::<String, String>(hash)
    })();
    let hash = match staged {
        Ok(hash) => hash,
        Err(error) => {
            discard_staging(&staging);
            return Err(error);
        }
    };
    if !out_flag.trim().is_empty() {
        let path = Path::new(out_flag);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        if let Err(error) = write_text(path, &lock_text) {
            discard_staging(&staging);
            return Err(format!("lock write: {error}"));
        }
    }
    if paths.dir.exists() {
        std::fs::rename(&paths.dir, &backup)
            .map_err(|error| format!("anvil: preserve previous generation: {error}"))?;
    }
    if let Err(error) = std::fs::rename(&staging, &paths.dir) {
        let restore = if backup.exists() {
            std::fs::rename(&backup, &paths.dir)
                .map_err(|restore| format!("; restore previous generation failed: {restore}"))
        } else {
            Ok(())
        };
        discard_staging(&staging);
        return Err(format!(
            "anvil: activate generation: {error}{}",
            restore.err().unwrap_or_default()
        ));
    }
    if backup.exists() {
        remove_directory_with_retry(&backup)?;
    }
    Ok(hash)
}


fn preserve_clarify_artifacts(from: &Path, to: &Path) -> Result<(), String> {
    use crate::utility::anvil::clarify::{CLARIFY_PACKET_FILE, CLARIFY_REQUIRED_SENTINEL};
    for name in [CLARIFY_PACKET_FILE, CLARIFY_REQUIRED_SENTINEL] {
        let src = from.join(name);
        if !src.is_file() {
            continue;
        }
        std::fs::create_dir_all(to).map_err(|error| error.to_string())?;
        std::fs::copy(&src, to.join(name)).map_err(|error| {
            format!("anvil: preserve {name}: {error}")
        })?;
    }
    Ok(())
}

fn discard_staging(path: &Path) {
    let _ = std::fs::remove_dir_all(path); // intentional recovery cleanup preserving primary error
}

fn remove_directory_with_retry(path: &Path) -> Result<(), String> {
    let mut last_error = None;
    for attempt in 0..5 {
        match std::fs::remove_dir_all(path) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => last_error = Some(error),
        }
        if attempt < 4 {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
    Err(format!(
        "anvil: remove previous generation {}: {}",
        path.display(),
        last_error
            .map(|error| error.to_string())
            .unwrap_or_else(|| "unknown error".to_string())
    ))
}

pub(crate) fn build_lock_value(
    goal: &str,
    quality_bar: &str,
    files: &[String],
    generation: &str,
) -> serde_json::Value {
    let fetch = format!("cmd:{quality_bar}");
    serde_json::json!({
        "version": 1,
        "generation": generation,
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
            "min_improvement_threshold": 0.05,
            "wall_timeout_secs": 300,
            "gate_timeout_secs": 120
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
    })
}
