//! Purpose: Shared Rust-native filesystem, repository layout, and process helpers for the keel CLI.
//! Caller: manager, review, runner, and utility command modules.
//! Dependencies: std::env, std::fs, std::io, std::path, and std::process.
//! Main Functions: discover_repository_layout, resolve_repository_root, resolve_claude_home, run_command.
//! Side Effects: Reads repository files, copies managed assets, creates directories, removes managed paths, and runs child processes when requested.

use std::env;
use std::fs;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command, Stdio};

pub const ROOT_GUIDANCE_RELATIVE_PATHS: &[&str] = &[
    "AGENTS.md",
    "00-skill-routing-and-escalation.md",
    "docs/runtime-guardrails-and-memory-protocols.md",
    "docs/open-source-memory-patterns.md",
    "docs/security-audit-status.md",
];
pub const COMMAND_COMPACTION_EVENTS_FILE_NAME: &str = "command-compaction-events.jsonl";

pub const SKILL_SYNC_DIRECTORIES: &[&str] = &[
    "references",
    "scripts",
    "data",
    "agents",
    "templates",
    "examples",
    "assets",
];

/// Top-level repository directories that hold cross-skill resources referenced
/// by `SKILL.md` files via relative paths (for example `_shared/common-discipline.md`).
/// Listed explicitly so the installer never picks up build outputs, scratch
/// folders, or other transient siblings of the skill directories.
pub const SHARED_RESOURCE_DIRECTORIES: &[&str] = &["_shared"];

#[derive(Debug, Clone)]
pub struct RepositoryLayout {
    pub root_path: PathBuf,
    pub root_files: Vec<String>,
    pub skills: Vec<SkillDefinition>,
    pub agent_names: Vec<String>,
    /// Names of directories from `SHARED_RESOURCE_DIRECTORIES` that exist on
    /// disk under `root_path`. These are copied verbatim to `<claude_home>/skills/<name>/`
    /// so SKILL.md relative references like `_shared/common-discipline.md`
    /// resolve from the installed skills directory.
    pub shared_resource_directories: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct SkillDefinition {
    pub name: String,
    pub skill_path: PathBuf,
    pub agent_configs: Vec<AgentConfig>,
}

#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub agent_name: String,
    pub config_path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct ProcessResult {
    pub code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    /// The true byte length of stdout before any capture cap was applied. Equals
    /// `stdout.len()` when no truncation occurred. Lets gain analytics report
    /// honest savings even when a runaway command's output was capped.
    pub original_stdout_bytes: usize,
    /// The true byte length of stderr before any capture cap was applied.
    pub original_stderr_bytes: usize,
}

/// Hard cap on captured command output per stream. A noisy or malicious command
/// can produce gigabytes; without a cap, `Command::output()` buffers the entire
/// stream into RAM and `RawStore::save` writes it all to disk before any check.
/// 64 MiB leaves room for full test logs (the legitimate large-output case)
/// while bounding the worst case. Mirrors the `MAX_EVENT_LOG_BYTES` precedent.
pub const MAX_CAPTURED_OUTPUT_BYTES: usize = 64 * 1024 * 1024;
/// Default wall-clock budget for internal command probes and `keel run`.
/// Override with `KEEL_COMMAND_TIMEOUT_SECS`; values are clamped to 5 minutes
/// through 1 hour so a malformed value cannot create an unbounded wait.
const DEFAULT_COMMAND_TIMEOUT_SECS: u64 = 300;

/// Truncate a captured stream to the cap, returning (bytes, original_len). When
/// truncation occurs, a trailing marker is appended so the model can see the
/// output was cut.
pub fn cap_captured_stream(bytes: Vec<u8>) -> (Vec<u8>, usize) {
    let original_len = bytes.len();
    if original_len <= MAX_CAPTURED_OUTPUT_BYTES {
        return (bytes, original_len);
    }
    let mut truncated = bytes[..MAX_CAPTURED_OUTPUT_BYTES].to_vec();
    let marker = b"\n[keel] captured output capped at MAX_CAPTURED_OUTPUT_BYTES; see raw output locally for the full stream\n";
    // Make room for the marker so the total stays at/under the cap.
    let needed = marker.len();
    if truncated.len() > needed {
        truncated.truncate(truncated.len() - needed);
    }
    truncated.extend_from_slice(marker);
    (truncated, original_len)
}

pub fn discover_repository_layout(repository_root: &Path) -> Result<RepositoryLayout, String> {
    if !repository_layout_is_complete(repository_root) {
        return Err(format!(
            "repository root is missing required keel files: {}",
            display_path(repository_root)
        ));
    }

    let directory_entries =
        fs::read_dir(repository_root).map_err(|error| format!("read repository root: {error}"))?;
    let mut layout = RepositoryLayout {
        root_path: repository_root.to_path_buf(),
        root_files: ROOT_GUIDANCE_RELATIVE_PATHS
            .iter()
            .map(|value| value.to_string())
            .collect(),
        skills: Vec::new(),
        agent_names: Vec::new(),
        shared_resource_directories: SHARED_RESOURCE_DIRECTORIES
            .iter()
            .filter(|name| repository_root.join(name).is_dir())
            .map(|name| name.to_string())
            .collect(),
    };

    for entry_result in directory_entries {
        let entry = entry_result.map_err(|error| format!("read repository entry: {error}"))?;
        let file_type = entry
            .file_type()
            .map_err(|error| format!("read repository entry type: {error}"))?;
        if !file_type.is_dir() {
            continue;
        }
        let skill_name = entry.file_name().to_string_lossy().to_string();
        if skill_name.starts_with('.') {
            continue;
        }

        let skill_path = repository_root.join(&skill_name);
        if !skill_path.join("SKILL.md").is_file() {
            continue;
        }

        let mut skill = SkillDefinition {
            name: skill_name.clone(),
            skill_path: skill_path.clone(),
            agent_configs: Vec::new(),
        };
        let agents_directory = skill_path.join("agents");
        if agents_directory.is_dir() {
            let mut config_paths = Vec::new();
            for config_entry_result in fs::read_dir(&agents_directory)
                .map_err(|error| format!("list agent configs for {skill_name}: {error}"))?
            {
                let config_entry = config_entry_result
                    .map_err(|error| format!("read agent config for {skill_name}: {error}"))?;
                let config_path = config_entry.path();
                if config_path.extension().and_then(|value| value.to_str()) == Some("yaml") {
                    config_paths.push(config_path);
                }
            }
            config_paths.sort();
            for config_path in config_paths {
                let agent_name = home_agent_name_from_config_path(&skill_name, &config_path);
                layout.agent_names.push(agent_name.clone());
                skill.agent_configs.push(AgentConfig {
                    agent_name,
                    config_path,
                });
            }
        }
        layout.skills.push(skill);
    }

    layout
        .skills
        .sort_by(|left, right| left.name.cmp(&right.name));
    layout.agent_names.sort();
    layout.agent_names.dedup();
    Ok(layout)
}

pub fn repository_layout_is_complete(repository_root: &Path) -> bool {
    [
        "AGENTS.md",
        "README.md",
        "00-skill-routing-and-escalation.md",
        "reviewer/SKILL.md",
    ]
    .iter()
    .all(|relative_path| repository_root.join(relative_path).is_file())
}

pub fn home_agent_name_from_config_path(skill_name: &str, config_path: &Path) -> String {
    let config_base_name = config_path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("");
    if config_base_name == "claude" || config_base_name == "openai" {
        skill_name.to_string()
    } else {
        config_base_name.to_string()
    }
}

pub fn resolve_repository_root(requested_repository_root: &str) -> Result<PathBuf, String> {
    let trimmed = requested_repository_root.trim();
    let candidate = if trimmed.is_empty() {
        env::current_dir().map_err(|error| format!("resolve current directory: {error}"))?
    } else {
        PathBuf::from(trimmed)
    };
    let absolute_candidate = if candidate.is_absolute() {
        candidate
    } else {
        env::current_dir()
            .map_err(|error| format!("resolve current directory: {error}"))?
            .join(candidate)
    };
    Ok(clean_path(&absolute_candidate))
}

/// True when `path` is absolute on ANY platform, not just the host. `Path::is_absolute`
/// is host-specific: a Windows drive path (`C:/x` / `D:\x`) reads as relative on Unix,
/// which would wrongly get the current directory prepended. A leading `/` or `\`, a
/// drive-letter prefix (`X:`), or a UNC root (`\\`) all count as absolute here so
/// cross-platform callers never rebase an already-rooted path.
#[cfg(test)]
pub fn is_absolute_any_platform(path: &str) -> bool {
    let trimmed = path.trim();
    if trimmed.starts_with(['/', '\\']) {
        return true;
    }
    let bytes = trimmed.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

/// The user's home directory (`$HOME`, falling back to `%USERPROFILE%`).
pub fn resolve_user_home() -> Result<PathBuf, String> {
    let home = env::var("HOME")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            env::var("USERPROFILE")
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
        .ok_or_else(|| "no user home directory available".to_string())?;
    Ok(clean_path(&PathBuf::from(home)))
}

/// keel's host-neutral root home: the binary, data, and state live here so
/// every host (claude, codex, opencode, cursor, pi, cowork) shares one
/// install. Resolution order: explicit flag → `KEEL_HOME` env →
/// legacy `CLAUDE_TARGET_OVERRIDE` env → `~/.keel` default.
pub fn resolve_keel_home(requested_home: &str) -> Result<PathBuf, String> {
    let trimmed = requested_home.trim();
    if !trimmed.is_empty() {
        return Ok(clean_path(&PathBuf::from(trimmed)));
    }
    for env_name in ["KEEL_HOME", "CLAUDE_TARGET_OVERRIDE"] {
        if let Ok(override_value) = env::var(env_name) {
            let trimmed_override = override_value.trim();
            if !trimmed_override.is_empty() {
                return Ok(clean_path(&PathBuf::from(trimmed_override)));
            }
        }
    }
    Ok(resolve_user_home()?.join(".keel"))
}

/// Resolve keel's root home. Historically named "claude home" because the
/// install lived under `~/.claude`; the default is now the host-neutral
/// `~/.keel` (see `resolve_keel_home`). Callers receive the root; claude-
/// harness-specific artifacts derive their own location from
/// `claude_engagement_home`.
pub fn resolve_claude_home(requested_claude_home: &str) -> Result<PathBuf, String> {
    resolve_keel_home(requested_claude_home)
}

/// Directory name of the standard host-neutral keel home.
pub const KEEL_HOME_DIRECTORY_NAME: &str = ".keel";

/// Where the claude-harness-specific engagement artifacts live (skills,
/// agents, commands, settings.json, user CLAUDE.md, the plugin): the harness
/// only reads them from `~/.claude`, so they can never move to `~/.keel`.
///
/// Split rule: a standard `.keel` uses its sibling `.claude`; a custom root
/// selected through `KEEL_HOME` uses the user's `.claude`. Explicit legacy
/// `--claude-home` and test roots still use the single-root behavior.
pub fn claude_engagement_home(keel_home: &Path) -> PathBuf {
    let configured_keel_home = env::var("KEEL_HOME")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    let user_home = resolve_user_home().ok();
    claude_engagement_home_for(
        keel_home,
        user_home.as_deref(),
        configured_keel_home.as_deref(),
    )
}

fn claude_engagement_home_for(
    keel_home: &Path,
    user_home: Option<&Path>,
    configured_keel_home: Option<&Path>,
) -> PathBuf {
    let is_standard_keel_home = keel_home
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| name == KEEL_HOME_DIRECTORY_NAME)
        .unwrap_or(false);
    if is_standard_keel_home {
        if let Some(parent) = keel_home.parent() {
            return parent.join(".claude");
        }
    }
    if configured_keel_home
        .map(|configured| clean_path(configured) == clean_path(keel_home))
        .unwrap_or(false)
    {
        if let Some(user_home) = user_home {
            return user_home.join(".claude");
        }
    }
    keel_home.to_path_buf()
}

/// True when `keel_home` is the standard `~/.keel` (basename check only; the
/// parent is not validated so temp-dir `.keel` fixtures also count).
pub fn is_standard_keel_home(keel_home: &Path) -> bool {
    keel_home
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| name == KEEL_HOME_DIRECTORY_NAME)
        .unwrap_or(false)
}

/// True when `keel_home` is the user's DEFAULT keel home (`~/.keel` from
/// [`resolve_user_home`]), as opposed to a `--claude-home`/`KEEL_HOME`
/// override or a test fixture. PATH wiring must only ever touch the
/// persistent user PATH for the default home: a temp-dir fixture passes
/// `is_standard_keel_home` (basename-only) and previously leaked dead
/// `keel-home-split-*\.keel` directories into the user PATH on every install.
pub fn is_default_keel_home(keel_home: &Path) -> bool {
    match resolve_user_home() {
        Ok(user_home) => is_default_keel_home_for(&user_home, keel_home),
        Err(_) => false,
    }
}
fn is_default_keel_home_for(user_home: &Path, keel_home: &Path) -> bool {
    clean_path(&user_home.join(KEEL_HOME_DIRECTORY_NAME)) == clean_path(keel_home)
}

/// Inverse of `claude_engagement_home`: given the claude-harness engagement
/// home, find the host-neutral keel home that holds the binary and data.
///
/// When the engagement home is a standard `.claude` and its sibling `.keel`
/// exists, that sibling is the keel home (the migrated layout). Otherwise the
/// engagement home IS the keel home (legacy installs and non-standard roots).
/// The existence check keeps pre-migration installs correct: until `~/.keel`
/// is created, the binary still lives under `~/.claude`.
pub fn keel_home_from_engagement(engagement_home: &Path) -> PathBuf {
    let is_standard_engagement = engagement_home
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| name == ".claude")
        .unwrap_or(false);
    if is_standard_engagement {
        if let Some(parent) = engagement_home.parent() {
            let sibling = parent.join(KEEL_HOME_DIRECTORY_NAME);
            if sibling.is_dir() {
                return sibling;
            }
        }
    }
    engagement_home.to_path_buf()
}

/// Validate that `candidate` is a single, safe path segment usable as one
/// directory or file-stem component — never a path that could escape its parent.
///
/// Returns the trimmed segment on success. The rule is platform-uniform so a
/// `~/.claude` tree that syncs across machines (OneDrive, Dropbox) cannot smuggle
/// a name that is benign on the writing OS but a traversal on the reading one:
///
/// 1. Reject any candidate containing `/`, `\\`, or `:` outright. On Windows
///    `Path::components` would flag `C:foo` / `\\server\share` as a `Prefix`,
///    but on Unix those are ordinary `Normal` characters — checking the raw
///    bytes makes the verdict identical everywhere.
/// 2. Then require exactly one `Normal` component, which additionally rejects
///    empty input, `.`, and `..`.
///
/// Callers join the result under a fixed base (skills dir, working-briefs dir)
/// to build a sandboxed path from caller-supplied names.
pub fn safe_path_segment(candidate: &str) -> Option<String> {
    let trimmed = candidate.trim();
    if trimmed.is_empty() || trimmed.contains(['/', '\\', ':']) {
        return None;
    }
    let mut components = Path::new(trimmed).components();
    let first = components.next()?;
    match (first, components.next()) {
        (Component::Normal(name), None) if name == std::ffi::OsStr::new(trimmed) => {
            Some(trimmed.to_string())
        }
        _ => None,
    }
}

pub fn clean_path(raw_path: &Path) -> PathBuf {
    let mut cleaned_path = PathBuf::new();
    for component in raw_path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                cleaned_path.pop();
            }
            other_component => cleaned_path.push(other_component.as_os_str()),
        }
    }
    if cleaned_path.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        cleaned_path
    }
}

pub fn display_path(path: &Path) -> String {
    let rendered = path.to_string_lossy().to_string();
    if cfg!(windows) {
        rendered.replace('/', "\\")
    } else {
        rendered
    }
}

/// Skills are a harness-engagement artifact: the claude harness only loads them
/// from `~/.claude/skills`, so the directory resolves against the engagement
/// home even when the keel root is `~/.keel`. Non-standard roots resolve to
/// themselves, preserving single-root behavior for tests and overrides.
pub fn skills_directory(claude_home: &Path) -> PathBuf {
    claude_engagement_home(claude_home).join("skills")
}

/// Subagent definitions: same engagement-home rule as `skills_directory`
/// (the harness reads them from `~/.claude/agents`).
pub fn agents_directory(claude_home: &Path) -> PathBuf {
    claude_engagement_home(claude_home).join("agents")
}

/// Slash commands: same engagement-home rule as `skills_directory`
/// (the harness reads them from `~/.claude/commands`).
pub fn commands_directory(claude_home: &Path) -> PathBuf {
    claude_engagement_home(claude_home).join("commands")
}

/// Managed agent profiles (`<name>.toml`) are keel-internal CLI config; the
/// harness never reads them. They install alongside the other managed pack
/// surfaces in the engagement home (`~/.claude`) so inventory tracking,
/// orphan cleanup, verify, and uninstall all share one tree.
pub fn agent_profiles_directory(claude_home: &Path) -> PathBuf {
    claude_home.join("agent-profiles")
}

/// Install inventories and other keel-owned state. Lives under the keel home
/// (`~/.keel/state`), never under `~/.claude`.
pub fn state_directory(claude_home: &Path) -> PathBuf {
    claude_home.join("state")
}

/// Pre-split inventory directory name. Install migrates this to [`state_directory`].
pub fn legacy_state_directory(claude_home: &Path) -> PathBuf {
    claude_home.join(".claude-skill-manager")
}

/// Transient update download/extract tree. Deleted after install/update.
pub fn update_cache_directory(claude_home: &Path) -> PathBuf {
    claude_home.join("cache")
}

pub fn config_path(claude_home: &Path) -> PathBuf {
    claude_home.join("config.toml")
}

pub fn executable_file_name() -> String {
    if cfg!(windows) {
        "keel.exe".to_string()
    } else {
        "keel".to_string()
    }
}

/// The installed keel binary. Host-neutral: it lives in the keel home
/// (`~/.keel` by default), not in `~/.claude`, so every host can invoke it.
pub fn installed_executable_path(keel_home: &Path) -> PathBuf {
    keel_home.join(executable_file_name())
}

/// Legacy install location (`~/.claude/keel[.exe]`). Used only to detect and
/// remove the old placement during migration; nothing new installs here.
pub fn legacy_claude_executable_path(keel_home: &Path) -> Option<PathBuf> {
    if !is_standard_keel_home(keel_home) {
        return None;
    }
    Some(claude_engagement_home(keel_home).join(executable_file_name()))
}

pub fn ensure_parent_directory(path: &Path) -> Result<(), String> {
    match path.parent() {
        Some(parent) => fs::create_dir_all(parent)
            .map_err(|error| format!("create {}: {error}", display_path(parent))),
        None => Ok(()),
    }
}

pub fn remove_path_if_exists(path: &Path) -> Result<(), String> {
    if path.is_dir() {
        fs::remove_dir_all(path).map_err(|error| format!("remove {}: {error}", display_path(path)))
    } else if path.is_file() {
        fs::remove_file(path).map_err(|error| format!("remove {}: {error}", display_path(path)))
    } else {
        Ok(())
    }
}

pub fn read_text_if_exists(path: &Path) -> Result<String, String> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(error) => Err(format!("read {}: {error}", display_path(path))),
    }
}

/// Atomically write `text` to `path`.
///
/// Writes to a sibling temp file, flushes it, then renames it over the target.
/// A reader either sees the complete old file or the complete new file — never a
/// half-written one. This matters because `write_text` backs every managed config
/// write: `~/.claude.json` (MCP registration, which also holds the user's auth and
/// project history), `~/.claude/settings.json` (hooks), `~/.claude/CLAUDE.md`,
/// `config.toml`, agent-profile TOML, and the install inventories. A bare
/// `fs::write` truncates first and writes second, so a crash, power loss, or a
/// concurrent reader landing mid-write would leave a torn or empty file — and an
/// empty `~/.claude.json` is a destructive loss of unrelated state.
///
/// The rename retries a few times to absorb the transient locks that frequently
/// firing the harness lifecycle hooks can hold on these files (mirrors the
/// executable-replace path's `rename_with_retry`). If staging into the same
/// directory fails (e.g. a permission quirk), it falls back to a direct write so
/// a non-atomic success still beats a hard failure.
pub fn write_text(path: &Path, text: &str) -> Result<(), String> {
    ensure_parent_directory(path)?;
    cleanup_stale_atomic_temps(path);

    let temp_path = atomic_temp_path(path);
    // Best-effort cleanup of any leftover temp from a previous interrupted write.
    let _ = fs::remove_file(&temp_path);

    let staged = (|| -> std::io::Result<()> {
        let mut file = fs::File::create(&temp_path)?;
        file.write_all(text.as_bytes())?;
        file.flush()?;
        // fsync so the bytes are durable before the rename publishes the file.
        file.sync_all()?;
        Ok(())
    })();

    if let Err(error) = staged {
        let _ = fs::remove_file(&temp_path);
        // Fall back to a direct write rather than failing outright: a non-atomic
        // write still leaves the file in the intended final state on success.
        return fs::write(path, text).map_err(|fallback| {
            format!(
                "write {}: {error} (fallback: {fallback})",
                display_path(path)
            )
        });
    }

    match rename_text_with_retry(&temp_path, path) {
        Ok(()) => Ok(()),
        Err(rename_error) => {
            // Rename failed (e.g. a stubborn lock on the target). Drop the temp
            // and fall back to a direct write so the install still completes.
            let direct = fs::write(path, text);
            let _ = fs::remove_file(&temp_path);
            direct.map_err(|error| {
                format!(
                    "write {}: rename failed ({rename_error}); direct write also failed: {error}",
                    display_path(path)
                )
            })
        }
    }
}

fn cleanup_stale_atomic_temps(target: &Path) {
    let Some(parent) = target.parent() else {
        return;
    };
    let Some(target_name) = target.file_name().and_then(|name| name.to_str()) else {
        return;
    };
    let prefix = format!("{target_name}.tmp-");
    let Ok(entries) = fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let Some(file_name) = file_name.to_str() else {
            continue;
        };
        let Some(suffix) = file_name.strip_prefix(&prefix) else {
            continue;
        };
        let Some((process_id, nonce)) = suffix.split_once('-') else {
            continue;
        };
        if nonce.is_empty() {
            continue;
        }
        let Ok(process_id) = process_id.parse::<u32>() else {
            continue;
        };
        if process_is_alive(process_id) == Some(false) {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// Sibling temp path for an atomic write. Kept in the same directory as the
/// target so the final step is a same-filesystem rename (atomic), never a
/// cross-device copy. A pid + nanosecond suffix avoids collisions between
/// concurrent writers staging the same target.
fn atomic_temp_path(target: &Path) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let mut name = target.file_name().map(|n| n.to_owned()).unwrap_or_default();
    name.push(format!(".tmp-{}-{nanos}", std::process::id()));
    target.with_file_name(name)
}

/// Rename `from` over `to`, retrying a few times to absorb transient locks from
/// concurrently-firing the harness hooks. `fs::rename` replaces an existing
/// target on both Unix (atomic inode swap) and Windows
/// (`MoveFileExW(MOVEFILE_REPLACE_EXISTING)`).
fn rename_text_with_retry(from: &Path, to: &Path) -> Result<(), String> {
    let mut last_error = None;
    for attempt in 0..5 {
        match fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(error) => {
                last_error = Some(error);
                if attempt < 4 {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
            }
        }
    }
    Err(last_error
        .map(|error| {
            format!(
                "rename {} -> {}: {error}",
                display_path(from),
                display_path(to)
            )
        })
        .unwrap_or_else(|| "rename failed".to_string()))
}

pub fn write_lines(path: &Path, lines: &[String]) -> Result<(), String> {
    let mut payload = String::new();
    for line in lines {
        payload.push_str(line);
        payload.push('\n');
    }
    write_text(path, &payload)
}

pub fn run_command(
    program: &str,
    arguments: &[String],
    working_directory: Option<&Path>,
) -> Result<ProcessResult, String> {
    run_command_with_timeout(program, arguments, working_directory, command_timeout())
}

/// Execute a child with bounded capture and complete process-tree cleanup.
/// Both stdout and stderr are drained concurrently so descendants cannot block
/// on full pipes. On timeout the whole tree is terminated before readers join.
pub fn run_command_with_timeout(
    program: &str,
    arguments: &[String],
    working_directory: Option<&Path>,
    timeout: std::time::Duration,
) -> Result<ProcessResult, String> {
    let mut command = Command::new(program);
    command.args(arguments);
    if let Some(directory) = working_directory {
        command.current_dir(directory);
    }
    command.stdin(Stdio::null());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    configure_process_group(&mut command);
    let mut child = command
        .spawn()
        .map_err(|error| format!("execute {program}: {error}"))?;
    let mut process_guard = match own_process_tree(&mut child) {
        Ok(guard) => guard,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("execute {program}: process ownership: {error}"));
        }
    };
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| format!("execute {program}: stdout pipe unavailable"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| format!("execute {program}: stderr pipe unavailable"))?;
    let stdout_thread = std::thread::spawn(|| capture_stream(stdout));
    let stderr_thread = std::thread::spawn(|| capture_stream(stderr));
    let deadline = std::time::Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            Ok(None) => {
                let kill_error = terminate_owned_process_tree(&mut child, &mut process_guard).err();
                let _ = child.wait(); // intentional cleanup after tree termination
                let _ = stdout_thread.join(); // intentional drain-thread cleanup
                let _ = stderr_thread.join(); // intentional drain-thread cleanup
                let suffix = kill_error
                    .map(|error| format!("; process-tree cleanup failed: {error}"))
                    .unwrap_or_default();
                return Err(format!(
                    "execute {program}: timed out after {}s{suffix}; retry with a smaller command or increase KEEL_COMMAND_TIMEOUT_SECS",
                    timeout.as_secs()
                ));
            }
            Err(error) => {
                let _ = terminate_owned_process_tree(&mut child, &mut process_guard);
                let _ = child.wait(); // intentional cleanup after tree termination
                let _ = stdout_thread.join(); // intentional drain-thread cleanup
                let _ = stderr_thread.join(); // intentional drain-thread cleanup
                return Err(format!("execute {program}: wait failed: {error}"));
            }
        }
    };
    let (stdout, original_stdout_bytes) = stdout_thread
        .join()
        .map_err(|_| format!("execute {program}: stdout reader panicked"))?;
    let (stderr, original_stderr_bytes) = stderr_thread
        .join()
        .map_err(|_| format!("execute {program}: stderr reader panicked"))?;
    Ok(ProcessResult {
        code: exit_status_code(&status),
        stdout,
        stderr,
        original_stdout_bytes,
        original_stderr_bytes,
    })
}

/// Run a command with inherited streams while still enforcing the same timeout.
pub fn run_command_inherit(
    program: &str,
    arguments: &[String],
    working_directory: Option<&Path>,
) -> Result<i32, String> {
    let mut command = Command::new(program);
    command.args(arguments);
    if let Some(directory) = working_directory {
        command.current_dir(directory);
    }
    command.stdin(Stdio::inherit());
    command.stdout(Stdio::inherit());
    command.stderr(Stdio::inherit());
    configure_process_group(&mut command);
    let mut child = command
        .spawn()
        .map_err(|error| format!("execute {program}: {error}"))?;
    let mut process_guard = match own_process_tree(&mut child) {
        Ok(guard) => guard,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("execute {program}: process ownership: {error}"));
        }
    };
    let deadline = std::time::Instant::now() + command_timeout();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(exit_status_code(&status)),
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            Ok(None) => {
                let kill_error = terminate_owned_process_tree(&mut child, &mut process_guard).err();
                let _ = child.wait(); // intentional cleanup after inherited-stream timeout
                let suffix = kill_error
                    .map(|error| format!("; process-tree cleanup failed: {error}"))
                    .unwrap_or_default();
                return Err(format!(
                    "execute {program}: timed out after {}s{suffix}; increase KEEL_COMMAND_TIMEOUT_SECS",
                    command_timeout().as_secs()
                ));
            }
            Err(error) => {
                let _ = terminate_owned_process_tree(&mut child, &mut process_guard);
                let _ = child.wait(); // intentional cleanup after tree termination
                return Err(format!("execute {program}: wait failed: {error}"));
            }
        }
    }
}

fn command_timeout() -> std::time::Duration {
    let seconds = env::var("KEEL_COMMAND_TIMEOUT_SECS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_COMMAND_TIMEOUT_SECS)
        .clamp(300, 3_600);
    std::time::Duration::from_secs(seconds)
}

fn capture_stream<R: Read>(mut reader: R) -> (Vec<u8>, usize) {
    let mut kept = Vec::new();
    let mut buffer = [0u8; 8192];
    let mut original = 0usize;
    loop {
        match reader.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(count) => {
                original = original.saturating_add(count);
                if kept.len() < MAX_CAPTURED_OUTPUT_BYTES {
                    let room = MAX_CAPTURED_OUTPUT_BYTES - kept.len();
                    kept.extend_from_slice(&buffer[..count.min(room)]);
                }
            }
        }
    }
    let (kept, _) = cap_captured_stream(kept);
    (kept, original)
}

fn configure_process_group(command: &mut Command) {
    #[cfg(not(unix))]
    let _ = command; // process groups are configured only on Unix
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            command.pre_exec(|| {
                if libc_setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
}

pub struct ChildProcessGuard {
    #[cfg(windows)]
    job_handle: isize,
}

impl Drop for ChildProcessGuard {
    fn drop(&mut self) {
        #[cfg(windows)]
        {
            let _ = windows_process::close_handle(&mut self.job_handle);
        }
    }
}

pub fn terminate_owned_process_tree(
    child: &mut Child,
    process_guard: &mut ChildProcessGuard,
) -> Result<(), String> {
    #[cfg(windows)]
    {
        match windows_process::close_handle(&mut process_guard.job_handle) {
            Ok(()) => Ok(()),
            Err(job_error) => terminate_process_tree(child).map_err(|fallback_error| {
                format!("{job_error}; fallback process-tree cleanup failed: {fallback_error}")
            }),
        }
    }
    #[cfg(unix)]
    {
        let _ = process_guard;
        terminate_process_tree(child)
    }
}

pub fn own_process_tree(child: &mut Child) -> Result<ChildProcessGuard, String> {
    #[cfg(windows)]
    {
        let job_handle = windows_process::create_kill_on_close_job(child.id())?;
        Ok(ChildProcessGuard { job_handle })
    }
    #[cfg(not(windows))]
    {
        let _ = child;
        Ok(ChildProcessGuard {})
    }
}

pub fn process_is_alive(process_id: u32) -> Option<bool> {
    #[cfg(windows)]
    {
        windows_process::process_is_alive(process_id)
    }
    #[cfg(unix)]
    {
        extern "C" {
            fn kill(process_id: i32, signal: i32) -> i32;
        }
        if process_id == 0 || process_id > i32::MAX as u32 {
            return Some(false);
        }
        Some(unsafe { kill(process_id as i32, 0) == 0 })
    }
}

#[cfg(windows)]
pub fn parent_process_id(process_id: u32) -> Option<u32> {
    windows_process::parent_process_id(process_id)
}

#[cfg(windows)]
mod windows_process {
    use std::ffi::c_void;
    use std::mem::size_of;
    use std::ptr;

    type Handle = *mut c_void;
    const TH32CS_SNAPPROCESS: u32 = 0x0000_0002;
    const PROCESS_TERMINATE: u32 = 0x0001;
    const PROCESS_SET_QUOTA: u32 = 0x0100;
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x0000_2000;
    const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION_CLASS: i32 = 9;
    const STILL_ACTIVE: u32 = 259;
    const ERROR_ACCESS_DENIED: u32 = 5;

    #[repr(C)]
    struct ProcessEntry32W {
        size: u32,
        usage_count: u32,
        process_id: u32,
        default_heap_id: usize,
        module_id: u32,
        thread_count: u32,
        parent_process_id: u32,
        base_priority: i32,
        flags: u32,
        executable_file: [u16; 260],
    }

    #[repr(C)]
    #[derive(Default)]
    struct JobObjectBasicLimitInformation {
        per_process_user_time_limit: i64,
        per_job_user_time_limit: i64,
        limit_flags: u32,
        minimum_working_set_size: usize,
        maximum_working_set_size: usize,
        active_process_limit: u32,
        affinity: usize,
        priority_class: u32,
        scheduling_class: u32,
    }

    #[repr(C)]
    #[derive(Default)]
    struct IoCounters {
        read_operation_count: u64,
        write_operation_count: u64,
        other_operation_count: u64,
        read_transfer_count: u64,
        write_transfer_count: u64,
        other_transfer_count: u64,
    }

    #[repr(C)]
    #[derive(Default)]
    struct JobObjectExtendedLimitInformation {
        basic_limit_information: JobObjectBasicLimitInformation,
        io_info: IoCounters,
        process_memory_limit: usize,
        job_memory_limit: usize,
        peak_process_memory_used: usize,
        peak_job_memory_used: usize,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateToolhelp32Snapshot(flags: u32, process_id: u32) -> Handle;
        fn Process32FirstW(snapshot: Handle, entry: *mut ProcessEntry32W) -> i32;
        fn Process32NextW(snapshot: Handle, entry: *mut ProcessEntry32W) -> i32;
        fn CreateJobObjectW(attributes: *mut c_void, name: *const u16) -> Handle;
        fn SetInformationJobObject(
            job: Handle,
            information_class: i32,
            information: *const c_void,
            information_length: u32,
        ) -> i32;
        fn OpenProcess(access: u32, inherit_handle: i32, process_id: u32) -> Handle;
        fn AssignProcessToJobObject(job: Handle, process: Handle) -> i32;
        fn GetExitCodeProcess(process: Handle, exit_code: *mut u32) -> i32;
        fn GetLastError() -> u32;
        fn CloseHandle(handle: Handle) -> i32;
    }

    fn valid_handle(handle: Handle) -> bool {
        !handle.is_null() && handle as isize != -1
    }

    pub(super) fn close_handle(handle: &mut isize) -> Result<(), String> {
        if *handle == 0 || *handle == -1 {
            return Ok(());
        }
        let closing = *handle;
        *handle = 0;
        // SAFETY: `closing` is an owned Job Object handle and is invalidated
        // before this single close so Drop cannot close it twice.
        if unsafe { CloseHandle(closing as Handle) } != 0 {
            Ok(())
        } else {
            Err(format!(
                "close Windows Job Object: {}",
                std::io::Error::last_os_error()
            ))
        }
    }

    pub(super) fn parent_process_id(process_id: u32) -> Option<u32> {
        // SAFETY: the snapshot handle is validated before use and closed below.
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
        if !valid_handle(snapshot) {
            return None;
        }
        // SAFETY: this repr(C) record is initialized to the Win32-required zero
        // state before its size field is populated.
        let mut entry: ProcessEntry32W = unsafe { std::mem::zeroed() };
        entry.size = size_of::<ProcessEntry32W>() as u32;
        let mut found = None;
        // SAFETY: `entry` points to writable storage with the required size.
        let mut has_entry = unsafe { Process32FirstW(snapshot, &mut entry) } != 0;
        while has_entry {
            if entry.process_id == process_id {
                found = Some(entry.parent_process_id);
                break;
            }
            // SAFETY: the validated snapshot and initialized record stay live.
            has_entry = unsafe { Process32NextW(snapshot, &mut entry) } != 0;
        }
        // SAFETY: `snapshot` is an owned valid handle and is closed once.
        unsafe {
            CloseHandle(snapshot);
        }
        found
    }

    pub(super) fn process_is_alive(process_id: u32) -> Option<bool> {
        if process_id == 0 {
            return Some(false);
        }
        // SAFETY: OpenProcess receives a concrete PID and no borrowed pointers.
        let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, process_id) };
        if !valid_handle(process) {
            // SAFETY: GetLastError has no preconditions and reads thread state.
            return (unsafe { GetLastError() } != ERROR_ACCESS_DENIED).then_some(false);
        }
        let mut exit_code = 0u32;
        // SAFETY: `process` is valid and `exit_code` is writable for the call.
        let queried = unsafe { GetExitCodeProcess(process, &mut exit_code) } != 0;
        // SAFETY: `process` is an owned valid handle and is closed once.
        unsafe {
            CloseHandle(process);
        }
        queried.then_some(exit_code == STILL_ACTIVE)
    }

    pub(super) fn create_kill_on_close_job(process_id: u32) -> Result<isize, String> {
        // SAFETY: null security attributes/name request an unnamed default job.
        let job = unsafe { CreateJobObjectW(ptr::null_mut(), ptr::null()) };
        if !valid_handle(job) {
            return Err(format!(
                "create Windows Job Object: {}",
                std::io::Error::last_os_error()
            ));
        }
        let mut limits = JobObjectExtendedLimitInformation::default();
        limits.basic_limit_information.limit_flags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: `limits` has the documented repr(C) layout and byte length.
        let configured = unsafe {
            SetInformationJobObject(
                job,
                JOB_OBJECT_EXTENDED_LIMIT_INFORMATION_CLASS,
                &limits as *const _ as *const c_void,
                size_of::<JobObjectExtendedLimitInformation>() as u32,
            )
        } != 0;
        if !configured {
            let error = std::io::Error::last_os_error();
            // SAFETY: `job` is an owned valid handle and is closed on failure.
            unsafe {
                CloseHandle(job);
            }
            return Err(format!("configure Windows Job Object: {error}"));
        }
        // SAFETY: OpenProcess receives a concrete child PID and no pointers.
        let process = unsafe { OpenProcess(PROCESS_TERMINATE | PROCESS_SET_QUOTA, 0, process_id) };
        if !valid_handle(process) {
            let error = std::io::Error::last_os_error();
            // SAFETY: `job` is an owned valid handle and is closed on failure.
            unsafe {
                CloseHandle(job);
            }
            return Err(format!("open child process {process_id}: {error}"));
        }
        // SAFETY: both handles are valid and remain live through assignment.
        let assigned = unsafe { AssignProcessToJobObject(job, process) } != 0;
        // SAFETY: `process` is an owned valid handle and is closed once.
        unsafe {
            CloseHandle(process);
        }
        if !assigned {
            let error = std::io::Error::last_os_error();
            // SAFETY: `job` is an owned valid handle and is closed on failure.
            unsafe {
                CloseHandle(job);
            }
            return Err(format!("assign child {process_id} to Job Object: {error}"));
        }
        Ok(job as isize)
    }
}

#[cfg(windows)]
pub fn terminate_process_tree(child: &mut Child) -> Result<(), String> {
    let pid = child.id().to_string();
    let status = Command::new("taskkill")
        .args(["/PID", &pid, "/T", "/F"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .status()
        .map_err(|error| format!("taskkill process tree: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("taskkill exited with {status}"))
    }
}

#[cfg(unix)]
pub fn terminate_process_tree(child: &mut Child) -> Result<(), String> {
    let pid = child.id() as i32;
    let result = libc_kill_process_group(pid);
    if result == 0 {
        Ok(())
    } else {
        Err(format!(
            "kill process group {pid}: {}",
            std::io::Error::last_os_error()
        ))
    }
}

#[cfg(unix)]
fn libc_setsid() -> i32 {
    extern "C" {
        fn setsid() -> i32;
    }
    unsafe { setsid() }
}

#[cfg(unix)]
fn libc_kill_process_group(pid: i32) -> i32 {
    extern "C" {
        fn kill(pid: i32, signal: i32) -> i32;
    }
    unsafe { kill(-pid, 9) }
}

/// Exit code for a finished child, using the shell's `128 + signal` convention.
/// why: `code()` is `None` for a signalled child, and reporting `1` made a
/// SIGKILLed command indistinguishable from ordinary failure.
fn exit_status_code(status: &std::process::ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return 128 + signal;
        }
    }
    1
}

/// Wrap a single shell command string into the (program, args) pair appropriate
/// for the current platform: PowerShell (`pwsh`/`powershell -Command`) on
/// Windows, `bash -lc "<command>"` everywhere else. Used by call sites that need
/// to delegate a composite shell expression (with pipes, redirects, env-var
/// assignments, or other shell metacharacters) to the host shell rather than
/// executing one program directly.
///
/// why: the previous `cmd /C` shape could not run PowerShell cmdlets
/// (`Get-Content`, `Select-Object`) and mangled quoted `findstr`/`rg` patterns,
/// so agents on Windows got "not recognized" for valid commands. PowerShell
/// still runs cmd builtins (`more`, `findstr`, `dir`) via native lookup/aliases,
/// so routing through it does not regress the cmd-native cases. `pwsh`
/// (PowerShell 7) is preferred over Windows PowerShell 5.1 (`powershell`) for
/// its saner quoting and `-Command` semantics; `cmd /C` is the last resort only
/// when neither is on PATH. For an EXPLICIT shell choice (no guessing at all),
/// use [`named_shell_command_parts`] instead.
pub fn platform_shell_command_parts(command: &str) -> (String, Vec<String>) {
    if cfg!(windows) {
        if let Some(shell) = powershell_executable() {
            return (
                shell,
                vec![
                    "-NoProfile".to_string(),
                    "-Command".to_string(),
                    command.to_string(),
                ],
            );
        }
        (
            "cmd".to_string(),
            vec!["/C".to_string(), command.to_string()],
        )
    } else {
        (
            "bash".to_string(),
            vec!["-lc".to_string(), command.to_string()],
        )
    }
}

/// Build the (program, args) pair that runs `script` through the EXPLICITLY
/// named shell: `powershell` (pwsh or Windows PowerShell 5.1), `cmd`
/// (Windows-only), or `bash`. Unlike [`platform_shell_command_parts`] this never
/// guesses and never substitutes a different shell — a named shell that cannot
/// be resolved is an error the caller reports to the agent. This is the shape
/// the MCP `run_command` `script` + `shell` form uses, so an agent that asks for
/// PowerShell gets PowerShell and an agent that asks for cmd gets cmd.
pub fn named_shell_command_parts(
    shell: &str,
    script: &str,
) -> Result<(String, Vec<String>), String> {
    match shell {
        "powershell" => {
            let executable = powershell_executable().ok_or_else(|| {
                "shell 'powershell' was requested but neither pwsh nor powershell is on PATH"
                    .to_string()
            })?;
            Ok((
                executable,
                vec![
                    "-NoProfile".to_string(),
                    "-Command".to_string(),
                    script.to_string(),
                ],
            ))
        }
        "cmd" => {
            if cfg!(windows) {
                Ok((
                    "cmd".to_string(),
                    vec!["/C".to_string(), script.to_string()],
                ))
            } else {
                Err("shell 'cmd' was requested but cmd.exe only exists on Windows".to_string())
            }
        }
        "bash" => Ok((
            "bash".to_string(),
            vec!["-lc".to_string(), script.to_string()],
        )),
        other => Err(format!(
            "unknown shell {other:?}: expected one of powershell, cmd, bash"
        )),
    }
}

/// First PowerShell executable on PATH, preferring `pwsh` (7+) over
/// `powershell` (5.1); `None` when neither resolves. Cached process-wide: a
/// long-lived `mcp serve` daemon resolves once instead of re-probing per tool
/// call. Works on every platform where pwsh is installed (Linux/macOS
/// included), not just Windows.
pub fn powershell_executable() -> Option<String> {
    use std::sync::LazyLock;
    static CACHED: LazyLock<Option<String>> = LazyLock::new(|| {
        ["pwsh", "powershell"]
            .iter()
            .find(|name| which::which(name).is_ok())
            .map(|name| (*name).to_string())
    });
    CACHED.clone()
}

pub fn forward_process_result(
    result: &ProcessResult,
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) {
    let _ = standard_output.write_all(&result.stdout);
    let _ = standard_error.write_all(&result.stderr);
}

pub fn git_short_head(repository_root: &Path) -> String {
    let arguments = vec![
        "-C".to_string(),
        display_path(repository_root),
        "rev-parse".to_string(),
        "--short".to_string(),
        "HEAD".to_string(),
    ];
    match run_command("git", &arguments, None) {
        Ok(result) if result.code == 0 => {
            String::from_utf8_lossy(&result.stdout).trim().to_string()
        }
        _ => "unknown".to_string(),
    }
}

#[cfg(test)]
mod absolute_any_platform_tests {
    use super::is_absolute_any_platform;

    #[test]
    fn windows_drive_paths_count_as_absolute_on_every_platform() {
        // why: Path::is_absolute is host-specific, so a drive path reads as
        // relative on Unix and would get the cwd prepended, splitting the lane.
        assert!(is_absolute_any_platform("D:/Nasri/Project/keel"));
        assert!(is_absolute_any_platform("C:\\Users\\HP"));
        assert!(is_absolute_any_platform("d:relative"));
    }

    #[test]
    fn unix_and_unc_roots_count_and_relative_does_not() {
        assert!(is_absolute_any_platform("/home/user/repo"));
        assert!(is_absolute_any_platform("\\\\server\\share"));
        assert!(!is_absolute_any_platform("relative/path"));
        assert!(!is_absolute_any_platform("./dot"));
        assert!(!is_absolute_any_platform("plain"));
    }
}

#[cfg(test)]
mod safe_path_segment_tests {
    use super::safe_path_segment;

    #[test]
    fn accepts_ordinary_names_and_trims() {
        assert_eq!(safe_path_segment("reviewer").as_deref(), Some("reviewer"));
        assert_eq!(
            safe_path_segment("  systematic-debugging  ").as_deref(),
            Some("systematic-debugging")
        );
        assert_eq!(safe_path_segment("wb-1a2b3c").as_deref(), Some("wb-1a2b3c"));
        assert_eq!(safe_path_segment("file.json").as_deref(), Some("file.json"));
    }

    #[test]
    fn rejects_empty_and_whitespace() {
        assert_eq!(safe_path_segment(""), None);
        assert_eq!(safe_path_segment("   "), None);
    }

    #[test]
    fn rejects_separators_and_traversal() {
        assert_eq!(safe_path_segment("a/b"), None);
        assert_eq!(safe_path_segment("a\\b"), None);
        assert_eq!(safe_path_segment(".."), None);
        assert_eq!(safe_path_segment("../evil"), None);
        assert_eq!(safe_path_segment("../../etc/passwd"), None);
        assert_eq!(safe_path_segment("."), None);
        assert_eq!(safe_path_segment("/abs"), None);
    }

    #[test]
    fn rejects_windows_drive_relative_and_unc_prefixes() {
        // The regression the substring guard missed: `C:foo` is not absolute
        // per Path::is_absolute on Windows, yet PathBuf::join would discard the
        // base and resolve against the drive's cwd. The Components-based check
        // rejects any Prefix component on every platform.
        assert_eq!(safe_path_segment("C:foo"), None);
        assert_eq!(safe_path_segment("C:"), None);
        assert_eq!(safe_path_segment("C:\\Windows\\System32"), None);
        assert_eq!(safe_path_segment("\\\\server\\share"), None);
    }
}

#[cfg(test)]
mod write_text_tests {
    use super::*;

    fn unique_dir(label: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let dir = std::env::temp_dir().join(format!(
            "keel-writetext-{label}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).expect("create test dir");
        dir
    }

    fn temp_siblings(dir: &Path, stem: &str) -> Vec<String> {
        fs::read_dir(dir)
            .expect("read dir")
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|name| name.starts_with(stem) && name.contains(".tmp-"))
            .collect()
    }

    #[test]
    fn writes_content_and_creates_parent() {
        let dir = unique_dir("create");
        // Nested path: write_text must create the missing parent directory.
        let target = dir.join("nested").join("config.toml");
        write_text(&target, "hello = 1\n").expect("write");
        assert_eq!(fs::read_to_string(&target).unwrap(), "hello = 1\n");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn overwrite_replaces_content_atomically() {
        let dir = unique_dir("overwrite");
        let target = dir.join("claude.json");
        write_text(&target, "{\"a\":1}").expect("first write");
        write_text(&target, "{\"b\":2}").expect("second write");
        // New content fully replaces old — no torn merge.
        assert_eq!(fs::read_to_string(&target).unwrap(), "{\"b\":2}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn leaves_no_temp_file_behind() {
        let dir = unique_dir("notmp");
        let target = dir.join("settings.json");
        write_text(&target, "x").expect("write");
        // The staged temp must be renamed away, not left as litter beside the target.
        assert!(
            temp_siblings(&dir, "settings.json").is_empty(),
            "atomic temp file leaked: {:?}",
            temp_siblings(&dir, "settings.json")
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn repeated_writes_are_stable() {
        // Simulate the install path rewriting the same managed file many times.
        let dir = unique_dir("repeat");
        let target = dir.join("CLAUDE.md");
        for i in 0..20 {
            write_text(&target, &format!("iteration {i}\n")).expect("write");
        }
        assert_eq!(fs::read_to_string(&target).unwrap(), "iteration 19\n");
        assert!(temp_siblings(&dir, "CLAUDE.md").is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_text_removes_only_stale_temps_for_its_target() {
        let root = std::env::temp_dir().join(format!(
            "keel-write-stale-temp-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or_default()
        ));
        std::fs::create_dir_all(&root).expect("root");
        let target = root.join("settings.json");
        let stale = root.join("settings.json.tmp-999999-1");
        let unrelated = root.join("other.json.tmp-999999-1");
        std::fs::write(&stale, "stale").expect("stale");
        std::fs::write(&unrelated, "keep").expect("unrelated");

        write_text(&target, "current").expect("write target");

        assert!(!stale.exists(), "stale sibling temp must be recovered");
        assert!(
            unrelated.exists(),
            "another target's temp must be preserved"
        );
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "current");
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod cap_captured_stream_tests {
    use super::{cap_captured_stream, MAX_CAPTURED_OUTPUT_BYTES};

    #[test]
    fn small_output_is_returned_unchanged() {
        let bytes = b"hello world".to_vec();
        let (capped, original) = cap_captured_stream(bytes.clone());
        assert_eq!(capped, bytes);
        assert_eq!(original, bytes.len());
    }

    #[test]
    fn output_at_the_cap_is_returned_unchanged() {
        let bytes = vec![b'x'; MAX_CAPTURED_OUTPUT_BYTES];
        let (capped, original) = cap_captured_stream(bytes.clone());
        assert_eq!(capped.len(), MAX_CAPTURED_OUTPUT_BYTES);
        assert_eq!(original, MAX_CAPTURED_OUTPUT_BYTES);
    }

    #[test]
    fn output_over_the_cap_is_truncated_with_marker_and_honest_count() {
        // 10 MiB over the cap: the capped buffer must be <= the cap, carry the
        // truncation marker, and the original count must reflect the true size.
        let over = MAX_CAPTURED_OUTPUT_BYTES + (10 * 1024 * 1024);
        let bytes = vec![b'x'; over];
        let (capped, original) = cap_captured_stream(bytes);
        assert_eq!(
            original, over,
            "original count must be the true pre-cap size"
        );
        assert!(
            capped.len() <= MAX_CAPTURED_OUTPUT_BYTES,
            "capped buffer must not exceed the cap: {}",
            capped.len()
        );
        assert!(
            capped.windows(b"[keel]".len()).any(|w| w == b"[keel]"),
            "capped buffer must carry the truncation marker"
        );
    }
}

#[cfg(test)]
mod keel_home_split_tests {
    use super::*;

    #[test]
    fn engagement_home_splits_to_sibling_dot_claude_for_keel_root() {
        let keel_home = PathBuf::from("/home/user/.keel");
        let engagement = claude_engagement_home(&keel_home);
        assert_eq!(engagement, PathBuf::from("/home/user/.claude"));
    }

    #[test]
    fn engagement_home_is_the_root_itself_for_non_standard_roots() {
        // Test temp dirs and legacy --claude-home overrides keep the
        // single-root behavior so hermetic tests stay hermetic.
        let custom = PathBuf::from("/tmp/keel-test-root");
        assert_eq!(claude_engagement_home(&custom), custom);
        let legacy = PathBuf::from("/home/user/.claude");
        assert_eq!(claude_engagement_home(&legacy), legacy);
    }

    #[test]
    fn configured_custom_keel_home_keeps_claude_engagement_under_user_home() {
        let user_home = PathBuf::from("/home/user");
        let custom_keel_home = PathBuf::from("/mnt/keel-data");
        assert_eq!(
            claude_engagement_home_for(
                &custom_keel_home,
                Some(&user_home),
                Some(&custom_keel_home),
            ),
            user_home.join(".claude")
        );
        assert_eq!(
            claude_engagement_home_for(&custom_keel_home, Some(&user_home), None),
            custom_keel_home,
            "an explicit legacy override remains a hermetic single root"
        );
    }

    #[test]
    fn is_standard_keel_home_checks_basename_only() {
        assert!(is_standard_keel_home(&PathBuf::from("/home/user/.keel")));
        assert!(is_standard_keel_home(&PathBuf::from("/tmp/fixture/.keel")));
        assert!(!is_standard_keel_home(&PathBuf::from("/home/user/.claude")));
        assert!(!is_standard_keel_home(&PathBuf::from("/home/user/keel")));
    }

    #[test]
    fn keel_home_from_engagement_prefers_existing_sibling_keel() {
        let dir = std::env::temp_dir().join(format!("keel-home-split-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let claude_dir = dir.join(".claude");
        let keel_dir = dir.join(".keel");
        std::fs::create_dir_all(&claude_dir).unwrap();

        // Before migration: no ~/.keel sibling, so the engagement home is the home.
        assert_eq!(keel_home_from_engagement(&claude_dir), claude_dir);

        // After migration: the sibling .keel wins.
        std::fs::create_dir_all(&keel_dir).unwrap();
        assert_eq!(keel_home_from_engagement(&claude_dir), keel_dir);

        // Non-standard engagement homes map to themselves.
        assert_eq!(keel_home_from_engagement(&dir), dir);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn legacy_claude_executable_path_only_for_standard_layout() {
        let keel_home = PathBuf::from("/home/user/.keel");
        let legacy = legacy_claude_executable_path(&keel_home).unwrap();
        assert_eq!(
            legacy,
            PathBuf::from("/home/user/.claude").join(executable_file_name())
        );
        assert!(
            legacy_claude_executable_path(&PathBuf::from("/tmp/custom")).is_none(),
            "non-standard roots have no legacy placement"
        );
    }

    #[test]
    fn engagement_artifacts_resolve_under_dot_claude_for_keel_root() {
        let keel_home = PathBuf::from("/home/user/.keel");
        assert_eq!(
            skills_directory(&keel_home),
            PathBuf::from("/home/user/.claude/skills")
        );
        assert_eq!(
            agents_directory(&keel_home),
            PathBuf::from("/home/user/.claude/agents")
        );
        assert_eq!(
            commands_directory(&keel_home),
            PathBuf::from("/home/user/.claude/commands")
        );
        // The binary stays in the neutral home.
        assert_eq!(
            installed_executable_path(&keel_home),
            keel_home.join(executable_file_name())
        );
        assert_eq!(state_directory(&keel_home), keel_home.join("state"));
        assert_eq!(update_cache_directory(&keel_home), keel_home.join("cache"));
        assert_ne!(
            state_directory(&keel_home),
            PathBuf::from("/home/user/.claude/state"),
            "keel inventories must not live under the engagement home"
        );
    }

    #[test]
    fn resolve_keel_home_explicit_flag_wins() {
        let custom = PathBuf::from("/tmp/my-keel-home");
        assert_eq!(resolve_keel_home("/tmp/my-keel-home").unwrap(), custom);
    }

    #[test]
    fn resolve_keel_home_default_is_dot_keel_under_user_home() {
        // KEEL_HOME/CLAUDE_TARGET_OVERRIDE may be set by other tests; assert
        // only the invariant that holds in every case: an absolute home.
        let home = resolve_keel_home("").unwrap();
        assert!(home.is_absolute(), "keel home must be absolute: {home:?}");
    }
}
#[cfg(test)]
mod process_lifecycle_tests {
    use super::*;

    #[test]
    fn timed_command_terminates_hanging_process() {
        let (program, arguments) = if cfg!(windows) {
            (
                "cmd",
                vec!["/C".to_string(), "ping -n 30 127.0.0.1 >nul".to_string()],
            )
        } else {
            ("sh", vec!["-c".to_string(), "sleep 30".to_string()])
        };
        let error = run_command_with_timeout(
            program,
            &arguments,
            None,
            std::time::Duration::from_millis(250),
        )
        .expect_err("hanging process must be terminated");
        assert!(error.contains("timed out"), "unexpected error: {error}");
    }

    #[cfg(windows)]
    #[test]
    fn closing_owned_job_terminates_child_without_taskkill() {
        let mut child = Command::new("cmd")
            .args(["/C", "ping -n 30 127.0.0.1 >nul"])
            .spawn()
            .expect("spawn child");
        let mut process_guard = own_process_tree(&mut child).expect("own child tree");

        terminate_owned_process_tree(&mut child, &mut process_guard).expect("close job");

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            match child.try_wait().expect("poll child") {
                Some(_) => break,
                None if std::time::Instant::now() < deadline => {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                None => {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("closing the Job Object did not terminate its child");
                }
            }
        }
    }

    #[cfg(windows)]
    #[test]
    fn native_process_probes_find_the_current_process_and_parent() {
        assert_eq!(process_is_alive(std::process::id()), Some(true));
        let parent = parent_process_id(std::process::id()).expect("parent process");
        assert_ne!(parent, 0);
    }

    #[test]
    fn short_command_returns_output_and_status() {
        let (program, arguments) = if cfg!(windows) {
            ("cmd", vec!["/C".to_string(), "echo ok".to_string()])
        } else {
            ("printf", vec!["ok".to_string()])
        };
        let result =
            run_command_with_timeout(program, &arguments, None, std::time::Duration::from_secs(5))
                .expect("short command");
        assert_eq!(result.code, 0);
        assert!(String::from_utf8_lossy(&result.stdout).contains("ok"));
    }
}
#[cfg(test)]
mod platform_shell_execution_tests {
    use super::*;

    #[test]
    fn platform_shell_executes_powershell_script_on_windows() {
        if !cfg!(windows) {
            return;
        }
        let (program, arguments) = platform_shell_command_parts("Write-Output 'keel-shell-ok'");
        let result = run_command_with_timeout(
            &program,
            &arguments,
            None,
            std::time::Duration::from_secs(5),
        )
        .expect("platform shell");
        assert_eq!(result.code, 0);
        assert!(String::from_utf8_lossy(&result.stdout).contains("keel-shell-ok"));
    }
}
