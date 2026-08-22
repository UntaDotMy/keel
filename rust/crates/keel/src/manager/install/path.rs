// Installer PATH helpers.
use crate::runtime::display_path;
#[cfg(not(windows))]
use crate::runtime::{read_text_if_exists, write_text};
use std::path::{Path, PathBuf};
/// Marker guarding the keel PATH export appended to unix shell rc files.
#[cfg(not(windows))]
pub(crate) const KEEL_PATH_MARKER: &str = "# keel PATH (managed by the keel installer)";

/// Ensure the keel home directory is on the user's PATH so every shell and
/// every host can invoke `keel` without a full path. Best-effort and
/// idempotent: existing PATH entries and rc files are never duplicated or
/// clobbered; failures report a status string instead of failing the install.
///
/// Windows: appends to the per-user `HKCU\Environment\Path` via `reg.exe`.
/// Unix: appends a marker-guarded `export PATH=` line to each existing
/// `~/.bashrc` / `~/.zshrc` / `~/.profile` (creating `~/.profile` when none
/// exists).
pub fn ensure_keel_home_on_path(keel_home: &Path) -> String {
    let dir = display_path(keel_home);
    if path_already_contains(keel_home) {
        return format!("PATH already contains {dir}");
    }
    #[cfg(windows)]
    {
        match windows_append_user_path(keel_home) {
            Ok(()) => format!("added {dir} to user PATH (HKCU\\Environment)"),
            Err(error) => format!("PATH update skipped ({error}); add {dir} manually"),
        }
    }
    #[cfg(not(windows))]
    {
        match unix_append_path_export(keel_home) {
            Ok(files) => {
                if files.is_empty() {
                    format!("PATH update skipped (no shell rc file found); add {dir} manually")
                } else {
                    format!("added {dir} to PATH via {}", files.join(", "))
                }
            }
            Err(error) => format!("PATH update skipped ({error}); add {dir} manually"),
        }
    }
}

/// True when the current-process PATH already lists `keel_home`.
fn path_already_contains(keel_home: &Path) -> bool {
    let Some(path_value) = std::env::var_os("PATH") else {
        return false;
    };
    for entry in std::env::split_paths(&path_value) {
        if entry == keel_home {
            return true;
        }
    }
    false
}

/// Remove dead `keel-home-split-*\.keel` entries that a buggy older build
/// appended to the persistent user PATH during test installs. Only touches
/// entries that (a) live under a directory whose name starts with
/// `keel-home-split-` and (b) no longer exist on disk, so a legitimate live
/// install is never removed. Best-effort: failures are swallowed because this
/// is cleanup, not the install itself.
#[cfg(windows)]
pub(crate) fn purge_stale_temp_keel_path_entries() {
    let Ok((current, _expand)) = windows_read_user_path() else {
        return;
    };
    let kept: Vec<&str> = current
        .split(';')
        .filter(|entry| !is_stale_temp_keel_entry(entry.trim()))
        .collect();
    let new_value = kept.join(";");
    if new_value == current {
        return;
    }
    let value_type = if new_value.contains('%') {
        "REG_EXPAND_SZ"
    } else {
        "REG_SZ"
    };
    let _ = std::process::Command::new("reg")
        .args([
            "add",
            "HKCU\\Environment",
            "/v",
            "Path",
            "/t",
            value_type,
            "/d",
            &new_value,
            "/f",
        ])
        .status();
}

#[cfg(not(windows))]
pub(crate) fn purge_stale_temp_keel_path_entries() {
    // Unix fixtures appended a marker-guarded export line to rc files. Sweep
    // the marker + following export when the referenced dir is a dead temp.
    let Ok(user_home) = crate::runtime::resolve_user_home() else {
        return;
    };
    for rc in [
        user_home.join(".bashrc"),
        user_home.join(".zshrc"),
        user_home.join(".profile"),
    ] {
        let Ok(text) = read_text_if_exists(&rc) else {
            continue;
        };
        let mut out: Vec<&str> = Vec::new();
        let mut lines = text.lines().peekable();
        let mut changed = false;
        while let Some(line) = lines.next() {
            if line.trim() == KEEL_PATH_MARKER {
                if let Some(next) = lines.peek() {
                    if let Some(dir) = next
                        .trim()
                        .strip_prefix("export PATH=\"")
                        .and_then(|rest| rest.strip_suffix(":$PATH\""))
                    {
                        if is_stale_temp_keel_entry(dir) {
                            lines.next();
                            changed = true;
                            continue;
                        }
                    }
                }
            }
            out.push(line);
        }
        if changed {
            let mut joined = out.join("\n");
            if !joined.is_empty() && !joined.ends_with('\n') {
                joined.push('\n');
            }
            let _ = write_text(&rc, &joined);
        }
    }
}

/// True when `entry` is a dead temp-dir keel home left by a test install:
/// its parent directory name starts with `keel-home-split-`, its own name is
/// the standard `.keel`, and the directory no longer exists.
pub(crate) fn is_stale_temp_keel_entry(entry: &str) -> bool {
    if entry.is_empty() {
        return false;
    }
    let path = PathBuf::from(entry);
    let is_keel = path
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| name == crate::runtime::KEEL_HOME_DIRECTORY_NAME)
        .unwrap_or(false);
    let is_temp_parent = path
        .parent()
        .and_then(|parent| parent.file_name())
        .and_then(|name| name.to_str())
        .map(|name| name.starts_with("keel-home-split-"))
        .unwrap_or(false);
    is_keel && is_temp_parent && !path.exists()
}

/// Read the per-user PATH value from `HKCU\Environment` (empty when absent).
#[cfg(windows)]
fn windows_read_user_path() -> Result<(String, bool), String> {
    let output = std::process::Command::new("reg")
        .args(["query", "HKCU\\Environment", "/v", "Path"])
        .output()
        .map_err(|error| format!("run reg.exe: {error}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut current = String::new();
    let mut has_expand = false;
    if output.status.success() {
        for line in stdout.lines() {
            let trimmed = line.trim_start();
            let Some(rest) = trimmed.strip_prefix("Path") else {
                continue;
            };
            let rest = rest.trim_start();
            if rest.starts_with("REG_EXPAND_SZ") {
                has_expand = true;
                current = rest
                    .trim_start_matches("REG_EXPAND_SZ")
                    .trim_start()
                    .to_string();
            } else if rest.starts_with("REG_SZ") {
                current = rest.trim_start_matches("REG_SZ").trim_start().to_string();
            }
        }
    }
    Ok((current, has_expand))
}

#[cfg(windows)]
fn windows_append_user_path(keel_home: &Path) -> Result<(), String> {
    let dir = display_path(keel_home);
    let (current, has_expand) = windows_read_user_path()?;
    // Case-insensitive duplicate guard: Windows PATH entries ignore case.
    let lower_current = current.to_lowercase();
    let lower_home = dir.to_lowercase();
    for entry in lower_current.split(';') {
        if entry.trim() == lower_home.trim() {
            return Ok(());
        }
    }
    let new_value = if current.trim().is_empty() {
        dir.to_string()
    } else if current.ends_with(';') {
        format!("{current}{dir}")
    } else {
        format!("{current};{dir}")
    };
    // REG_EXPAND_SZ when the existing value uses %VAR% expansion, so reg.exe
    // does not silently convert it to a REG_SZ and break expansion.
    let value_type = if has_expand || new_value.contains('%') {
        "REG_EXPAND_SZ"
    } else {
        "REG_SZ"
    };
    let status = std::process::Command::new("reg")
        .args([
            "add",
            "HKCU\\Environment",
            "/v",
            "Path",
            "/t",
            value_type,
            "/d",
            &new_value,
            "/f",
        ])
        .status()
        .map_err(|error| format!("run reg.exe add: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("reg.exe add exited with {status}"))
    }
}

#[cfg(not(windows))]
fn unix_append_path_export(keel_home: &Path) -> Result<Vec<String>, String> {
    let user_home = crate::runtime::resolve_user_home()?;
    let export_line = format!(
        "{KEEL_PATH_MARKER}\nexport PATH=\"{dir}:$PATH\"\n",
        dir = display_path(keel_home)
    );
    let candidates = [
        user_home.join(".bashrc"),
        user_home.join(".zshrc"),
        user_home.join(".profile"),
    ];
    let mut touched = Vec::new();
    for rc in &candidates {
        if !rc.is_file() {
            continue;
        }
        let text = read_text_if_exists(rc).unwrap_or_default();
        // Marker-guarded: never append twice, never touch unmanaged lines.
        if text.contains(KEEL_PATH_MARKER) {
            continue;
        }
        let mut updated = text.clone();
        if !updated.is_empty() && !updated.ends_with('\n') {
            updated.push('\n');
        }
        updated.push_str(&export_line);
        write_text(rc, &updated)?;
        touched.push(display_path(rc));
    }
    if touched.is_empty() && !candidates.iter().any(|rc| rc.is_file()) {
        // No rc file at all: create ~/.profile so sh-based sessions pick it up.
        let profile = user_home.join(".profile");
        write_text(&profile, &export_line)?;
        touched.push(display_path(&profile));
    }
    Ok(touched)
}
