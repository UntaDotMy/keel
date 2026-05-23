use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[test]
fn install_generates_reasoning_without_model_pin() {
    let repository_root = repository_root();
    let claude_home = unique_temp_dir("claude-skills-agent-profile-install");
    let _ = fs::remove_dir_all(&claude_home);

    let output = Command::new(env!("CARGO_BIN_EXE_claude-skills"))
        .arg("install")
        .arg("--repo-root")
        .arg(&repository_root)
        .arg("--claude-home")
        .arg(&claude_home)
        .output()
        .expect("run claude-skills install");

    assert!(
        output.status.success(),
        "install failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let reviewer_profile =
        fs::read_to_string(claude_home.join("agent-profiles").join("reviewer.toml"))
            .expect("read generated reviewer profile");

    assert!(
        reviewer_profile.contains("model_reasoning_effort = \"high\""),
        "generated reviewer profile should preserve high reasoning:\n{reviewer_profile}"
    );
    assert!(
        !reviewer_profile
            .lines()
            .any(|line| line.starts_with("model = ")),
        "generated reviewer profile must not pin a model:\n{reviewer_profile}"
    );

    // Every skill references _shared/common-discipline.md via a relative path
    // that resolves against ~/.claude/skills/<skill>/. If the installer does
    // not stage the _shared directory, the references silently dangle even
    // though the source file lives in the repo. Assert the file lands where
    // skill text expects it.
    let installed_shared_discipline = claude_home
        .join("skills")
        .join("_shared")
        .join("common-discipline.md");
    assert!(
        installed_shared_discipline.is_file(),
        "installer must stage _shared/common-discipline.md so skill references resolve: {}",
        installed_shared_discipline.display()
    );

    // Subagents read `_shared/subagent-iron-law.md` from their CWD (the repo
    // root, which they inherit from the parent). When the user's CWD is
    // outside the repo, the same file must still be reachable through the
    // installed shared-resource directory. Assert the installer stages it
    // alongside common-discipline.md.
    let installed_iron_law = claude_home
        .join("skills")
        .join("_shared")
        .join("subagent-iron-law.md");
    assert!(
        installed_iron_law.is_file(),
        "installer must stage _shared/subagent-iron-law.md so subagents can find the bootstrap from a clean install: {}",
        installed_iron_law.display()
    );

    let _ = fs::remove_dir_all(claude_home);
}

#[test]
fn status_uses_installed_inventory_when_source_is_unavailable() {
    let repository_root = repository_root();
    let claude_home = unique_temp_dir("claude-skills-status-inventory");
    let non_repository_directory = unique_temp_dir("claude-skills-status-cwd");
    let _ = fs::remove_dir_all(&claude_home);
    let _ = fs::remove_dir_all(&non_repository_directory);
    fs::create_dir_all(&non_repository_directory).expect("create non-repository cwd");

    let install_output = Command::new(env!("CARGO_BIN_EXE_claude-skills"))
        .arg("install")
        .arg("--repo-root")
        .arg(&repository_root)
        .arg("--claude-home")
        .arg(&claude_home)
        .output()
        .expect("run claude-skills install");

    assert!(
        install_output.status.success(),
        "install failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&install_output.stdout),
        String::from_utf8_lossy(&install_output.stderr)
    );

    let metadata_path = claude_home
        .join(".claude-skill-manager")
        .join("install-metadata.txt");
    let metadata = fs::read_to_string(&metadata_path).expect("read install metadata");
    fs::write(
        &metadata_path,
        metadata
            .lines()
            .map(|line| {
                if line.starts_with("repository_root=") {
                    format!(
                        "repository_root={}",
                        non_repository_directory
                            .join("deleted-release-bundle")
                            .display()
                    )
                } else if line.starts_with("repo_version=") {
                    "repo_version=unknown".to_string()
                } else if line.starts_with("manager_version=") {
                    "manager_version=bootstrap-8c0eb1cf6c20".to_string()
                } else {
                    line.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .expect("rewrite install metadata");
    let managed_skill_count = fs::read_to_string(
        claude_home
            .join(".claude-skill-manager")
            .join("managed-skills.txt"),
    )
    .expect("read managed skill inventory")
    .lines()
    .filter(|line| !line.trim().is_empty())
    .count();

    let status_output = Command::new(env!("CARGO_BIN_EXE_claude-skills"))
        .arg("status")
        .arg("--claude-home")
        .arg(&claude_home)
        .current_dir(&non_repository_directory)
        .output()
        .expect("run claude-skills status");

    assert!(
        status_output.status.success(),
        "status failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&status_output.stdout),
        String::from_utf8_lossy(&status_output.stderr)
    );
    let stdout = String::from_utf8_lossy(&status_output.stdout);
    assert!(
        stdout.contains("Skill pack update status: current"),
        "status should use installed inventory when source is unavailable:\n{stdout}"
    );
    assert!(
        stdout.contains(&format!(
            "Synced skills: {managed_skill_count}/{managed_skill_count}"
        )),
        "status should not report installed skills against a zero source count:\n{stdout}"
    );
    assert!(
        stdout.contains("Source: installed inventory"),
        "status should explain that source layout is unavailable:\n{stdout}"
    );
    assert!(
        !stdout.contains(&format!("Synced skills: {managed_skill_count}/0")),
        "status must not render the misleading installer denominator:\n{stdout}"
    );
    assert!(
        !stdout.contains("go fallback"),
        "normal status output should avoid internal fallback wording:\n{stdout}"
    );
    assert!(
        !stdout.contains("Repo version: unknown"),
        "status should avoid unknown source-version wording when source is unavailable:\n{stdout}"
    );
    assert!(
        stdout.contains("Repo version: 8c0eb1c"),
        "status should recover the bootstrap commit from installed metadata when source git metadata is unavailable:\n{stdout}"
    );

    let _ = fs::remove_dir_all(claude_home);
    let _ = fs::remove_dir_all(non_repository_directory);
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("workspace repository root")
        .to_path_buf()
}

fn unique_temp_dir(prefix: &str) -> PathBuf {
    std::env::temp_dir().join(format!("{prefix}-{}", std::process::id()))
}

/// Every project-scoped subagent definition under `.claude/agents/` must point
/// at `_shared/subagent-iron-law.md` so the spawned subagent rebootstraps the
/// research-first contract without relying on the parent's SessionStart skill,
/// which never reaches the subagent's context window. The bootstrap file
/// itself must exist at that exact relative path. This test guards both
/// invariants so a future agent file does not silently ship without the
/// preamble.
#[test]
fn project_subagents_reference_iron_law_bootstrap() {
    let repository_root = repository_root();
    let bootstrap_path = repository_root.join("_shared").join("subagent-iron-law.md");
    assert!(
        bootstrap_path.is_file(),
        "subagent iron-law bootstrap must exist at {}",
        bootstrap_path.display()
    );
    let bootstrap_text =
        fs::read_to_string(&bootstrap_path).expect("read subagent iron-law bootstrap");
    assert!(
        bootstrap_text.contains("Trust the codebase"),
        "bootstrap text should restate the research-first contract; got:\n{bootstrap_text}"
    );

    let agents_directory = repository_root.join(".claude").join("agents");
    let entries = fs::read_dir(&agents_directory).unwrap_or_else(|error| {
        panic!(
            "list project agent definitions at {}: {error}",
            agents_directory.display()
        )
    });

    let mut agent_files: Vec<PathBuf> = entries
        .filter_map(|entry_result| entry_result.ok())
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("md"))
        .collect();
    agent_files.sort();

    assert!(
        !agent_files.is_empty(),
        "expected at least one .md subagent definition under {}",
        agents_directory.display()
    );

    let mut missing_reference: Vec<String> = Vec::new();
    for agent_file in &agent_files {
        let body = fs::read_to_string(agent_file)
            .unwrap_or_else(|error| panic!("read {}: {error}", agent_file.display()));
        if !body.contains("_shared/subagent-iron-law.md") {
            missing_reference.push(
                agent_file
                    .file_name()
                    .and_then(|value| value.to_str())
                    .unwrap_or("<unnamed>")
                    .to_string(),
            );
        }
    }

    assert!(
        missing_reference.is_empty(),
        "every .claude/agents/*.md must reference _shared/subagent-iron-law.md; missing in: {missing_reference:?}"
    );
}
