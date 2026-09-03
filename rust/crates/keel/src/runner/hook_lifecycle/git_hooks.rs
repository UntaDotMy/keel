//! Hook lifecycle git_hooks responsibility split.

use super::*;

pub(super) fn set_core_hooks_path(git_config: &str, value: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut core_insert_at: Option<usize> = None;

    for line in git_config.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            if trimmed.eq_ignore_ascii_case("[core]") {
                // Insert position = right after this header line.
                core_insert_at = Some(lines.len() + 1);
            }
            lines.push(line.to_string());
            continue;
        }
        // A hooksPath key line in any section is dropped; a single canonical
        // entry is re-inserted under [core] below.
        if let Some((key, _)) = trimmed.split_once('=') {
            if key.trim().eq_ignore_ascii_case("hookspath") {
                continue;
            }
        }
        lines.push(line.to_string());
    }

    let entry = format!("\thooksPath = {value}");
    if let Some(index) = core_insert_at {
        lines.insert(index, entry);
    } else {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.push("[core]".to_string());
        lines.push(entry);
    }

    let line_ending = if git_config.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let mut joined = lines.join(line_ending);
    if git_config.ends_with('\n') && !joined.ends_with(line_ending) {
        joined.push_str(line_ending);
    }
    joined
}

pub(super) fn run_hook_git_hooks(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flag_set = FlagSet::new("hook git-hooks");
    flag_set.string_flag("repo-root", "");

    let mut args = arguments.to_vec();
    if args.first().map(|s| s.as_str()) == Some("install") {
        args.remove(0);
    }

    if let Err(parse_error) = flag_set.parse(&args) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 1;
    }

    let repo_root = match resolve_repository_root(flag_set.string_value("repo-root")) {
        Ok(path) => path,
        Err(error) => {
            let _ = writeln!(standard_error, "{error}");
            return 1;
        }
    };

    let githooks_dir = repo_root.join(".githooks");

    if !githooks_dir.exists() {
        let _ = writeln!(
            standard_error,
            "No .githooks directory found in {}",
            display_path(&repo_root)
        );
        return 1;
    }

    let hooks = ["pre-commit", "pre-push"];

    for hook_name in &hooks {
        let hook_path = githooks_dir.join(hook_name);
        if !hook_path.exists() {
            let _ = writeln!(standard_error, "Hook file not found: {}", hook_name);
            continue;
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = match fs::metadata(&hook_path) {
                Ok(metadata) => metadata.permissions(),
                Err(e) => {
                    let _ = writeln!(
                        standard_error,
                        "Failed to read permissions for {}: {}",
                        hook_name, e
                    );
                    continue;
                }
            };
            perms.set_mode(0o755);
            if let Err(e) = fs::set_permissions(&hook_path, perms) {
                let _ = writeln!(
                    standard_error,
                    "Failed to make {} executable: {}",
                    hook_name, e
                );
                continue;
            }
        }

        let _ = writeln!(
            standard_output,
            "  {}",
            hook_path
                .strip_prefix(&repo_root)
                .unwrap_or(&hook_path)
                .display()
        );
    }

    let git_config_path = repo_root.join(".git").join("config");
    let hooks_path_value = ".githooks";

    if git_config_path.exists() {
        // a failed read (perms, AV lock) must never turn into an
        // unconditional overwrite that replaces a real config with a stub.
        let git_config = match fs::read_to_string(&git_config_path) {
            Ok(text) => text,
            Err(error) => {
                let _ = writeln!(
                    standard_error,
                    "Refusing to edit {}: unreadable ({error})",
                    display_path(&git_config_path)
                );
                return 1;
            }
        };
        let updated_config = set_core_hooks_path(&git_config, hooks_path_value);
        if let Err(e) = fs::write(&git_config_path, &updated_config) {
            let _ = writeln!(standard_error, "Failed to update .git/config: {}", e);
            return 1;
        }
    } else {
        let _ = writeln!(
            standard_error,
            "Warning: .git/config not found. Git hooks may not work."
        );
    }

    let _ = writeln!(
        standard_output,
        "Installed git hooks: {}",
        hooks
            .iter()
            .filter(|h| githooks_dir.join(h).exists())
            .copied()
            .collect::<Vec<_>>()
            .join(", ")
    );

    0
}
