//! Purpose: Verify and validate install logic for keel manager.
//! Caller: commands.rs via run_verify_command, run_validate_command, run_all_command, run_menu_command.
//! Dependencies: std::fs, std::io, std::path, crate::args, crate::runtime.
//! Main Functions: run_verify_command, verify_install, compare_skill, compare_directory_subset, compare_file_bytes, verify_installed_markdown_dir, count_installed_skills, count_learned_skills, is_learned_skill_name, count_managed_skills, install_metadata_path, read_inventory_lines, metadata_value, run_validate_command, run_all_command, run_menu_command.
//! Side Effects: Reads installed files and compares against source; runs cargo/git validation commands.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::args::FlagSet;
use crate::runtime::{
    agent_profiles_directory, agents_directory, commands_directory, discover_repository_layout,
    display_path, forward_process_result, installed_executable_path, read_text_if_exists,
    resolve_claude_home, resolve_repository_root, run_command, skills_directory, state_directory,
    SkillDefinition, SKILL_SYNC_DIRECTORIES,
};

pub fn run_verify_command(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flag_set = FlagSet::new("verify");
    flag_set.string_flag("repo-root", "");
    flag_set.string_flag("claude-home", "");
    if let Err(parse_error) = flag_set.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 1;
    }
    if flag_set.positional.len() > 1 {
        let _ = writeln!(
            standard_error,
            "Native Rust verify accepts at most one optional skill name, got {} arguments",
            flag_set.positional.len()
        );
        return 1;
    }
    match verify_install(
        &flag_set,
        flag_set.positional.first().map(String::as_str),
        standard_output,
    ) {
        Ok(()) => 0,
        Err(error) => {
            let _ = writeln!(standard_error, "Native Rust verify failed: {error}");
            1
        }
    }
}

fn verify_install(
    flag_set: &FlagSet,
    requested_skill_name: Option<&str>,
    standard_output: &mut dyn Write,
) -> Result<(), String> {
    let repository_root = resolve_repository_root(flag_set.string_value("repo-root"))?;
    let claude_home = resolve_claude_home(flag_set.string_value("claude-home"))?;
    let layout = discover_repository_layout(&repository_root)?;
    if !skills_directory(&claude_home).is_dir() {
        return Err(format!(
            "the harness skill pack is not installed in harness home: {}",
            display_path(&claude_home)
        ));
    }
    for relative_path_name in &layout.root_files {
        let source_path = repository_root.join(relative_path_name);
        let target_path = claude_home.join(relative_path_name);
        compare_file_bytes(&source_path, &target_path)?;
        let _ = writeln!(
            standard_output,
            "Content verified for {}",
            relative_path_name
        );
    }
    let selected_skills: Vec<&SkillDefinition> = match requested_skill_name {
        Some(skill_name) if !skill_name.trim().is_empty() => layout
            .skills
            .iter()
            .filter(|skill| skill.name == skill_name.trim())
            .collect(),
        _ => layout.skills.iter().collect(),
    };
    if requested_skill_name.is_some() && selected_skills.is_empty() {
        return Err(format!(
            "Skill does not exist in repo: {}",
            requested_skill_name.unwrap_or_default()
        ));
    }
    for skill in selected_skills {
        compare_skill(skill, &claude_home)?;
        let _ = writeln!(
            standard_output,
            "Content verified for skill: {}",
            skill.name
        );
    }
    for shared_directory_name in &layout.shared_resource_directories {
        compare_directory_subset(
            &repository_root.join(shared_directory_name),
            &skills_directory(&claude_home).join(shared_directory_name),
        )?;
        let _ = writeln!(
            standard_output,
            "Content verified for shared resources: {}",
            shared_directory_name
        );
    }
    for agent_name in &layout.agent_names {
        if !agent_profiles_directory(&claude_home)
            .join(format!("{agent_name}.toml"))
            .is_file()
        {
            return Err(format!(
                "Agent profile is not installed in harness home: {agent_name}"
            ));
        }
    }
    // Subagent `.md` definitions and custom slash commands are synced by install
    // (sync_subagent_definitions / sync_commands) but were historically not
    // re-verified. Confirm each source `.md` is byte-identical in harness home so
    // a drifted or partially-synced command/subagent is caught here, not at runtime.
    verify_installed_markdown_dir(
        &repository_root.join(".claude").join("agents"),
        &agents_directory(&claude_home),
        "subagent definition",
        standard_output,
    )?;
    verify_installed_markdown_dir(
        &repository_root.join("commands"),
        &commands_directory(&claude_home),
        "command",
        standard_output,
    )?;
    if !installed_executable_path(&claude_home).is_file() {
        return Err(format!(
            "installed Rust executable is missing {}",
            display_path(&installed_executable_path(&claude_home))
        ));
    }
    let _ = writeln!(standard_output, "All Rust verification checks passed");
    Ok(())
}

fn compare_skill(skill: &SkillDefinition, claude_home: &Path) -> Result<(), String> {
    compare_file_bytes(
        &skill.skill_path.join("SKILL.md"),
        &skills_directory(claude_home)
            .join(&skill.name)
            .join("SKILL.md"),
    )?;
    for relative_directory in SKILL_SYNC_DIRECTORIES {
        compare_directory_subset(
            &skill.skill_path.join(relative_directory),
            &skills_directory(claude_home)
                .join(&skill.name)
                .join(relative_directory),
        )?;
    }
    Ok(())
}

fn compare_directory_subset(
    source_directory: &Path,
    target_directory: &Path,
) -> Result<(), String> {
    if !source_directory.is_dir() {
        return Ok(());
    }
    for entry_result in fs::read_dir(source_directory)
        .map_err(|error| format!("read {}: {error}", display_path(source_directory)))?
    {
        let entry = entry_result.map_err(|error| format!("read directory entry: {error}"))?;
        let source_path = entry.path();
        let target_path = target_directory.join(entry.file_name());
        let file_type = entry.file_type().map_err(|error| {
            format!("read file type for {}: {error}", display_path(&source_path))
        })?;
        if file_type.is_dir() {
            compare_directory_subset(&source_path, &target_path)?;
        } else if file_type.is_file() {
            compare_file_bytes(&source_path, &target_path)?;
        }
    }
    Ok(())
}

fn compare_file_bytes(source_path: &Path, target_path: &Path) -> Result<(), String> {
    let source_bytes = fs::read(source_path)
        .map_err(|error| format!("read source {}: {error}", display_path(source_path)))?;
    let target_bytes = fs::read(target_path)
        .map_err(|error| format!("read target {}: {error}", display_path(target_path)))?;
    if source_bytes != target_bytes {
        return Err(format!(
            "content mismatch for {}",
            display_path(source_path)
        ));
    }
    Ok(())
}

/// Names of managed skills whose installed `SKILL.md` is missing or not
/// byte-identical to the source checkout. Used by `status`/`doctor` so "current"
/// means content-fresh, not merely "same skill count". Caps listed names so a
/// large drift does not flood the status output.
pub fn stale_managed_skill_names(
    layout: &crate::runtime::RepositoryLayout,
    claude_home: &Path,
) -> Vec<String> {
    let mut stale = Vec::new();
    for skill in &layout.skills {
        let source = skill.skill_path.join("SKILL.md");
        let target = skills_directory(claude_home)
            .join(&skill.name)
            .join("SKILL.md");
        match compare_file_bytes(&source, &target) {
            Ok(()) => {}
            Err(_) => stale.push(skill.name.clone()),
        }
    }
    stale.sort();
    stale
}

/// Verify every `*.md` file in `source_dir` is installed byte-identical in
/// `target_dir`. Used for subagent definitions (`.claude/agents/`) and custom
/// slash commands (`commands/`), which install copies `.md` files verbatim. A
/// missing source directory is not an error (the repo may ship neither);
/// `label` names the artifact class in messages.
fn verify_installed_markdown_dir(
    source_dir: &Path,
    target_dir: &Path,
    label: &str,
    standard_output: &mut dyn Write,
) -> Result<(), String> {
    if !source_dir.is_dir() {
        return Ok(());
    }
    for entry_result in fs::read_dir(source_dir)
        .map_err(|error| format!("read {}: {error}", display_path(source_dir)))?
    {
        let entry = entry_result.map_err(|error| format!("read directory entry: {error}"))?;
        let source_path = entry.path();
        if source_path.extension().and_then(|value| value.to_str()) != Some("md") {
            continue;
        }
        let Some(file_name) = source_path.file_name() else {
            continue;
        };
        let target_path = target_dir.join(file_name);
        if !target_path.is_file() {
            return Err(format!(
                "{label} is not installed in harness home: {}",
                display_path(&target_path)
            ));
        }
        compare_file_bytes(&source_path, &target_path)?;
        let _ = writeln!(
            standard_output,
            "Content verified for {label}: {}",
            file_name.to_string_lossy()
        );
    }
    Ok(())
}

/// Name prefix for loop-generated skills written by the learning cycle
/// (`~/.claude/skills/learned-*`). These must never inflate the managed
/// skill-pack sync numerator used by `status` / `doctor`.
const LEARNED_SKILL_PREFIX: &str = "learned-";

/// True when `name` is a learning-loop skill directory (`learned-*`).
pub fn is_learned_skill_name(name: &str) -> bool {
    name.starts_with(LEARNED_SKILL_PREFIX)
}

/// Count of managed installed skills: directories under `skills/` that contain
/// `SKILL.md` and are **not** `learned-*`. Used as the sync numerator so
/// loop-generated skills do not force "refresh recommended".
pub fn count_installed_skills(claude_home: &Path) -> usize {
    count_skill_dirs(claude_home, SkillCountKind::Managed)
}

/// Count of loop-generated `learned-*` skill directories with a `SKILL.md`.
pub fn count_learned_skills(claude_home: &Path) -> usize {
    count_skill_dirs(claude_home, SkillCountKind::Learned)
}

enum SkillCountKind {
    Managed,
    Learned,
}

fn count_skill_dirs(claude_home: &Path, kind: SkillCountKind) -> usize {
    fs::read_dir(skills_directory(claude_home))
        .ok()
        .into_iter()
        .flat_map(|entries| entries.filter_map(Result::ok))
        .filter(|entry| entry.path().join("SKILL.md").is_file())
        .filter(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            match kind {
                SkillCountKind::Managed => !is_learned_skill_name(&name),
                SkillCountKind::Learned => is_learned_skill_name(&name),
            }
        })
        .count()
}

pub fn count_managed_skills(claude_home: &Path) -> Option<usize> {
    let inventory_path = state_directory(claude_home).join("managed-skills.txt");
    if !inventory_path.is_file() {
        return None;
    }
    Some(read_inventory_lines(&inventory_path).len())
}

pub fn install_metadata_path(claude_home: &Path) -> PathBuf {
    state_directory(claude_home).join("install-metadata.txt")
}

pub fn read_inventory_lines(path: &Path) -> Vec<String> {
    read_text_if_exists(path)
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

pub fn metadata_value<'a>(metadata: &'a str, key: &str) -> Option<&'a str> {
    for line in metadata.lines() {
        if let Some((line_key, line_value)) = line.split_once('=') {
            if line_key == key {
                return Some(line_value);
            }
        }
    }
    None
}

pub fn run_validate_command(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flag_set = FlagSet::new("validate");
    flag_set.string_flag("repo-root", "");
    flag_set.string_flag("profile", "full");
    if let Err(parse_error) = flag_set.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 1;
    }
    if !flag_set.positional.is_empty() {
        let _ = writeln!(
            standard_error,
            "Native Rust validate does not accept positional arguments, got {}",
            flag_set.positional.len()
        );
        return 1;
    }
    let repository_root = match resolve_repository_root(flag_set.string_value("repo-root")) {
        Ok(path) => path,
        Err(error) => {
            let _ = writeln!(standard_error, "{error}");
            return 1;
        }
    };
    let profile = flag_set.string_value("profile").trim();
    let mut commands: Vec<(&str, Vec<String>)> = vec![
        (
            "cargo",
            vec![
                "fmt".to_string(),
                "--all".to_string(),
                "--check".to_string(),
            ],
        ),
        ("cargo", vec!["test".to_string(), "--workspace".to_string()]),
    ];
    if profile != "smoke" {
        commands.push(("cargo", vec!["build".to_string(), "--release".to_string()]));
    }
    commands.push(("git", vec!["diff".to_string(), "--check".to_string()]));

    for (program, command_arguments) in commands {
        let _ = writeln!(
            standard_output,
            "[validate] {} {}",
            program,
            command_arguments.join(" ")
        );
        match run_command(program, &command_arguments, Some(&repository_root)) {
            Ok(result) => {
                forward_process_result(&result, standard_output, standard_error);
                if result.code != 0 {
                    let _ = writeln!(standard_error, "Native Rust validate failed in {program}");
                    return result.code.clamp(1, 255) as u8;
                }
            }
            Err(error) => {
                let _ = writeln!(standard_error, "{error}");
                return 1;
            }
        }
    }
    let _ = writeln!(standard_output, "Native Rust validate passed");
    0
}

pub fn run_all_command(
    build_version: &str,
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let validate_code = run_validate_command(arguments, standard_output, standard_error);
    if validate_code != 0 {
        return validate_code;
    }
    let mut flag_set = FlagSet::new("all");
    flag_set.string_flag("repo-root", "");
    flag_set.string_flag("claude-home", "");
    flag_set.string_flag("profile", "full");
    if let Err(parse_error) = flag_set.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 1;
    }
    let mut install_arguments = Vec::new();
    if !flag_set.string_value("repo-root").trim().is_empty() {
        install_arguments.push("--repo-root".to_string());
        install_arguments.push(flag_set.string_value("repo-root").to_string());
    }
    if !flag_set.string_value("claude-home").trim().is_empty() {
        install_arguments.push("--claude-home".to_string());
        install_arguments.push(flag_set.string_value("claude-home").to_string());
    }
    super::run_install_command(
        build_version,
        &install_arguments,
        standard_output,
        standard_error,
    )
}

pub fn run_menu_command(standard_output: &mut dyn Write) -> u8 {
    let _ = writeln!(standard_output, "the harness Skill Manager");
    let _ = writeln!(
        standard_output,
        "  [1] install  - install or refresh managed Rust-native files"
    );
    let _ = writeln!(
        standard_output,
        "  [2] update   - fast-forward checkout, then install"
    );
    let _ = writeln!(
        standard_output,
        "  [3] status   - show install and runtime state"
    );
    let _ = writeln!(standard_output, "  [4] doctor   - diagnose install state");
    let _ = writeln!(standard_output);
    let _ = writeln!(
        standard_output,
        "Run the named command directly, for example `keel status`."
    );
    0
}

// why: clippy::items_after_test_module fires when any item follows this block.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::RepositoryLayout;
    use std::path::PathBuf;

    #[test]
    fn stale_managed_skill_names_detects_content_drift() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let root = std::env::temp_dir().join(format!(
            "keel-stale-skills-{}-{}",
            std::process::id(),
            nanos
        ));
        let home = root.join("claude-home");
        let src_skill = root.join("repo").join("demo-skill");
        let dst_skill = skills_directory(&home).join("demo-skill");
        fs::create_dir_all(&src_skill).unwrap();
        fs::create_dir_all(&dst_skill).unwrap();
        fs::write(
            src_skill.join("SKILL.md"),
            "---\nname: demo-skill\n---\nnew\n",
        )
        .unwrap();
        fs::write(
            dst_skill.join("SKILL.md"),
            "---\nname: demo-skill\n---\nold\n",
        )
        .unwrap();

        let layout = RepositoryLayout {
            root_path: root.join("repo"),
            skills: vec![SkillDefinition {
                name: "demo-skill".into(),
                skill_path: src_skill.clone(),
                agent_configs: vec![],
            }],
            root_files: vec![],
            shared_resource_directories: vec![],
            agent_names: vec![],
        };
        let stale = stale_managed_skill_names(&layout, &home);
        assert_eq!(stale, vec!["demo-skill".to_string()]);

        // After syncing content, drift clears.
        fs::write(
            dst_skill.join("SKILL.md"),
            "---\nname: demo-skill\n---\nnew\n",
        )
        .unwrap();
        assert!(stale_managed_skill_names(&layout, &home).is_empty());

        let _ = fs::remove_dir_all(&root);
        let _ = PathBuf::from("."); // silence unused if any
    }
}
