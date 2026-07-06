//! Integration tests for platform auto-detection and uninstall cleanup.
//!
//! These tests verify that `keel install` wires adapters only when the
//! corresponding platform is detected (config dir or binary on PATH), and
//! that `keel uninstall` removes all wired artifacts cleanly.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

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

fn fake_home_with_claude(prefix: &str) -> (PathBuf, PathBuf) {
    let home = unique_temp_dir(prefix);
    let _ = fs::remove_dir_all(&home);
    let claude_home = home.join(".claude");
    let _ = fs::create_dir_all(&claude_home);
    (home, claude_home)
}

fn run_install(repo_root: &Path, claude_home: &Path, extra: &[&str]) {
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

fn run_uninstall(repo_root: &Path, claude_home: &Path) {
    let output = Command::new(env!("CARGO_BIN_EXE_keel"))
        .arg("uninstall")
        .arg("--repo-root")
        .arg(repo_root)
        .arg("--claude-home")
        .arg(claude_home)
        .output()
        .expect("run keel uninstall");
    assert!(
        output.status.success(),
        "uninstall failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

// ---------------------------------------------------------------------------
// Detection tests (10)
// ---------------------------------------------------------------------------

#[test]
fn detect_none_when_nothing_installed() {
    let repo = repository_root();
    let (home, claude_home) = fake_home_with_claude("keel-detect-none");
    run_install(&repo, &claude_home, &["--without", "opencode,codex,pi"]);
    let opencode_plugin = home
        .join(".config")
        .join("opencode")
        .join("plugins")
        .join("keel.ts");
    assert!(
        !opencode_plugin.exists(),
        "opencode plugin should not exist"
    );
    let codex_dir = home.join(".codex").join("plugins").join("keel");
    assert!(!codex_dir.exists(), "codex plugin dir should not exist");
    let pi_agents = home.join(".pi").join("agent").join("AGENTS.md");
    assert!(!pi_agents.exists(), "pi AGENTS.md should not exist");
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn detect_opencode_via_config_dir() {
    let repo = repository_root();
    let (home, claude_home) = fake_home_with_claude("keel-detect-opencode-cfg");
    let _ = fs::create_dir_all(home.join(".config").join("opencode"));
    run_install(&repo, &claude_home, &[]);
    let plugin = home
        .join(".config")
        .join("opencode")
        .join("plugins")
        .join("keel.ts");
    assert!(
        plugin.is_file(),
        "opencode plugin should exist when config dir present: {}",
        plugin.display()
    );
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn detect_codex_via_config_dir() {
    let repo = repository_root();
    let (home, claude_home) = fake_home_with_claude("keel-detect-codex-cfg");
    let _ = fs::create_dir_all(home.join(".codex"));
    run_install(&repo, &claude_home, &[]);
    let hooks = home
        .join(".codex")
        .join("plugins")
        .join("keel")
        .join("hooks")
        .join("hooks.json");
    assert!(
        hooks.is_file(),
        "codex hooks.json should exist when config dir present: {}",
        hooks.display()
    );
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn detect_pi_via_config_dir() {
    let repo = repository_root();
    let (home, claude_home) = fake_home_with_claude("keel-detect-pi-cfg");
    let _ = fs::create_dir_all(home.join(".pi").join("agent"));
    run_install(&repo, &claude_home, &[]);
    let agents_md = home.join(".pi").join("agent").join("AGENTS.md");
    assert!(
        agents_md.is_file(),
        "pi AGENTS.md should exist when config dir present: {}",
        agents_md.display()
    );
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn detect_multiple_platforms() {
    let repo = repository_root();
    let (home, claude_home) = fake_home_with_claude("keel-detect-multi");
    let _ = fs::create_dir_all(home.join(".config").join("opencode"));
    let _ = fs::create_dir_all(home.join(".codex"));
    run_install(&repo, &claude_home, &[]);
    let opencode_plugin = home
        .join(".config")
        .join("opencode")
        .join("plugins")
        .join("keel.ts");
    assert!(opencode_plugin.is_file(), "opencode plugin should exist");
    let codex_hooks = home
        .join(".codex")
        .join("plugins")
        .join("keel")
        .join("hooks")
        .join("hooks.json");
    assert!(codex_hooks.is_file(), "codex hooks.json should exist");
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn detect_all_three_platforms() {
    let repo = repository_root();
    let (home, claude_home) = fake_home_with_claude("keel-detect-all-three");
    let _ = fs::create_dir_all(home.join(".config").join("opencode"));
    let _ = fs::create_dir_all(home.join(".codex"));
    let _ = fs::create_dir_all(home.join(".pi").join("agent"));
    run_install(&repo, &claude_home, &[]);
    let opencode_plugin = home
        .join(".config")
        .join("opencode")
        .join("plugins")
        .join("keel.ts");
    assert!(opencode_plugin.is_file(), "opencode plugin should exist");
    let codex_hooks = home
        .join(".codex")
        .join("plugins")
        .join("keel")
        .join("hooks")
        .join("hooks.json");
    assert!(codex_hooks.is_file(), "codex hooks.json should exist");
    let pi_agents = home.join(".pi").join("agent").join("AGENTS.md");
    assert!(pi_agents.is_file(), "pi AGENTS.md should exist");
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn cursor_never_auto_detected() {
    let repo = repository_root();
    let (home, claude_home) = fake_home_with_claude("keel-cursor-no-auto");
    run_install(&repo, &claude_home, &[]);
    let cursorrules = home.join(".cursorrules");
    assert!(
        !cursorrules.exists(),
        "cursorrules should NOT be created without --with cursor"
    );
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn with_flag_forces_opencode_when_not_detected() {
    let repo = repository_root();
    let (home, claude_home) = fake_home_with_claude("keel-with-opencode");
    run_install(&repo, &claude_home, &["--with", "opencode"]);
    let plugin = home
        .join(".config")
        .join("opencode")
        .join("plugins")
        .join("keel.ts");
    assert!(
        plugin.is_file(),
        "opencode plugin should exist with --with opencode even without config dir"
    );
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn without_flag_overrides_detection() {
    let repo = repository_root();
    let (home, claude_home) = fake_home_with_claude("keel-without-overrides");
    let _ = fs::create_dir_all(home.join(".config").join("opencode"));
    run_install(&repo, &claude_home, &["--without", "opencode"]);
    let plugin = home
        .join(".config")
        .join("opencode")
        .join("plugins")
        .join("keel.ts");
    assert!(
        !plugin.exists(),
        "opencode plugin should NOT exist when --without opencode overrides detection"
    );
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn with_cursor_forces_cursor_wiring() {
    let repo = repository_root();
    let (home, claude_home) = fake_home_with_claude("keel-with-cursor");
    run_install(&repo, &claude_home, &["--with", "cursor"]);
    let cursorrules = home.join(".cursorrules");
    assert!(
        cursorrules.is_file(),
        "cursorrules should exist with --with cursor: {}",
        cursorrules.display()
    );
    let _ = fs::remove_dir_all(&home);
}

// ---------------------------------------------------------------------------
// Uninstall cleanup tests (6)
// ---------------------------------------------------------------------------

#[test]
fn uninstall_removes_opencode_plugin() {
    let repo = repository_root();
    let (home, claude_home) = fake_home_with_claude("keel-uninstall-opencode");
    let _ = fs::create_dir_all(home.join(".config").join("opencode"));
    run_install(&repo, &claude_home, &[]);
    let plugin = home
        .join(".config")
        .join("opencode")
        .join("plugins")
        .join("keel.ts");
    assert!(plugin.is_file(), "plugin should exist after install");
    run_uninstall(&repo, &claude_home);
    assert!(
        !plugin.exists(),
        "opencode plugin should be removed after uninstall"
    );
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn uninstall_removes_codex_plugin() {
    let repo = repository_root();
    let (home, claude_home) = fake_home_with_claude("keel-uninstall-codex");
    let _ = fs::create_dir_all(home.join(".codex"));
    run_install(&repo, &claude_home, &[]);
    let codex_dir = home.join(".codex").join("plugins").join("keel");
    assert!(
        codex_dir.is_dir(),
        "codex plugin dir should exist after install"
    );
    run_uninstall(&repo, &claude_home);
    assert!(
        !codex_dir.exists(),
        "codex plugin dir should be removed after uninstall"
    );
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn uninstall_removes_cursor_rules() {
    let repo = repository_root();
    let (home, claude_home) = fake_home_with_claude("keel-uninstall-cursor");
    run_install(&repo, &claude_home, &["--with", "cursor"]);
    let cursorrules = home.join(".cursorrules");
    assert!(
        cursorrules.is_file(),
        "cursorrules should exist after install with --with cursor"
    );
    run_uninstall(&repo, &claude_home);
    assert!(
        !cursorrules.exists(),
        "keel-managed .cursorrules should be removed after uninstall"
    );
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn uninstall_removes_pi_agents_md() {
    let repo = repository_root();
    let (home, claude_home) = fake_home_with_claude("keel-uninstall-pi-agents");
    let _ = fs::create_dir_all(home.join(".pi").join("agent"));
    run_install(&repo, &claude_home, &[]);
    let agents_md = home.join(".pi").join("agent").join("AGENTS.md");
    assert!(
        agents_md.is_file(),
        "pi AGENTS.md should exist after install"
    );
    run_uninstall(&repo, &claude_home);
    assert!(
        !agents_md.exists(),
        "keel-managed pi AGENTS.md should be removed after uninstall"
    );
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn uninstall_removes_pi_mcp_entry() {
    let repo = repository_root();
    let (home, claude_home) = fake_home_with_claude("keel-uninstall-pi-mcp");
    let _ = fs::create_dir_all(home.join(".pi").join("agent"));
    run_install(&repo, &claude_home, &[]);
    let mcp_json = home.join(".config").join("mcp").join("mcp.json");
    if mcp_json.is_file() {
        let content = fs::read_to_string(&mcp_json).expect("read mcp.json");
        assert!(
            content.contains("\"keel\""),
            "mcp.json should contain keel entry after install"
        );
        run_uninstall(&repo, &claude_home);
        if mcp_json.is_file() {
            let after = fs::read_to_string(&mcp_json).unwrap_or_default();
            assert!(
                !after.contains("\"keel\""),
                "keel entry should be removed from mcp.json after uninstall"
            );
        }
    }
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn uninstall_preserves_user_cursorrules() {
    let repo = repository_root();
    let (home, claude_home) = fake_home_with_claude("keel-uninstall-preserve-cursor");
    let cursorrules = home.join(".cursorrules");
    let custom = "# My custom rules\nDo not touch.\n";
    let _ = fs::write(&cursorrules, custom);
    run_install(&repo, &claude_home, &[]);
    let after_install = fs::read_to_string(&cursorrules).unwrap_or_default();
    assert!(
        after_install.contains("# My custom rules"),
        "user cursorrules must be preserved during install"
    );
    run_uninstall(&repo, &claude_home);
    let after_uninstall = fs::read_to_string(&cursorrules).unwrap_or_default();
    assert!(
        after_uninstall.contains("# My custom rules"),
        "user cursorrules must survive uninstall"
    );
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn uninstall_removes_opencode_mcp_entry() {
    // install↔uninstall symmetry: install merges mcp.keel into opencode.json,
    // so uninstall must remove that entry or OpenCode keeps spawning the
    // now-deleted keel binary every session.
    let repo = repository_root();
    let (home, claude_home) = fake_home_with_claude("keel-uninstall-opencode-mcp");
    let _ = fs::create_dir_all(home.join(".config").join("opencode"));
    run_install(&repo, &claude_home, &[]);
    let opencode_json = home.join(".config").join("opencode").join("opencode.json");
    assert!(
        opencode_json.is_file(),
        "install must create opencode.json for this test to be meaningful"
    );
    let before = fs::read_to_string(&opencode_json).expect("read opencode.json");
    assert!(
        before.contains("\"keel\""),
        "opencode.json should contain keel MCP entry after install"
    );
    run_uninstall(&repo, &claude_home);
    let after = fs::read_to_string(&opencode_json).unwrap_or_default();
    assert!(
        !after.contains("\"keel\""),
        "keel MCP entry must be removed from opencode.json after uninstall"
    );
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn uninstall_removes_cursor_hooks() {
    // install↔uninstall symmetry: install writes ~/.cursor/hooks/{hooks.json,
    // keel-cursor.sh}, so uninstall must remove both or Cursor keeps invoking a
    // hook that shells to the now-deleted keel binary on every tool call.
    let repo = repository_root();
    let (home, claude_home) = fake_home_with_claude("keel-uninstall-cursor-hooks");
    run_install(&repo, &claude_home, &["--with", "cursor"]);
    let hooks_json = home.join(".cursor").join("hooks").join("hooks.json");
    let hook_script = home.join(".cursor").join("hooks").join("keel-cursor.sh");
    assert!(
        hooks_json.is_file(),
        "cursor hooks.json should exist after install with --with cursor"
    );
    assert!(
        hook_script.is_file(),
        "cursor keel-cursor.sh should exist after install with --with cursor"
    );
    run_uninstall(&repo, &claude_home);
    assert!(
        !hooks_json.exists(),
        "cursor hooks.json must be removed after uninstall"
    );
    assert!(
        !hook_script.exists(),
        "cursor keel-cursor.sh must be removed after uninstall"
    );
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn install_copies_codex_mcp_config() {
    // Codex registers its MCP server via a plugin-bundled .mcp.json referenced
    // by the manifest's mcpServers field. install must copy it alongside the
    // other plugin files so Codex loads the keel MCP server.
    let repo = repository_root();
    let (home, claude_home) = fake_home_with_claude("keel-install-codex-mcp");
    let _ = fs::create_dir_all(home.join(".codex"));
    run_install(&repo, &claude_home, &[]);
    let mcp_json = home
        .join(".codex")
        .join("plugins")
        .join("keel")
        .join(".mcp.json");
    assert!(
        mcp_json.is_file(),
        "codex .mcp.json should exist after install"
    );
    let manifest = home
        .join(".codex")
        .join("plugins")
        .join("keel")
        .join(".codex-plugin")
        .join("plugin.json");
    let manifest_text = fs::read_to_string(&manifest).expect("read codex plugin.json");
    assert!(
        manifest_text.contains("\"mcpServers\""),
        "codex plugin.json manifest must reference the bundled MCP config"
    );
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn uninstall_removes_cursor_mcp_entry() {
    // install↔uninstall symmetry: install merges the `keel` entry into
    // ~/.cursor/mcp.json, so uninstall must remove that entry (preserving any
    // other MCP servers the user configured) or Cursor keeps spawning the
    // now-deleted keel binary.
    let repo = repository_root();
    let (home, claude_home) = fake_home_with_claude("keel-uninstall-cursor-mcp");
    // Pre-existing user MCP server that uninstall must preserve.
    let mcp_json = home.join(".cursor").join("mcp.json");
    let _ = fs::create_dir_all(mcp_json.parent().unwrap());
    let user_preexisting = r#"{
  "mcpServers": {
    "user-other": { "command": "other-binary", "args": ["run"] }
  }
}"#;
    let _ = fs::write(&mcp_json, user_preexisting);

    run_install(&repo, &claude_home, &["--with", "cursor"]);
    let after_install = fs::read_to_string(&mcp_json).expect("read cursor mcp.json after install");
    assert!(
        after_install.contains("\"keel\""),
        "cursor mcp.json should contain keel entry after install"
    );
    assert!(
        after_install.contains("\"user-other\""),
        "install must preserve the user's pre-existing MCP servers"
    );

    run_uninstall(&repo, &claude_home);
    let after_uninstall = fs::read_to_string(&mcp_json).unwrap_or_default();
    assert!(
        !after_uninstall.contains("\"keel\""),
        "keel entry must be removed from cursor mcp.json after uninstall"
    );
    assert!(
        after_uninstall.contains("\"user-other\""),
        "uninstall must preserve the user's pre-existing MCP servers"
    );
    let _ = fs::remove_dir_all(&home);
}
