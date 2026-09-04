//! Integration tests for platform auto-detection and uninstall cleanup.
//!
//! These tests verify that `keel install` wires adapters only when the
//! corresponding platform is detected (config dir or binary on PATH), and
//! that `keel uninstall` removes all wired artifacts cleanly.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

struct TestDirectory(PathBuf);

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
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

fn assert_antigravity_hooks_are_plugin_relative(plugin: &Path) {
    let text = fs::read_to_string(plugin.join("hooks.json")).expect("read Antigravity hooks.json");
    let document: serde_json::Value =
        serde_json::from_str(&text).expect("Antigravity hooks.json must be valid JSON");
    let commands = [
        document["keel"]["PreToolUse"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap_or_default(),
        document["keel"]["PostToolUse"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap_or_default(),
        document["keel"]["PreInvocation"][0]["command"]
            .as_str()
            .unwrap_or_default(),
        document["keel"]["Stop"][0]["command"]
            .as_str()
            .unwrap_or_default(),
    ];
    assert_eq!(
        commands,
        [
            "node keel-antigravity.js pre-tool-use",
            "node keel-antigravity.js post-tool-use",
            "node keel-antigravity.js pre-invocation",
            "node keel-antigravity.js stop",
        ]
    );
    assert!(
        !text.contains("\\\""),
        "installed Antigravity hooks must not quote the adapter path: {text}"
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
// Detection and explicit host-wiring tests
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
fn opencode_install_includes_its_runtime_bridge_dependency() {
    let repo = repository_root();
    let (home, claude_home) = fake_home_with_claude("keel-opencode-bridge-core");
    run_install(&repo, &claude_home, &["--with", "opencode"]);

    assert!(
        home.join(".config")
            .join("opencode")
            .join("_shared")
            .join("ts")
            .join("bridge-core.ts")
            .is_file(),
        "the installed OpenCode plugin imports ../_shared/ts/bridge-core and must include it"
    );
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn pi_install_includes_its_runtime_bridge_dependency() {
    let repo = repository_root();
    let (home, claude_home) = fake_home_with_claude("keel-pi-bridge-core");
    run_install(&repo, &claude_home, &["--with", "pi"]);

    assert!(
        home.join(".pi")
            .join("agent")
            .join("_shared")
            .join("ts")
            .join("bridge-core.ts")
            .is_file(),
        "the installed Pi extension imports ../_shared/ts/bridge-core and must include it"
    );
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn commandcode_install_includes_its_runtime_bridge_dependency() {
    let repo = repository_root();
    let (home, claude_home) = fake_home_with_claude("keel-commandcode-bridge-core");
    run_install(&repo, &claude_home, &["--with", "commandcode"]);

    assert!(
        home.join(".commandcode")
            .join("_shared")
            .join("ts")
            .join("bridge-core.ts")
            .is_file(),
        "the installed Command Code mod imports ../_shared/ts/bridge-core and must include it"
    );
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn codex_install_publishes_the_host_neutral_gateway_skill() {
    let repo = repository_root();
    let (home, claude_home) = fake_home_with_claude("keel-codex-agent-skill");
    run_install(&repo, &claude_home, &["--with", "codex"]);

    assert!(
        home.join(".agents")
            .join("skills")
            .join("using-keel")
            .join("SKILL.md")
            .is_file(),
        "Codex discovers personal skills from ~/.agents/skills"
    );
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn with_flag_wires_oh_my_pi_native_surfaces() {
    let repo = repository_root();
    let (home, claude_home) = fake_home_with_claude("keel-with-omp");
    run_install(&repo, &claude_home, &["--with", "omp"]);
    let omp = home.join(".omp").join("agent");

    assert!(omp.join("extensions").join("keel-pi.ts").is_file());
    assert!(omp
        .join("_shared")
        .join("ts")
        .join("bridge-core.ts")
        .is_file());
    assert!(omp.join("mcp.json").is_file());
    assert!(omp.join("AGENTS.md").is_file());
    assert!(omp
        .join("skills")
        .join("using-keel")
        .join("SKILL.md")
        .is_file());
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn with_flag_wires_zcode_native_surfaces() {
    let repo = repository_root();
    let (home, claude_home) = fake_home_with_claude("keel-with-zcode");
    run_install(&repo, &claude_home, &["--with", "zcode"]);
    let zcode = home.join(".zcode");

    assert!(zcode.join("AGENTS.md").is_file());
    assert!(zcode
        .join("skills")
        .join("using-keel")
        .join("SKILL.md")
        .is_file());
    let config: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(zcode.join("cli").join("config.json")).unwrap())
            .unwrap();
    assert!(config["mcp"]["servers"]["keel"].is_object());
    assert_eq!(config["hooks"]["enabled"], true);
    assert!(config["hooks"]["events"]["PreToolUse"].is_array());
    assert!(
        config["hooks"]["events"]["PreToolUse"][0]["hooks"][0]["command"]
            .as_str()
            .is_some_and(|command| command.contains("keel"))
    );
    let stop_commands = config["hooks"]["events"]["Stop"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|group| group["hooks"][0]["args"].as_array())
        .map(|args| {
            args.iter()
                .filter_map(serde_json::Value::as_str)
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect::<Vec<_>>();
    assert!(stop_commands.iter().any(|command| command == "hook stop"));
    assert!(
        stop_commands
            .iter()
            .any(|command| command == "hook session-end"),
        "ZCode has no SessionEnd event, so Stop must also run learning/session capture"
    );
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn zcode_install_preserves_user_config_and_explicitly_disabled_hooks() {
    let repo = repository_root();
    let (home, claude_home) = fake_home_with_claude("keel-zcode-preserve");
    let config_path = home.join(".zcode").join("cli").join("config.json");
    fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    fs::write(
        &config_path,
        r#"{"theme":"dark","hooks":{"enabled":false},"mcp":{"servers":{"user":{"command":"user-server"}}}}"#,
    )
    .unwrap();

    run_install(&repo, &claude_home, &["--with", "zcode"]);
    let config: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&config_path).unwrap()).unwrap();
    assert_eq!(config["theme"], "dark");
    assert_eq!(config["hooks"]["enabled"], false);
    assert_eq!(config["mcp"]["servers"]["user"]["command"], "user-server");
    assert!(config["mcp"]["servers"]["keel"].is_object());
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn with_flag_wires_antigravity_global_plugin() {
    let repo = repository_root();
    let (home, claude_home) = fake_home_with_claude("keel-with-antigravity");
    run_install(&repo, &claude_home, &["--with", "antigravity"]);
    let plugin = home
        .join(".gemini")
        .join("config")
        .join("plugins")
        .join("keel");

    assert!(plugin.join("plugin.json").is_file());
    assert!(plugin.join("mcp_config.json").is_file());
    assert!(plugin.join("hooks.json").is_file());
    assert!(plugin.join("keel-antigravity.js").is_file());
    assert!(plugin.join("rules").join("keel.md").is_file());
    assert!(plugin
        .join("skills")
        .join("using-keel")
        .join("SKILL.md")
        .is_file());
    assert_antigravity_hooks_are_plugin_relative(&plugin);

    let global_mcp_path = home.join(".gemini").join("config").join("mcp_config.json");
    let global_mcp: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(&global_mcp_path).expect("read Antigravity global MCP config"),
    )
    .expect("Antigravity global MCP config must be valid JSON");
    assert_eq!(
        global_mcp["mcpServers"]["keel"]["command"].as_str(),
        Some(
            claude_home
                .join(if cfg!(windows) { "keel.exe" } else { "keel" })
                .to_string_lossy()
                .as_ref()
        ),
        "Antigravity IDE must receive a global MCP registration even before its plugin is enabled"
    );
    assert_eq!(
        global_mcp["mcpServers"]["keel"]["args"],
        serde_json::json!(["mcp", "serve"])
    );
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn antigravity_global_mcp_merge_preserves_existing_servers() {
    let repo = repository_root();
    let (home, claude_home) = fake_home_with_claude("keel-antigravity-global-mcp-preserve");
    let global_mcp_path = home.join(".gemini").join("config").join("mcp_config.json");
    fs::create_dir_all(global_mcp_path.parent().unwrap()).unwrap();
    fs::write(
        &global_mcp_path,
        r#"{"mcpServers":{"user":{"command":"user-server"}},"keep":true}"#,
    )
    .unwrap();

    run_install(&repo, &claude_home, &["--with", "antigravity"]);

    let global_mcp: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&global_mcp_path).unwrap()).unwrap();
    assert_eq!(global_mcp["keep"], true);
    assert_eq!(global_mcp["mcpServers"]["user"]["command"], "user-server");
    assert!(global_mcp["mcpServers"]["keel"].is_object());
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn existing_antigravity_cli_home_receives_the_cli_plugin() {
    let repo = repository_root();
    let (home, claude_home) = fake_home_with_claude("keel-antigravity-cli");
    fs::create_dir_all(home.join(".gemini").join("antigravity-cli")).unwrap();
    run_install(&repo, &claude_home, &[]);

    let plugin = home
        .join(".gemini")
        .join("antigravity-cli")
        .join("plugins")
        .join("keel");
    assert!(plugin.join("plugin.json").is_file());
    assert!(plugin.join("mcp_config.json").is_file());
    assert!(plugin.join("hooks.json").is_file());
    assert!(plugin.join("keel-antigravity.js").is_file());
    assert!(plugin
        .join("skills")
        .join("using-keel")
        .join("SKILL.md")
        .is_file());
    assert_antigravity_hooks_are_plugin_relative(&plugin);
    run_uninstall(&repo, &claude_home);
    assert!(
        !plugin.exists(),
        "uninstall must remove the managed CLI plugin"
    );
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn uninstall_removes_new_host_wiring_without_removing_user_config() {
    let repo = repository_root();
    let (home, claude_home) = fake_home_with_claude("keel-uninstall-new-hosts");
    let zcode_config = home.join(".zcode").join("cli").join("config.json");
    fs::create_dir_all(zcode_config.parent().unwrap()).unwrap();
    fs::write(&zcode_config, r#"{"userKeep":true}"#).unwrap();
    let antigravity_mcp = home.join(".gemini").join("config").join("mcp_config.json");
    fs::create_dir_all(antigravity_mcp.parent().unwrap()).unwrap();
    fs::write(
        &antigravity_mcp,
        r#"{"mcpServers":{"user":{"command":"user-server"}},"userKeep":true}"#,
    )
    .unwrap();

    run_install(&repo, &claude_home, &["--with", "omp,zcode,antigravity"]);
    run_uninstall(&repo, &claude_home);

    assert!(!home
        .join(".agents")
        .join("skills")
        .join("using-keel")
        .exists());
    assert!(!home
        .join(".omp")
        .join("agent")
        .join("extensions")
        .join("keel-pi.ts")
        .exists());
    assert!(!home
        .join(".omp")
        .join("agent")
        .join("_shared")
        .join("ts")
        .join("bridge-core.ts")
        .exists());
    assert!(!home
        .join(".omp")
        .join("agent")
        .join("skills")
        .join("using-keel")
        .exists());
    assert!(!home
        .join(".gemini")
        .join("config")
        .join("plugins")
        .join("keel")
        .exists());

    let antigravity_after: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&antigravity_mcp).unwrap()).unwrap();
    assert_eq!(antigravity_after["userKeep"], true);
    assert_eq!(
        antigravity_after["mcpServers"]["user"]["command"],
        "user-server"
    );
    assert!(antigravity_after["mcpServers"].get("keel").is_none());

    let zcode_after: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&zcode_config).unwrap()).unwrap();
    assert_eq!(zcode_after["userKeep"], true);
    assert!(zcode_after["mcp"]["servers"].get("keel").is_none());
    let pre_tool = zcode_after["hooks"]["events"]["PreToolUse"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(pre_tool
        .iter()
        .all(|entry| !entry.to_string().contains("Keel managed lifecycle hook")));
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

#[test]
fn with_grok_reuses_default_claude_compatible_hooks_without_duplicates() {
    let repo = repository_root();
    let (home, claude_home) = fake_home_with_claude("keel-with-grok");
    run_install(&repo, &claude_home, &["--with", "grok"]);

    let config = home.join(".grok").join("config.toml");
    let config_text = fs::read_to_string(&config).expect("read Grok config.toml");
    assert!(
        config_text.contains("[mcp_servers.keel]"),
        "Grok config must contain the native keel MCP server"
    );
    assert!(
        config_text.contains("args = [\"mcp\", \"serve\"]"),
        "Grok MCP server must launch keel's stdio transport"
    );
    let expected_binary = claude_home.join(if cfg!(windows) { "keel.exe" } else { "keel" });
    let config_doc: toml::Value = toml::from_str(&config_text).expect("valid Grok TOML");
    assert_eq!(
        config_doc["mcp_servers"]["keel"]["command"].as_str(),
        Some(expected_binary.to_string_lossy().as_ref()),
        "Grok MCP command must use the installed binary: {}",
        expected_binary.display()
    );

    let hooks = home.join(".grok").join("hooks").join("keel.json");
    assert!(
        !hooks.exists(),
        "Grok must not duplicate the managed Claude hooks it loads by default"
    );
    assert!(
        claude_home.join("settings.json").is_file(),
        "the Claude-compatible hook source must exist"
    );
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn with_grok_writes_native_hooks_when_claude_hook_compatibility_is_disabled() {
    let repo = repository_root();
    let (home, claude_home) = fake_home_with_claude("keel-with-grok-native-hooks");
    let grok_home = home.join(".grok");
    fs::create_dir_all(&grok_home).unwrap();
    fs::write(
        grok_home.join("config.toml"),
        "[compat.claude]\nhooks = false\n",
    )
    .unwrap();

    run_install(&repo, &claude_home, &["--with", "grok"]);

    let hooks = grok_home.join("hooks").join("keel.json");
    assert!(
        hooks.is_file(),
        "Grok needs native hooks when Claude hook compatibility is disabled"
    );
    let _ = fs::remove_dir_all(&home);
}

// ---------------------------------------------------------------------------
// Uninstall cleanup tests
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
fn uninstall_removes_only_keel_owned_grok_config() {
    let repo = repository_root();
    let (home, claude_home) = fake_home_with_claude("keel-uninstall-grok");
    let grok_home = home.join(".grok");
    fs::create_dir_all(&grok_home).unwrap();
    let config = grok_home.join("config.toml");
    fs::write(
        &config,
        "[display]\ntheme = \"dark\"\n[compat.claude]\nhooks = false\n",
    )
    .unwrap();

    run_install(&repo, &claude_home, &["--with", "grok"]);
    let hooks = grok_home.join("hooks").join("keel.json");
    assert!(hooks.is_file(), "Grok hook must exist after install");
    run_uninstall(&repo, &claude_home);

    let after = fs::read_to_string(&config).expect("user Grok config must remain");
    assert!(after.contains("[display]"));
    assert!(after.contains("theme = \"dark\""));
    assert!(!after.contains("[mcp_servers.keel]"));
    assert!(!hooks.exists(), "keel-owned Grok hook must be removed");
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn uninstall_custom_home_does_not_remove_user_host_adapters() {
    let repo = repository_root();
    let user_home = unique_temp_dir("keel-uninstall-custom-user-home");
    let custom_home = unique_temp_dir("keel-uninstall-custom-root");
    let _ = fs::remove_dir_all(&user_home);
    let _ = fs::remove_dir_all(&custom_home);
    let _user_cleanup = TestDirectory(user_home.clone());
    let _custom_cleanup = TestDirectory(custom_home.clone());
    fs::create_dir_all(&custom_home).unwrap();

    let grok_home = user_home.join(".grok");
    let hook_path = grok_home.join("hooks").join("keel.json");
    fs::create_dir_all(hook_path.parent().unwrap()).unwrap();
    fs::write(&hook_path, r#"{"command":"keel hook session-start"}"#).unwrap();
    let config_path = grok_home.join("config.toml");
    fs::write(
        &config_path,
        "[mcp_servers.keel]\ncommand = \"keel\"\nargs = [\"mcp\", \"serve\"]\n",
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_keel"))
        .arg("uninstall")
        .arg("--repo-root")
        .arg(&repo)
        .arg("--claude-home")
        .arg(&custom_home)
        .env("HOME", &user_home)
        .env("USERPROFILE", &user_home)
        .env_remove("KEEL_HOME")
        .env_remove("CLAUDE_TARGET_OVERRIDE")
        .output()
        .expect("run keel uninstall with a custom home");
    assert!(
        output.status.success(),
        "uninstall failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        hook_path.is_file(),
        "a custom install root must not remove user-level Grok hooks"
    );
    assert!(
        fs::read_to_string(&config_path)
            .unwrap()
            .contains("mcp_servers.keel"),
        "a custom install root must not remove user-level Grok MCP config"
    );
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
    // install↔uninstall symmetry: install writes ~/.cursor/hooks.json and
    // ~/.cursor/hooks/keel-cursor.sh, so uninstall must remove both or Cursor
    // keeps invoking a hook that shells to the now-deleted keel binary.
    let repo = repository_root();
    let (home, claude_home) = fake_home_with_claude("keel-uninstall-cursor-hooks");
    run_install(&repo, &claude_home, &["--with", "cursor"]);
    let hooks_json = home.join(".cursor").join("hooks.json");
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
    // other plugin files so Codex loads the keel MCP server. It must also
    // rewrite the MCP `command` to the absolute keel binary path, because
    // Codex resolves `command` via PATH only and the bare `keel` from the
    // shipped template fails with "program not found" when ~/.claude is not on
    // PATH (the common case on Windows, where install does not touch PATH).
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
    // The MCP command must be the absolute installed-binary path, not the
    // bare `keel` from the shipped template.
    let mcp_text = fs::read_to_string(&mcp_json).expect("read codex .mcp.json");
    let mcp_doc: serde_json::Value =
        serde_json::from_str(&mcp_text).expect("codex .mcp.json must be valid JSON");
    let command = mcp_doc
        .get("mcp_servers")
        .and_then(|s| s.get("keel"))
        .and_then(|s| s.get("command"))
        .and_then(|v| v.as_str())
        .expect("codex .mcp.json must have mcp_servers.keel.command");
    assert_ne!(
        command, "keel",
        "codex MCP command must not be the bare PATH-dependent template value"
    );
    // It must point at the binary under the install claude-home.
    let exe_name = if cfg!(windows) { "keel.exe" } else { "keel" };
    let expected_binary = claude_home.join(exe_name);
    assert!(
        command.contains(&expected_binary.to_string_lossy().to_string())
            || command.ends_with(exe_name),
        "codex MCP command `{command}` should resolve to the installed binary at {}",
        expected_binary.display(),
    );
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn keel_home_splits_engagement_to_sibling_dot_claude() {
    // The host-neutral layout: `--claude-home <tmp>/.keel` publishes binary
    // and data into `.keel`; skills/agents/commands land in sibling `.claude`.
    let repo = repository_root();
    let (home, keel_home) = {
        let home = unique_temp_dir("keel-home-split");
        let _ = fs::remove_dir_all(&home);
        let keel_home = home.join(".keel");
        let _ = fs::create_dir_all(&keel_home);
        (home, keel_home)
    };
    run_install(&repo, &keel_home, &[]);

    // The neutral home carries keel's own config/state surfaces.
    assert!(
        keel_home.join("config.toml").is_file(),
        "the keel root must hold keel's managed config.toml"
    );
    // Engagement files live in the sibling .claude, not the .keel root.
    let claude_home = home.join(".claude");
    assert!(
        claude_home.join("skills").is_dir(),
        "skills must land in the sibling ~/.claude, got none under {}",
        claude_home.display()
    );
    assert!(
        claude_home.join("agents").is_dir(),
        "agents must land in the sibling ~/.claude"
    );
    assert!(
        !keel_home.join("skills").is_dir(),
        "skills must NOT be duplicated into the .keel root"
    );
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn codex_install_registers_marketplace_and_enablement() {
    // The "installed but not wired" regression: Codex discovers plugins via
    // the marketplace manifest and loads only plugins enabled in config.toml.
    let repo = repository_root();
    let (home, claude_home) = fake_home_with_claude("keel-codex-discovery");
    let _ = fs::create_dir_all(home.join(".codex"));
    run_install(&repo, &claude_home, &[]);

    let marketplace = home
        .join(".agents")
        .join("plugins")
        .join("marketplace.json");
    assert!(
        marketplace.is_file(),
        "install must register the keel plugin in the personal marketplace manifest"
    );
    let marketplace_doc: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&marketplace).unwrap())
            .expect("marketplace.json must be valid JSON");
    let keel_entry = marketplace_doc
        .get("plugins")
        .and_then(|p| p.as_array())
        .and_then(|entries| {
            entries
                .iter()
                .find(|entry| entry.get("name").and_then(|n| n.as_str()) == Some("keel"))
        })
        .expect("marketplace.json must contain the keel entry");
    assert_eq!(
        keel_entry
            .get("source")
            .and_then(|source| source.get("path"))
            .and_then(|path| path.as_str()),
        Some("./.codex/plugins/keel"),
        "local marketplace paths must use Codex's required ./ relative form"
    );

    let config_toml = home.join(".codex").join("config.toml");
    assert!(
        config_toml.is_file(),
        "install must ensure codex config.toml exists for enablement"
    );
    let config_text = fs::read_to_string(&config_toml).unwrap();
    let parsed: toml::Value =
        toml::from_str(&config_text).expect("codex config.toml must remain valid TOML");
    assert_eq!(
        parsed
            .get("plugins")
            .and_then(|p| p.get("keel@personal-keel"))
            .and_then(|entry| entry.get("enabled"))
            .and_then(|v| v.as_bool()),
        Some(true),
        "codex config.toml must enable the keel plugin"
    );

    // Uninstall reverses both discovery surfaces.
    run_uninstall(&repo, &claude_home);
    assert!(
        !marketplace.exists(),
        "uninstall must remove the keel-only marketplace manifest"
    );
    if config_toml.is_file() {
        let after = fs::read_to_string(&config_toml).unwrap();
        assert!(
            !after.contains("keel@personal-keel"),
            "uninstall must remove the keel plugin enablement section"
        );
    }
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn codex_enablement_preserves_user_config_and_disable_choice() {
    // install must never clobber unrelated config.toml keys and must respect a
    // user's explicit `enabled = false`.
    let repo = repository_root();
    let (home, claude_home) = fake_home_with_claude("keel-codex-preserve");
    let _ = fs::create_dir_all(home.join(".codex"));
    let config_toml = home.join(".codex").join("config.toml");
    fs::write(
        &config_toml,
        "model = \"user-model\"\n\n[plugins.\"keel@personal-keel\"]\nenabled = false\n",
    )
    .unwrap();
    run_install(&repo, &claude_home, &[]);

    let parsed: toml::Value = toml::from_str(&fs::read_to_string(&config_toml).unwrap())
        .expect("config.toml must remain valid TOML");
    assert_eq!(
        parsed.get("model").and_then(|v| v.as_str()),
        Some("user-model"),
        "install must preserve unrelated config keys"
    );
    assert_eq!(
        parsed
            .get("plugins")
            .and_then(|p| p.get("keel@personal-keel"))
            .and_then(|entry| entry.get("enabled"))
            .and_then(|v| v.as_bool()),
        Some(false),
        "an explicit user disable must survive install"
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
