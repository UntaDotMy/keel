use std::io::Write;

use crate::args::FlagSet;
use crate::runtime::write_text;
use crate::utility::anvil::cache;
use crate::utility::anvil::job;
use crate::utility::anvil::prefix;
use crate::utility::anvil::sieve;
use crate::utility::anvil::supervisor;
use crate::utility::anvil::workspace;

use std::env;
use std::path::Path;
use std::time::Duration;

struct PendingResultBatch {
    root: std::path::PathBuf,
    destinations: Vec<(std::path::PathBuf, std::path::PathBuf)>,
}

impl PendingResultBatch {
    fn new(job_dir: &Path) -> Result<Self, String> {
        let root = job_dir.join(format!(
            "pending-casts-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir(&root).map_err(|error| error.to_string())?;
        Ok(Self {
            root,
            destinations: Vec::new(),
        })
    }

    fn stage(&mut self, id: &str, destination: std::path::PathBuf) -> std::path::PathBuf {
        let staging = self.root.join(id);
        self.destinations.push((staging.clone(), destination));
        staging
    }

    fn activate(mut self, paths: &job::JobPaths, only_piece: &str) -> Result<(), String> {
        let rollback = self.root.join("rollback");
        std::fs::create_dir(&rollback).map_err(|error| error.to_string())?;
        let replaced = replaced_result_paths(paths, only_piece)?;
        let mut preserved = Vec::new();
        for source in replaced {
            let destination =
                rollback.join(source.file_name().ok_or_else(|| {
                    format!("anvil cast: invalid result path {}", source.display())
                })?);
            if let Err(error) = std::fs::rename(&source, &destination) {
                restore_preserved_results(&preserved);
                return Err(format!(
                    "preserve {} before activation: {error}",
                    source.display()
                ));
            }
            preserved.push((destination, source));
        }
        let mut activated = Vec::new();
        for (staging, destination) in &self.destinations {
            if let Err(error) = std::fs::rename(staging, destination) {
                for path in activated.iter().rev() {
                    let _ = std::fs::remove_dir_all(path);
                }
                restore_preserved_results(&preserved);
                return Err(format!(
                    "activate {} as {}: {error}",
                    staging.display(),
                    destination.display()
                ));
            }
            activated.push(destination.clone());
        }
        self.destinations.clear();
        std::fs::remove_dir_all(&rollback).map_err(|error| {
            format!(
                "remove previous candidate batch {}: {error}",
                rollback.display()
            )
        })?;
        std::fs::remove_dir(&self.root).map_err(|error| error.to_string())?;
        Ok(())
    }
}

fn restore_preserved_results(preserved: &[(std::path::PathBuf, std::path::PathBuf)]) {
    for (backup, original) in preserved.iter().rev() {
        let _ = std::fs::rename(backup, original);
    }
}

impl Drop for PendingResultBatch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn builder_argv() -> Result<Vec<String>, String> {
    let raw = env::var("KEEL_ANVIL_BUILDER_ARGV").map_err(|_| {
        "anvil cast: non-dry runs require KEEL_ANVIL_BUILDER_ARGV as a JSON argv array".to_string()
    })?;
    let argv: Vec<String> = match serde_json::from_str(&raw) {
        Ok(argv) => argv,
        Err(error) => {
            return Err(format!(
                "anvil cast: invalid KEEL_ANVIL_BUILDER_ARGV: {error}"
            ))
        }
    };
    if argv.is_empty() || argv[0].trim().is_empty() {
        return Err("anvil cast: KEEL_ANVIL_BUILDER_ARGV must contain a program".to_string());
    }
    Ok(argv)
}

fn expand_builder_arg(value: &str, workspace: &Path, piece: &str, gates: &[String]) -> String {
    value
        .replace("{workspace}", &workspace.display().to_string())
        .replace("{piece}", piece)
        .replace("{gates}", &gates.join(" | "))
}

pub(crate) fn run_builder_with_budget(
    workspace: &Path,
    piece: &str,
    gates: &[String],
    timeout: Duration,
    max_tool_chars: usize,
    max_tokens: u64,
) -> Result<String, String> {
    let argv = builder_argv()?;
    let program = &argv[0];
    let arguments: Vec<String> = argv[1..]
        .iter()
        .map(|value| expand_builder_arg(value, workspace, piece, gates))
        .collect();
    let command_line = std::iter::once(program.as_str())
        .chain(arguments.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(" ");
    if supervisor::is_denied_argv(program, &arguments) {
        return Err(format!("anvil builder: denied command: {command_line}"));
    }
    let result =
        crate::runtime::run_command_with_timeout(program, &arguments, Some(workspace), timeout)?;
    let captured_bytes = result
        .original_stdout_bytes
        .saturating_add(result.original_stderr_bytes) as u64;
    let estimated_tokens = captured_bytes.saturating_add(3) / 4;
    if estimated_tokens > max_tokens {
        return Err(format!(
            "anvil builder: estimated output tokens {estimated_tokens} exceed configured token budget {max_tokens}"
        ));
    }
    let logs = format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    if result.code != 0 {
        return Err(format!(
            "builder exited with code {}\n{}",
            result.code,
            supervisor::clip_output(&logs, max_tool_chars)
        ));
    }
    Ok(supervisor::clip_output(&logs, max_tool_chars))
}

pub fn run_cast(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flags = FlagSet::new("anvil cast");
    flags.bool_flag("dry-run", false);
    flags.string_flag("piece", "");
    flags.string_flag("workspace-root", "");
    flags.string_flag("claude-home", "");
    flags.bool_flag("json", false);
    if let Err(error) = flags.parse(arguments) {
        let _ = writeln!(standard_error, "{}", error.message);
        return 1;
    }
    let dry_run = flags.bool_value("dry-run");
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
    let lock = match job::load_lock(&paths) {
        Ok(value) => value,
        Err(error) => {
            let _ = writeln!(standard_error, "{error}");
            return 1;
        }
    };
    let pieces = match job::pieces_from_lock(&lock, flags.string_value("piece")) {
        Ok(value) => value,
        Err(error) => {
            let _ = writeln!(standard_error, "{error}");
            return 1;
        }
    };
    let generation = match job::generation(&lock) {
        Ok(value) => value.to_string(),
        Err(error) => {
            let _ = writeln!(standard_error, "{error}");
            return 1;
        }
    };
    let casts = job::n_casts(&lock);
    let budget = match job::budget_from_lock(&lock) {
        Ok(value) => value,
        Err(error) => {
            let _ = writeln!(standard_error, "{error}");
            return 1;
        }
    };
    if dry_run {
        for piece in &pieces {
            if let Err(error) = workspace::validate_workspace_files(&paths.workspace, &piece.files)
            {
                let _ = writeln!(standard_error, "anvil cast: dry-run: {error}");
                return 1;
            }
        }
        let _ = writeln!(
            standard_output,
            "anvil cast: dry-run plan pieces={} casts={} results={} writes=0 executes=0",
            pieces.len(),
            casts,
            pieces.len() as u64 * casts
        );
        return 0;
    }
    if !cfg!(test) {
        let (_, cleanup_errors) =
            workspace::cleanup_stale_workspaces(std::time::Duration::from_secs(7_200));
        for error in cleanup_errors {
            let _ = writeln!(
                standard_error,
                "anvil cast: stale workspace cleanup: {error}"
            );
        }
    }
    let dir = match paths.ensure_dir() {
        Ok(path) => path.to_path_buf(),
        Err(error) => {
            let _ = writeln!(standard_error, "{error}");
            return 1;
        }
    };
    let prefix_text = workspace::paginated_read(&paths.prefix_path(), 0, 80)
        .unwrap_or_else(|_| prefix::build_static_prefix("anvil", "host-cli"));
    let headers = cache::cache_headers_for("openai");
    let mut written = 0u64;
    let deadline = std::time::Instant::now() + budget.wall_timeout;
    let mut pending = match PendingResultBatch::new(&dir) {
        Ok(batch) => batch,
        Err(error) => {
            let _ = writeln!(standard_error, "anvil cast: stage results: {error}");
            return 1;
        }
    };
    for piece in &pieces {
        for _index in 0..casts {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                let _ = writeln!(standard_error, "anvil cast: wall-clock budget exhausted");
                return 1;
            }
            let isolated = match workspace::create_temporary_workspace(
                &paths.workspace,
                &piece.files,
                &piece.gates,
            ) {
                Ok(workspace) => workspace,
                Err(error) => {
                    let _ = writeln!(standard_error, "anvil cast: workspace: {error}");
                    return 1;
                }
            };
            let brief = format!(
                "Anvil builder brief\n\
                 Use the current host CLI tools (Read/Write/run). Do not call an external LLM API.\n\
                 Workspace: {}\n\
                 Piece: {}\n\
                 Gates: {}\n\
                 Tools: read_file, write_file, run (cwd=workspace)\n\
                 Forbidden: git commit, git push, git rebase, git branch\n\
                 ---\n\
                 {prefix_text}",
                isolated.path().display(),
                piece.id,
                piece.gates.join(" | ")
            );
            if let Err(error) = write_text(&isolated.path().join("BUILDER.md"), &brief) {
                let _ = writeln!(standard_error, "anvil cast: builder: {error}");
                return 1;
            }
            let mut builder_result = Err("anvil builder did not run".to_string());
            for _attempt in 0..=budget.builder_retries {
                let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                if remaining.is_zero() {
                    builder_result = Err("anvil cast: wall-clock budget exhausted".to_string());
                    break;
                }
                builder_result = run_builder_with_budget(
                    isolated.path(),
                    &piece.id,
                    &piece.gates,
                    remaining,
                    budget.max_tool_chars,
                    budget.max_tokens_cast,
                );
                if builder_result.is_ok()
                    || builder_result
                        .as_ref()
                        .err()
                        .is_some_and(|error| error.contains("denied command"))
                {
                    break;
                }
            }
            let builder_logs = match builder_result {
                Ok(logs) => logs,
                Err(error) => {
                    let _ = writeln!(standard_error, "{error}");
                    return 1;
                }
            };
            let scored = sieve::run_gates_scored_bounded(
                &piece.gates,
                Some(isolated.path()),
                budget.gate_timeout,
                Some(deadline),
            );
            let gate_ok = scored.ok;
            let gate_logs = scored.logs;
            let logs = if builder_logs.is_empty() {
                gate_logs
            } else {
                format!("{builder_logs}\ngates:\n{gate_logs}")
            };
            let clipped = supervisor::clip_output(&logs, budget.max_tool_chars);
            if let Err(error) = std::fs::remove_file(isolated.path().join("BUILDER.md")) {
                let _ = writeln!(standard_error, "anvil cast: remove builder brief: {error}");
                return 1;
            }
            let result_id = format!("cast_{written}");
            let result_dir = dir.join(&result_id);
            let staging = pending.stage(&result_id, result_dir.clone());
            if let Err(error) = std::fs::create_dir_all(&staging) {
                let _ = writeln!(standard_error, "anvil cast: {error}");
                return 1;
            }
            let bank_workspace = result_dir.join("workspace");
            if let Err(error) = workspace::copy_tree(isolated.path(), &staging.join("workspace")) {
                let _ = std::fs::remove_dir_all(&staging);
                let _ = writeln!(standard_error, "anvil cast: {error}");
                return 1;
            }
            let current_lock = match job::load_lock(&paths) {
                Ok(lock) => lock,
                Err(error) => {
                    let _ = std::fs::remove_dir_all(&staging);
                    let _ = writeln!(standard_error, "anvil cast: {error}");
                    return 1;
                }
            };
            if job::generation(&current_lock).ok() != Some(generation.as_str()) {
                let _ = std::fs::remove_dir_all(&staging);
                let _ = writeln!(
                    standard_error,
                    "anvil cast: lock generation changed while candidates were running"
                );
                return 1;
            }
            let payload = serde_json::json!({
                "id": result_id,
                "generation": generation,
                "piece": piece.id,
                "workspace": bank_workspace.display().to_string(),
                "dry_run": dry_run,
                "gate_ok": gate_ok,
                "headers": headers.len(),
                "clipped_len": clipped.len(),
                "model": job::env_model("ANVIL_CAST_MODEL", "host-cli")
            });
            if let Err(error) = write_text(&staging.join("result.json"), &payload.to_string()) {
                let _ = std::fs::remove_dir_all(&staging);
                let _ = writeln!(standard_error, "anvil cast: result: {error}");
                return 1;
            }
            written += 1;
        }
    }
    if let Err(error) = pending.activate(&paths, flags.string_value("piece")) {
        let _ = writeln!(standard_error, "anvil cast: activate results: {error}");
        return 1;
    }
    let _ = writeln!(
        standard_output,
        "anvil cast: pieces={} casts={} results={} dry_run={} host-cli headers={}",
        pieces.len(),
        casts,
        written,
        dry_run,
        headers.len()
    );
    0
}

fn replaced_result_paths(
    paths: &job::JobPaths,
    only_piece: &str,
) -> Result<Vec<std::path::PathBuf>, String> {
    let mut replaced = Vec::new();
    if paths.dir.is_dir() {
        for entry in std::fs::read_dir(&paths.dir).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.starts_with("cast_") {
                continue;
            }
            let remove = if only_piece.is_empty() {
                true
            } else {
                let evidence_piece = std::fs::read_to_string(entry.path().join("result.json"))
                    .ok()
                    .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
                    .and_then(|value| {
                        value
                            .get("piece")
                            .and_then(|piece| piece.as_str())
                            .map(str::to_string)
                    });
                match evidence_piece {
                    Some(piece) => piece == only_piece,
                    None => true,
                }
            };
            if remove {
                replaced.push(entry.path());
            }
        }
    }
    if paths.out_dir().exists() {
        replaced.push(paths.out_dir());
    }
    if paths.report_path().exists() {
        replaced.push(paths.report_path());
    }
    Ok(replaced)
}
