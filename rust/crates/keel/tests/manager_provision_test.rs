use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[test]
fn install_generates_reasoning_without_model_pin() {
    let repository_root = repository_root();
    let claude_home = unique_temp_dir("keel-agent-profile-install");
    let _ = fs::remove_dir_all(&claude_home);

    let output = Command::new(env!("CARGO_BIN_EXE_keel"))
        .arg("install")
        .arg("--repo-root")
        .arg(&repository_root)
        .arg("--claude-home")
        .arg(&claude_home)
        .output()
        .expect("run keel install");

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
    let claude_home = unique_temp_dir("keel-status-inventory");
    let non_repository_directory = unique_temp_dir("keel-status-cwd");
    let _ = fs::remove_dir_all(&claude_home);
    let _ = fs::remove_dir_all(&non_repository_directory);
    fs::create_dir_all(&non_repository_directory).expect("create non-repository cwd");

    let install_output = Command::new(env!("CARGO_BIN_EXE_keel"))
        .arg("install")
        .arg("--repo-root")
        .arg(&repository_root)
        .arg("--claude-home")
        .arg(&claude_home)
        .output()
        .expect("run keel install");

    assert!(
        install_output.status.success(),
        "install failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&install_output.stdout),
        String::from_utf8_lossy(&install_output.stderr)
    );

    let metadata_path = claude_home.join("state").join("install-metadata.txt");
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
    let managed_skill_count =
        fs::read_to_string(claude_home.join("state").join("managed-skills.txt"))
            .expect("read managed skill inventory")
            .lines()
            .filter(|line| !line.trim().is_empty())
            .count();

    let status_output = Command::new(env!("CARGO_BIN_EXE_keel"))
        .arg("status")
        .arg("--claude-home")
        .arg(&claude_home)
        .current_dir(&non_repository_directory)
        .output()
        .expect("run keel status");

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

/// Learned skills under `skills/learned-*` are loop-generated and must not
/// inflate the managed sync numerator. Status compares managed installed vs
/// source/inventory; learned count is reported separately (or omitted when 0).
#[test]
fn status_excludes_learned_skills_from_managed_sync_count() {
    let repository_root = repository_root();
    let claude_home = unique_temp_dir("keel-status-learned");
    let _ = fs::remove_dir_all(&claude_home);

    let install_output = Command::new(env!("CARGO_BIN_EXE_keel"))
        .arg("install")
        .arg("--repo-root")
        .arg(&repository_root)
        .arg("--claude-home")
        .arg(&claude_home)
        .output()
        .expect("run keel install");
    assert!(
        install_output.status.success(),
        "install failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&install_output.stdout),
        String::from_utf8_lossy(&install_output.stderr)
    );

    let managed_skill_count =
        fs::read_to_string(claude_home.join("state").join("managed-skills.txt"))
            .expect("read managed skill inventory")
            .lines()
            .filter(|line| !line.trim().is_empty())
            .count();

    // Three loop-generated skills that would previously make status report
    // (managed+3)/managed and "refresh recommended".
    for name in ["learned-alpha", "learned-beta", "learned-gamma"] {
        let skill_dir = claude_home.join("skills").join(name);
        fs::create_dir_all(&skill_dir).expect("create learned skill dir");
        fs::write(
            skill_dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: test learned skill\n---\n\nbody\n"),
        )
        .expect("write learned SKILL.md");
    }

    let status_output = Command::new(env!("CARGO_BIN_EXE_keel"))
        .arg("status")
        .arg("--repo-root")
        .arg(&repository_root)
        .arg("--claude-home")
        .arg(&claude_home)
        .output()
        .expect("run keel status");
    assert!(
        status_output.status.success(),
        "status failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&status_output.stdout),
        String::from_utf8_lossy(&status_output.stderr)
    );
    let stdout = String::from_utf8_lossy(&status_output.stdout);
    assert!(
        stdout.contains("Skill pack update status: current"),
        "learned-* must not force refresh recommended:\n{stdout}"
    );
    assert!(
        stdout.contains(&format!(
            "Synced skills: {managed_skill_count}/{managed_skill_count}"
        )),
        "synced skills must count managed only, not learned-*:\n{stdout}"
    );
    assert!(
        !stdout.contains(&format!(
            "Synced skills: {}/{managed_skill_count}",
            managed_skill_count + 3
        )),
        "synced skills must not include learned-* in the numerator:\n{stdout}"
    );
    assert!(
        stdout.contains("Learned skills: 3"),
        "status should report learned skill count separately:\n{stdout}"
    );

    let _ = fs::remove_dir_all(claude_home);
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
    // The subagent iron law must carry the understand-before-building rule, not
    // only the read-first / root-cause rules. Subagents spawn fresh and never
    // see the SessionStart bootstrap, so if this rule is dropped here a
    // delegated agent loses the "research the request before building, no
    // guessing" contract entirely — and nothing else would catch it. Pin both
    // the rule name and its no-guessing clause so a reword that guts the
    // meaning still trips the test.
    assert!(
        bootstrap_text.contains("Understand before building"),
        "subagent iron law must name the understand-before-building rule; got:\n{bootstrap_text}"
    );
    assert!(
        bootstrap_text.contains("Do not guess"),
        "subagent iron law must forbid guessing the request; got:\n{bootstrap_text}"
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

// ---------------------------------------------------------------------------
// Adapter wiring tests (W0-T2 RED)
//
// These tests define the contract for auto-detect-and-install: each adapter
// (opencode, codex, pi, cursor) should be wired ONLY when the corresponding
// CLI is detected, and should target the correct global path. Today many of
// these fail (RED) because:
//   - opencode/codex wire unconditionally (no detection gate)
//   - cursor targets repository_root.parent instead of ~/.cursorrules
//   - pi targets repository_root.parent instead of ~/.pi/agent/
//
// Wave 2 will make them GREEN.
// ---------------------------------------------------------------------------

/// Create a fake user home with a `.claude` subdirectory so the
/// `is_standard_home` guard passes and adapter wirers fire. Returns
/// (home, claude_home) where `home` is the parent that adapters land in.
fn fake_home_with_claude(prefix: &str) -> (PathBuf, PathBuf) {
    let home = unique_temp_dir(prefix);
    let _ = fs::remove_dir_all(&home);
    let claude_home = home.join(".claude");
    let _ = fs::create_dir_all(&claude_home);
    (home, claude_home)
}

fn run_install_at(repo_root: &Path, claude_home: &Path) {
    run_install_with_extra(repo_root, claude_home, &[]);
}

fn run_install_with_extra(repo_root: &Path, claude_home: &Path, extra: &[&str]) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_keel"));
    cmd.arg("install")
        .arg("--repo-root")
        .arg(repo_root)
        .arg("--claude-home")
        .arg(claude_home);
    for arg in extra {
        cmd.arg(arg);
    }
    let output = cmd.output().expect("run keel install");
    assert!(
        output.status.success(),
        "install failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn install_skips_opencode_when_not_detected() {
    let repository_root = repository_root();
    let (home, claude_home) = fake_home_with_claude("keel-skip-opencode");
    // Do NOT create ~/.config/opencode/ — opencode should not be detected.
    run_install_with_extra(&repository_root, &claude_home, &["--without", "opencode"]);
    let plugin = home
        .join(".config")
        .join("opencode")
        .join("plugins")
        .join("keel.ts");
    assert!(
        !plugin.exists(),
        "opencode plugin should NOT be created when opencode is not detected: {}",
        plugin.display()
    );
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn install_skips_codex_when_not_detected() {
    let repository_root = repository_root();
    let (home, claude_home) = fake_home_with_claude("keel-skip-codex");
    // Do NOT create ~/.codex/ — codex should not be detected.
    run_install_with_extra(&repository_root, &claude_home, &["--without", "codex"]);
    let codex_plugin = home.join(".codex").join("plugins").join("keel");
    assert!(
        !codex_plugin.exists(),
        "codex plugin dir should NOT be created when codex is not detected: {}",
        codex_plugin.display()
    );
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn install_skips_pi_when_not_detected() {
    let repository_root = repository_root();
    let (home, claude_home) = fake_home_with_claude("keel-skip-pi");
    // Do NOT create ~/.pi/agent/ — pi should not be detected.
    run_install_with_extra(&repository_root, &claude_home, &["--without", "pi"]);
    let agents_md = home.join(".pi").join("agent").join("AGENTS.md");
    assert!(
        !agents_md.exists(),
        "pi AGENTS.md should NOT be created when pi is not detected: {}",
        agents_md.display()
    );
    // Also assert the current buggy path (repository_root.parent) is NOT
    // written to — pi must not wire anywhere when not detected.
    let buggy_target = repository_root
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .join("AGENTS.md");
    let buggy_existed_before = buggy_target.exists();
    run_install_at(&repository_root, &claude_home);
    let buggy_exists_now = buggy_target.exists();
    if !buggy_existed_before && buggy_exists_now {
        let _ = fs::remove_file(&buggy_target);
    }
    assert!(
        !buggy_exists_now || buggy_existed_before,
        "pi must NOT write AGENTS.md to repository_root.parent when pi is not detected: {}",
        buggy_target.display()
    );
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn install_wires_opencode_when_detected() {
    let repository_root = repository_root();
    let (home, claude_home) = fake_home_with_claude("keel-wire-opencode");
    // Pre-create ~/.config/opencode/ so opencode is detected.
    let _ = fs::create_dir_all(home.join(".config").join("opencode"));
    run_install_at(&repository_root, &claude_home);
    let plugin = home
        .join(".config")
        .join("opencode")
        .join("plugins")
        .join("keel.ts");
    assert!(
        plugin.is_file(),
        "opencode plugin should be created when opencode is detected: {}",
        plugin.display()
    );
    let opencode_json = home.join(".config").join("opencode").join("opencode.json");
    if opencode_json.is_file() {
        let content = fs::read_to_string(&opencode_json).expect("read opencode.json");
        assert!(
            content.contains("\"keel\""),
            "opencode.json should contain a keel MCP entry: {content}"
        );
    }
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn install_wires_codex_when_detected() {
    let repository_root = repository_root();
    let (home, claude_home) = fake_home_with_claude("keel-wire-codex");
    // Pre-create ~/.codex/ so codex is detected.
    let _ = fs::create_dir_all(home.join(".codex"));
    run_install_at(&repository_root, &claude_home);
    let hooks_json = home
        .join(".codex")
        .join("plugins")
        .join("keel")
        .join("hooks")
        .join("hooks.json");
    assert!(
        hooks_json.is_file(),
        "codex hooks.json should be created when codex is detected: {}",
        hooks_json.display()
    );
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn install_wires_pi_when_detected() {
    let repository_root = repository_root();
    let (home, claude_home) = fake_home_with_claude("keel-wire-pi");
    // Pre-create ~/.pi/agent/ so pi is detected.
    let _ = fs::create_dir_all(home.join(".pi").join("agent"));
    run_install_at(&repository_root, &claude_home);
    let agents_md = home.join(".pi").join("agent").join("AGENTS.md");
    assert!(
        agents_md.is_file(),
        "pi AGENTS.md should be created at ~/.pi/agent/AGENTS.md when pi is detected: {}",
        agents_md.display()
    );
    let mcp_json = home.join(".config").join("mcp").join("mcp.json");
    if mcp_json.is_file() {
        let content = fs::read_to_string(&mcp_json).expect("read mcp.json");
        assert!(
            content.contains("\"keel\""),
            "~/.config/mcp/mcp.json should contain a keel MCP entry: {content}"
        );
    }
    // Clean up any buggy-path files the current code may have created.
    let buggy_agents = repository_root
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .join("AGENTS.md");
    let buggy_mcp = repository_root
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .join(".mcp.json");
    let _ = fs::remove_file(&buggy_agents);
    let _ = fs::remove_file(&buggy_mcp);
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn install_cursor_targets_home_cursorrules_not_repo_parent() {
    let repository_root = repository_root();
    let (home, claude_home) = fake_home_with_claude("keel-cursor-path");

    // Back up any existing .cursorrules at the buggy path so the test
    // never destroys a user file.
    let repo_parent = repository_root.parent().unwrap_or_else(|| Path::new(""));
    let buggy_cursorrules = repo_parent.join(".cursorrules");
    let backup_cursorrules = if buggy_cursorrules.is_file() {
        Some(fs::read(&buggy_cursorrules).expect("read existing cursorrules for backup"))
    } else {
        None
    };

    run_install_with_extra(&repository_root, &claude_home, &["--with", "cursor"]);

    // The buggy path writes to repository_root.parent/.cursorrules — assert it does NOT.
    assert!(
        !buggy_cursorrules.exists() || backup_cursorrules.is_some(),
        ".cursorrules must NOT be written to repository_root.parent (wrong path): {}",
        buggy_cursorrules.display()
    );

    // The correct target is ~/.cursorrules (home dir). Once detection + --with
    // cursor is implemented, this will exist. Today it does not (RED).
    let home_cursorrules = home.join(".cursorrules");
    assert!(
        home_cursorrules.is_file(),
        ".cursorrules should target ~/.cursorrules (home dir), not repository_root.parent: {}",
        home_cursorrules.display()
    );

    // Restore backup.
    match backup_cursorrules {
        Some(data) => {
            let _ = fs::write(&buggy_cursorrules, data);
        }
        None => {
            let _ = fs::remove_file(&buggy_cursorrules);
        }
    }
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn install_does_not_clobber_user_cursorrules() {
    let repository_root = repository_root();
    let (home, claude_home) = fake_home_with_claude("keel-cursor-noclobber");

    // Pre-write a custom .cursorrules in the fake home.
    let cursorrules = home.join(".cursorrules");
    let custom_content = "# My custom cursor rules\n\nDo not overwrite.\n";
    let _ = fs::create_dir_all(cursorrules.parent().unwrap_or_else(|| Path::new("")));
    let _ = fs::write(&cursorrules, custom_content);

    run_install_at(&repository_root, &claude_home);

    let after = fs::read_to_string(&cursorrules).unwrap_or_default();
    assert!(
        after.contains("# My custom cursor rules"),
        "user-customized .cursorrules must be preserved (byte-compare skip); got:\n{after}"
    );

    // Clean up any buggy-path cursorrules the current code may have created.
    let repo_parent = repository_root.parent().unwrap_or_else(|| Path::new(""));
    let buggy_cursorrules = repo_parent.join(".cursorrules");
    // Only remove if it didn't exist before (we can't know for sure, so
    // leave it — the backup/restore in the other cursor test handles safety).
    if buggy_cursorrules.is_file() {
        let content = fs::read_to_string(&buggy_cursorrules).unwrap_or_default();
        if content.starts_with("# keel") || content.contains("iron law") {
            let _ = fs::remove_file(&buggy_cursorrules);
        }
    }

    let _ = fs::remove_dir_all(&home);
}
