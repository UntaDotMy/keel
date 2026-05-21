//! Purpose: Shared Rust-native filesystem, repository layout, and process helpers for the claude-skills CLI.
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

#[derive(Debug, Clone)]
pub struct RepositoryLayout {
    pub root_path: PathBuf,
    pub root_files: Vec<String>,
    pub skills: Vec<SkillDefinition>,
    pub agent_names: Vec<String>,
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
            "repository root is missing required claude_skills files: {}",
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
        "claude-skills.exe".to_string()
    } else {
        "claude-skills".to_string()
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

pub fn write_text(path: &Path, text: &str) -> Result<(), String> {
    ensure_parent_directory(path)?;
    fs::write(path, text).map_err(|error| format!("write {}: {error}", display_path(path)))
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
/// gate: when `claude-skills run -- ...` is invoked from a plain shell (not a
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
