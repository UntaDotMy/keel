//! Doc-parity integration test.
//!
//! Purpose: enforce the manifest⇆disk⇆docs correspondence the competitive audit
//!   flagged as drift (CLAUDE.md/using-keel claimed "43 directories" and
//!   that `requesting-code-review` is "not a directory" — both wrong by one).
//!   These assertions derive the truth from disk + `.claude-plugin/plugin.json`
//!   so the documented counts can no longer rot silently: add or remove a skill
//!   and the count assertion fails CI until the docs and this test are updated
//!   together.
//!
//! Why a test instead of trusting prose: numbers typed into Markdown have no
//! mechanical link to the tree they describe. This is the same gate philosophy
//! as skill-lint and config-audit — catch a malformed/stale artifact before it
//! is trusted, deterministically, without invoking the live model.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

/// Repo root: the keel crate lives at `rust/crates/keel`, so
/// the workspace root is three ancestors up. Mirrors `manager_provision_test`.
fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("workspace repository root")
        .to_path_buf()
}

/// Skills listed in the plugin manifest, normalized to bare directory names
/// (the manifest stores `"./<name>"` entries).
fn manifest_skills(repo_root: &Path) -> Vec<String> {
    let manifest_path = repo_root.join(".claude-plugin").join("plugin.json");
    let text = fs::read_to_string(&manifest_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", manifest_path.display()));
    let manifest: Value = serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("parse {}: {e}", manifest_path.display()));
    manifest
        .get("skills")
        .and_then(Value::as_array)
        .expect("plugin.json has a `skills` array")
        .iter()
        .map(|entry| {
            entry
                .as_str()
                .expect("each skills entry is a string")
                .trim_start_matches("./")
                .to_string()
        })
        .collect()
}

/// First-party skill directories: a `<name>/SKILL.md` directly under the repo
/// root. Excludes hidden dirs and the vendored `karpathy-skills-cmp/` tree,
/// which holds a benchmark-artifact SKILL.md that is not a keel skill.
fn first_party_skill_dirs(repo_root: &Path) -> BTreeSet<String> {
    let mut dirs = BTreeSet::new();
    for entry in fs::read_dir(repo_root).expect("read repo root") {
        let entry = entry.expect("read dir entry");
        if !entry.path().is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || name == "karpathy-skills-cmp" {
            continue;
        }
        if entry.path().join("SKILL.md").is_file() {
            dirs.insert(name);
        }
    }
    dirs
}

/// Every first-party skill directory (except the `using-keel` bootstrap,
/// which loads at SessionStart rather than being matcher-invoked) must be listed
/// in the manifest. A skill dir absent from the manifest never ships.
#[test]
fn every_first_party_skill_is_in_manifest() {
    let repo_root = repository_root();
    let manifest: BTreeSet<String> = manifest_skills(&repo_root).into_iter().collect();
    let on_disk = first_party_skill_dirs(&repo_root);

    let not_in_manifest: Vec<&String> = on_disk
        .iter()
        .filter(|name| name.as_str() != "using-keel" && !manifest.contains(*name))
        .collect();
    assert!(
        not_in_manifest.is_empty(),
        "skill directories present on disk but missing from plugin.json: {not_in_manifest:?}"
    );
}

/// Every manifest skill resolves to a real directory on disk.
#[test]
fn manifest_skills_all_resolve_to_directories() {
    let repo_root = repository_root();
    let on_disk = first_party_skill_dirs(&repo_root);
    let missing: Vec<String> = manifest_skills(&repo_root)
        .into_iter()
        .filter(|name| !on_disk.contains(name))
        .collect();
    assert!(
        missing.is_empty(),
        "plugin.json lists skills with no <name>/SKILL.md on disk: {missing:?}"
    );
}

/// `requesting-code-review` is a REAL directory holding a thin alias skill, and
/// it is in the manifest. The docs previously claimed it was "an alias pointer,
/// not a directory" — this pins the corrected truth.
#[test]
fn requesting_code_review_is_a_real_manifest_directory() {
    let repo_root = repository_root();
    assert!(
        repo_root
            .join("requesting-code-review")
            .join("SKILL.md")
            .is_file(),
        "requesting-code-review/SKILL.md must exist on disk (it is a real alias directory)"
    );
    assert!(
        manifest_skills(&repo_root)
            .iter()
            .any(|name| name == "requesting-code-review"),
        "requesting-code-review must be listed in plugin.json"
    );
}

/// The structural invariant between disk and manifest: the `using-keel`
/// bootstrap is the ONLY first-party skill directory on disk that is NOT in
/// the plugin manifest (it loads at SessionStart, not via the matcher). So
/// `on_disk.len()` must equal `manifest.len() + 1`, and the +1 must be exactly
/// `using-keel`. This asserts the *relationship* rather than a hardcoded
/// count, so adding or removing a skill never requires editing this test.
/// The drift is still caught (an orphan skill dir, or a manifest entry with
/// no directory) without magic numbers. Run `keel skill-lint` for the live
/// count; the docs describe the structure, not a number.
#[test]
fn disk_and_manifest_differ_by_exactly_the_bootstrap() {
    let repo_root = repository_root();
    let manifest: BTreeSet<String> = manifest_skills(&repo_root).into_iter().collect();
    let on_disk = first_party_skill_dirs(&repo_root);

    assert!(
        on_disk.contains("using-keel"),
        "using-keel bootstrap directory must exist on disk"
    );
    assert!(
        !manifest.contains("using-keel"),
        "the bootstrap must NOT be in the manifest (it loads at SessionStart, not via the matcher)"
    );
    let bootstrap = String::from("using-keel");
    let non_manifest: BTreeSet<&String> =
        on_disk.iter().filter(|n| !manifest.contains(*n)).collect();
    let expected: BTreeSet<&String> = std::iter::once(&bootstrap).collect();
    assert_eq!(
        non_manifest,
        expected,
        "the bootstrap must be the ONLY first-party skill on disk but not in the manifest; \
         found {} non-manifest skill dir(s): {non_manifest:?}. \
         Either add the new skill to .claude-plugin/plugin.json, or it is an orphan to remove.",
        non_manifest.len()
    );
    assert_eq!(
        on_disk.len(),
        manifest.len() + 1,
        "disk skill dirs ({}) must equal manifest skills ({}) + 1 bootstrap; \
         if this fails, a skill was added/removed on one side only",
        on_disk.len(),
        manifest.len()
    );
}

/// Read a scalar field from the leading `---`-delimited YAML frontmatter block of
/// a Markdown file. Line-based on purpose: the subagent frontmatter we assert on
/// is flat `key: value` pairs, so this stays dependency-free (no serde_yaml) and
/// deterministic, matching the gate philosophy of the rest of this file. Returns
/// the trimmed value with surrounding quotes stripped, or `None` if absent.
fn frontmatter_field(markdown: &str, key: &str) -> Option<String> {
    let mut lines = markdown.lines();
    // The file must open with a frontmatter fence.
    if lines.next().map(str::trim) != Some("---") {
        return None;
    }
    for line in lines {
        let trimmed = line.trim();
        if trimmed == "---" {
            break; // end of frontmatter
        }
        if let Some((found_key, value)) = trimmed.split_once(':') {
            if found_key.trim() == key {
                let value = value.trim().trim_matches(|c| c == '"' || c == '\'');
                return Some(value.to_string());
            }
        }
    }
    None
}

/// Cross-model review invariant (finding #5): the `reviewer` subagent must pin an
/// explicit, fixed model that is NOT `inherit`. This guarantees the review gate
/// runs on a known model independent of whatever model authored the code, so the
/// review is a genuine second opinion rather than the same model grading its own
/// work. Without this assertion the cross-model property holds only by accident
/// (implementer happens to run a different model than the reviewer default); a
/// future edit to `model: inherit` would silently collapse review onto the
/// author's model with no visible failure. The test makes that regression loud.
#[test]
fn reviewer_subagent_pins_an_explicit_non_inherit_model() {
    let repo_root = repository_root();
    let reviewer_path = repo_root.join(".claude").join("agents").join("reviewer.md");
    let text = fs::read_to_string(&reviewer_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", reviewer_path.display()));

    let model = frontmatter_field(&text, "model").unwrap_or_else(|| {
        panic!(
            "{} frontmatter must declare an explicit `model:` so review runs on a fixed model",
            reviewer_path.display()
        )
    });

    assert!(
        !model.is_empty(),
        "reviewer.md `model:` must not be empty — it must name a concrete model for cross-model review"
    );
    assert_ne!(
        model, "inherit",
        "reviewer.md must NOT use `model: inherit` — review must run on a fixed model independent of \
         the implementer's model, otherwise the gate loses its cross-model second-opinion property"
    );
}

/// The MCP server's tool count is asserted in CLAUDE.md prose ("gets N tools").
/// Nothing mechanically tied that number to `mcp/tools.rs`, so the competitive
/// audit caught CLAUDE.md still claiming 14 tools after `sprint` and
/// `user_story_lint` were added (making 16). This test counts the tool
/// definitions in code (each carries exactly one `"inputSchema":` key in
/// `handle_tools_list`) and asserts CLAUDE.md documents that same count, so the
/// number can no longer drift silently: add or remove an MCP tool and this fails
/// until the prose is updated to match.
#[test]
fn mcp_tool_count_matches_documentation() {
    let repo_root = repository_root();
    let tools_src = fs::read_to_string(
        repo_root
            .join("rust")
            .join("crates")
            .join("keel")
            .join("src")
            .join("mcp")
            .join("tools.rs"),
    )
    .expect("read mcp/tools.rs");

    // Each tool definition in `handle_tools_list` has exactly one inputSchema.
    // The count is DERIVED from source, not hardcoded here, so adding a tool
    // never requires editing this test. Docs point at this test, not a number.
    let tool_count = tools_src.matches("\"inputSchema\":").count();
    assert!(
        tool_count >= 20,
        "expected a healthy MCP tool surface (≥20 definitions, one `\"inputSchema\":` each); \
         got {tool_count}. If tools were removed intentionally, confirm the surface is still healthy."
    );

    let claude_md = fs::read_to_string(repo_root.join("CLAUDE.md")).expect("read CLAUDE.md");
    assert!(
        claude_md.contains("MCP tool count") || claude_md.contains("tool definitions in `mcp/tools.rs`"),
        "CLAUDE.md must point at this test for the MCP tool count rather than hardcoding a number; \
         the prose should say the count is asserted by `tests/doc_parity_test.rs` over `mcp/tools.rs`."
    );
}

/// Commands wired in `commands.rs` that the competitive audit flagged as
/// undocumented (`dispatch`, `observe`) plus the real `eval` measurement must
/// appear in CLAUDE.md so the Commands section reflects the shipped CLI surface.
/// This is narrow on purpose — it pins the specific drift the audit found rather
/// than asserting every internal verb (e.g. `raw`, `replay`, `__self-replace`)
/// is documented — but it makes "shipped a new user-facing command without
/// documenting it" a red CI check for these three.
#[test]
fn audit_flagged_commands_are_documented() {
    let repo_root = repository_root();
    let commands_src = fs::read_to_string(
        repo_root
            .join("rust")
            .join("crates")
            .join("keel")
            .join("src")
            .join("commands.rs"),
    )
    .expect("read commands.rs");
    let claude_md = fs::read_to_string(repo_root.join("CLAUDE.md")).expect("read CLAUDE.md");

    for command in ["dispatch", "observe", "eval"] {
        // Confirm the command is actually wired (a match arm) before requiring docs.
        assert!(
            commands_src.contains(&format!("\"{command}\" =>")),
            "expected `{command}` to be a wired command arm in commands.rs"
        );
        assert!(
            claude_md.contains(&format!("keel {command}")),
            "CLAUDE.md must document the `{command}` command (it is wired in commands.rs but missing \
             from the Commands section — the exact drift the competitive audit flagged)."
        );
    }
}

/// The release bundle must stage the cross-agent adapter source directories
/// (opencode/codex/pi/cursor/cowork) so `keel install` run from a release-bundle
/// extract can wire non-Claude-Code targets. `maybe_wire_opencode` /
/// `maybe_wire_codex` / `maybe_wire_pi` / `maybe_wire_cursor` / `maybe_wire_cowork` in
/// `manager/install.rs` read these dirs from `repository_root`; if the release
/// staging omits them, install silently reports "plugin source absent" /
/// "source absent" for every adapter — the exact bug that shipped. This test
/// pins the staging so the gap cannot recur: remove the staging and CI fails.
///
/// Bridge subcommand parity: the `keel bridge <event>` match arms in
/// `runner/bridge.rs` are the single source of truth for which subcommands
/// exist. CLAUDE.md's OpenCode-host section and the in-binary `render_bridge_help`
/// text must each mention every wired arm — otherwise a maintainer reading either
/// the docs or the `keel bridge help` output would not know the full surface.
/// This was the exact drift a competitive audit flagged (CLAUDE.md enumerated six
/// subcommands while the match surface had eight, omitting `pre-tool-use` and
/// `rewrite`, both actively called by the OpenCode/Codex/Pi/Cursor adapters).
/// Add or rename a bridge arm and this test fails CI until the docs and help
/// text are updated together.
#[test]
fn bridge_subcommands_are_documented_and_in_help() {
    let repo_root = repository_root();
    let bridge_src = fs::read_to_string(
        repo_root
            .join("rust")
            .join("crates")
            .join("keel")
            .join("src")
            .join("runner")
            .join("bridge.rs"),
    )
    .expect("read bridge.rs");

    // Derive the wired subcommand set from the `match arguments[0].as_str()` arms.
    // Each arm looks like `"session-start" => run_bridge_session_start(...)`; we
    // capture the quoted slug before ` =>`. This is the dispatch surface, not the
    // hand-maintained help text, so it cannot itself drift.
    let mut wired: BTreeSet<String> = BTreeSet::new();
    for line in bridge_src.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix('"') {
            if let Some(end) = rest.find('"') {
                let slug = &rest[..end];
                if slug.chars().all(|c| c.is_ascii_lowercase() || c == '-') && !slug.is_empty() {
                    // Only collect arms inside the dispatch match (the help renderer
                    // also has quoted strings, but those are indented differently and
                    // are not `"<slug>" =>` arms). Confirm the ` =>` follows.
                    let after = rest[end + 1..].trim_start();
                    if after.starts_with("=>") {
                        wired.insert(slug.to_string());
                    }
                }
            }
        }
    }
    assert!(
        !wired.is_empty(),
        "failed to parse any bridge match arms from bridge.rs — the parser is stale"
    );
    // Sanity: the known bridge subcommands must all be present; if this fails,
    // the arm-parsing logic above drifted, not the docs.
    for expected in [
        "session-start",
        "user-prompt",
        "observe",
        "session-end",
        "post-compact",
        "gate-status",
        "pre-tool-use",
        "rewrite",
    ] {
        assert!(
            wired.contains(expected),
            "expected `{expected}` to be a wired bridge arm; the test parser may need updating \
             if the match shape changed"
        );
    }

    let claude_md = fs::read_to_string(repo_root.join("CLAUDE.md")).expect("read CLAUDE.md");

    // The help text is rendered by `render_bridge_help` inside bridge.rs itself.
    for slug in &wired {
        assert!(
            bridge_src.contains(&format!(" {slug} ")),
            "`render_bridge_help` in bridge.rs must list the `{slug}` subcommand in its \
             Subcommands block; it is a wired match arm but missing from the help text"
        );
        assert!(
            claude_md.contains(slug.as_str()),
            "CLAUDE.md must document the `{slug}` bridge subcommand (it is a wired match arm in \
             bridge.rs but missing from the OpenCode-host bridge section — the exact drift the \
             competitive audit flagged). Add it to the `keel bridge <event>` enumeration."
        );
    }
}

/// The release bundle must stage the cross-agent adapter source directories
/// (opencode/codex/pi/cursor/cowork) so `keel install` run from a release-bundle
/// extract can wire non-Claude-Code targets. `maybe_wire_opencode` /
/// `maybe_wire_codex` / `maybe_wire_pi` / `maybe_wire_cursor` / `maybe_wire_cowork` in
/// `manager/install.rs` read these dirs from `repository_root`; if the release
/// staging omits them, install silently reports "plugin source absent" /
/// "source absent" for every adapter — the exact bug that shipped. This test
/// pins the staging so the gap cannot recur: remove the staging and CI fails.
#[test]
fn release_bundle_stages_adapter_source_dirs() {
    let repo_root = repository_root();
    let release_yml = fs::read_to_string(
        repo_root
            .join(".github")
            .join("workflows")
            .join("release.yml"),
    )
    .expect("read .github/workflows/release.yml");

    for adapter in ["opencode", "codex", "pi", "cursor", "cowork"] {
        let staged_explicit = release_yml.contains(&format!("cp -R \"{adapter}\""))
            || release_yml.contains(&format!("cp -R {adapter} "));
        let staged_loop = release_yml.contains("for adapter in opencode codex pi cursor cowork");
        assert!(
            staged_explicit || staged_loop,
            "release.yml must stage the `{adapter}` adapter dir into the release bundle; \
             without it `keel install` reports \"plugin source absent\" / \"source absent\" \
             for the {adapter} adapter when run from a release-bundle extract. \
             Add the adapter to the staging loop or as an explicit `cp -R`."
        );
    }
}
