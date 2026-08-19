use std::io::Write;

use crate::args::FlagSet;
use crate::utility::anvil::cache;
use crate::utility::anvil::job;
use crate::utility::anvil::prefix;
use crate::utility::anvil::sieve;
use crate::utility::anvil::supervisor;
use crate::utility::anvil::workspace;

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
    let casts = job::n_casts(&lock);
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
    for piece in &pieces {
        for index in 0..casts {
            let isolated = match workspace::create_workspace(&piece.files, &piece.gates) {
                Ok(path) => path,
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
                isolated.display(),
                piece.id,
                piece.gates.join(" | ")
            );
            if let Err(error) = std::fs::write(isolated.join("BUILDER.md"), brief) {
                let _ = workspace::remove_workspace(&isolated);
                let _ = writeln!(standard_error, "anvil cast: builder: {error}");
                return 1;
            }
            let (gate_ok, logs) = if dry_run {
                (true, String::new())
            } else {
                sieve::run_gates(&piece.gates)
            };
            let clipped = supervisor::clip_output(&logs, 4000);
            let result_dir = dir.join(format!("cast_{index}"));
            if let Err(error) = std::fs::create_dir_all(&result_dir) {
                let _ = workspace::remove_workspace(&isolated);
                let _ = writeln!(standard_error, "anvil cast: {error}");
                return 1;
            }
            let bank_workspace = result_dir.join("workspace");
            if let Err(error) = workspace::copy_tree(&isolated, &bank_workspace) {
                let _ = workspace::remove_workspace(&isolated);
                let _ = writeln!(standard_error, "anvil cast: {error}");
                return 1;
            }
            let _ = workspace::remove_workspace(&isolated);
            let payload = serde_json::json!({
                "id": format!("cast_{index}"),
                "piece": piece.id,
                "workspace": bank_workspace.display().to_string(),
                "dry_run": dry_run,
                "gate_ok": gate_ok,
                "headers": headers.len(),
                "clipped_len": clipped.len(),
                "model": job::env_model("ANVIL_CAST_MODEL", "host-cli")
            });
            if let Err(error) = std::fs::write(result_dir.join("result.json"), payload.to_string())
            {
                let _ = writeln!(standard_error, "anvil cast: result: {error}");
                return 1;
            }
            written += 1;
        }
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
