//! Purpose: Shared Rust-native filesystem, repository layout, and process helpers for the keel CLI.
//! Caller: manager, review, runner, and utility command modules.
//! Dependencies: std::env, std::fs, std::io, std::path, and std::process.
//! Main Functions: discover_repository_layout, resolve_repository_root, resolve_claude_home, run_command.
//! Side Effects: Reads repository files, copies managed assets, creates directories, removes managed paths, and runs child processes when requested.

use std::env;
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

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

pub fn resolve_claude_home(requested_claude_home: &str) -> Result<PathBuf, String> {
    let trimmed = requested_claude_home.trim();
    if !trimmed.is_empty() {
        return Ok(clean_path(&PathBuf::from(trimmed)));
    }
    if let Ok(override_value) = env::var("CLAUDE_TARGET_OVERRIDE") {
        let trimmed_override = override_value.trim();
        if !trimmed_override.is_empty() {
            return Ok(clean_path(&PathBuf::from(trimmed_override)));
        }
    }
    let home = env::var("HOME")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            env::var("USERPROFILE")
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
        .ok_or_else(|| "no user home directory available".to_string())?;
    Ok(clean_path(&PathBuf::from(home).join(".claude")))
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

pub fn skills_directory(claude_home: &Path) -> PathBuf {
    claude_home.join("skills")
}

pub fn agents_directory(claude_home: &Path) -> PathBuf {
    claude_home.join("agents")
}

pub fn commands_directory(claude_home: &Path) -> PathBuf {
    claude_home.join("commands")
}

pub fn agent_profiles_directory(claude_home: &Path) -> PathBuf {
    claude_home.join("agent-profiles")
}

pub fn state_directory(claude_home: &Path) -> PathBuf {
    claude_home.join(".claude-skill-manager")
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

pub fn installed_executable_path(claude_home: &Path) -> PathBuf {
    claude_home.join(executable_file_name())
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
/// firing Claude Code lifecycle hooks can hold on these files (mirrors the
/// executable-replace path's `rename_with_retry`). If staging into the same
/// directory fails (e.g. a permission quirk), it falls back to a direct write so
/// a non-atomic success still beats a hard failure.
pub fn write_text(path: &Path, text: &str) -> Result<(), String> {
    ensure_parent_directory(path)?;

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
/// concurrently-firing Claude Code hooks. `fs::rename` replaces an existing
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
    let mut command = Command::new(program);
    command.args(arguments);
    if let Some(directory) = working_directory {
        command.current_dir(directory);
    }
    command.stdin(Stdio::inherit());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    let output = command
        .output()
        .map_err(|error| format!("execute {program}: {error}"))?;
    Ok(ProcessResult {
        code: output.status.code().unwrap_or(1),
        stdout: output.stdout,
        stderr: output.stderr,
    })
}

/// Run a command as the *direct* parent of the child: no pipes, no capture, no
/// rewriting. Stdin/stdout/stderr are inherited so the user sees output exactly
/// as if they had run the program themselves. Used by the proxy passthrough
/// gate: when `keel run -- ...` is invoked from a plain shell (not a
/// Claude Code hook), capturing output and writing recovery artifacts would
/// surprise the user — we behave as a transparent forwarder instead.
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
    let status = command
        .status()
        .map_err(|error| format!("execute {program}: {error}"))?;
    Ok(status.code().unwrap_or(1))
}

/// Wrap a single shell command string into the (program, args) pair appropriate
/// for the current platform: `cmd /C "<command>"` on Windows, `bash -lc
/// "<command>"` everywhere else. Used by call sites that need to delegate a
/// composite shell expression (with pipes, redirects, env-var assignments, or
/// other shell metacharacters) to the host shell rather than executing one
/// program directly.
pub fn platform_shell_command_parts(command: &str) -> (String, Vec<String>) {
    if cfg!(windows) {
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
}
