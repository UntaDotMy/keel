// Installer PATH helpers. Native `keel install` is the only PATH writer.
use crate::runtime::display_path;
#[cfg(not(windows))]
use crate::runtime::{read_text_if_exists, write_text};
use std::path::{Path, PathBuf};

/// Marker guarding keel-owned PATH lines in unix shell rc / fish conf.d files.
/// Match is whole-line trim equality, never a substring.
#[cfg(not(windows))]
pub(crate) const KEEL_PATH_MARKER: &str = "# keel PATH (managed by the keel installer)";

const FORBIDDEN_PATH_CHARS: &[char] = &['\n', '\r', '"', '\'', '`', '$', ';', '&', '|', '%'];

#[cfg(not(windows))]
const UNIX_PATH_WRITTEN: &str = "\
keel home is on PATH for bash, zsh, sh/dash (via .profile), and fish in new shells.
If `keel` is not found in this session, open a new terminal
or run: ~/.keel/keel status";

#[cfg(not(windows))]
const UNIX_PATH_WRITTEN_SHORT: &str = "keel home is on PATH.";

#[cfg(any(windows, test))]
const WINDOWS_PATH_WRITTEN: &str = "\
keel is on your User PATH for new sessions.
This window will not see it. Open a new console or a new Windows Terminal window,
or run: %USERPROFILE%\\.keel\\keel.exe status";

#[cfg(any(windows, test))]
const WINDOWS_PATH_WRITTEN_SHORT: &str = "keel is on your User PATH.";

const PATH_ALREADY_CONFIGURED: &str = "PATH already configured.";

/// Registry + WM_SETTINGCHANGE seam. Production uses `LivePathPersist`
/// (`reg.exe` HKCU + broadcast). Tests inject a double so Linux CI can prove
/// the writer contract without touching the live HKCU hive.
#[cfg(any(windows, test))]
pub(crate) trait PathPersist {
    fn read_user_path(&self) -> Result<(String, bool), String>;
    fn write_user_path(&self, value: &str, expand: bool) -> Result<(), String>;
    fn broadcast_environment(&self) -> Result<(), String>;
}

/// Ensure the keel binary directory is on the persistent user PATH.
///
/// Callers already gate `published_executable && is_default_keel_home`. This
/// function does **not** skip persistent writers because the process PATH
/// already lists `keel_home` — that early return was a defect. Process PATH is
/// used only for success-copy (this-session resolve).
///
/// Fail-closed: relative, empty, `.`, NUL, or a display path containing
/// newline / CR / quotes / backtick / `$` / `;` / `&` / `|` / `%` skips the
/// write and reports. Only `keel_home` itself is prepended, never a parent.
pub fn ensure_keel_home_on_path(keel_home: &Path) -> String {
    #[cfg(windows)]
    {
        ensure_windows_path(keel_home, &LivePathPersist)
    }
    #[cfg(not(windows))]
    {
        ensure_unix_path(keel_home)
    }
}

/// Reverse PATH files written by [`ensure_keel_home_on_path`]. Silent: uninstall
/// stdout must not claim PATH was restored.
pub(crate) fn remove_keel_home_from_path(keel_home: &Path) {
    #[cfg(windows)]
    {
        let _ = remove_windows_path(keel_home, &LivePathPersist);
    }
    #[cfg(not(windows))]
    {
        let Ok(user_home) = crate::runtime::resolve_user_home() else {
            return;
        };
        let _ = unix_remove_path_into(keel_home, &user_home);
    }
}

fn path_already_contains(keel_home: &Path) -> bool {
    let Some(path_value) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path_value).any(|entry| entry == keel_home)
}

fn is_windows_style_absolute(path: &Path) -> bool {
    let rendered = path.to_string_lossy();
    let trimmed = rendered.trim();
    if trimmed.starts_with('\\') {
        return true;
    }
    let bytes = trimmed.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/')
}

/// Windows Path entries compare case-insensitively and treat `/` as `\\`.
#[cfg(any(windows, test))]
fn windows_path_entry_key(entry: &str) -> String {
    entry.trim().replace('/', "\\").to_lowercase()
}

fn is_absolute_keel_home(path: &Path) -> bool {
    path.is_absolute() || is_windows_style_absolute(path)
}

fn path_contains_nul_component(path: &Path) -> bool {
    if path.as_os_str().as_encoded_bytes().contains(&0) {
        return true;
    }
    path.iter().any(|component| {
        component
            .to_str()
            .is_some_and(|name| name.eq_ignore_ascii_case("nul"))
    })
}

/// Fail-closed input check. On success returns the display path used as DATA
/// in env files / registry values — never interpolated unvalidated.
pub(crate) fn validate_keel_home(keel_home: &Path) -> Result<String, String> {
    if keel_home.as_os_str().is_empty() {
        return Err("keel home is empty".to_string());
    }
    if keel_home == Path::new(".") || display_path(keel_home) == "." {
        return Err("keel home is not an absolute path".to_string());
    }
    if path_contains_nul_component(keel_home) {
        return Err("keel home contains NUL".to_string());
    }
    if !is_absolute_keel_home(keel_home) {
        return Err("keel home is not an absolute path".to_string());
    }
    let dir = display_path(keel_home);
    if dir.chars().any(|ch| FORBIDDEN_PATH_CHARS.contains(&ch)) {
        return Err("keel home contains a forbidden character".to_string());
    }
    Ok(dir)
}

fn skipped_copy(reason: &str) -> String {
    #[cfg(windows)]
    {
        format!("PATH write skipped ({reason}). Use %USERPROFILE%\\.keel\\keel.exe")
    }
    #[cfg(not(windows))]
    {
        format!("PATH write skipped ({reason}). Use ~/.keel/keel")
    }
}

#[cfg(not(windows))]
fn unix_success_copy(wrote: bool, session_has: bool) -> String {
    if wrote {
        if session_has {
            UNIX_PATH_WRITTEN_SHORT.to_string()
        } else {
            UNIX_PATH_WRITTEN.to_string()
        }
    } else if session_has {
        PATH_ALREADY_CONFIGURED.to_string()
    } else {
        format!("{PATH_ALREADY_CONFIGURED}\n{UNIX_PATH_WRITTEN}")
    }
}

#[cfg(any(windows, test))]
fn windows_success_copy(wrote: bool, session_has: bool) -> String {
    if wrote {
        if session_has {
            WINDOWS_PATH_WRITTEN_SHORT.to_string()
        } else {
            WINDOWS_PATH_WRITTEN.to_string()
        }
    } else if session_has {
        PATH_ALREADY_CONFIGURED.to_string()
    } else {
        format!("{PATH_ALREADY_CONFIGURED}\n{WINDOWS_PATH_WRITTEN}")
    }
}

/// Remove dead `keel-home-split-*\.keel` entries that a buggy older build
/// appended to the persistent user PATH during test installs. Only touches
/// entries that (a) live under a directory whose name starts with
/// `keel-home-split-` and (b) no longer exist on disk, so a legitimate live
/// install is never removed. Best-effort: failures are swallowed because this
/// is cleanup, not the install itself.
///
/// Production Windows purge stays scoped to those dead temp dirs only.
pub(crate) fn purge_stale_temp_keel_path_entries() {
    #[cfg(windows)]
    {
        let _ = purge_stale_windows(&LivePathPersist);
    }
    #[cfg(not(windows))]
    {
        let Ok(user_home) = crate::runtime::resolve_user_home() else {
            return;
        };
        let _ = purge_stale_unix_into(&user_home);
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

// ---------------------------------------------------------------------------
// Windows PATH: HKCU\Environment Path + WM_SETTINGCHANGE (algorithm is OS-agnostic)
// ---------------------------------------------------------------------------

/// Append `keel_home` to the user Path and broadcast `Environment`.
/// Compiled on every OS so Linux CI can assert the writer via a test double.
#[cfg(any(windows, test))]
pub(crate) fn ensure_windows_path(keel_home: &Path, persist: &dyn PathPersist) -> String {
    let dir = match validate_keel_home(keel_home) {
        Ok(dir) => dir,
        Err(reason) => return skipped_copy(&reason),
    };
    match persist_windows_user_path(&dir, persist) {
        Ok(wrote) => windows_success_copy(wrote, path_already_contains(keel_home)),
        Err(error) => skipped_copy(&error),
    }
}

/// Returns `true` when the registry value was mutated.
#[cfg(any(windows, test))]
pub(crate) fn persist_windows_user_path(
    dir: &str,
    persist: &dyn PathPersist,
) -> Result<bool, String> {
    let (current, has_expand) = persist.read_user_path()?;
    let home_key = windows_path_entry_key(dir);
    for entry in current.split(';') {
        if windows_path_entry_key(entry) == home_key {
            return Ok(false);
        }
    }
    let new_value = if current.trim().is_empty() {
        dir.to_string()
    } else if current.ends_with(';') {
        format!("{current}{dir}")
    } else {
        format!("{current};{dir}")
    };
    // Preserve REG_EXPAND_SZ when the existing value expands variables.
    // Keel's own entry is a literal path with no `%` (rejected at validate).
    let expand = has_expand || new_value.contains('%');
    persist.write_user_path(&new_value, expand)?;
    let _ = persist.broadcast_environment();
    Ok(true)
}

#[cfg(any(windows, test))]
pub(crate) fn remove_windows_path(
    keel_home: &Path,
    persist: &dyn PathPersist,
) -> Result<bool, String> {
    let dir = match validate_keel_home(keel_home) {
        Ok(dir) => dir,
        Err(_) => return Ok(false),
    };
    let (current, has_expand) = persist.read_user_path()?;
    let home_key = windows_path_entry_key(&dir);
    let kept: Vec<&str> = current
        .split(';')
        .filter(|entry| windows_path_entry_key(entry) != home_key)
        .collect();
    let new_value = kept.join(";");
    if new_value == current {
        return Ok(false);
    }
    persist.write_user_path(&new_value, has_expand || new_value.contains('%'))?;
    let _ = persist.broadcast_environment();
    Ok(true)
}

#[cfg(any(windows, test))]
pub(crate) fn purge_stale_windows(persist: &dyn PathPersist) -> Result<bool, String> {
    let (current, has_expand) = persist.read_user_path()?;
    let kept: Vec<&str> = current
        .split(';')
        .filter(|entry| !is_stale_temp_keel_entry(entry.trim()))
        .collect();
    let new_value = kept.join(";");
    if new_value == current {
        return Ok(false);
    }
    persist.write_user_path(&new_value, has_expand || new_value.contains('%'))?;
    Ok(true)
}

#[cfg(windows)]
struct LivePathPersist;

#[cfg(windows)]
impl PathPersist for LivePathPersist {
    fn read_user_path(&self) -> Result<(String, bool), String> {
        windows_read_user_path()
    }

    fn write_user_path(&self, value: &str, expand: bool) -> Result<(), String> {
        windows_reg_write_user_path(value, expand)
    }

    fn broadcast_environment(&self) -> Result<(), String> {
        windows_broadcast_environment()
    }
}

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
fn windows_reg_write_user_path(value: &str, expand: bool) -> Result<(), String> {
    let value_type = if expand { "REG_EXPAND_SZ" } else { "REG_SZ" };
    let status = std::process::Command::new("reg")
        .args([
            "add",
            "HKCU\\Environment",
            "/v",
            "Path",
            "/t",
            value_type,
            "/d",
            value,
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

/// Broadcast WM_SETTINGCHANGE with lParam = Environment so newly started
/// consoles pick up HKCU Path. The already-open console is not updated.
#[cfg(windows)]
fn windows_broadcast_environment() -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;

    const HWND_BROADCAST: isize = 0xffff;
    const WM_SETTINGCHANGE: u32 = 0x001A;
    const SMTO_ABORTIFHUNG: u32 = 0x0002;

    #[link(name = "user32")]
    extern "system" {
        fn SendMessageTimeoutW(
            hwnd: isize,
            msg: u32,
            wparam: usize,
            lparam: *const u16,
            fu_flags: u32,
            u_timeout: u32,
            lpdw_result: *mut usize,
        ) -> isize;
    }

    let mut environment: Vec<u16> = std::ffi::OsStr::new("Environment")
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut result: usize = 0;
    let sent = unsafe {
        SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            0,
            environment.as_mut_ptr(),
            SMTO_ABORTIFHUNG,
            5_000,
            &mut result,
        )
    };
    if sent == 0 {
        Err("WM_SETTINGCHANGE broadcast timed out or failed".to_string())
    } else {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Unix PATH: rustup-shaped shared env + per-shell source
// ---------------------------------------------------------------------------

#[cfg(not(windows))]
fn ensure_unix_path(keel_home: &Path) -> String {
    let user_home = match crate::runtime::resolve_user_home() {
        Ok(home) => home,
        Err(error) => return skipped_copy(&error),
    };
    ensure_unix_path_for_home(keel_home, &user_home)
}

#[cfg(not(windows))]
pub(crate) fn ensure_unix_path_for_home(keel_home: &Path, user_home: &Path) -> String {
    if let Err(reason) = validate_keel_home(keel_home) {
        return skipped_copy(&reason);
    }
    match unix_write_path_into(keel_home, user_home) {
        Ok(wrote) => unix_success_copy(wrote, path_already_contains(keel_home)),
        Err(error) => skipped_copy(&error),
    }
}

#[cfg(not(windows))]
fn has_marker_line(text: &str) -> bool {
    text.lines().any(|line| line.trim() == KEEL_PATH_MARKER)
}

#[cfg(not(windows))]
fn posix_env_contents(dir: &str) -> String {
    format!(
        "{KEEL_PATH_MARKER}\n\
case \":${{PATH}}:\" in\n\
    *:\"{dir}\":*)\n\
        ;;\n\
    *)\n\
        export PATH=\"{dir}:$PATH\"\n\
        ;;\n\
esac\n"
    )
}

#[cfg(not(windows))]
fn fish_env_contents(dir: &str) -> String {
    format!(
        "{KEEL_PATH_MARKER}\n\
if not contains \"{dir}\" $PATH\n\
    set -x PATH \"{dir}\" $PATH\n\
end\n"
    )
}

#[cfg(not(windows))]
fn posix_source_line(env_path: &str) -> String {
    format!(". \"{env_path}\"")
}

#[cfg(not(windows))]
fn fish_source_line(env_path: &str) -> String {
    format!("source \"{env_path}\"")
}

#[cfg(not(windows))]
fn is_old_triplicate_export(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with("export PATH=\"") && trimmed.ends_with(":$PATH\"")
}

#[cfg(not(windows))]
fn realpath_existing(path: &Path) -> Result<PathBuf, String> {
    fs_canonicalize(path)
}

#[cfg(not(windows))]
fn fs_canonicalize(path: &Path) -> Result<PathBuf, String> {
    std::fs::canonicalize(path).map_err(|error| format!("resolve {}: {error}", display_path(path)))
}

#[cfg(not(windows))]
fn realpath_existing_prefix(path: &Path) -> Result<PathBuf, String> {
    if path.exists() {
        return realpath_existing(path);
    }
    let mut suffix = Vec::new();
    let mut current = path.to_path_buf();
    loop {
        if let Some(name) = current.file_name() {
            suffix.push(name.to_os_string());
        }
        match current.parent() {
            Some(parent) if parent != current => current = parent.to_path_buf(),
            _ => break,
        }
        if current.exists() {
            let mut resolved = realpath_existing(&current)?;
            for name in suffix.iter().rev() {
                resolved.push(name);
            }
            return Ok(resolved);
        }
    }
    Err(format!(
        "resolve {}: no existing prefix",
        display_path(path)
    ))
}

#[cfg(not(windows))]
fn destination_stays_under_home(path: &Path, home: &Path) -> Result<(), String> {
    let resolved_home = realpath_existing(home)?;
    let resolved_path = realpath_existing_prefix(path)?;
    if resolved_path == resolved_home || resolved_path.starts_with(&resolved_home) {
        Ok(())
    } else {
        Err("keel home destination escapes HOME".to_string())
    }
}

#[cfg(not(windows))]
fn limit_mode_0644(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let metadata =
        std::fs::metadata(path).map_err(|error| format!("stat {}: {error}", display_path(path)))?;
    let mode = metadata.permissions().mode();
    let limited = mode & 0o644;
    if limited != mode {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(limited))
            .map_err(|error| format!("chmod {}: {error}", display_path(path)))?;
    }
    Ok(())
}

#[cfg(not(windows))]
fn write_env_file(path: &Path, contents: &str, home: &Path) -> Result<bool, String> {
    destination_stays_under_home(path, home)?;
    let existing = read_text_if_exists(path).unwrap_or_default();
    if existing == contents {
        limit_mode_0644(path)?;
        return Ok(false);
    }
    write_text(path, contents)?;
    limit_mode_0644(path)?;
    Ok(true)
}

#[cfg(not(windows))]
enum RcKind {
    Posix,
    Fish,
}

#[cfg(not(windows))]
fn apply_source_block(
    path: &Path,
    home: &Path,
    env_path: &str,
    kind: RcKind,
    create_if_missing: bool,
) -> Result<bool, String> {
    destination_stays_under_home(path, home)?;
    let exists = path.is_file();
    if !exists && !create_if_missing {
        return Ok(false);
    }
    let text = if exists {
        read_text_if_exists(path).unwrap_or_default()
    } else {
        String::new()
    };
    let source_line = match kind {
        RcKind::Posix => posix_source_line(env_path),
        RcKind::Fish => fish_source_line(env_path),
    };
    if has_marker_line(&text) {
        return migrate_or_skip_marker(&text, path, &source_line);
    }
    let mut updated = text;
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    updated.push_str(KEEL_PATH_MARKER);
    updated.push('\n');
    updated.push_str(&source_line);
    updated.push('\n');
    write_text(path, &updated)?;
    Ok(true)
}

#[cfg(not(windows))]
fn migrate_or_skip_marker(text: &str, path: &Path, source_line: &str) -> Result<bool, String> {
    let mut out = Vec::new();
    let mut lines = text.lines().peekable();
    let mut changed = false;
    while let Some(line) = lines.next() {
        if line.trim() == KEEL_PATH_MARKER {
            out.push(line.to_string());
            if let Some(next) = lines.peek() {
                if is_old_triplicate_export(next) {
                    lines.next();
                    out.push(source_line.to_string());
                    changed = true;
                    continue;
                }
                if next.trim() == source_line {
                    out.push(lines.next().unwrap().to_string());
                    continue;
                }
            }
            continue;
        }
        out.push(line.to_string());
    }
    if !changed {
        return Ok(false);
    }
    let mut joined = out.join("\n");
    if !joined.is_empty() && !joined.ends_with('\n') {
        joined.push('\n');
    }
    write_text(path, &joined)?;
    Ok(true)
}

/// Write rustup-shaped env files and marker-guarded per-shell sources under
/// `user_home`. Returns whether any file was mutated.
#[cfg(not(windows))]
pub(crate) fn unix_write_path_into(keel_home: &Path, user_home: &Path) -> Result<bool, String> {
    let dir = validate_keel_home(keel_home)?;
    if !user_home.exists() {
        return Err("user HOME does not exist".to_string());
    }
    destination_stays_under_home(keel_home, user_home)?;

    let env_path = keel_home.join("env");
    let fish_env_path = keel_home.join("env.fish");
    let env_display = display_path(&env_path);
    let fish_env_display = display_path(&fish_env_path);

    let mut wrote = false;
    wrote |= write_env_file(&env_path, &posix_env_contents(&dir), user_home)?;
    wrote |= write_env_file(&fish_env_path, &fish_env_contents(&dir), user_home)?;

    let profile = user_home.join(".profile");
    let bashrc = user_home.join(".bashrc");
    let bash_profile = user_home.join(".bash_profile");
    let zshenv = user_home.join(".zshenv");
    let zshrc = user_home.join(".zshrc");
    let fish_conf = user_home
        .join(".config")
        .join("fish")
        .join("conf.d")
        .join("keel.fish");

    wrote |= apply_source_block(&profile, user_home, &env_display, RcKind::Posix, true)?;
    wrote |= apply_source_block(&bashrc, user_home, &env_display, RcKind::Posix, false)?;
    wrote |= apply_source_block(&bash_profile, user_home, &env_display, RcKind::Posix, false)?;
    wrote |= apply_source_block(&zshenv, user_home, &env_display, RcKind::Posix, true)?;
    wrote |= apply_source_block(&zshrc, user_home, &env_display, RcKind::Posix, false)?;
    wrote |= apply_source_block(&fish_conf, user_home, &fish_env_display, RcKind::Fish, true)?;
    Ok(wrote)
}

#[cfg(not(windows))]
fn strip_managed_blocks(text: &str, drop_old_export: bool) -> (String, bool) {
    let mut out: Vec<&str> = Vec::new();
    let mut lines = text.lines().peekable();
    let mut changed = false;
    while let Some(line) = lines.next() {
        if line.trim() == KEEL_PATH_MARKER {
            if let Some(next) = lines.peek() {
                let follow = next.trim();
                let drop = follow.starts_with(". \"")
                    || follow.starts_with("source \"")
                    || (drop_old_export && is_old_triplicate_export(next));
                if drop {
                    lines.next();
                    changed = true;
                    continue;
                }
                out.push(line);
                continue;
            }
            changed = true;
            continue;
        }
        out.push(line);
    }
    let mut joined = out.join("\n");
    if !joined.is_empty() && !joined.ends_with('\n') {
        joined.push('\n');
    }
    if text.is_empty() {
        joined.clear();
    }
    (joined, changed)
}

#[cfg(not(windows))]
fn rewrite_if_changed(
    path: &Path,
    text: &str,
    updated: String,
    changed: bool,
) -> Result<bool, String> {
    if !changed {
        return Ok(false);
    }
    if updated == text {
        return Ok(false);
    }
    write_text(path, &updated)?;
    Ok(true)
}

#[cfg(not(windows))]
pub(crate) fn unix_remove_path_into(keel_home: &Path, user_home: &Path) -> Result<bool, String> {
    let mut changed = false;
    for name in ["env", "env.fish"] {
        let path = keel_home.join(name);
        if path.is_file() {
            match std::fs::remove_file(&path) {
                Ok(()) => changed = true,
                Err(error) => {
                    return Err(format!("remove {}: {error}", display_path(&path)));
                }
            }
        }
    }
    let posix_rcs = [
        user_home.join(".profile"),
        user_home.join(".bashrc"),
        user_home.join(".bash_profile"),
        user_home.join(".zshenv"),
        user_home.join(".zshrc"),
    ];
    for rc in posix_rcs {
        if !rc.is_file() {
            continue;
        }
        let text = read_text_if_exists(&rc).unwrap_or_default();
        let (updated, did) = strip_managed_blocks(&text, true);
        changed |= rewrite_if_changed(&rc, &text, updated, did)?;
    }
    let fish_conf = user_home
        .join(".config")
        .join("fish")
        .join("conf.d")
        .join("keel.fish");
    if fish_conf.is_file() {
        let text = read_text_if_exists(&fish_conf).unwrap_or_default();
        if has_marker_line(&text) {
            let (updated, did) = strip_managed_blocks(&text, true);
            if updated.trim().is_empty() {
                std::fs::remove_file(&fish_conf)
                    .map_err(|error| format!("remove {}: {error}", display_path(&fish_conf)))?;
                changed = true;
            } else {
                changed |= rewrite_if_changed(&fish_conf, &text, updated, did)?;
            }
        }
    }
    Ok(changed)
}

/// Sweep old triplicate `export PATH="…:$PATH"` marker pairs, and managed
/// source blocks that point at a dead `keel-home-split-*\.keel`.
#[cfg(not(windows))]
pub(crate) fn purge_stale_unix_into(user_home: &Path) -> Result<bool, String> {
    let mut changed = false;
    let rcs = [
        user_home.join(".profile"),
        user_home.join(".bashrc"),
        user_home.join(".bash_profile"),
        user_home.join(".zshenv"),
        user_home.join(".zshrc"),
    ];
    for rc in rcs {
        if !rc.is_file() {
            continue;
        }
        let text = read_text_if_exists(&rc).unwrap_or_default();
        let mut out: Vec<&str> = Vec::new();
        let mut lines = text.lines().peekable();
        let mut file_changed = false;
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
                            file_changed = true;
                            continue;
                        }
                    }
                    if let Some(quoted) = next
                        .trim()
                        .strip_prefix(". \"")
                        .and_then(|rest| rest.strip_suffix('"'))
                    {
                        if let Some(dir) = Path::new(quoted).parent() {
                            if is_stale_temp_keel_entry(&display_path(dir)) {
                                lines.next();
                                file_changed = true;
                                continue;
                            }
                        }
                    }
                }
            }
            out.push(line);
        }
        if file_changed {
            let mut joined = out.join("\n");
            if !joined.is_empty() && !joined.ends_with('\n') {
                joined.push('\n');
            }
            write_text(&rc, &joined)?;
            changed = true;
        }
    }
    Ok(changed)
}

// ---------------------------------------------------------------------------
// Test double (all OS) — live `reg add HKCU\Environment` in tests is a defect
// ---------------------------------------------------------------------------

#[cfg(test)]
#[derive(Default)]
pub(crate) struct RecordingPathPersist {
    current: std::sync::Mutex<(String, bool)>,
    pub writes: std::sync::Mutex<Vec<(String, bool)>>,
    pub broadcasts: std::sync::Mutex<Vec<String>>,
}

#[cfg(test)]
impl RecordingPathPersist {
    pub(crate) fn new(current: &str, expand: bool) -> Self {
        Self {
            current: std::sync::Mutex::new((current.to_string(), expand)),
            writes: std::sync::Mutex::new(Vec::new()),
            broadcasts: std::sync::Mutex::new(Vec::new()),
        }
    }
}

#[cfg(test)]
impl PathPersist for RecordingPathPersist {
    fn read_user_path(&self) -> Result<(String, bool), String> {
        Ok(self.current.lock().expect("persist lock").clone())
    }

    fn write_user_path(&self, value: &str, expand: bool) -> Result<(), String> {
        *self.current.lock().expect("persist lock") = (value.to_string(), expand);
        self.writes
            .lock()
            .expect("persist lock")
            .push((value.to_string(), expand));
        Ok(())
    }

    fn broadcast_environment(&self) -> Result<(), String> {
        self.broadcasts
            .lock()
            .expect("persist lock")
            .push("Environment".to_string());
        Ok(())
    }
}
