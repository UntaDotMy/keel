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

/// The documented counts: 43 matcher-invocable skills in the manifest, 44
/// first-party SKILL.md directories on disk (the 43 + the bootstrap), and the
/// bootstrap is the only first-party skill NOT in the manifest. If a skill is
/// added or removed, this assertion fails until CLAUDE.md / using-keel
/// and this test are updated together — that is the anti-drift mechanism.
#[test]
fn documented_skill_counts_match_disk_and_manifest() {
    let repo_root = repository_root();
    let manifest = manifest_skills(&repo_root);
    let on_disk = first_party_skill_dirs(&repo_root);

    assert_eq!(
        manifest.len(),
        43,
        "expected 43 manifest skills (24 specialists + 18 technique + requesting-code-review); \
         got {}. If this changed intentionally, update CLAUDE.md and using-keel/SKILL.md.",
        manifest.len()
    );
    assert_eq!(
        on_disk.len(),
        44,
        "expected 44 first-party SKILL.md dirs (43 manifest skills + using-keel bootstrap); \
         got {}. Update the docs and this test together.",
        on_disk.len()
    );
    assert!(
        on_disk.contains("using-keel"),
        "using-keel bootstrap directory must exist"
    );
    assert!(
        !manifest.iter().any(|name| name == "using-keel"),
        "the bootstrap must NOT be in the manifest (it loads at SessionStart, not via the matcher)"
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
    let tool_count = tools_src.matches("\"inputSchema\":").count();
    assert_eq!(
        tool_count, 16,
        "expected 16 MCP tool definitions in mcp/tools.rs (one `\"inputSchema\":` each); got {tool_count}. \
         If this changed intentionally, update the MCP server tool count in CLAUDE.md and this test together."
    );

    let claude_md = fs::read_to_string(repo_root.join("CLAUDE.md")).expect("read CLAUDE.md");
    assert!(
        claude_md.contains(&format!("{tool_count} tools")),
        "CLAUDE.md must document the MCP server as exposing {tool_count} tools (the count in mcp/tools.rs); \
         the prose says a different number, which is exactly the drift this test prevents."
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
