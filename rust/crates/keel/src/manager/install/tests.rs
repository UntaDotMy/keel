// Installer tests.
use super::commands::remove_wired_adapters;
use super::*;
use crate::runtime::{
    display_path, executable_file_name, installed_executable_path, legacy_state_directory,
    state_directory, update_cache_directory,
};
use keel_platform::detect_current_target;
use std::fs;
use std::path::{Path, PathBuf};

#[test]
fn merge_json_mcp_opencode_preserves_existing_keys_and_adds_keel() {
    let dir = std::env::temp_dir().join(format!("ulw-mcp-merge-{}", std::process::id()));
    let _ = fs::create_dir_all(&dir);
    let config = dir.join("opencode.json");
    fs::write(
        &config,
        r#"{"theme":"dark","mcp":{"existing":{"type":"local","command":["foo"],"enabled":true}}}"#,
    )
    .unwrap();
    let entry = serde_json::json!({"type":"local","command":["bin","mcp","serve"],"enabled":true});
    let result = merge_json_mcp(&config, "mcp", "keel", &entry, None);
    assert!(matches!(result, Ok(JsonMcpMergeResult::Added)));
    let parsed: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&config).unwrap()).unwrap();
    assert_eq!(parsed["theme"], "dark");
    assert_eq!(parsed["mcp"]["existing"]["command"][0], "foo");
    assert_eq!(parsed["mcp"]["keel"]["command"][0], "bin");
    let _ = fs::remove_dir_all(&dir);
}
#[test]
fn merge_json_mcp_preserves_conflicting_user_entry() {
    let dir = std::env::temp_dir().join(format!("ulw-mcp-conflict-{}", std::process::id()));
    let _ = fs::create_dir_all(&dir);
    let config = dir.join("mcp.json");
    fs::write(
        &config,
        r#"{"mcp":{"keel":{"type":"local","command":["user-keel","serve"],"enabled":true}}}"#,
    )
    .unwrap();
    let entry = serde_json::json!({"type":"local","command":["bin","mcp","serve"],"enabled":true});
    let result = merge_json_mcp(&config, "mcp", "keel", &entry, None);
    assert!(result.is_err());
    let text = fs::read_to_string(&config).unwrap();
    assert!(text.contains("user-keel"));
    assert!(!text.contains("\"bin\""));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn remove_json_mcp_entry_preserves_unmanaged_entry() {
    let dir = std::env::temp_dir().join(format!("ulw-mcp-remove-{}", std::process::id()));
    let _ = fs::create_dir_all(&dir);
    let config = dir.join("mcp.json");
    fs::write(
        &config,
        r#"{"mcpServers":{"keel":{"type":"stdio","command":"user-keel","args":["serve"]}}}"#,
    )
    .unwrap();
    assert_eq!(remove_json_mcp_entry(&config, "mcpServers"), 0);
    assert!(fs::read_to_string(&config).unwrap().contains("user-keel"));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn merge_json_mcp_tolerates_utf8_bom() {
    let dir = std::env::temp_dir().join(format!("ulw-mcp-bom-{}", std::process::id()));
    let _ = fs::create_dir_all(&dir);
    let config = dir.join("opencode.json");
    let with_bom = format!("\u{feff}{}", r#"{"mcp":{}}"#);
    fs::write(&config, with_bom).unwrap();
    let entry = serde_json::json!({"type":"local","command":["bin","mcp","serve"],"enabled":true});
    let result = merge_json_mcp(&config, "mcp", "keel", &entry, None);
    assert!(
        matches!(result, Ok(JsonMcpMergeResult::Added)),
        "BOM-prefixed config must parse, got {result:?}"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn merge_json_mcp_creates_config_when_absent() {
    let dir = std::env::temp_dir().join(format!("ulw-mcp-absent-{}", std::process::id()));
    let _ = fs::create_dir_all(&dir);
    let config = dir.join("opencode.json");
    let entry = serde_json::json!({"type":"local","command":["bin","mcp","serve"],"enabled":true});
    let result = merge_json_mcp(&config, "mcp", "keel", &entry, None);
    assert!(matches!(result, Ok(JsonMcpMergeResult::Added)));
    assert!(config.is_file());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn stale_temp_keel_entry_detection() {
    // A dead temp fixture dir is stale and purgeable.
    let temp = std::env::temp_dir();
    let dead = temp
        .join(format!("keel-home-split-{}", std::process::id()))
        .join(".keel");
    let _ = fs::remove_dir_all(dead.parent().unwrap());
    assert!(
        is_stale_temp_keel_entry(&dead.to_string_lossy()),
        "nonexistent keel-home-split-*/.keel must be stale"
    );

    // A LIVE temp dir is NOT stale ; purge must never remove a real install.
    let live = temp
        .join(format!("keel-home-split-live-{}", std::process::id()))
        .join(".keel");
    let _ = fs::create_dir_all(&live);
    assert!(
        !is_stale_temp_keel_entry(&live.to_string_lossy()),
        "existing dir must not be purged"
    );
    let _ = fs::remove_dir_all(live.parent().unwrap());

    // The real default home is never stale regardless of existence.
    assert!(
        !is_stale_temp_keel_entry("C:\\Users\\me\\.keel"),
        "default home must never match the temp pattern"
    );
    assert!(!is_stale_temp_keel_entry(""), "empty entry is not stale");
}

#[test]
fn default_home_guard_excludes_temp_fixtures() {
    // The guard that stops test installs from touching the user PATH.
    let temp = std::env::temp_dir();
    let fixture = temp
        .join(format!("keel-home-split-{}", std::process::id()))
        .join(".keel");
    assert!(
        crate::runtime::is_standard_keel_home(&fixture),
        "fixture passes the basename check (why the old guard leaked)"
    );
    assert!(
        !crate::runtime::is_default_keel_home(&fixture),
        "fixture must NOT pass the default-home guard"
    );
}

#[test]
fn wire_opencode_lands_under_claude_home_parent_not_env_home() {
    let base = std::env::temp_dir().join(format!("ulw-wire-herm-{}", std::process::id()));
    let claude_home = base.join("owner-home").join(".claude");
    let _ = fs::create_dir_all(&claude_home);
    let repo = create_minimal_layout("wire-opencode-herm-repo");
    let _ = fs::create_dir_all(repo.join("opencode"));
    let _ = fs::write(
        repo.join("opencode").join("keel.ts"),
        "export default async () => ({});\n",
    );

    let summary = maybe_wire_opencode(&repo, &claude_home, true);
    assert!(
        summary.is_some(),
        "standard .claude home must wire OpenCode"
    );

    let owner_config = base
        .join("owner-home")
        .join(".config")
        .join("opencode")
        .join("plugins")
        .join("keel.ts");
    assert!(
        owner_config.is_file(),
        "plugin must land under claude_home's parent"
    );

    let _ = fs::remove_dir_all(&base);
    let _ = fs::remove_dir_all(&repo);
}
#[test]
fn wire_opencode_preserves_custom_plugin_file() {
    let base = std::env::temp_dir().join(format!("ulw-wire-owner-{}", std::process::id()));
    let claude_home = base.join(".claude");
    let repo = create_minimal_layout("wire-opencode-owner-repo");
    let source = repo.join("opencode").join("keel.ts");
    let _ = fs::create_dir_all(source.parent().unwrap());
    fs::write(&source, "export default shippedPlugin;\n").unwrap();
    let target = base
        .join(".config")
        .join("opencode")
        .join("plugins")
        .join("keel.ts");
    let _ = fs::create_dir_all(target.parent().unwrap());
    fs::write(&target, "export default userPlugin;\n").unwrap();

    let summary = maybe_wire_opencode(&repo, &claude_home, true).unwrap();
    assert!(summary.contains("user-customized"));
    assert_eq!(
        fs::read_to_string(&target).unwrap(),
        "export default userPlugin;\n"
    );

    let _ = fs::remove_dir_all(&base);
    let _ = fs::remove_dir_all(&repo);
}
#[test]
fn remove_wired_adapters_preserves_custom_files_and_hooks() {
    let base = std::env::temp_dir().join(format!("ulw-unwire-owner-{}", std::process::id()));
    let claude_home = base.join(".claude");
    let opencode_plugin = base
        .join(".config")
        .join("opencode")
        .join("plugins")
        .join("keel.ts");
    let _ = fs::create_dir_all(opencode_plugin.parent().unwrap());
    fs::write(&opencode_plugin, "export default customPlugin;\n").unwrap();

    let cursor_dir = base.join(".cursor");
    let _ = fs::create_dir_all(cursor_dir.join("hooks"));
    fs::write(
        cursor_dir.join("hooks.json"),
        r#"{"version":1,"hooks":{"preToolUse":[{"command":"custom-hook"} ,{"command":"bash ~/.cursor/hooks/keel-cursor.sh"}]}}"#,
    )
    .unwrap();
    fs::write(
        cursor_dir.join("hooks").join("keel-cursor.sh"),
        "#!/bin/sh\ncustom cursor hook\n",
    )
    .unwrap();

    let pi_extension = base
        .join(".pi")
        .join("agent")
        .join("extensions")
        .join("keel-pi.ts");
    let _ = fs::create_dir_all(pi_extension.parent().unwrap());
    fs::write(&pi_extension, "// keel Pi Agent Extension\n").unwrap();

    let cmdc_mod = base.join(".commandcode").join("mods").join("keel-cmdc.ts");
    let _ = fs::create_dir_all(cmdc_mod.parent().unwrap());
    fs::write(&cmdc_mod, "// keel Command Code (cmdc) Mod\n").unwrap();

    let grok_home = base.join(".grok");
    fs::create_dir_all(grok_home.join("hooks")).unwrap();
    fs::write(
        grok_home.join("hooks").join("keel.json"),
        r#"{"command":"keel hook session-start"}"#,
    )
    .unwrap();
    fs::write(
        grok_home.join("config.toml"),
        "theme = \"dark\"\n\n[mcp_servers.keel]\ncommand = \"keel\"\nargs = [\"mcp\", \"serve\"]\n\n[mcp_servers.user]\ncommand = \"custom\"\n",
    )
    .unwrap();

    let removed = remove_wired_adapters(&claude_home);
    assert!(removed >= 2);
    assert_eq!(
        fs::read_to_string(&opencode_plugin).unwrap(),
        "export default customPlugin;\n"
    );
    assert!(fs::read_to_string(cursor_dir.join("hooks.json"))
        .unwrap()
        .contains("custom-hook"));
    assert!(cursor_dir.join("hooks").join("keel-cursor.sh").is_file());
    assert!(!pi_extension.exists());
    assert!(!cmdc_mod.exists());
    assert!(!grok_home.join("hooks").join("keel.json").exists());
    let grok_config = fs::read_to_string(grok_home.join("config.toml")).unwrap();
    assert!(!grok_config.contains("mcp_servers.keel"));
    assert!(grok_config.contains("mcp_servers.user"));
    assert!(grok_config.contains("theme = \"dark\""));

    let _ = fs::remove_dir_all(&base);
}

#[test]
fn resolve_install_repository_root_prefers_current_directory() {
    let root = create_minimal_layout("resolve-install-repo-root");
    let result = resolve_install_repository_root_from_candidates(&[Some(root.clone())]);
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), root);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn resolve_install_repository_root_falls_back_to_executable_parent() {
    let root = create_minimal_layout("resolve-install-repo-root-fallback");
    let result = resolve_install_repository_root_from_candidates(&[None, Some(root.clone())]);
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), root);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn resolve_install_repository_root_fails_when_no_candidate_is_complete() {
    let result = resolve_install_repository_root_from_candidates(&[None, None]);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Repository root not found"));
}

#[test]
fn remove_managed_block_removes_block_from_config() {
    let text =
        "key=value\n# BEGIN MANAGED BLOCK (123)\nold=data\n# END MANAGED BLOCK\nother=line\n";
    let result = remove_managed_block(text);
    assert_eq!(result, "key=value\nother=line");
}

#[test]
fn remove_managed_block_preserves_text_without_block() {
    let text = "key=value\nother=line\n";
    let result = remove_managed_block(text);
    // lines().join("\n") drops the trailing newline; that is expected behavior
    assert_eq!(result, "key=value\nother=line");
}

#[test]
fn repo_version_prefers_meaningful_build_version() {
    let root = create_minimal_layout("repo-version-build");
    let result = repo_version_for_source("1.2.3", &root);
    assert_eq!(result, "1.2.3");
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn repo_version_falls_back_to_git_short_head() {
    let root = create_minimal_layout("repo-version-git");
    let result = repo_version_for_source("dev", &root);
    assert!(!result.is_empty());
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn repo_version_recovers_from_installed_metadata() {
    let metadata = "repo_version=1.2.3\nmanager_version=dev-abc123\n";
    assert_eq!(
        repo_version_from_metadata_or_build(metadata, "dev").as_deref(),
        Some("1.2.3")
    );
}

#[test]
fn repo_version_recovers_bootstrap_commit_from_installed_metadata() {
    let metadata = "repo_version=unknown\nmanager_version=bootstrap-8c0eb1cf6c20\n";
    assert_eq!(
        repo_version_from_metadata_or_build(metadata, "dev").as_deref(),
        Some("8c0eb1c")
    );
}

#[test]
fn install_metadata_records_a_durable_checkout_source() {
    let (repo, keel_home) = unique_paths("metadata-checkout-source");
    stage_minimal_layout(&repo);
    fs::create_dir_all(&keel_home).unwrap();
    write_install_metadata("dev", &repo, &keel_home).unwrap();
    let metadata =
        fs::read_to_string(super::super::verify::install_metadata_path(&keel_home)).unwrap();
    assert_eq!(
        super::super::verify::metadata_value(&metadata, "source_kind"),
        Some("checkout")
    );
    assert_eq!(
        super::super::verify::metadata_value(&metadata, "source_root"),
        Some(display_path(&repo).as_str())
    );
    let _ = fs::remove_dir_all(repo);
    let _ = fs::remove_dir_all(keel_home);
}

#[test]
fn release_install_metadata_caches_source_before_the_extract_tree_is_deleted() {
    let (repo, keel_home) = unique_paths("metadata-release-source");
    stage_minimal_layout(&repo);
    fs::create_dir_all(&keel_home).unwrap();
    fs::write(
        repo.join("keel-release-manifest.json"),
        r#"{"repository_slug":"UntaDotMy/keel","release_tag":"v1","build_version":"1"}"#,
    )
    .unwrap();
    write_install_metadata("1", &repo, &keel_home).unwrap();
    let metadata =
        fs::read_to_string(super::super::verify::install_metadata_path(&keel_home)).unwrap();
    assert_eq!(
        super::super::verify::metadata_value(&metadata, "source_kind"),
        Some("release")
    );
    assert_eq!(
        super::super::verify::metadata_value(&metadata, "repository_slug"),
        Some("UntaDotMy/keel")
    );
    let cached =
        PathBuf::from(super::super::verify::metadata_value(&metadata, "source_root").unwrap());
    assert!(cached.join("AGENTS.md").is_file());
    assert!(cached.join("reviewer/SKILL.md").is_file());
    fs::remove_dir_all(&repo).unwrap();
    assert_eq!(
        super::super::verify::resolve_manager_repository_root("", &keel_home).unwrap(),
        cached
    );
    let _ = fs::remove_dir_all(keel_home);
}

#[test]
fn publish_native_executable_skips_stale_identical_release_and_uses_debug() {
    let (repo, claude_home) = unique_paths("publish-skip-stale-release");
    fs::create_dir_all(&claude_home).unwrap();
    let release_dir = repo.join("target").join("release");
    let debug_dir = repo.join("target").join("debug");
    fs::create_dir_all(&release_dir).unwrap();
    fs::create_dir_all(&debug_dir).unwrap();
    fs::write(
        release_dir.join(executable_file_name()),
        b"old-binary-contents",
    )
    .unwrap();
    fs::write(debug_dir.join(executable_file_name()), b"fresh-debug").unwrap();
    let installed = installed_executable_path(&claude_home);
    fs::write(&installed, b"old-binary-contents").unwrap();
    let published = publish_native_executable(&repo, &claude_home).unwrap();
    assert!(
        published,
        "stale identical release must be skipped so debug can refresh PATH/MCP"
    );
    assert_eq!(fs::read(&installed).unwrap(), b"fresh-debug");
    let _ = fs::remove_dir_all(&repo);
    let _ = fs::remove_dir_all(&claude_home);
}

#[test]
fn publish_native_executable_falls_back_to_debug_when_no_release() {
    let (repo, claude_home) = unique_paths("publish-debug-fallback");
    fs::create_dir_all(&claude_home).unwrap();
    let debug_dir = repo.join("target").join("debug");
    fs::create_dir_all(&debug_dir).unwrap();
    fs::write(debug_dir.join(executable_file_name()), b"debug-build").unwrap();
    let installed = installed_executable_path(&claude_home);
    fs::write(&installed, b"old-binary-contents").unwrap();
    let published = publish_native_executable(&repo, &claude_home).unwrap();
    assert!(
        published,
        "debug artifact must publish when release is absent"
    );
    assert_eq!(fs::read(&installed).unwrap(), b"debug-build");
    let _ = fs::remove_dir_all(&repo);
    let _ = fs::remove_dir_all(&claude_home);
}

#[test]
fn publish_native_executable_prefers_release_over_debug() {
    let (repo, claude_home) = unique_paths("publish-release-wins");
    fs::create_dir_all(&claude_home).unwrap();
    let release_dir = repo.join("target").join("release");
    let debug_dir = repo.join("target").join("debug");
    fs::create_dir_all(&release_dir).unwrap();
    fs::create_dir_all(&debug_dir).unwrap();
    fs::write(release_dir.join(executable_file_name()), b"release-build").unwrap();
    fs::write(debug_dir.join(executable_file_name()), b"debug-build").unwrap();
    let installed = installed_executable_path(&claude_home);
    let published = publish_native_executable(&repo, &claude_home).unwrap();
    assert!(published);
    assert_eq!(fs::read(&installed).unwrap(), b"release-build");
    let _ = fs::remove_dir_all(&repo);
    let _ = fs::remove_dir_all(&claude_home);
}

#[test]
fn publish_native_executable_falls_back_to_bundle_root_binary() {
    // Release archives stage the binary at `<bundle>/keel.exe`; the fallback
    // handles that layout when no Cargo target artifact exists.
    let (bundle, claude_home) = unique_paths("publish-bundle-root");
    fs::create_dir_all(&bundle).unwrap();
    fs::create_dir_all(&claude_home).unwrap();

    let bundle_executable = bundle.join(executable_file_name());
    fs::write(&bundle_executable, b"new-binary-contents").unwrap();

    let installed = installed_executable_path(&claude_home);
    fs::write(&installed, b"old-binary-contents").unwrap();

    let published = publish_native_executable(&bundle, &claude_home).unwrap();
    assert!(
        published,
        "publish must report true when copying from bundle root"
    );
    assert_eq!(fs::read(&installed).unwrap(), b"new-binary-contents");

    let _ = fs::remove_dir_all(&bundle);
    let _ = fs::remove_dir_all(&claude_home);
}

#[test]
fn publish_native_executable_prefers_cargo_built_over_bundle_root() {
    // When both layouts exist (a developer running `install` from a
    let (repo, claude_home) = unique_paths("publish-prefer-cargo");
    fs::create_dir_all(&claude_home).unwrap();

    let target = detect_current_target().unwrap();
    let cargo_dir = repo
        .join("target")
        .join(target.directory_name())
        .join("release");
    fs::create_dir_all(&cargo_dir).unwrap();
    fs::write(cargo_dir.join(executable_file_name()), b"cargo-built").unwrap();
    fs::write(repo.join(executable_file_name()), b"bundle-root").unwrap();

    let installed = installed_executable_path(&claude_home);
    let published = publish_native_executable(&repo, &claude_home).unwrap();
    assert!(published);
    assert_eq!(fs::read(&installed).unwrap(), b"cargo-built");

    let _ = fs::remove_dir_all(&repo);
    let _ = fs::remove_dir_all(&claude_home);
}

#[test]
fn publish_native_executable_picks_up_cargo_host_default_layout() {
    // Plain `cargo build --release` writes the host-default target layout.
    let (repo, claude_home) = unique_paths("publish-host-default");
    fs::create_dir_all(&claude_home).unwrap();

    let host_default_dir = repo.join("target").join("release");
    fs::create_dir_all(&host_default_dir).unwrap();
    fs::write(
        host_default_dir.join(executable_file_name()),
        b"host-default-build",
    )
    .unwrap();

    let installed = installed_executable_path(&claude_home);
    fs::write(&installed, b"old-binary-contents").unwrap();

    let published = publish_native_executable(&repo, &claude_home).unwrap();
    assert!(
        published,
        "publish must report true when copying from target/release host-default layout"
    );
    assert_eq!(fs::read(&installed).unwrap(), b"host-default-build");

    let _ = fs::remove_dir_all(&repo);
    let _ = fs::remove_dir_all(&claude_home);
}

#[test]
fn publish_native_executable_prefers_cargo_targeted_over_host_default() {
    // CI / cross-compile runs use `cargo build --release --target <triple>`
    let (repo, claude_home) = unique_paths("publish-prefer-targeted");
    fs::create_dir_all(&claude_home).unwrap();

    let target = detect_current_target().unwrap();
    let targeted_dir = repo
        .join("target")
        .join(target.directory_name())
        .join("release");
    let host_default_dir = repo.join("target").join("release");
    fs::create_dir_all(&targeted_dir).unwrap();
    fs::create_dir_all(&host_default_dir).unwrap();
    fs::write(targeted_dir.join(executable_file_name()), b"cargo-targeted").unwrap();
    fs::write(
        host_default_dir.join(executable_file_name()),
        b"host-default",
    )
    .unwrap();

    let installed = installed_executable_path(&claude_home);
    let published = publish_native_executable(&repo, &claude_home).unwrap();
    assert!(published);
    assert_eq!(fs::read(&installed).unwrap(), b"cargo-targeted");

    let _ = fs::remove_dir_all(&repo);
    let _ = fs::remove_dir_all(&claude_home);
}

#[test]
fn publish_native_executable_prefers_host_default_over_bundle_root() {
    // When a Cargo-direct workspace also has a leftover bundle-root
    let (repo, claude_home) = unique_paths("publish-host-over-bundle");
    fs::create_dir_all(&claude_home).unwrap();

    let host_default_dir = repo.join("target").join("release");
    fs::create_dir_all(&host_default_dir).unwrap();
    fs::write(
        host_default_dir.join(executable_file_name()),
        b"host-default-fresh",
    )
    .unwrap();
    fs::write(repo.join(executable_file_name()), b"bundle-leftover").unwrap();

    let installed = installed_executable_path(&claude_home);
    let published = publish_native_executable(&repo, &claude_home).unwrap();
    assert!(published);
    assert_eq!(fs::read(&installed).unwrap(), b"host-default-fresh");

    let _ = fs::remove_dir_all(&repo);
    let _ = fs::remove_dir_all(&claude_home);
}

#[test]
fn replace_executable_in_place_overwrites_existing_target() {
    // The core of the Windows re-install fix: replacing an existing
    let (dir, _) = unique_paths("replace-in-place");
    fs::create_dir_all(&dir).unwrap();
    let target = dir.join(executable_file_name());
    fs::write(&target, b"old-installed-bytes").unwrap();
    let temp = sibling_temp_path(&target);
    fs::write(&temp, b"freshly-staged-bytes").unwrap();

    replace_executable_in_place(&temp, &target).unwrap();

    assert_eq!(fs::read(&target).unwrap(), b"freshly-staged-bytes");
    assert!(!temp.exists(), "staged .new temp must be consumed");
    // No `.stale-*` orphan should survive a successful replace.
    let orphans = find_executable_orphans(&dir);
    assert!(
        orphans.is_empty(),
        "no orphans expected after a clean replace, found {orphans:?}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn replace_executable_in_place_creates_target_when_absent() {
    // First-ever install: there is no existing binary to move aside, so
    // the staged temp is renamed straight into the target name.
    let (dir, _) = unique_paths("replace-fresh");
    fs::create_dir_all(&dir).unwrap();
    let target = dir.join(executable_file_name());
    let temp = sibling_temp_path(&target);
    fs::write(&temp, b"first-install-bytes").unwrap();

    replace_executable_in_place(&temp, &target).unwrap();

    assert_eq!(fs::read(&target).unwrap(), b"first-install-bytes");
    assert!(!temp.exists(), "staged .new temp must be consumed");

    let _ = fs::remove_dir_all(&dir);
}

fn create_minimal_layout(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("reviewer")).unwrap();
    fs::write(root.join("AGENTS.md"), "").unwrap();
    fs::write(root.join("README.md"), "").unwrap();
    fs::write(root.join("00-skill-routing-and-escalation.md"), "").unwrap();
    fs::write(root.join("reviewer").join("SKILL.md"), "").unwrap();
    root
}

fn stage_minimal_layout(root: &Path) {
    fs::create_dir_all(root.join("reviewer")).unwrap();
    fs::write(root.join("AGENTS.md"), "").unwrap();
    fs::write(root.join("README.md"), "").unwrap();
    fs::write(root.join("00-skill-routing-and-escalation.md"), "").unwrap();
    fs::write(root.join("reviewer").join("SKILL.md"), "").unwrap();
}

fn unique_paths(name: &str) -> (PathBuf, PathBuf) {
    let suffix = format!(
        "{}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
        name,
    );
    let repo = std::env::temp_dir().join(format!("delta-repo-{suffix}"));
    let home = std::env::temp_dir().join(format!("delta-home-{suffix}"));
    let _ = fs::remove_dir_all(&repo);
    let _ = fs::remove_dir_all(&home);
    (repo, home)
}

fn write_skill_with_reference(root: &Path, skill: &str, reference_file: &str) {
    let skill_dir = root.join(skill);
    let references_dir = skill_dir.join("references");
    fs::create_dir_all(&references_dir).unwrap();
    fs::write(skill_dir.join("SKILL.md"), format!("# {skill}\n")).unwrap();
    fs::write(references_dir.join(reference_file), "reference body\n").unwrap();
}

fn seed_repo(root: &Path) {
    fs::create_dir_all(root.join("docs")).unwrap();
    fs::write(root.join("AGENTS.md"), "agents\n").unwrap();
    fs::write(root.join("README.md"), "readme\n").unwrap();
    fs::write(root.join("00-skill-routing-and-escalation.md"), "routing\n").unwrap();
    fs::write(
        root.join("docs/runtime-guardrails-and-memory-protocols.md"),
        "guardrails\n",
    )
    .unwrap();
    fs::write(
        root.join("docs/open-source-memory-patterns.md"),
        "patterns\n",
    )
    .unwrap();
    fs::write(root.join("docs/security-audit-status.md"), "audit\n").unwrap();
}

#[test]
fn delta_installer_removes_renamed_reference_file() {
    let (repo, home) = unique_paths("rename");
    seed_repo(&repo);
    write_skill_with_reference(&repo, "reviewer", "10-old.md");
    install_from_paths("dev", &repo, &home, &InstallOverrides::default(), true).unwrap();
    let old_file = home.join("skills/reviewer/references/10-old.md");
    assert!(
        old_file.is_file(),
        "first install should have written reference"
    );

    fs::remove_file(repo.join("reviewer/references/10-old.md")).unwrap();
    fs::write(
        repo.join("reviewer/references/11-new.md"),
        "reference body\n",
    )
    .unwrap();
    install_from_paths("dev", &repo, &home, &InstallOverrides::default(), true).unwrap();

    assert!(
        !old_file.is_file(),
        "renamed reference file must be removed from claude home"
    );
    assert!(
        home.join("skills/reviewer/references/11-new.md").is_file(),
        "new reference file must be present"
    );
    let _ = fs::remove_dir_all(&repo);
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn delta_installer_removes_orphaned_skill_directory() {
    let (repo, home) = unique_paths("orphan-skill");
    seed_repo(&repo);
    write_skill_with_reference(&repo, "reviewer", "10-r.md");
    write_skill_with_reference(&repo, "git-expert", "10-g.md");
    install_from_paths("dev", &repo, &home, &InstallOverrides::default(), true).unwrap();
    let orphan_dir = home.join("skills/git-expert");
    assert!(orphan_dir.is_dir(), "second skill must install");

    fs::remove_dir_all(repo.join("git-expert")).unwrap();
    install_from_paths("dev", &repo, &home, &InstallOverrides::default(), true).unwrap();

    assert!(
        !orphan_dir.exists(),
        "removed skill must be cleaned up entirely"
    );
    assert!(
        home.join("skills/reviewer/SKILL.md").is_file(),
        "remaining skill must stay in place"
    );
    let _ = fs::remove_dir_all(&repo);
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn delta_installer_preserves_unchanged_files_across_installs() {
    let (repo, home) = unique_paths("unchanged");
    seed_repo(&repo);
    write_skill_with_reference(&repo, "reviewer", "10-r.md");
    install_from_paths("dev", &repo, &home, &InstallOverrides::default(), true).unwrap();

    let target = home.join("skills/reviewer/references/10-r.md");
    let mtime_before = fs::metadata(&target).unwrap().modified().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(50));
    let summary =
        install_from_paths("dev", &repo, &home, &InstallOverrides::default(), true).unwrap();
    let mtime_after = fs::metadata(&target).unwrap().modified().unwrap();

    assert_eq!(
        mtime_before, mtime_after,
        "unchanged file must not be rewritten on second install"
    );
    assert_eq!(summary.removed_stale_files, 0);
    let _ = fs::remove_dir_all(&repo);
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn delta_installer_first_install_without_inventory_creates_no_false_orphans() {
    let (repo, home) = unique_paths("first-install");
    seed_repo(&repo);
    write_skill_with_reference(&repo, "reviewer", "10-r.md");
    let summary =
        install_from_paths("dev", &repo, &home, &InstallOverrides::default(), true).unwrap();

    assert_eq!(
        summary.removed_stale_files, 0,
        "first install must not delete anything"
    );
    assert!(home.join("skills/reviewer/SKILL.md").is_file());
    assert!(
        managed_files_inventory_path(&home).is_file(),
        "per-file inventory must be written"
    );
    let _ = fs::remove_dir_all(&repo);
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn install_into_standard_home_writes_lifecycle_hooks() {
    // Standard-home installs exercise the lifecycle hook write branch.
    let suffix = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
    );
    let parent = std::env::temp_dir().join(format!("hookhome-{suffix}"));
    let repo = std::env::temp_dir().join(format!("hookrepo-{suffix}"));
    let home = parent.join(".claude");
    let _ = fs::remove_dir_all(&parent);
    let _ = fs::remove_dir_all(&repo);
    seed_repo(&repo);
    write_skill_with_reference(&repo, "reviewer", "10-r.md");

    let summary =
        install_from_paths("dev", &repo, &home, &InstallOverrides::default(), true).unwrap();

    // Summary reports the install, not a skip.
    let status = summary
        .hooks_installation
        .expect("standard .claude home must attempt hook install");
    assert!(
        status.starts_with("installed at"),
        "expected an install, got: {status}"
    );

    // settings.json exists and carries managed lifecycle stanzas pointing at
    // the published binary.
    let settings_path = home.join(crate::hooks::claude::SETTINGS_FILE_NAME);
    assert!(settings_path.is_file(), "settings.json must be written");
    let text = fs::read_to_string(&settings_path).unwrap();
    let document: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert!(
        document["hooks"]["SessionStart"].is_array(),
        "SessionStart hook stanza must be present"
    );
    assert!(
        document["hooks"]["UserPromptSubmit"].is_array(),
        "UserPromptSubmit hook stanza must be present"
    );
    // The budget knob folded in by build_hooks_payload lands at the new default.
    assert_eq!(
        document
            .get("skillListingBudgetFraction")
            .and_then(serde_json::Value::as_f64),
        Some(0.06),
    );

    // Re-install is idempotent and preserves an unrelated user key.
    let mut reparsed: serde_json::Value = serde_json::from_str(&text).unwrap();
    reparsed["userCustomKey"] = serde_json::json!("keep-me");
    fs::write(
        &settings_path,
        serde_json::to_string_pretty(&reparsed).unwrap(),
    )
    .unwrap();
    install_from_paths("dev", &repo, &home, &InstallOverrides::default(), true).unwrap();
    let after: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&settings_path).unwrap()).unwrap();
    assert_eq!(
        after["userCustomKey"], "keep-me",
        "unrelated user keys must survive a re-install"
    );
    assert!(
        after["hooks"]["SessionStart"].is_array(),
        "managed hooks must still be present after re-install"
    );

    let _ = fs::remove_dir_all(&parent);
    let _ = fs::remove_dir_all(&repo);
}

#[test]
fn merge_managed_claude_md_into_empty_prepends_block() {
    let block = managed_claude_md_block();
    let merged = merge_managed_claude_md("", &block);
    assert!(merged.contains(MANAGED_CLAUDE_MD_BEGIN));
    assert!(merged.contains(MANAGED_CLAUDE_MD_END));
    assert!(merged.contains("Iron Law"));
}

#[test]
fn merge_managed_claude_md_preserves_user_content() {
    // A user with their own global CLAUDE.md must keep it; the managed block
    // is prepended, the user's prose survives below.
    let user = "# My personal notes\n\nAlways use tabs, never spaces.\n";
    let block = managed_claude_md_block();
    let merged = merge_managed_claude_md(user, &block);
    assert!(merged.contains("My personal notes"));
    assert!(merged.contains("Always use tabs, never spaces."));
    assert!(merged.contains(MANAGED_CLAUDE_MD_BEGIN));
    // Managed block comes first so the contract is read before user prose.
    assert!(
        merged.find(MANAGED_CLAUDE_MD_BEGIN).unwrap() < merged.find("My personal notes").unwrap()
    );
}

#[test]
fn merge_managed_claude_md_replaces_existing_block_in_place() {
    // A re-install must refresh the managed region without duplicating it or
    // disturbing user content above and below.
    let user_above = "# Top notes\n\n";
    let stale_block =
        format!("{MANAGED_CLAUDE_MD_BEGIN}\nOLD STALE CONTRACT\n{MANAGED_CLAUDE_MD_END}");
    let user_below = "\n\n# Bottom notes\n";
    let existing = format!("{user_above}{stale_block}{user_below}");
    let merged = merge_managed_claude_md(&existing, &managed_claude_md_block());

    assert!(merged.contains("Top notes"));
    assert!(merged.contains("Bottom notes"));
    assert!(
        !merged.contains("OLD STALE CONTRACT"),
        "stale managed content must be replaced"
    );
    assert!(merged.contains("Iron Law"));
    // Exactly one managed region remains.
    assert_eq!(merged.matches(MANAGED_CLAUDE_MD_BEGIN).count(), 1);
    assert_eq!(merged.matches(MANAGED_CLAUDE_MD_END).count(), 1);
}

#[test]
fn merge_managed_claude_md_is_idempotent() {
    let block = managed_claude_md_block();
    let once = merge_managed_claude_md("", &block);
    let twice = merge_managed_claude_md(&once, &block);
    assert_eq!(once, twice, "re-merging an already-current file is a no-op");
}

#[test]
fn strip_managed_claude_md_removes_block_keeps_user_content() {
    let user_above = "# Top notes\n";
    let block = managed_claude_md_block();
    let user_below = "# Bottom notes\n";
    let existing = format!("{user_above}\n\n{block}\n\n{user_below}");
    let stripped = strip_managed_claude_md(&existing);
    assert!(stripped.contains("Top notes"));
    assert!(stripped.contains("Bottom notes"));
    assert!(!stripped.contains(MANAGED_CLAUDE_MD_BEGIN));
    assert!(!stripped.contains("Iron Law"));
}

#[test]
fn strip_managed_claude_md_all_managed_collapses_to_empty() {
    // A file that is ONLY the block must strip to empty so the caller deletes it.
    let only_block = format!("{}\n", managed_claude_md_block());
    let stripped = strip_managed_claude_md(&only_block);
    assert!(stripped.trim().is_empty());
}

#[test]
fn install_into_standard_home_writes_user_claude_md() {
    // End-to-end: a real `.claude`-named home must get ~/.claude/CLAUDE.md
    let suffix = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
    );
    let parent = std::env::temp_dir().join(format!("cmdhome-{suffix}"));
    let repo = std::env::temp_dir().join(format!("cmdrepo-{suffix}"));
    let home = parent.join(".claude");
    let _ = fs::remove_dir_all(&parent);
    let _ = fs::remove_dir_all(&repo);
    seed_repo(&repo);
    write_skill_with_reference(&repo, "reviewer", "10-r.md");

    // Pre-seed a user-authored CLAUDE.md to prove preservation.
    fs::create_dir_all(&home).unwrap();
    fs::write(
        home.join("CLAUDE.md"),
        "# My global prefs\n\nUse 2-space indent.\n",
    )
    .unwrap();

    let summary =
        install_from_paths("dev", &repo, &home, &InstallOverrides::default(), true).unwrap();
    let status = summary
        .user_claude_md
        .expect("standard home must write user CLAUDE.md");
    assert!(status.starts_with("written to"), "got: {status}");

    let claude_md = home.join("CLAUDE.md");
    let text = fs::read_to_string(&claude_md).unwrap();
    assert!(
        text.contains("Iron Law"),
        "managed contract must be present"
    );
    assert!(
        text.contains("keel MCP tools"),
        "MCP imperative must be present"
    );
    assert!(
        text.contains("Use 2-space indent."),
        "user content must be preserved"
    );

    // Re-install is idempotent.
    let resummary =
        install_from_paths("dev", &repo, &home, &InstallOverrides::default(), true).unwrap();
    assert_eq!(
        resummary.user_claude_md.as_deref(),
        Some("already current"),
        "second install must detect the block is already current"
    );

    // Uninstall strips the managed block but keeps user content.
    let mut out = Vec::new();
    let mut err = Vec::new();
    let code = run_uninstall_command(
        &["--claude-home".to_string(), display_path(&home)],
        &mut out,
        &mut err,
    );
    assert_eq!(
        code,
        0,
        "uninstall stderr: {}",
        String::from_utf8_lossy(&err)
    );
    let after = fs::read_to_string(&claude_md).expect("user CLAUDE.md must survive uninstall");
    assert!(
        !after.contains("Iron Law"),
        "managed block must be stripped"
    );
    assert!(
        after.contains("Use 2-space indent."),
        "user content must survive uninstall"
    );

    let _ = fs::remove_dir_all(&parent);
    let _ = fs::remove_dir_all(&repo);
}

#[test]
fn install_copies_shared_resources_alongside_skills() {
    // SKILL.md files reference _shared/common-discipline.md via relative
    let (repo, home) = unique_paths("shared-resources");
    seed_repo(&repo);
    write_skill_with_reference(&repo, "reviewer", "10-r.md");
    let shared_dir = repo.join("_shared");
    fs::create_dir_all(shared_dir.join("nested")).unwrap();
    fs::write(shared_dir.join("common-discipline.md"), "discipline body\n").unwrap();
    fs::write(shared_dir.join("nested/extra.md"), "nested body\n").unwrap();

    install_from_paths("dev", &repo, &home, &InstallOverrides::default(), true).unwrap();

    let installed_shared = home.join("skills/_shared");
    assert!(
        installed_shared.join("common-discipline.md").is_file(),
        "top-level shared file must be installed alongside skills"
    );
    assert!(
        installed_shared.join("nested/extra.md").is_file(),
        "nested shared file must be installed alongside skills"
    );

    // Rename and reinstall ; the previously installed file should be
    // cleaned up exactly like a renamed skill reference.
    fs::remove_file(shared_dir.join("common-discipline.md")).unwrap();
    fs::write(
        shared_dir.join("common-discipline-v2.md"),
        "discipline body\n",
    )
    .unwrap();
    install_from_paths("dev", &repo, &home, &InstallOverrides::default(), true).unwrap();

    assert!(
        !installed_shared.join("common-discipline.md").is_file(),
        "renamed shared file must be removed from claude home"
    );
    assert!(
        installed_shared.join("common-discipline-v2.md").is_file(),
        "new shared file must be installed"
    );
    let _ = fs::remove_dir_all(&repo);
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn reinstall_is_zero_churn_when_nothing_changed() {
    // Delta-patch guarantee: a re-install with an unchanged repo must report
    let (repo, home) = unique_paths("zero-churn");
    seed_repo(&repo);
    write_skill_with_reference(&repo, "reviewer", "10-r.md");
    let shared_dir = repo.join("_shared");
    fs::create_dir_all(&shared_dir).unwrap();
    fs::write(shared_dir.join("common-discipline.md"), "discipline body\n").unwrap();

    let first =
        install_from_paths("dev", &repo, &home, &InstallOverrides::default(), true).unwrap();
    assert!(
        first.synced_shared_resources >= 1,
        "first install must actually write the shared resource"
    );

    let second =
        install_from_paths("dev", &repo, &home, &InstallOverrides::default(), true).unwrap();
    assert_eq!(second.synced_skills, 0, "no skill churn on no-op reinstall");
    assert_eq!(second.synced_agents, 0, "no agent churn on no-op reinstall");
    assert_eq!(
        second.synced_subagent_definitions, 0,
        "no subagent churn on no-op reinstall"
    );
    assert_eq!(
        second.synced_commands, 0,
        "no command churn on no-op reinstall"
    );
    assert_eq!(
        second.synced_root_files, 0,
        "no root-file churn on no-op reinstall"
    );
    assert_eq!(
        second.synced_shared_resources, 0,
        "no shared-resource churn on no-op reinstall (the fixed bug)"
    );
    assert_eq!(
        second.removed_stale_files, 0,
        "nothing stale to remove on no-op reinstall"
    );
    let _ = fs::remove_dir_all(&repo);
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn install_removes_shared_resource_directory_when_dropped_from_repo() {
    // When the entire `_shared/` directory is removed from the repo
    let (repo, home) = unique_paths("shared-dropped");
    seed_repo(&repo);
    write_skill_with_reference(&repo, "reviewer", "10-r.md");
    let shared_dir = repo.join("_shared");
    fs::create_dir_all(&shared_dir).unwrap();
    fs::write(shared_dir.join("common-discipline.md"), "discipline body\n").unwrap();

    install_from_paths("dev", &repo, &home, &InstallOverrides::default(), true).unwrap();
    let installed_shared = home.join("skills/_shared");
    assert!(installed_shared.is_dir(), "first install seeds shared dir");

    // Drop the whole _shared directory from the repo and reinstall.
    fs::remove_dir_all(&shared_dir).unwrap();
    install_from_paths("dev", &repo, &home, &InstallOverrides::default(), true).unwrap();

    assert!(
        !installed_shared.exists(),
        "removed shared directory must be cleaned up entirely (not just its files)"
    );
    assert!(
        home.join("skills/reviewer/SKILL.md").is_file(),
        "untouched skill must stay in place"
    );
    let _ = fs::remove_dir_all(&repo);
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn remove_executable_orphans_deletes_legacy_stale_siblings() {
    // Pre-`33bf860` installer used a `.stale-<timestamp>` naming scheme
    let (_repo, home) = unique_paths("orphan-stale");
    fs::create_dir_all(&home).unwrap();
    let executable = installed_executable_path(&home);
    if let Some(parent) = executable.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(&executable, b"installed").unwrap();
    let stale_a = executable.with_file_name(format!("{}.stale-1778857819", executable_file_name()));
    let stale_b = executable.with_file_name(format!("{}.stale-1234567890", executable_file_name()));
    fs::write(&stale_a, b"legacy").unwrap();
    fs::write(&stale_b, b"legacy").unwrap();

    let removed = remove_executable_orphans(&home).unwrap();

    assert_eq!(removed, 2, "both legacy stale siblings must be cleaned up");
    assert!(!stale_a.is_file());
    assert!(!stale_b.is_file());
    assert!(
        executable.is_file(),
        "installed executable must not be touched"
    );
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn remove_executable_orphans_skips_fresh_dot_new_to_avoid_racing_install() {
    // atomic_copy_executable writes to a `.new` sibling before renaming
    let (_repo, home) = unique_paths("orphan-new-fresh");
    fs::create_dir_all(&home).unwrap();
    let executable = installed_executable_path(&home);
    if let Some(parent) = executable.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(&executable, b"installed").unwrap();
    // Sleep so the .new mtime is strictly after the installed mtime.
    std::thread::sleep(std::time::Duration::from_millis(50));
    let dot_new = executable.with_file_name(format!("{}.new", executable_file_name()));
    fs::write(&dot_new, b"in-flight").unwrap();

    let removed = remove_executable_orphans(&home).unwrap();

    assert_eq!(
        removed, 0,
        "fresh .new must not be deleted — would race a concurrent install"
    );
    assert!(dot_new.is_file());
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn remove_executable_orphans_deletes_abandoned_dot_new() {
    // A `.new` older than the installed executable is a crash artifact ;
    let (_repo, home) = unique_paths("orphan-new-stale");
    fs::create_dir_all(&home).unwrap();
    let executable = installed_executable_path(&home);
    if let Some(parent) = executable.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    let dot_new = executable.with_file_name(format!("{}.new", executable_file_name()));
    // Write the orphan first, then sleep, then write the installed
    // executable so it is strictly newer.
    fs::write(&dot_new, b"crash-leftover").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(50));
    fs::write(&executable, b"installed").unwrap();

    let removed = remove_executable_orphans(&home).unwrap();

    assert_eq!(removed, 1, "abandoned .new must be cleaned up");
    assert!(!dot_new.is_file());
    assert!(executable.is_file());
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn install_copies_subagent_definitions_into_user_global_agents_directory() {
    // Without this step, the project-scoped `.claude/agents/<name>.md`
    let (repo, home) = unique_paths("subagent-defs");
    seed_repo(&repo);
    write_skill_with_reference(&repo, "reviewer", "10-r.md");
    let agents_source = repo.join(".claude").join("agents");
    fs::create_dir_all(&agents_source).unwrap();
    fs::write(
        agents_source.join("reviewer.md"),
        "---\nname: reviewer\n---\nbody\n",
    )
    .unwrap();
    fs::write(
        agents_source.join("git-expert.md"),
        "---\nname: git-expert\n---\nbody\n",
    )
    .unwrap();

    let summary =
        install_from_paths("dev", &repo, &home, &InstallOverrides::default(), true).unwrap();
    assert_eq!(
        summary.synced_subagent_definitions, 2,
        "first install must report two newly written subagent definitions"
    );

    let installed_reviewer = home.join("agents/reviewer.md");
    let installed_git = home.join("agents/git-expert.md");
    assert!(
        installed_reviewer.is_file(),
        "reviewer subagent definition must land in user-global agents dir"
    );
    assert!(
        installed_git.is_file(),
        "git-expert subagent definition must land in user-global agents dir"
    );

    // Reinstall with no source change must report zero writes.
    let summary =
        install_from_paths("dev", &repo, &home, &InstallOverrides::default(), true).unwrap();
    assert_eq!(
        summary.synced_subagent_definitions, 0,
        "no-op reinstall must not rewrite unchanged subagent definitions"
    );

    // Rename one definition and reinstall ; the old file must be cleaned
    // up by the same per-file orphan sweep that handles skill references.
    fs::remove_file(agents_source.join("git-expert.md")).unwrap();
    fs::write(
        agents_source.join("git-helper.md"),
        "---\nname: git-helper\n---\nbody\n",
    )
    .unwrap();
    install_from_paths("dev", &repo, &home, &InstallOverrides::default(), true).unwrap();

    assert!(
        !installed_git.is_file(),
        "renamed subagent definition must be removed from claude home"
    );
    assert!(
        home.join("agents/git-helper.md").is_file(),
        "new subagent definition must be installed"
    );
    let _ = fs::remove_dir_all(&repo);
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn install_copies_slash_commands_into_user_global_commands_directory() {
    // Custom slash commands live in `<repo>/commands/*.md` and ship through
    let (repo, home) = unique_paths("slash-commands");
    seed_repo(&repo);
    write_skill_with_reference(&repo, "reviewer", "10-r.md");
    let commands_source = repo.join("commands");
    fs::create_dir_all(&commands_source).unwrap();
    fs::write(
        commands_source.join("workflow.md"),
        "---\ndescription: drive a workflow\n---\nbody\n",
    )
    .unwrap();
    fs::write(
        commands_source.join("recall.md"),
        "---\ndescription: search memory\n---\nbody\n",
    )
    .unwrap();
    // A non-markdown sibling must be ignored, matching the .md-only filter.
    fs::write(commands_source.join("notes.txt"), "ignore me\n").unwrap();

    let summary =
        install_from_paths("dev", &repo, &home, &InstallOverrides::default(), true).unwrap();
    assert_eq!(
        summary.synced_commands, 2,
        "first install must report two newly written command definitions"
    );

    let installed_workflow = home.join("commands/workflow.md");
    let installed_recall = home.join("commands/recall.md");
    assert!(
        installed_workflow.is_file(),
        "workflow command must land in user-global commands dir"
    );
    assert!(
        installed_recall.is_file(),
        "recall command must land in user-global commands dir"
    );
    assert!(
        !home.join("commands/notes.txt").is_file(),
        "non-markdown files must not be copied"
    );

    // Reinstall with no source change must report zero writes.
    let summary =
        install_from_paths("dev", &repo, &home, &InstallOverrides::default(), true).unwrap();
    assert_eq!(
        summary.synced_commands, 0,
        "no-op reinstall must not rewrite unchanged command definitions"
    );

    // Rename one command and reinstall ; the old file must be cleaned up by
    // the same per-file orphan sweep that handles skill references.
    fs::remove_file(commands_source.join("recall.md")).unwrap();
    fs::write(
        commands_source.join("gain.md"),
        "---\ndescription: report savings\n---\nbody\n",
    )
    .unwrap();
    install_from_paths("dev", &repo, &home, &InstallOverrides::default(), true).unwrap();

    assert!(
        !installed_recall.is_file(),
        "renamed command must be removed from claude home"
    );
    assert!(
        home.join("commands/gain.md").is_file(),
        "new command must be installed"
    );
    let _ = fs::remove_dir_all(&repo);
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn install_never_deletes_protected_user_data_even_if_inventory_lists_them() {
    // Simulate a corrupted managed-files inventory that names user data.
    // Install must refuse to delete sessions/projects/history/etc.
    let (repo, home) = unique_paths("protect-user-data");
    seed_repo(&repo);
    write_skill_with_reference(&repo, "reviewer", "10-r.md");
    install_from_paths("dev", &repo, &home, &InstallOverrides::default(), true).unwrap();

    let sessions = home.join("sessions").join("important.jsonl");
    fs::create_dir_all(sessions.parent().unwrap()).unwrap();
    fs::write(&sessions, "user-session-data\n").unwrap();
    let projects = home.join("projects").join("proj").join("meta.json");
    fs::create_dir_all(projects.parent().unwrap()).unwrap();
    fs::write(&projects, "{\"keep\":true}\n").unwrap();
    let history = home.join("history.jsonl");
    fs::write(&history, "chat-history\n").unwrap();

    // Poison inventory with protected paths + path traversal.
    let inventory = managed_files_inventory_path(&home);
    let mut lines = super::super::verify::read_inventory_lines(&inventory);
    lines.push("sessions/important.jsonl".into());
    lines.push("projects/proj/meta.json".into());
    lines.push("history.jsonl".into());
    lines.push("../outside.txt".into());
    lines.push("skills/../../history.jsonl".into());
    crate::runtime::write_lines(&inventory, &lines).unwrap();

    // Reinstall with purge on ; protected paths must survive.
    install_from_paths("dev", &repo, &home, &InstallOverrides::default(), true).unwrap();

    assert_eq!(
        fs::read_to_string(&sessions).unwrap().trim(),
        "user-session-data",
        "sessions must never be deleted by install purge"
    );
    assert_eq!(
        fs::read_to_string(&projects).unwrap().trim(),
        "{\"keep\":true}",
        "projects must never be deleted by install purge"
    );
    assert_eq!(
        fs::read_to_string(&history).unwrap().trim(),
        "chat-history",
        "history.jsonl must never be deleted by install purge"
    );
    let _ = fs::remove_dir_all(&repo);
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn install_default_purge_off_leaves_dropped_skill_directory() {
    // One-line installer default: no orphan deletes (data-safety first).
    let (repo, home) = unique_paths("no-purge-default");
    seed_repo(&repo);
    write_skill_with_reference(&repo, "reviewer", "10-r.md");
    write_skill_with_reference(&repo, "git-expert", "10-g.md");
    install_from_paths("dev", &repo, &home, &InstallOverrides::default(), true).unwrap();
    let orphan_dir = home.join("skills/git-expert");
    assert!(orphan_dir.is_dir());

    fs::remove_dir_all(repo.join("git-expert")).unwrap();
    // purge_stale = false (default one-line install)
    let summary =
        install_from_paths("dev", &repo, &home, &InstallOverrides::default(), false).unwrap();
    assert_eq!(summary.removed_stale_files, 0);
    assert!(
        orphan_dir.is_dir(),
        "without purge, dropped managed skill dir must remain"
    );
    let _ = fs::remove_dir_all(&repo);
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn is_allowed_managed_orphan_rejects_protected_and_traversal() {
    assert!(!is_allowed_managed_orphan_relative("sessions/x"));
    assert!(!is_allowed_managed_orphan_relative("projects/a/b"));
    assert!(!is_allowed_managed_orphan_relative("history.jsonl"));
    assert!(!is_allowed_managed_orphan_relative("memories/workspaces/x"));
    assert!(!is_allowed_managed_orphan_relative("../etc/passwd"));
    assert!(!is_allowed_managed_orphan_relative(
        "skills/../../history.jsonl"
    ));
    assert!(!is_allowed_managed_orphan_relative("CLAUDE.md"));
    assert!(!is_allowed_managed_orphan_relative("skills/learned-myproj"));
    assert!(is_allowed_managed_orphan_relative(
        "skills/reviewer/SKILL.md"
    ));
    assert!(is_allowed_managed_orphan_relative("agents/reviewer.md"));
    assert!(is_allowed_managed_orphan_relative("AGENTS.md"));
}

#[test]
fn wire_pi_copies_agents_and_mcp_to_project_root() {
    let base = std::env::temp_dir().join(format!("ulw-wire-pi-{}", std::process::id()));
    let claude_home = base.join("owner-home").join(".claude");
    let _ = fs::create_dir_all(&claude_home);
    let repo = create_minimal_layout("wire-pi-repo");
    let _ = fs::create_dir_all(repo.join("pi"));
    let _ = fs::write(repo.join("pi").join("AGENTS.md"), "# Pi Agent\n");
    let _ = fs::write(
        repo.join("pi").join(".mcp.json"),
        r#"{"mcpServers":{"keel":{"command":"keel","args":["mcp","serve"]}}}"#,
    );
    let _ = fs::write(repo.join("pi").join("keel-pi.ts"), "// keel pi extension\n");

    let summary = maybe_wire_pi(&repo, &claude_home, true);
    assert!(
        summary.is_some(),
        "standard .claude home must wire Pi Agent"
    );
    let status = summary.unwrap();
    assert!(
        status.contains("AGENTS.md"),
        "must report AGENTS.md wired, got: {status}"
    );
    assert!(
        status.contains("MCP"),
        "must report MCP registered, got: {status}"
    );
    assert!(
        status.contains("keel-pi.ts"),
        "must report keel-pi.ts wired, got: {status}"
    );

    let home = claude_home.parent().unwrap();
    assert!(
        home.join(".pi").join("agent").join("AGENTS.md").is_file(),
        "Pi AGENTS.md must land in ~/.pi/agent/"
    );
    assert!(
            home.join(".pi").join("agent").join("mcp.json").is_file(),
            "Pi MCP config must land in ~/.pi/agent/mcp.json (Pi's documented location, not ~/.config/mcp/)"
        );
    assert!(
            home.join(".pi")
                .join("agent")
                .join("extensions")
                .join("keel-pi.ts")
                .is_file(),
            "Pi extension must land in ~/.pi/agent/extensions/ (Pi's auto-discovery path, not ~/.pi/extensions/)"
        );

    let _ = fs::remove_dir_all(&base);
    let _ = fs::remove_dir_all(&repo);
}

#[test]
fn wire_pi_returns_none_for_non_standard_home() {
    let repo = create_minimal_layout("wire-pi-nonstd");
    let _ = fs::create_dir_all(repo.join("pi"));
    let _ = fs::write(repo.join("pi").join("AGENTS.md"), "# Pi Agent\n");
    let _ = fs::write(repo.join("pi").join(".mcp.json"), r#"{"mcpServers":{}}"#);

    let claude_home =
        std::env::temp_dir().join(format!("ulw-wire-pi-nonstd-{}", std::process::id()));
    let _ = fs::create_dir_all(&claude_home);
    let result = maybe_wire_pi(&repo, &claude_home, true);
    assert!(
        result.is_none(),
        "non-standard .claude home must return None"
    );

    let _ = fs::remove_dir_all(&claude_home);
    let _ = fs::remove_dir_all(&repo);
}

#[test]
fn rewrite_codex_mcp_command_wrapped_shape() {
    // The shipped .mcp.json uses the wrapped mcp_servers shape with a
    let mut doc = serde_json::json!({
        "mcp_servers": {
            "keel": { "command": "keel", "args": ["mcp", "serve"] }
        }
    });
    let mutated = rewrite_codex_mcp_command(&mut doc, "/home/u/.claude/keel");
    assert!(mutated, "bare keel command must be rewritten");
    assert_eq!(
        doc["mcp_servers"]["keel"]["command"], "/home/u/.claude/keel",
        "command must be the absolute binary path"
    );
    // args must be preserved.
    assert_eq!(doc["mcp_servers"]["keel"]["args"][0], "mcp");
}

#[test]
fn rewrite_codex_mcp_command_idempotent() {
    // A second pass over an already-absolute command must report no
    // mutation (idempotent) so re-install/update is a no-op.
    let absolute = "/home/u/.claude/keel";
    let mut doc = serde_json::json!({
        "mcp_servers": {
            "keel": { "command": absolute, "args": ["mcp", "serve"] }
        }
    });
    let mutated = rewrite_codex_mcp_command(&mut doc, absolute);
    assert!(!mutated, "already-absolute command must not be rewritten");
}

#[test]
fn rewrite_codex_mcp_command_direct_shape() {
    // A direct {"keel": {...}} shape (no mcp_servers wrapper) must also be
    // handled, for robustness against alternative Codex manifests.
    let mut doc = serde_json::json!({
        "keel": { "command": "keel", "args": ["mcp", "serve"] }
    });
    let mutated = rewrite_codex_mcp_command(&mut doc, "/x/keel.exe");
    assert!(mutated, "direct-shape bare command must be rewritten");
    assert_eq!(doc["keel"]["command"], "/x/keel.exe");
}

#[test]
fn rewrite_codex_mcp_command_absent_keel_is_noop() {
    // When the keel entry is absent (or the doc is not an object), the
    // helper must report no mutation rather than panic.
    let mut doc = serde_json::json!({ "mcp_servers": {} });
    assert!(!rewrite_codex_mcp_command(&mut doc, "/x/keel"));
    let mut non_object = serde_json::json!(42);
    assert!(!rewrite_codex_mcp_command(&mut non_object, "/x/keel"));
}

#[test]
fn rewrite_mcp_entry_command_rewrites_bare_command() {
    // The shipped cursor/mcp.json and pi/.mcp.json template a stdio entry.
    let mut entry = serde_json::json!({
        "type": "stdio",
        "command": "keel",
        "args": ["mcp", "serve"],
    });
    assert!(rewrite_mcp_entry_command(&mut entry, "/x/keel.exe"));
    assert_eq!(entry["command"], "/x/keel.exe");
    // Non-command fields must be preserved.
    assert_eq!(entry["args"][0], "mcp");
}

#[test]
fn rewrite_mcp_entry_command_idempotent_and_robust() {
    // Already-absolute command to no mutation (re-install is a no-op).
    let mut entry = serde_json::json!({ "command": "/x/keel", "args": [] });
    assert!(!rewrite_mcp_entry_command(&mut entry, "/x/keel"));
    // Non-object entries must not panic.
    let mut not_object = serde_json::json!(null);
    assert!(!rewrite_mcp_entry_command(&mut not_object, "/x/keel"));
}

#[test]
fn wire_cursor_rewrites_mcp_command_to_absolute() {
    // Install must land the absolute installed-binary path in
    // ~/.cursor/mcp.json, not the bare PATH-dependent template value.
    let base = std::env::temp_dir().join(format!("ulw-wire-cursor-abs-{}", std::process::id()));
    let _ = fs::remove_dir_all(&base);
    let claude_home = base.join(".claude");
    let _ = fs::create_dir_all(&claude_home);
    let repo = create_minimal_layout("wire-cursor-abs");
    let _ = fs::create_dir_all(repo.join("cursor"));
    let _ = fs::write(
        repo.join("cursor").join("mcp.json"),
        r#"{"mcpServers":{"keel":{"type":"stdio","command":"keel","args":["mcp","serve"]}}}"#,
    );

    let summary = maybe_wire_cursor(&repo, &claude_home, true);
    assert!(summary.is_some());

    let home = claude_home.parent().unwrap();
    let mcp_target = home.join(".cursor").join("mcp.json");
    assert!(mcp_target.is_file(), "cursor mcp.json must be merged");
    let doc: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&mcp_target).expect("read cursor mcp.json"))
            .expect("cursor mcp.json must be valid JSON");
    let command = doc["mcpServers"]["keel"]["command"]
        .as_str()
        .expect("cursor entry must have a command");
    assert_eq!(doc["mcpServers"]["keel"]["type"], "stdio");
    assert_ne!(command, "keel", "must not keep the bare template command");
    assert_eq!(
        command,
        display_path(&installed_executable_path(&claude_home)),
        "cursor MCP command must be the absolute installed binary path"
    );

    let _ = fs::remove_dir_all(&base);
    let _ = fs::remove_dir_all(&repo);
}

#[test]
fn wire_pi_rewrites_mcp_command_to_absolute() {
    // Install must land the absolute installed-binary path in
    // ~/.pi/agent/mcp.json, not the bare PATH-dependent template value.
    let base = std::env::temp_dir().join(format!("ulw-wire-pi-abs-{}", std::process::id()));
    let _ = fs::remove_dir_all(&base);
    let claude_home = base.join(".claude");
    let _ = fs::create_dir_all(&claude_home);
    let repo = create_minimal_layout("wire-pi-abs");
    let _ = fs::create_dir_all(repo.join("pi"));
    let _ = fs::write(repo.join("pi").join("AGENTS.md"), "# Pi Agent\n");
    let _ = fs::write(
        repo.join("pi").join(".mcp.json"),
        r#"{"settings":{"idleTimeout":60},"mcpServers":{"keel":{"command":"keel","args":["mcp","serve"],"lifecycle":"lazy","directTools":true}}}"#,
    );

    let summary = maybe_wire_pi(&repo, &claude_home, true);
    assert!(summary.is_some());

    let home = claude_home.parent().unwrap();
    let mcp_target = home.join(".pi").join("agent").join("mcp.json");
    assert!(mcp_target.is_file(), "pi mcp.json must be merged");
    let doc: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&mcp_target).expect("read pi mcp.json"))
            .expect("pi mcp.json must be valid JSON");
    let command = doc["mcpServers"]["keel"]["command"]
        .as_str()
        .expect("pi entry must have a command");
    assert_ne!(command, "keel", "must not keep the bare template command");
    assert_eq!(
        command,
        display_path(&installed_executable_path(&claude_home)),
        "pi MCP command must be the absolute installed binary path"
    );
    // Sibling fields from the shipped template must survive the rewrite.
    assert_eq!(doc["mcpServers"]["keel"]["lifecycle"], "lazy");
    assert_eq!(doc["mcpServers"]["keel"]["directTools"], true);
    assert_eq!(doc["settings"]["idleTimeout"], 60);

    let _ = fs::remove_dir_all(&base);
    let _ = fs::remove_dir_all(&repo);
}

fn unique_codex_test_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("keel-codex-{label}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Build a fake user home holding `.keel` (the neutral root) and `.claude`
/// (the engagement home) so migration tests run hermetically. Returns
/// `(home, keel_home, claude_home)`.
fn legacy_home_fixture(label: &str) -> (PathBuf, PathBuf, PathBuf) {
    let home = std::env::temp_dir().join(format!("keel-migrate-{label}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&home);
    let keel_home = home.join(".keel");
    let claude_home = home.join(".claude");
    fs::create_dir_all(&keel_home).unwrap();
    fs::create_dir_all(&claude_home).unwrap();
    (home, keel_home, claude_home)
}

#[test]
fn migration_copies_keel_owned_data_and_retains_legacy_source() {
    let (_home, keel_home, claude_home) = legacy_home_fixture("copy");
    fs::create_dir_all(claude_home.join("working-briefs")).unwrap();
    fs::write(claude_home.join("working-briefs/brief.json"), "{}").unwrap();
    fs::write(claude_home.join("config.toml"), "x = 1").unwrap();
    fs::create_dir_all(claude_home.join("memories")).unwrap();

    let report = migrate_from_legacy_claude_home(&keel_home, &claude_home);
    assert!(report.is_some(), "migration must report copied data");
    assert!(keel_home.join("working-briefs/brief.json").is_file());
    assert!(keel_home.join("config.toml").is_file());
    assert!(keel_home.join("memories").is_dir());
    assert!(claude_home.join("working-briefs/brief.json").is_file());
    assert!(claude_home.join("config.toml").is_file());
    let _ = fs::remove_dir_all(keel_home.parent().unwrap());
}

#[test]
fn migration_merges_legacy_dir_without_removing_source() {
    let (_home, keel_home, claude_home) = legacy_home_fixture("nooverwrite");
    fs::create_dir_all(keel_home.join("working-briefs")).unwrap();
    fs::write(keel_home.join("working-briefs/new.json"), "kept").unwrap();
    fs::create_dir_all(claude_home.join("working-briefs")).unwrap();
    fs::write(claude_home.join("working-briefs/old.json"), "legacy").unwrap();

    let report = migrate_from_legacy_claude_home(&keel_home, &claude_home);
    assert!(report.is_some());
    assert_eq!(
        fs::read_to_string(keel_home.join("working-briefs/new.json")).unwrap(),
        "kept"
    );
    assert_eq!(
        fs::read_to_string(keel_home.join("working-briefs/old.json")).unwrap(),
        "legacy"
    );
    assert!(claude_home.join("working-briefs/old.json").is_file());
    let _ = fs::remove_dir_all(keel_home.parent().unwrap());
}

#[test]
fn migration_exact_path_conflict_keeps_destination_and_source() {
    let (_home, keel_home, claude_home) = legacy_home_fixture("conflict");
    // Same relative path on both sides: destination wins, the conflicting
    // legacy copy is left in place (never deleted, never overwritten).
    fs::create_dir_all(keel_home.join("memories")).unwrap();
    fs::write(keel_home.join("memories/note.md"), "fresh").unwrap();
    fs::create_dir_all(claude_home.join("memories")).unwrap();
    fs::write(claude_home.join("memories/note.md"), "legacy").unwrap();

    let report = migrate_from_legacy_claude_home(&keel_home, &claude_home);
    assert!(report.is_some());
    assert_eq!(
        fs::read_to_string(keel_home.join("memories/note.md")).unwrap(),
        "fresh",
        "destination content must win an exact-path conflict"
    );
    assert_eq!(
        fs::read_to_string(claude_home.join("memories/note.md")).unwrap(),
        "legacy",
        "the conflicting legacy copy must stay for manual reconciliation"
    );
    let _ = fs::remove_dir_all(keel_home.parent().unwrap());
}

#[test]
fn migration_is_noop_when_same_root_or_non_standard() {
    // Non-standard root: engagement == root, so nothing migrates.
    let dir = unique_codex_test_dir("noop");
    let result = migrate_from_legacy_claude_home(&dir, &dir);
    assert!(result.is_none(), "non-standard roots must not migrate");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn migration_noop_when_legacy_home_absent() {
    let home = std::env::temp_dir().join(format!("keel-migrate-absent-{}", std::process::id()));
    let _ = fs::remove_dir_all(&home);
    let keel_home = home.join(".keel");
    fs::create_dir_all(&keel_home).unwrap();
    let claude_home = home.join(".claude"); // does not exist
    let result = migrate_from_legacy_claude_home(&keel_home, &claude_home);
    assert!(result.is_none(), "no legacy home means nothing to migrate");
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn migration_copies_empty_and_non_empty_destinations_without_deleting_sources() {
    let (_home, keel_home, claude_home) = legacy_home_fixture("emptydest");
    fs::create_dir_all(keel_home.join("memories")).unwrap();
    fs::create_dir_all(claude_home.join("memories")).unwrap();
    fs::write(claude_home.join("memories/note.md"), "real").unwrap();
    fs::create_dir_all(keel_home.join("raw-output")).unwrap();
    fs::write(keel_home.join("raw-output/new.json"), "fresh").unwrap();
    fs::create_dir_all(claude_home.join("raw-output")).unwrap();
    fs::write(claude_home.join("raw-output/old.json"), "legacy").unwrap();

    let report = migrate_from_legacy_claude_home(&keel_home, &claude_home);
    assert!(report.is_some());
    assert_eq!(
        fs::read_to_string(keel_home.join("memories/note.md")).unwrap(),
        "real"
    );
    assert!(claude_home.join("memories/note.md").is_file());
    assert!(keel_home.join("raw-output/new.json").is_file());
    assert!(keel_home.join("raw-output/old.json").is_file());
    assert!(claude_home.join("raw-output/old.json").is_file());
    let _ = fs::remove_dir_all(keel_home.parent().unwrap());
}

#[test]
fn migration_idempotent_second_run_is_quiet() {
    let (_home, keel_home, claude_home) = legacy_home_fixture("idem");
    fs::write(claude_home.join("config.toml"), "x = 1").unwrap();
    let first = migrate_from_legacy_claude_home(&keel_home, &claude_home);
    assert!(first.is_some());
    let second = migrate_from_legacy_claude_home(&keel_home, &claude_home);
    assert!(
        second.is_none(),
        "second run must be a no-op once migrated, got {second:?}"
    );
    let _ = fs::remove_dir_all(keel_home.parent().unwrap());
}

#[test]
fn cleanup_removes_only_verified_legacy_duplicates() {
    let (_home, keel_home, claude_home) = legacy_home_fixture("cleanup");
    fs::create_dir_all(keel_home.join("memories")).unwrap();
    fs::create_dir_all(claude_home.join("memories")).unwrap();
    fs::write(keel_home.join("memories/same.md"), "same").unwrap();
    fs::write(claude_home.join("memories/same.md"), "same").unwrap();
    fs::write(keel_home.join("memories/different.md"), "new").unwrap();
    fs::write(claude_home.join("memories/different.md"), "old").unwrap();
    fs::create_dir_all(claude_home.join("cache/user")).unwrap();
    fs::write(claude_home.join("cache/user/cache.json"), "keep").unwrap();
    fs::create_dir_all(keel_home.join("state")).unwrap();
    fs::create_dir_all(claude_home.join(".claude-skill-manager")).unwrap();
    fs::write(keel_home.join("state/state.json"), "state").unwrap();
    fs::write(
        claude_home.join(".claude-skill-manager/state.json"),
        "state",
    )
    .unwrap();

    let removed = cleanup_identical_legacy_data(&keel_home, &claude_home);

    assert!(removed >= 2);
    assert!(!claude_home.join("memories/same.md").exists());
    assert!(claude_home.join("memories/different.md").is_file());
    assert!(claude_home.join("cache/user/cache.json").is_file());
    assert!(!claude_home.join(".claude-skill-manager").exists());
    let _ = fs::remove_dir_all(keel_home.parent().unwrap());
}

#[test]
fn copy_tree_copies_nested_directories() {
    let dir = unique_codex_test_dir("copytree");
    let src = dir.join("src");
    let dst = dir.join("dst");
    fs::create_dir_all(src.join("a/b")).unwrap();
    fs::write(src.join("root.txt"), "root").unwrap();
    fs::write(src.join("a/b/deep.txt"), "deep").unwrap();

    assert!(copy_tree(&src, &dst));
    assert_eq!(fs::read_to_string(dst.join("root.txt")).unwrap(), "root");
    assert_eq!(
        fs::read_to_string(dst.join("a/b/deep.txt")).unwrap(),
        "deep"
    );
    let _ = fs::remove_dir_all(&dir);
}
#[test]
fn update_temp_trees_are_deleted_but_legacy_state_stays() {
    let dir = unique_codex_test_dir("upd-cache");
    let keel_home = dir.join(".keel");
    let engagement = dir.join(".claude");
    fs::create_dir_all(keel_home.join("cache/update/v1")).unwrap();
    fs::write(keel_home.join("cache/update/v1/bin"), "tmp").unwrap();
    fs::create_dir_all(keel_home.join("cache/installed-source")).unwrap();
    fs::write(
        keel_home.join("cache/installed-source/keel-release-manifest.json"),
        "{}",
    )
    .unwrap();
    fs::create_dir_all(state_directory(&keel_home)).unwrap();
    fs::write(state_directory(&keel_home).join("managed-files.txt"), "x").unwrap();
    fs::create_dir_all(legacy_state_directory(&engagement).join("bin")).unwrap();
    fs::write(legacy_state_directory(&engagement).join("bin/old"), "stale").unwrap();
    fs::create_dir_all(state_directory(&engagement).join("bin")).unwrap();
    fs::write(state_directory(&engagement).join("bin/old"), "stale").unwrap();
    fs::create_dir_all(engagement.join("cache/user")).unwrap();
    fs::write(engagement.join("cache/user/preferences.json"), "keep").unwrap();
    fs::write(
        legacy_state_directory(&engagement).join("user-data.json"),
        "keep",
    )
    .unwrap();
    remove_update_temp_trees(&keel_home, &engagement);
    assert!(!update_cache_directory(&keel_home).join("update").exists());
    assert!(update_cache_directory(&keel_home)
        .join("installed-source/keel-release-manifest.json")
        .is_file());
    assert!(state_directory(&keel_home)
        .join("managed-files.txt")
        .is_file());
    assert!(!legacy_state_directory(&engagement).join("bin").exists());
    assert!(!state_directory(&engagement).join("bin").exists());
    assert_eq!(
        fs::read_to_string(engagement.join("cache/user/preferences.json")).unwrap(),
        "keep"
    );
    assert_eq!(
        fs::read_to_string(legacy_state_directory(&engagement).join("user-data.json")).unwrap(),
        "keep"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn uninstall_removes_the_owned_installed_source_cache() {
    let dir = unique_codex_test_dir("uninstall-installed-source");
    let keel_home = dir.join(".keel");
    let cached_source = keel_home.join("cache/installed-source");
    fs::create_dir_all(&cached_source).unwrap();
    fs::write(cached_source.join("keel-release-manifest.json"), "{}").unwrap();

    super::commands::uninstall_managed_files(&keel_home).unwrap();

    assert!(
        !keel_home.join("cache").exists(),
        "uninstall must not retain the packaged release source cache"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn uninstall_removes_old_claude_home_keel_leftovers() {
    let dir = unique_codex_test_dir("uninst-legacy");
    let keel_home = dir.join(".keel");
    let engagement = dir.join(".claude");
    fs::create_dir_all(&keel_home).unwrap();
    fs::create_dir_all(engagement.join("working-briefs")).unwrap();
    fs::write(engagement.join("working-briefs/old.json"), "{}").unwrap();
    fs::write(engagement.join("command-compaction-events.jsonl"), "").unwrap();
    fs::write(engagement.join("config.toml"), "x=1").unwrap();
    fs::write(engagement.join(executable_file_name()), "old-bin").unwrap();
    fs::create_dir_all(engagement.join("workflow")).unwrap();
    let removed = remove_legacy_keel_leftovers(&keel_home, &engagement);
    assert!(removed > 0);
    assert!(!engagement.join("working-briefs").exists());
    assert!(!engagement.join("command-compaction-events.jsonl").exists());
    assert!(!engagement.join("config.toml").exists());
    assert!(!engagement.join("workflow").exists());
    assert!(!engagement.join(executable_file_name()).exists());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn install_without_purge_stale_still_removes_dropped_first_party_surfaces() {
    let (repo, home) = unique_paths("drop-sprint");
    seed_repo(&repo);
    write_skill_with_reference(&repo, "reviewer", "10-r.md");
    fs::create_dir_all(home.join("skills/running-a-sprint")).unwrap();
    fs::write(
        home.join("skills/running-a-sprint/SKILL.md"),
        "old sprint\n",
    )
    .unwrap();
    fs::create_dir_all(home.join("commands")).unwrap();
    fs::write(home.join("commands/sprint.md"), "old sprint cmd\n").unwrap();
    fs::write(home.join("commands/user-story.md"), "old story\n").unwrap();
    fs::write(home.join("commands/workflow.md"), "old workflow\n").unwrap();

    let summary =
        install_from_paths("dev", &repo, &home, &InstallOverrides::default(), false).unwrap();
    assert!(
        summary.removed_stale_files >= 4,
        "dropped first-party leftovers must be counted: {}",
        summary.removed_stale_files
    );
    assert!(!home.join("skills/running-a-sprint").exists());
    assert!(!home.join("commands/sprint.md").exists());
    assert!(!home.join("commands/user-story.md").exists());
    assert!(!home.join("commands/workflow.md").exists());
    assert!(home.join("skills/reviewer/SKILL.md").is_file());
    let _ = fs::remove_dir_all(&repo);
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn dropped_command_stays_when_current_pack_still_ships_it() {
    let (repo, home) = unique_paths("keep-workflow-in-pack");
    seed_repo(&repo);
    write_skill_with_reference(&repo, "reviewer", "10-r.md");
    let commands_source = repo.join("commands");
    fs::create_dir_all(&commands_source).unwrap();
    fs::write(
        commands_source.join("workflow.md"),
        "---\ndescription: still in pack\n---\n",
    )
    .unwrap();
    install_from_paths("dev", &repo, &home, &InstallOverrides::default(), false).unwrap();
    assert!(
        home.join("commands/workflow.md").is_file(),
        "a command still in the source pack must not be treated as dropped"
    );
    let _ = fs::remove_dir_all(&repo);
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn uninstall_removes_dropped_first_party_surfaces_missing_from_inventory() {
    let (repo, home) = unique_paths("uninst-drop");
    seed_repo(&repo);
    write_skill_with_reference(&repo, "reviewer", "10-r.md");
    install_from_paths("dev", &repo, &home, &InstallOverrides::default(), false).unwrap();
    fs::create_dir_all(home.join("skills/writing-user-stories")).unwrap();
    fs::write(
        home.join("skills/writing-user-stories/SKILL.md"),
        "old stories\n",
    )
    .unwrap();
    fs::create_dir_all(home.join("commands")).unwrap();
    fs::write(home.join("commands/sprint.md"), "old\n").unwrap();

    let code = run_uninstall_command(
        &["--claude-home".to_string(), home.display().to_string()],
        &mut Vec::new(),
        &mut Vec::new(),
    );
    assert_eq!(code, 0);
    assert!(!home.join("skills/writing-user-stories").exists());
    assert!(!home.join("commands/sprint.md").exists());
    let _ = fs::remove_dir_all(&repo);
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn copy_path_preserving_keeps_source_and_destination() {
    let dir = unique_codex_test_dir("copypath");
    let src_file = dir.join("a.txt");
    let dst_file = dir.join("b.txt");
    fs::write(&src_file, "hello").unwrap();
    assert!(copy_path_preserving(&src_file, &dst_file));
    assert!(src_file.is_file());
    assert_eq!(fs::read_to_string(&dst_file).unwrap(), "hello");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn managed_overwrite_keeps_previous_bytes_in_unique_backup() {
    let home = unique_codex_test_dir("managed-backup");
    let source = home.join("source.txt");
    let target = home.join("target.txt");
    fs::write(&source, "new").unwrap();
    fs::write(&target, "old").unwrap();
    let tracker = FileTracker::new(&home);

    assert!(copy_file_if_changed(&source, &target, &tracker).unwrap());
    assert_eq!(fs::read_to_string(&target).unwrap(), "new");
    let backup_files: Vec<PathBuf> = fs::read_dir(home.join("backups"))
        .unwrap()
        .flat_map(|entry| {
            fs::read_dir(entry.unwrap().path())
                .unwrap()
                .map(|nested| nested.unwrap().path())
        })
        .collect();
    assert_eq!(backup_files.len(), 1);
    assert_eq!(fs::read_to_string(&backup_files[0]).unwrap(), "old");
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn merge_codex_marketplace_creates_catalog_when_absent() {
    let dir = unique_codex_test_dir("market-create");
    let path = dir.join(".agents/plugins/marketplace.json");
    let result = merge_codex_marketplace(&path).unwrap();
    assert!(
        matches!(result, CodexMarketplaceResult::Added),
        "absent manifest must report Added, got {result:?}"
    );
    let doc: serde_json::Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(doc["name"], CODEX_PERSONAL_MARKETPLACE_NAME);
    let plugins = doc["plugins"].as_array().unwrap();
    assert_eq!(plugins.len(), 1);
    assert_eq!(plugins[0]["name"], "keel");
    assert_eq!(plugins[0]["source"]["path"], "~/.codex/plugins/keel");
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn merge_codex_marketplace_preserves_siblings_and_is_idempotent() {
    let dir = unique_codex_test_dir("market-idem");
    let path = dir.join("marketplace.json");
    fs::write(
            &path,
            r#"{"name":"user-catalog","plugins":[{"name":"other-plugin","source":{"source":"local","path":"~/p/other"}}]}"#,
        )
        .unwrap();
    let first = merge_codex_marketplace(&path).unwrap();
    assert!(matches!(first, CodexMarketplaceResult::Added));
    let second = merge_codex_marketplace(&path).unwrap();
    assert!(
        matches!(second, CodexMarketplaceResult::AlreadyCurrent),
        "second merge must be a no-op, got {second:?}"
    );
    let doc: serde_json::Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(doc["name"], "user-catalog", "user metadata preserved");
    let names: Vec<&str> = doc["plugins"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"other-plugin"), "sibling entry preserved");
    assert!(names.contains(&"keel"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn merge_codex_marketplace_updates_stale_keel_entry() {
    let dir = unique_codex_test_dir("market-stale");
    let path = dir.join("marketplace.json");
    fs::write(
            &path,
            r#"{"name":"user-catalog","plugins":[{"name":"keel","source":{"source":"local","path":"/old/path"}}]}"#,
        )
        .unwrap();
    let result = merge_codex_marketplace(&path).unwrap();
    assert!(
        matches!(result, CodexMarketplaceResult::Updated),
        "stale keel entry must report Updated, got {result:?}"
    );
    let doc: serde_json::Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(doc["plugins"][0]["source"]["path"], "~/.codex/plugins/keel");
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn ensure_codex_plugin_enabled_appends_section_when_absent() {
    let dir = unique_codex_test_dir("enable-absent");
    let path = dir.join("config.toml");
    fs::write(&path, "model = \"some-model\"\n").unwrap();
    let result = ensure_codex_plugin_enabled(&path).unwrap();
    assert!(matches!(result, CodexEnableResult::Added));
    let text = fs::read_to_string(&path).unwrap();
    assert!(text.contains(CODEX_PLUGIN_CONFIG_SECTION));
    assert!(text.contains("enabled = true"));
    assert!(
        text.contains("model = \"some-model\""),
        "existing keys must survive the append"
    );
    // The result must still be valid TOML with the enabled flag set.
    let doc: toml::Value = toml::from_str(&text).unwrap();
    assert_eq!(
        doc["plugins"]["keel@personal-keel"]["enabled"].as_bool(),
        Some(true)
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn ensure_codex_plugin_enabled_creates_missing_file() {
    let dir = unique_codex_test_dir("enable-newfile");
    let path = dir.join("config.toml");
    let result = ensure_codex_plugin_enabled(&path).unwrap();
    assert!(matches!(result, CodexEnableResult::Added));
    assert!(path.is_file());
    let doc: toml::Value = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(
        doc["plugins"]["keel@personal-keel"]["enabled"].as_bool(),
        Some(true)
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn ensure_codex_plugin_enabled_is_idempotent() {
    let dir = unique_codex_test_dir("enable-idem");
    let path = dir.join("config.toml");
    assert!(matches!(
        ensure_codex_plugin_enabled(&path).unwrap(),
        CodexEnableResult::Added
    ));
    assert!(matches!(
        ensure_codex_plugin_enabled(&path).unwrap(),
        CodexEnableResult::AlreadyEnabled
    ));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn ensure_codex_plugin_enabled_respects_user_disable() {
    let dir = unique_codex_test_dir("enable-disabled");
    let path = dir.join("config.toml");
    let body = format!("{CODEX_PLUGIN_CONFIG_SECTION}\nenabled = false\n");
    fs::write(&path, &body).unwrap();
    let result = ensure_codex_plugin_enabled(&path).unwrap();
    assert!(
        matches!(result, CodexEnableResult::UnchangedDisabled),
        "an explicit user disable must win, got {result:?}"
    );
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        body,
        "the file must be untouched when the user disabled the plugin"
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn ensure_codex_plugin_enabled_inserts_under_existing_header() {
    let dir = unique_codex_test_dir("enable-header");
    let path = dir.join("config.toml");
    fs::write(
        &path,
        format!("model = \"x\"\n{CODEX_PLUGIN_CONFIG_SECTION}\nother_key = 1\n"),
    )
    .unwrap();
    let result = ensure_codex_plugin_enabled(&path).unwrap();
    assert!(matches!(result, CodexEnableResult::Added));
    let doc: toml::Value = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    let entry = &doc["plugins"]["keel@personal-keel"];
    assert_eq!(entry["enabled"].as_bool(), Some(true));
    assert_eq!(entry["other_key"].as_integer(), Some(1));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn ensure_codex_plugin_enabled_refuses_unparseable_toml() {
    let dir = unique_codex_test_dir("enable-badtoml");
    let path = dir.join("config.toml");
    let broken = "model = \"unterminated\n";
    fs::write(&path, broken).unwrap();
    assert!(
        ensure_codex_plugin_enabled(&path).is_err(),
        "unparseable config.toml must be refused, never mutated"
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), broken);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn remove_codex_marketplace_entry_removes_keel_and_keeps_siblings() {
    let dir = unique_codex_test_dir("market-remove");
    let path = dir.join("marketplace.json");
    fs::write(
            &path,
            r#"{"name":"user-catalog","plugins":[{"name":"keel","source":{}},{"name":"keep-me","source":{}}]}"#,
        )
        .unwrap();
    assert_eq!(remove_codex_marketplace_entry(&path), 1);
    let doc: serde_json::Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    let names: Vec<&str> = doc["plugins"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["keep-me"]);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn remove_codex_marketplace_entry_deletes_keel_only_catalog() {
    let dir = unique_codex_test_dir("market-remove-only");
    let path = dir.join("marketplace.json");
    fs::write(
        &path,
        r#"{"name":"personal-keel","plugins":[{"name":"keel"}]}"#,
    )
    .unwrap();
    assert!(remove_codex_marketplace_entry(&path) >= 1);
    assert!(
        !path.exists(),
        "a catalog that held only keel must be removed wholesale"
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn ensure_codex_native_mcp_appends_section_when_absent() {
    let dir = unique_codex_test_dir("natmcp-absent");
    let path = dir.join("config.toml");
    fs::write(&path, "model = \"some-model\"\n").unwrap();
    let binary = dir.join("keel.exe");
    let result = ensure_codex_native_mcp(&path, &binary).unwrap();
    assert!(matches!(result, CodexNativeMcpResult::Added));
    let doc: toml::Value = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    let entry = &doc["mcp_servers"]["keel"];
    assert_eq!(
        entry["command"].as_str().unwrap(),
        display_path(&binary),
        "command must be the absolute binary path"
    );
    let args: Vec<&str> = entry["args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(args, vec!["mcp", "serve"]);
    assert!(
        fs::read_to_string(&path)
            .unwrap()
            .contains("model = \"some-model\""),
        "existing keys must survive the append"
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn ensure_codex_native_mcp_is_idempotent() {
    let dir = unique_codex_test_dir("natmcp-idem");
    let path = dir.join("config.toml");
    let binary = dir.join("keel.exe");
    assert!(matches!(
        ensure_codex_native_mcp(&path, &binary).unwrap(),
        CodexNativeMcpResult::Added
    ));
    assert!(matches!(
        ensure_codex_native_mcp(&path, &binary).unwrap(),
        CodexNativeMcpResult::AlreadyCurrent
    ));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn ensure_codex_native_mcp_updates_stale_command_preserving_siblings() {
    let dir = unique_codex_test_dir("natmcp-stale");
    let path = dir.join("config.toml");
    fs::write(
            &path,
            "[mcp_servers.other]\ncommand = \"other-mcp\"\n\n[mcp_servers.keel]\ncommand = \"old\"\nargs = [\"mcp\", \"serve\"]\n",
        )
        .unwrap();
    let binary = dir.join("keel.exe");
    let result = ensure_codex_native_mcp(&path, &binary).unwrap();
    assert!(matches!(result, CodexNativeMcpResult::Updated));
    let doc: toml::Value = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(
        doc["mcp_servers"]["keel"]["command"].as_str().unwrap(),
        display_path(&binary)
    );
    assert_eq!(
        doc["mcp_servers"]["other"]["command"].as_str().unwrap(),
        "other-mcp",
        "a sibling MCP server must survive untouched"
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn ensure_codex_native_mcp_escapes_windows_backslashes() {
    let dir = unique_codex_test_dir("natmcp-escape");
    let path = dir.join("config.toml");
    let binary = dir.join("sub dir").join("keel.exe");
    let result = ensure_codex_native_mcp(&path, &binary).unwrap();
    assert!(matches!(result, CodexNativeMcpResult::Added));
    // The written TOML must parse back to the exact path (escaping valid).
    let doc: toml::Value = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(
        doc["mcp_servers"]["keel"]["command"].as_str().unwrap(),
        display_path(&binary)
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn sync_codex_agents_md_writes_contract_and_preserves_user_content() {
    let dir = unique_codex_test_dir("agents-md");
    let path = dir.join("AGENTS.md");
    fs::write(&path, "# My codex notes\n").unwrap();
    let status = sync_codex_agents_md(&path).unwrap();
    assert!(status.starts_with("AGENTS.md written"));
    let text = fs::read_to_string(&path).unwrap();
    assert!(
        text.contains("Iron Law"),
        "the contract must carry the Iron Law"
    );
    assert!(text.contains("My codex notes"), "user content must survive");
    assert!(text.contains(MANAGED_CODEX_AGENTS_BEGIN));
    // Second run is a no-op (already current) and never duplicates.
    let again = sync_codex_agents_md(&path).unwrap();
    assert_eq!(again, "AGENTS.md already current");
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn strip_managed_region_removes_block_keeps_user_content() {
    let user_above = "# Top notes\n";
    let block = format!("{MANAGED_CODEX_AGENTS_BEGIN}\ncontract\n{MANAGED_CODEX_AGENTS_END}");
    let user_below = "# Bottom notes\n";
    let existing = format!("{user_above}\n{block}\n\n{user_below}");
    let stripped = strip_managed_region(
        &existing,
        MANAGED_CODEX_AGENTS_BEGIN,
        MANAGED_CODEX_AGENTS_END,
    )
    .unwrap();
    assert!(stripped.contains("Top notes"));
    assert!(stripped.contains("Bottom notes"));
    assert!(!stripped.contains("contract"));
    // A block-only file collapses to empty so the caller can delete it.
    let only_block =
        strip_managed_region(&block, MANAGED_CODEX_AGENTS_BEGIN, MANAGED_CODEX_AGENTS_END).unwrap();
    assert!(only_block.trim().is_empty());
    // No block present -> None.
    assert!(strip_managed_region(
        "# just user\n",
        MANAGED_CODEX_AGENTS_BEGIN,
        MANAGED_CODEX_AGENTS_END
    )
    .is_none());
}

#[test]
fn remove_codex_native_mcp_section_removes_only_keel() {
    let dir = unique_codex_test_dir("natmcp-remove");
    let path = dir.join("config.toml");
    fs::write(
        &path,
        "[mcp_servers.keel]\ncommand = \"x\"\nargs = []\n\n[mcp_servers.keep]\ncommand = \"y\"\n",
    )
    .unwrap();
    assert_eq!(remove_codex_native_mcp_section(&path), 1);
    let text = fs::read_to_string(&path).unwrap();
    assert!(!text.contains("mcp_servers.keel"));
    assert!(text.contains("mcp_servers.keep"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn remove_codex_managed_agents_md_deletes_keel_only_file() {
    let dir = unique_codex_test_dir("agents-md-remove");
    let path = dir.join("AGENTS.md");
    let block = format!("{MANAGED_CODEX_AGENTS_BEGIN}\ncontract\n{MANAGED_CODEX_AGENTS_END}");
    fs::write(&path, &block).unwrap();
    assert!(remove_codex_managed_agents_md(&path) >= 1);
    assert!(!path.exists(), "a keel-only AGENTS.md must be removed");
    // With user content, the block is stripped but the file stays.
    fs::write(&path, format!("user stuff\n\n{block}\n")).unwrap();
    assert_eq!(remove_codex_managed_agents_md(&path), 1);
    let text = fs::read_to_string(&path).unwrap();
    assert!(text.contains("user stuff"));
    assert!(!text.contains(MANAGED_CODEX_AGENTS_BEGIN));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn remove_codex_plugin_section_removes_only_keel_section() {
    let dir = unique_codex_test_dir("section-remove");
    let path = dir.join("config.toml");
    fs::write(
            &path,
            format!(
                "# user comment\nmodel = \"x\"\n\n{CODEX_PLUGIN_CONFIG_SECTION}\nenabled = true\n\n[plugins.\"other@market\"]\nenabled = false\n"
            ),
        )
        .unwrap();
    assert_eq!(remove_codex_plugin_section(&path), 1);
    let text = fs::read_to_string(&path).unwrap();
    assert!(!text.contains("keel@personal-keel"));
    assert!(text.contains("[plugins.\"other@market\"]"));
    assert!(text.contains("# user comment"), "comments must survive");
    let doc: toml::Value = toml::from_str(&text).unwrap();
    assert_eq!(
        doc["plugins"]["other@market"]["enabled"].as_bool(),
        Some(false),
        "sibling plugin sections must be untouched"
    );
    let _ = fs::remove_dir_all(dir);
}

#[cfg(windows)]
#[test]
fn grok_hook_command_executes_spaced_path_and_forwards_stdin_in_powershell() {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let root = crate::test_support::unique_temp_dir("keel grok hook command");
    let hook_probe = root.join("hook stdin probe.cmd");
    fs::write(
        &hook_probe,
        "@echo off\r\nset /p hook_input=\r\necho %hook_input%\r\n",
    )
    .unwrap();

    let hook_document = grok_hooks_payload(&hook_probe);
    let command = hook_document["hooks"]["PreToolUse"][0]["hooks"][0]["command"]
        .as_str()
        .expect("PreToolUse command");
    let (powershell, arguments) = crate::runtime::named_shell_command_parts("powershell", command)
        .expect("resolve PowerShell");
    let mut child = Command::new(powershell)
        .args(arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start PowerShell");
    let mut standard_input = child.stdin.take().expect("PowerShell stdin");
    standard_input
        .write_all(b"grok-hook-input\r\n")
        .expect("write hook input");
    drop(standard_input);
    let output = child.wait_with_output().expect("wait for PowerShell");

    assert!(
        output.status.success(),
        "generated Grok hook command must execute in PowerShell: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("grok-hook-input"),
        "hook process must receive Grok stdin: {}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[cfg(windows)]
#[test]
fn grok_hooks_are_current_rejects_legacy_windows_command() {
    let root = crate::test_support::unique_temp_dir("keel-grok-legacy-hook");
    let hook_path = root.join("keel.json");
    let binary = root.join("Keel Install").join("keel.exe");
    let mut legacy_document = grok_hooks_payload(&binary);
    legacy_document["hooks"]["PreToolUse"][0]["hooks"][0]["command"] =
        serde_json::Value::String(format!(
            "\"{}\" hook pre-tool-use",
            display_path(&binary).replace('"', "\\\"")
        ));
    fs::write(
        &hook_path,
        serde_json::to_string_pretty(&legacy_document).unwrap(),
    )
    .unwrap();

    assert!(
        !grok_hooks_are_current(&hook_path, &binary),
        "the PowerShell-incompatible quoted command must require repair"
    );
}

#[test]
fn grok_stop_hook_is_silent_not_post_tool_batch() {
    let root = std::env::temp_dir().join(format!(
        "keel-grok-stop-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let keel_home = root.join(".keel");
    let grok_dir = root.join(".grok");
    fs::create_dir_all(&keel_home).unwrap();
    fs::create_dir_all(&grok_dir).unwrap();
    let status = maybe_wire_grok(&keel_home, true).expect("wire when .grok exists");
    assert!(
        !status.contains("skipped"),
        "standard .keel + detected .grok must write hooks: {status}"
    );
    let text = fs::read_to_string(grok_dir.join("hooks").join("keel.json")).unwrap();
    let hook_document: serde_json::Value = serde_json::from_str(&text).unwrap();
    let session_command = hook_document["hooks"]["SessionStart"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap();
    assert!(
        session_command.starts_with(if cfg!(windows) { "& '" } else { "'" }),
        "Grok hook executable must be quoted: {session_command}"
    );
    assert!(
        text.contains("hook stop"),
        "Grok Stop must call silent hook stop: {text}"
    );
    assert!(
        text.contains("hook session-end"),
        "Grok SessionEnd must drive the learning cycle: {text}"
    );
    assert!(
        text.contains("hook pre-compact"),
        "Grok PreCompact must checkpoint before compaction: {text}"
    );
    assert!(
        text.contains("hook post-compact"),
        "Grok PostCompact must re-push context: {text}"
    );
    assert!(
        !text.contains("post-tool-batch"),
        "Grok has no PostToolBatch event; Stop must not inject post-tool-batch closeout: {text}"
    );
    let config = fs::read_to_string(grok_dir.join("config.toml")).unwrap();
    let document: toml::Value = toml::from_str(&config).unwrap();
    assert_eq!(
        document["mcp_servers"]["keel"]["command"].as_str(),
        Some(display_path(&installed_executable_path(&keel_home)).as_str())
    );
    assert_eq!(
        document["mcp_servers"]["keel"]["args"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(toml::Value::as_str)
            .collect::<Vec<_>>(),
        ["mcp", "serve"]
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn grok_platform_override_can_force_and_skip_detection() {
    let forced = parse_overrides("grok", "");
    let detected = apply_overrides(
        crate::manager::platform_detect::DetectedPlatforms::default(),
        &forced,
    );
    assert!(detected.grok);

    let skipped = parse_overrides("", "grok");
    let detected = apply_overrides(
        crate::manager::platform_detect::DetectedPlatforms {
            grok: true,
            ..Default::default()
        },
        &skipped,
    );
    assert!(!detected.grok);
}

// ---------------------------------------------------------------------------
// PATH honesty: temp HOME / PathPersist double. Live HKCU writes are a defect.
// ---------------------------------------------------------------------------

fn path_test_root(label: &str) -> PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "keel-path-honesty-{}-{}-{}",
        label,
        std::process::id(),
        n
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    root
}

#[test]
fn fail_closed_keel_home_rejects_relative_empty_dot_and_special_chars() {
    for bad in [
        PathBuf::from(""),
        PathBuf::from("."),
        PathBuf::from("relative/.keel"),
        PathBuf::from("/tmp/foo$bar/.keel"),
        PathBuf::from("/tmp/foo;bar/.keel"),
        PathBuf::from("/tmp/foo&bar/.keel"),
        PathBuf::from("/tmp/foo|bar/.keel"),
        PathBuf::from("/tmp/foo%bar/.keel"),
        PathBuf::from("/tmp/foo`bar/.keel"),
        PathBuf::from("/tmp/foo'bar/.keel"),
        PathBuf::from("/tmp/foo\"bar/.keel"),
        PathBuf::from("/tmp/foo\nbar/.keel"),
        PathBuf::from("/tmp/foo\rbar/.keel"),
    ] {
        let result = super::path::validate_keel_home(&bad);
        assert!(
            result.is_err(),
            "expected reject for {:?}, got {result:?}",
            bad
        );
    }
}

#[test]
fn windows_path_persist_appends_and_broadcasts_environment_without_live_reg() {
    let persist = super::path::RecordingPathPersist::new(r"%USERPROFILE%\bin", true);
    let keel_home = PathBuf::from("C:/Users/fixture/.keel");
    let status = super::path::ensure_windows_path(&keel_home, &persist);
    assert!(
        status.contains("keel is on your User PATH"),
        "success copy must name User PATH: {status}"
    );
    assert!(
        status.contains("new console") || status.contains("PATH already configured"),
        "must tell the user this window will not see it unless session already has keel: {status}"
    );
    let writes = persist.writes.lock().expect("lock").clone();
    assert_eq!(writes.len(), 1, "exactly one Path write");
    assert!(
        writes[0].0.contains(r"%USERPROFILE%\bin"),
        "must preserve existing expand entry: {}",
        writes[0].0
    );
    assert!(
        writes[0]
            .0
            .to_lowercase()
            .contains("c:/users/fixture/.keel")
            || writes[0]
                .0
                .to_lowercase()
                .contains("c:\\users\\fixture\\.keel"),
        "must append keel home: {}",
        writes[0].0
    );
    assert!(writes[0].1, "REG_EXPAND_SZ must be preserved");
    let broadcasts = persist.broadcasts.lock().expect("lock").clone();
    assert_eq!(
        broadcasts,
        vec!["Environment".to_string()],
        "WM_SETTINGCHANGE lParam must be Environment only"
    );
}

#[test]
fn windows_path_persist_is_idempotent_and_case_insensitive() {
    let persist = super::path::RecordingPathPersist::new("C:/Users/fixture/.keel", false);
    let keel_home = PathBuf::from("C:/Users/FIXTURE/.keel");
    let status = super::path::ensure_windows_path(&keel_home, &persist);
    assert!(
        status.contains("PATH already configured") || status.contains("keel is on your User PATH"),
        "{status}"
    );
    assert!(
        persist.writes.lock().expect("lock").is_empty(),
        "duplicate Path write is a defect"
    );
    assert!(
        persist.broadcasts.lock().expect("lock").is_empty(),
        "no broadcast when Path is unchanged"
    );
}

#[test]
fn windows_path_persist_process_path_does_not_skip_writer() {
    let persist = super::path::RecordingPathPersist::new("", false);
    let keel_home = PathBuf::from("C:/Users/fixture/.keel");
    let _restore = RestorePath(std::env::var("PATH").ok());
    let mut with_keel = display_path(&keel_home);
    if let Some(existing) = &_restore.0 {
        if cfg!(windows) {
            with_keel = format!("{with_keel};{existing}");
        } else {
            with_keel = format!("{with_keel}:{existing}");
        }
    }
    std::env::set_var("PATH", &with_keel);
    let status = super::path::ensure_windows_path(&keel_home, &persist);
    assert!(
        !persist.writes.lock().expect("lock").is_empty(),
        "process PATH containing keel_home must not skip the persistent writer: {status}"
    );
    assert_eq!(
        persist.broadcasts.lock().expect("lock").as_slice(),
        ["Environment"]
    );
}

#[test]
fn windows_path_persist_rejects_percent_and_does_not_write() {
    let persist = super::path::RecordingPathPersist::new("C:\\Windows", false);
    let keel_home = PathBuf::from("C:/Users/fi%xture/.keel");
    let status = super::path::ensure_windows_path(&keel_home, &persist);
    assert!(
        status.contains("PATH write skipped"),
        "forbidden % must fail closed: {status}"
    );
    assert!(persist.writes.lock().expect("lock").is_empty());
    assert!(persist.broadcasts.lock().expect("lock").is_empty());
}

#[test]
fn windows_purge_stale_uses_seam_not_live_hive() {
    let dead = std::env::temp_dir()
        .join("keel-home-split-purge-fixture")
        .join(".keel");
    let _ = fs::remove_dir_all(dead.parent().unwrap());
    let current = format!("C:\\Windows;{};C:\\Tools", dead.display());
    let persist = super::path::RecordingPathPersist::new(&current, false);
    super::path::purge_stale_windows(&persist).unwrap();
    let writes = persist.writes.lock().expect("lock").clone();
    assert_eq!(writes.len(), 1);
    assert!(
        !writes[0].0.contains("keel-home-split-"),
        "stale temp entry must be removed: {}",
        writes[0].0
    );
    assert!(writes[0].0.contains("C:\\Windows"));
    assert!(writes[0].0.contains("C:\\Tools"));
}

#[cfg(not(windows))]
fn assert_real_home_untouched(before: &[(PathBuf, Option<String>)]) {
    for (path, previous) in before {
        let now = fs::read_to_string(path).ok();
        assert_eq!(
            now.as_deref(),
            previous.as_deref(),
            "real user HOME rc must be untouched: {}",
            path.display()
        );
    }
}

#[cfg(not(windows))]
fn snapshot_real_home_rcs() -> Vec<(PathBuf, Option<String>)> {
    let Ok(home) = crate::runtime::resolve_user_home() else {
        return Vec::new();
    };
    [".profile", ".bashrc", ".bash_profile", ".zshenv", ".zshrc"]
        .into_iter()
        .map(|name| {
            let path = home.join(name);
            (path.clone(), fs::read_to_string(&path).ok())
        })
        .collect()
}

#[cfg(not(windows))]
#[test]
fn unix_path_writes_shared_env_profile_zshenv_and_fish() {
    let snapshot = snapshot_real_home_rcs();
    let root = path_test_root("unix-write");
    let home = root.join("home");
    fs::create_dir_all(&home).unwrap();
    fs::write(home.join(".bashrc"), "# existing bashrc\n").unwrap();
    fs::write(home.join(".zshrc"), "# existing zshrc\n").unwrap();
    let keel_home = home.join(".keel");
    fs::create_dir_all(&keel_home).unwrap();
    let dir = display_path(&keel_home);
    let wrote = super::path::unix_write_path_into(&keel_home, &home).unwrap();
    assert!(wrote);

    let env = fs::read_to_string(keel_home.join("env")).unwrap();
    assert!(
        env.contains(&format!("export PATH=\"{dir}:$PATH\"")),
        "posix env must PATH-prepend the validated dir: {env}"
    );
    assert!(
        env.contains("case \":${PATH}:\"") || env.contains("case \":${PATH}:\""),
        "posix env must be rustup-shaped: {env}"
    );

    let fish_env = fs::read_to_string(keel_home.join("env.fish")).unwrap();
    assert!(
        fish_env.contains("if not contains") && fish_env.contains("set -x PATH"),
        "fish env.fish must prepend PATH in fish syntax: {fish_env}"
    );
    assert!(
        !fish_env.contains("export"),
        "fish must not use export: {fish_env}"
    );
    assert!(
        !fish_env.contains("fish_add_path"),
        "fish_add_path must not be the mechanism: {fish_env}"
    );

    let profile = fs::read_to_string(home.join(".profile")).unwrap();
    assert!(
        profile
            .lines()
            .any(|line| line.trim() == super::path::KEEL_PATH_MARKER),
        "marker must be a whole line: {profile}"
    );
    assert!(
        profile.contains(&format!(". \"{}/env\"", dir)),
        ".profile must source the shared env: {profile}"
    );
    assert!(
        !profile.contains("export PATH=") || profile.contains(". \""),
        "raw export PATH must not be the only mechanism: {profile}"
    );

    let zshenv = fs::read_to_string(home.join(".zshenv")).unwrap();
    assert!(
        zshenv.contains(&format!(". \"{}/env\"", dir)),
        ".zshenv must source the shared env so zsh -c works: {zshenv}"
    );
    let zshrc = fs::read_to_string(home.join(".zshrc")).unwrap();
    assert!(
        zshrc.contains(&format!(". \"{}/env\"", dir)),
        "existing .zshrc must also source the env: {zshrc}"
    );
    let bashrc = fs::read_to_string(home.join(".bashrc")).unwrap();
    assert!(
        bashrc.contains(&format!(". \"{}/env\"", dir)),
        "existing .bashrc must source the env: {bashrc}"
    );
    assert!(
        !home.join(".bash_profile").exists(),
        "must not create .bash_profile"
    );

    let fish_conf = home
        .join(".config")
        .join("fish")
        .join("conf.d")
        .join("keel.fish");
    let fish_conf_text = fs::read_to_string(&fish_conf).unwrap();
    assert!(
        fish_conf_text.contains(&format!("source \"{}/env.fish\"", dir)),
        "fish conf.d must source env.fish: {fish_conf_text}"
    );
    assert!(
        !fish_conf_text.contains("export"),
        "fish conf.d must not use export: {fish_conf_text}"
    );

    let env_mode = fs::metadata(keel_home.join("env")).unwrap().permissions();
    let fish_mode = fs::metadata(keel_home.join("env.fish"))
        .unwrap()
        .permissions();
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(env_mode.mode() & 0o777, env_mode.mode() & 0o644);
    assert_eq!(fish_mode.mode() & 0o777, fish_mode.mode() & 0o644);

    let wrote_again = super::path::unix_write_path_into(&keel_home, &home).unwrap();
    assert!(!wrote_again, "second install must be idempotent");
    let profile_again = fs::read_to_string(home.join(".profile")).unwrap();
    assert_eq!(
        profile.matches(super::path::KEEL_PATH_MARKER).count(),
        profile_again.matches(super::path::KEEL_PATH_MARKER).count()
    );

    assert_real_home_untouched(&snapshot);
    let _ = fs::remove_dir_all(&root);
}

#[cfg(not(windows))]
#[test]
fn unix_path_updates_existing_bash_profile_only() {
    let root = path_test_root("unix-bash-profile");
    let home = root.join("home");
    fs::create_dir_all(&home).unwrap();
    fs::write(home.join(".bash_profile"), "# existing\n").unwrap();
    let keel_home = home.join(".keel");
    fs::create_dir_all(&keel_home).unwrap();
    let dir = display_path(&keel_home);
    super::path::unix_write_path_into(&keel_home, &home).unwrap();
    let text = fs::read_to_string(home.join(".bash_profile")).unwrap();
    assert!(text.contains("# existing"));
    assert!(text.contains(&format!(". \"{}/env\"", dir)));
    let _ = fs::remove_dir_all(&root);
}

#[cfg(not(windows))]
#[test]
fn unix_fail_closed_skips_write() {
    let snapshot = snapshot_real_home_rcs();
    let root = path_test_root("unix-reject");
    let home = root.join("home");
    fs::create_dir_all(&home).unwrap();
    let relative = PathBuf::from("relative/.keel");
    let status = super::path::validate_keel_home(&relative);
    assert!(status.is_err());
    assert!(!home.join(".profile").exists());
    assert!(!home.join(".zshenv").exists());
    let special = home.join("foo$bar").join(".keel");
    assert!(super::path::validate_keel_home(&special).is_err());
    assert_real_home_untouched(&snapshot);
    let _ = fs::remove_dir_all(&root);
}

struct RestorePath(Option<String>);
impl Drop for RestorePath {
    fn drop(&mut self) {
        match &self.0 {
            Some(value) => std::env::set_var("PATH", value),
            None => std::env::remove_var("PATH"),
        }
    }
}

#[cfg(not(windows))]
#[test]
fn unix_process_path_does_not_skip_persistent_writers() {
    let root = path_test_root("unix-process-path");
    let home = root.join("home");
    fs::create_dir_all(&home).unwrap();
    let keel_home = home.join(".keel");
    fs::create_dir_all(&keel_home).unwrap();
    let _restore = RestorePath(std::env::var("PATH").ok());
    let mut path_value = display_path(&keel_home);
    if let Some(existing) = &_restore.0 {
        path_value = format!("{path_value}:{existing}");
    }
    std::env::set_var("PATH", &path_value);
    let status = super::path::ensure_unix_path_for_home(&keel_home, &home);
    assert!(
        keel_home.join("env").is_file(),
        "process PATH must not skip env write: {status}"
    );
    assert!(
        keel_home.join("env.fish").is_file(),
        "process PATH must not skip fish env write: {status}"
    );
    assert!(
        home.join(".profile").is_file(),
        "process PATH must not skip .profile: {status}"
    );
    let _ = fs::remove_dir_all(&root);
}

#[cfg(not(windows))]
#[test]
fn unix_uninstall_sweeps_old_triplicate_export() {
    let root = path_test_root("unix-sweep");
    let home = root.join("home");
    fs::create_dir_all(&home).unwrap();
    let keel_home = home.join(".keel");
    fs::create_dir_all(&keel_home).unwrap();
    let dir = display_path(&keel_home);
    let old = format!(
        "# user stuff\n{}\nexport PATH=\"{dir}:$PATH\"\n# more\n",
        super::path::KEEL_PATH_MARKER
    );
    fs::write(home.join(".profile"), &old).unwrap();
    fs::write(home.join(".bashrc"), &old).unwrap();
    super::path::unix_remove_path_into(&keel_home, &home).unwrap();
    let profile = fs::read_to_string(home.join(".profile")).unwrap();
    assert!(
        profile.contains("# user stuff"),
        "unmanaged lines must stay: {profile}"
    );
    assert!(
        !profile.contains("export PATH="),
        "old triplicate export must be swept: {profile}"
    );
    assert!(
        !profile.contains(super::path::KEEL_PATH_MARKER),
        "marker must be removed: {profile}"
    );
    let bashrc = fs::read_to_string(home.join(".bashrc")).unwrap();
    assert!(!bashrc.contains("export PATH="));
    let _ = fs::remove_dir_all(&root);
}

#[cfg(not(windows))]
#[test]
fn unix_purge_stale_sweeps_dead_temp_export_only() {
    let root = path_test_root("unix-purge");
    let home = root.join("home");
    fs::create_dir_all(&home).unwrap();
    let dead = root
        .join(format!("keel-home-split-{}", std::process::id()))
        .join(".keel");
    let _ = fs::remove_dir_all(dead.parent().unwrap());
    let stale_export = format!(
        "{}\nexport PATH=\"{}:$PATH\"\n",
        super::path::KEEL_PATH_MARKER,
        dead.display()
    );
    fs::write(home.join(".profile"), format!("# keep\n{stale_export}")).unwrap();
    super::path::purge_stale_unix_into(&home).unwrap();
    let profile = fs::read_to_string(home.join(".profile")).unwrap();
    assert!(profile.contains("# keep"));
    assert!(!profile.contains("export PATH="));
    assert!(!profile.contains(super::path::KEEL_PATH_MARKER));
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn windows_remove_path_uses_seam_not_live_hive() {
    let persist = super::path::RecordingPathPersist::new(
        r"C:\Windows;C:/Users/fixture/.keel;C:\Tools",
        false,
    );
    let keel_home = PathBuf::from("C:/Users/fixture/.keel");
    assert!(super::path::remove_windows_path(&keel_home, &persist).unwrap());
    let writes = persist.writes.lock().expect("lock").clone();
    assert_eq!(writes.len(), 1);
    let lowered = writes[0].0.to_lowercase();
    assert!(
        !lowered.contains("fixture"),
        "keel home must leave user Path: {}",
        writes[0].0
    );
    assert!(lowered.contains(r"c:\windows"), "{}", writes[0].0);
    assert!(lowered.contains(r"c:\tools"), "{}", writes[0].0);
    assert_eq!(
        persist.broadcasts.lock().expect("lock").as_slice(),
        ["Environment"]
    );
}
