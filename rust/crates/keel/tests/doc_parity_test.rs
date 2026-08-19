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
/// root. Excludes hidden dirs and an optional vendored `karpathy-skills-cmp/`
/// tree (historical benchmark artifact; may be absent; never a keel skill).
fn first_party_skill_dirs(repo_root: &Path) -> BTreeSet<String> {
    let mut dirs = BTreeSet::new();
    for entry in fs::read_dir(repo_root).expect("read repo root") {
        let entry = entry.expect("read dir entry");
        if !entry.path().is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        // Keep the skip so a reintroduced vendor tree is not treated as a skill.
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

/// Every MCP tool name defined in `mcp/tools.rs` must be listed in README's MCP
/// row. The count-only guard below cannot catch a name going undocumented:
/// README enumerated far fewer tools than `tools.rs` defined, leaving `flow`,
/// `work`, `code_graph`, `learn`, `observe`, `rewrite`, `skill_eval`,
/// `dispatch`, and `design_intelligence` invisible to anyone reading the docs.
/// Names are derived from source (each definition pairs one `"name":` with one
/// `"inputSchema":`), so adding a tool fails CI until README lists it.
#[test]
fn every_mcp_tool_is_listed_in_readme() {
    let repo_root = repository_root();
    let tools = mcp_tool_names(&repo_root);
    assert!(
        tools.len() >= 20,
        "expected a healthy MCP tool surface, parsed {} name(s); the parser may be stale",
        tools.len()
    );

    let readme = fs::read_to_string(repo_root.join("README.md")).expect("read README.md");
    let missing: Vec<&String> = tools
        .iter()
        .filter(|name| !readme.contains(&format!("`{name}`")))
        .collect();
    assert!(
        missing.is_empty(),
        "MCP tools defined in mcp/tools.rs but absent from README: {missing:?}. \
         Add them to the MCP server row so the documented surface matches the shipped one."
    );
}

/// Tool names from `mcp/tools.rs`, derived by pairing each `"inputSchema":` key
/// with the nearest preceding `"name":`, the shape every definition in
/// `handle_tools_list` uses. Scoping to `inputSchema` keeps unrelated `"name":`
/// literals elsewhere in the file out of the set.
fn mcp_tool_names(repo_root: &Path) -> BTreeSet<String> {
    let source = fs::read_to_string(
        repo_root
            .join("rust")
            .join("crates")
            .join("keel")
            .join("src")
            .join("mcp")
            .join("tools.rs"),
    )
    .expect("read mcp/tools.rs");

    let mut names = BTreeSet::new();
    let mut pending: Option<String> = None;
    // Scope to `handle_tools_list`: an earlier `#[cfg(test)]` module and later
    // test fixtures both use `"name":`/`"inputSchema":` shapes without being tools.
    let mut inside = false;
    for line in source.lines() {
        let trimmed = line.trim();
        if !inside {
            inside = trimmed.starts_with("pub(super) fn handle_tools_list");
            continue;
        }
        if trimmed.starts_with("#[cfg(test)]") {
            break;
        }
        if let Some(rest) = trimmed.strip_prefix("\"name\":") {
            let value = rest.trim().trim_end_matches(',').trim().trim_matches('"');
            if !value.is_empty() {
                pending = Some(value.to_string());
            }
        } else if trimmed.starts_with("\"inputSchema\":") {
            if let Some(name) = pending.take() {
                names.insert(name);
            }
        }
    }
    names
}

/// Guards the MCP tool surface against collapsing, and pins the docs contract:
/// CLAUDE.md must point at this test rather than hardcode a number. It
/// deliberately does not compare a documented count to the source count, because
/// the docs policy is to state no number at all. Enforcement that a specific tool
/// is documented lives in `every_mcp_tool_is_listed_in_readme`.
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

    // why: a curated list, not every arm, since internal verbs like `raw` and
    // `menu` are not operator-facing and requiring docs for them is noise.
    for command in [
        "anvil",
        "observe",
        "eval",
        "design-intelligence",
        "skill-eval",
        "telemetry",
        "session",
        "learn",
    ] {
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
        "pre-compact",
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

/// Every long flag advertised in the operator help must actually be read by the
/// code. Three phantom flags shipped before this guard existed: `--review-surface`
/// and `--review-base-ref` were registered but never read, and `--repo-test-policy`
/// was advertised in help while `gates check` ran the suite unconditionally. Two
/// more (`--allow-claude-code-wording`, `--memory-base`) were advertised but never
/// registered at all, so following the help produced a hard error. Flags are
/// derived from the help text and matched against `*_value("name")` reads.
#[test]
fn every_help_advertised_flag_is_read_by_the_code() {
    let repo_root = repository_root();
    let src = repo_root
        .join("rust")
        .join("crates")
        .join("keel")
        .join("src");
    let help = fs::read_to_string(src.join("help_operator.txt")).expect("read help_operator.txt");

    // Long flags named in the help text, e.g. `[--base-ref <ref>]`.
    let mut advertised: BTreeSet<String> = BTreeSet::new();
    for token in help.split(|c: char| c.is_whitespace() || c == '[' || c == ']' || c == '|') {
        if let Some(name) = token.strip_prefix("--") {
            let name = name.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-');
            if name.len() > 1 && name.contains(|c: char| c.is_ascii_alphabetic()) {
                advertised.insert(name.to_string());
            }
        }
    }
    assert!(
        advertised.len() > 20,
        "parsed only {} help flags; the parser is stale",
        advertised.len()
    );

    // Every `*_value("name")` read anywhere in the crate.
    let mut read: BTreeSet<String> = BTreeSet::new();
    for path in rust_source_files(&src) {
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        let mut from = 0usize;
        while let Some(found) = text[from..].find("_value(\"") {
            let start = from + found + "_value(\"".len();
            if let Some(end) = text[start..].find('"') {
                read.insert(text[start..start + end].to_string());
                from = start + end;
            } else {
                break;
            }
        }
    }

    let phantom: Vec<&String> = advertised.difference(&read).collect();
    assert!(
        phantom.is_empty(),
        "flags advertised in help_operator.txt but never read via *_value(): {phantom:?}. \
         Either wire the flag or remove it from the help — a documented flag that does \
         nothing (or is not registered at all) misleads every operator who follows the help."
    );
}

/// Every host adapter that speaks the `keel bridge` protocol. Claude Desktop
/// (cowork) is deliberately absent: Desktop exposes no hook API, so that host is
/// MCP-only and ships no adapter script.
const BRIDGE_ADAPTER_FILES: &[&str] = &[
    "opencode/keel.ts",
    "codex/keel-codex.ts",
    "pi/keel-pi.ts",
    "cursor/hooks/keel-cursor.sh",
    "commandcode/keel-cmdc.ts",
];

/// Read an adapter, failing loudly when it is absent.
///
/// why: these tests used `let Ok(text) = read else { continue }`, so a listed
/// adapter that did not exist passed every check silently. `cowork/keel.ts` was
/// listed, documented in two places, and had never existed.
fn read_adapter(path: &Path, adapter: &str) -> String {
    fs::read_to_string(path).unwrap_or_else(|error| {
        panic!(
            "adapter `{adapter}` is listed in BRIDGE_ADAPTER_FILES but could not be read \
             ({error}). Either ship the file or remove it from the list, so the checks below \
             cannot pass vacuously."
        )
    })
}

/// The gate-clearing subcommand list, parsed out of the Rust source of truth.
fn rust_research_subcommands(repo_root: &Path) -> BTreeSet<String> {
    let source =
        fs::read_to_string(repo_root.join("rust/crates/keel/src/runner/hook_lifecycle/mod.rs"))
            .expect("read hook_lifecycle/mod.rs");
    let start = source
        .find("const HITS: &[&str] = &[")
        .expect("locate the HITS list in is_keel_research_command");
    let body = &source[start..];
    let end = body.find("];").expect("HITS list terminator");
    quoted_literals(&body[..end])
}

/// Every `"..."` literal in `text`, as a set.
fn quoted_literals(text: &str) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let mut rest = text;
    while let Some(open) = rest.find('"') {
        rest = &rest[open + 1..];
        let Some(close) = rest.find('"') else { break };
        let value = &rest[..close];
        if !value.is_empty() {
            found.insert(value.to_string());
        }
        rest = &rest[close + 1..];
    }
    found
}

/// Every adapter's Iron Law gate-clearing list must equal the Rust one, and every
/// adapter must refuse compound commands.
///
/// why: the Rust list dropped `help`/`status`/`brief`/`workflow` and added
/// compound-command rejection, but three TypeScript adapters and one shell
/// adapter kept byte-identical copies of the old permissive list. All hosts write
/// the SAME `iron-law-satisfied/<key>` marker, so a lax clear on any one of them
/// cleared the Rust gate too. Drift here is a security hole, not a style nit.
#[test]
fn adapter_gate_lists_match_the_rust_source_of_truth() {
    let repo_root = repository_root();
    let expected = rust_research_subcommands(&repo_root);
    assert!(
        expected.contains("recall") && !expected.contains("help"),
        "parser looks stale; got {expected:?}"
    );

    for adapter in BRIDGE_ADAPTER_FILES {
        let text = read_adapter(&repo_root.join(adapter), adapter);

        // Shell keeps the list in one space-separated string on a single line;
        // TypeScript uses one literal per entry up to the closing `];`.
        let actual: BTreeSet<String> =
            if adapter.ends_with(".sh") {
                let line = text
                    .lines()
                    .find(|line| line.trim_start().starts_with("KEEL_RESEARCH_SUBCOMMANDS="))
                    .unwrap_or_else(|| {
                        panic!("{adapter} must declare KEEL_RESEARCH_SUBCOMMANDS on one line")
                    });
                quoted_literals(line)
                    .iter()
                    .flat_map(|value| value.split_whitespace())
                    .map(str::to_string)
                    .collect()
            } else {
                let start = text.find("KEEL_RESEARCH_SUBCOMMANDS = [").unwrap_or_else(|| {
                panic!("{adapter} must declare KEEL_RESEARCH_SUBCOMMANDS mirroring the Rust list")
            });
                let body = &text[start..];
                let end = body
                    .find("];")
                    .expect("KEEL_RESEARCH_SUBCOMMANDS terminator");
                quoted_literals(&body[..end])
            };

        // The shell adapter cannot express the two-word entries in its
        // whitespace-split list, so it matches them in a separate `case`.
        let two_word: BTreeSet<String> = expected
            .iter()
            .filter(|hit| hit.contains(' '))
            .cloned()
            .collect();
        let mut covered = actual.clone();
        for hit in &two_word {
            if text.contains(hit.as_str()) {
                covered.insert(hit.clone());
            }
        }

        let missing: Vec<&String> = expected.difference(&covered).collect();
        assert!(
            missing.is_empty(),
            "{adapter} is missing gate-clearing entries {missing:?} present in the Rust list"
        );
        let extra: Vec<&String> = covered.difference(&expected).collect();
        assert!(
            extra.is_empty(),
            "{adapter} clears the Iron Law gate on {extra:?}, which the Rust core does not. \
             A looser adapter list writes the shared marker and unlocks the Rust gate too."
        );

        let rejects_compound =
            text.contains("$(") && (text.contains("[&|;`\\n]") || text.contains("*\"&\"*"));
        assert!(
            rejects_compound,
            "{adapter} must reject compound commands so `keel doctor && curl evil` cannot \
             clear the gate"
        );
    }
}

/// `keel bridge rewrite` requires `--tool`: with an empty tool name it fails the
/// shell-tool check and prints nothing, so the caller silently gets no compaction.
/// The Cursor hook omitted the flag and its reroute never fired once. Any adapter
/// invoking the subcommand must pass the flag.
#[test]
fn adapters_calling_bridge_rewrite_pass_a_tool_flag() {
    let repo_root = repository_root();
    for adapter in BRIDGE_ADAPTER_FILES {
        let path = repo_root.join(adapter);
        let text = read_adapter(&path, adapter);
        // Only adapters that actually invoke the subcommand are constrained.
        let invokes = text.contains("bridge rewrite") || text.contains("\"rewrite\"");
        if !invokes {
            continue;
        }
        assert!(
            text.contains("--tool"),
            "{adapter} invokes `bridge rewrite` without passing --tool; the bridge sees an \
             empty tool name, fails is_shell_tool_name, and returns no rewrite, so command \
             compaction silently never runs on that host."
        );
    }
}

/// The Iron Law gate decision must be read by prefix, not by substring, and deny
/// must be tested before allow. An unanchored `includes("ALLOW")` checked first
/// turns a deny whose reason prose contains ALLOW into an allow, failing open on
/// a security gate. Every adapter that consumes the decision uses the prefix form.
#[test]
fn adapters_match_gate_decision_by_prefix_not_substring() {
    let repo_root = repository_root();
    for adapter in BRIDGE_ADAPTER_FILES {
        let path = repo_root.join(adapter);
        let text = read_adapter(&path, adapter);
        if !text.contains("KEEL_GATE") {
            continue;
        }
        // TypeScript uses startsWith; the shell adapter uses a `case` prefix glob.
        let anchored_deny =
            text.contains("startsWith(\"KEEL_GATE_DENY\")") || text.contains("KEEL_GATE_DENY*)");
        assert!(
            anchored_deny,
            "{adapter} must detect a deny by prefix: startsWith(\"KEEL_GATE_DENY\") \
             or a `KEEL_GATE_DENY*)` case pattern"
        );
        // Comment lines are prose about the rule, not the check itself.
        let code_only: String = text
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !code_only.contains("includes(\"ALLOW\")") && !code_only.contains("includes(\"DENY\")"),
            "{adapter} matches the gate decision by bare substring; use the KEEL_GATE_ prefix \
             so free-form reason text cannot flip the decision."
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

    for adapter in ["opencode", "codex", "pi", "cursor", "cowork", "commandcode"] {
        let staged_explicit = release_yml.contains(&format!("cp -R \"{adapter}\""))
            || release_yml.contains(&format!("cp -R {adapter} "));
        let staged_loop =
            release_yml.contains("for adapter in opencode codex pi cursor cowork commandcode");
        assert!(
            staged_explicit || staged_loop,
            "release.yml must stage the `{adapter}` adapter dir into the release bundle; \
             without it `keel install` reports \"plugin source absent\" / \"source absent\" \
             for the {adapter} adapter when run from a release-bundle extract. \
             Add the adapter to the staging loop or as an explicit `cp -R`."
        );
    }
}

/// H5/H6 guard: docs and host-bridge READMEs must not hardcode a literal
/// specialist-skill count or MCP-tool count. The counts are asserted from disk
/// (manifest + mcp/tools.rs) by other tests in this file; a literal number in
/// prose rots the moment a skill or tool is added or removed. This test scans
/// the known stale-count surfaces and fails if any literal count reappears.
#[test]
fn no_stale_literal_counts_in_docs_or_host_readmes() {
    let repo_root = repository_root();
    // Files that previously carried stale "24 specialist" / "31 tools" literals.
    let surfaces = [
        "AGENTS.md",
        "AGENTS/references/99-source-anchors.md",
        "AGENTS/references/20-skill-routing.md",
        "00-skill-routing-and-escalation.md",
        "using-keel/SKILL.md",
        "README.md",
        "pi/README.md",
        "cowork/README.md",
        ".githooks/README.md",
        ".claude-plugin/marketplace.json",
        "codex/.codex-plugin/plugin.json",
    ];
    // Regex would be cleaner, but a substring scan keeps this dependency-free and
    // matches the line-based style of the rest of this test file. We flag a
    // literal count adjacent to specialist/tool language.
    let forbidden_patterns = [
        "24 specialist",
        "24 managed",
        "24 delegation",
        "24 profiles",
        "31 tools",
        "31 MCP",
    ];
    for surface in &surfaces {
        let path = repo_root.join(surface);
        let text = match fs::read_to_string(&path) {
            Ok(t) => t,
            Err(_) => continue, // a surface may not exist on every checkout
        };
        for pattern in &forbidden_patterns {
            assert!(
                !text.contains(pattern),
                "{surface} still hardcodes the stale literal \"{pattern}\". \
                 The specialist/tool counts are asserted from disk by doc_parity_test; \
                 a literal number in prose rots when a skill or tool is added/removed. \
                 Replace it with a reference to the test-asserted roster.",
            );
        }
    }
}

/// The literal-count guard above scans only Markdown and JSON, so the stalest
/// counts in the product survived it. The SessionStart bootstrap string in
/// `hook_lifecycle` baked in skill and subagent totals that no longer matched
/// disk, and that string is injected into every session of every project. Counts
/// belong to `skill-lint` or `keel doctor` at runtime, never to a baked string.
/// This scans the Rust sources that build agent-facing prose.
#[test]
fn rust_sources_do_not_hardcode_skill_or_subagent_counts() {
    let repo_root = repository_root();
    // Phrases that only ever appear in agent-facing prose about the pack size.
    let phrases = [
        "specialist skills",
        "specialist skill",
        "matching subagents",
        "subagents in",
        "MCP tools",
        "skills are installed",
    ];

    // Only shipped crate sources: this test file names the offending strings in
    // its own docstring, and test prose never reaches a session.
    let mut sources: Vec<PathBuf> = Vec::new();
    if let Ok(crates) = fs::read_dir(repo_root.join("rust").join("crates")) {
        for entry in crates.flatten() {
            sources.extend(rust_source_files(&entry.path().join("src")));
        }
    }

    let mut offenders: Vec<String> = Vec::new();
    for path in sources {
        let text = match fs::read_to_string(&path) {
            Ok(t) => t,
            Err(_) => continue,
        };
        for phrase in &phrases {
            let mut from = 0usize;
            while let Some(found) = text[from..].find(phrase) {
                let absolute = from + found;
                // A digit immediately before the phrase means a baked count.
                let preceding = text[..absolute].trim_end();
                if preceding
                    .chars()
                    .last()
                    .map(|c| c.is_ascii_digit())
                    .unwrap_or(false)
                {
                    let number: String = preceding
                        .chars()
                        .rev()
                        .take_while(|c| c.is_ascii_digit())
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                        .collect();
                    offenders.push(format!(
                        "{}: \"{number} {phrase}\"",
                        path.strip_prefix(&repo_root).unwrap_or(&path).display()
                    ));
                }
                from = absolute + phrase.len();
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "Rust source hardcodes a skill/subagent/tool count: {offenders:?}. \
         These strings ship to every session and rot the moment the pack changes. \
         State the capability without a number, or compute it at runtime."
    );
}

/// Every `.rs` file under `dir`, recursively. Skips `target/` build output.
fn rust_source_files(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return found;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().and_then(|n| n.to_str()) == Some("target") {
                continue;
            }
            found.extend(rust_source_files(&path));
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            found.push(path);
        }
    }
    found
}

/// H5 guard: every specialist agent file in .claude/agents/ must have a matching
/// managed profile at <name>/agents/claude.yaml, and vice versa. Pins the
/// 1:1 agent⇄profile correspondence the docs now describe count-free.
#[test]
fn specialist_agent_and_profile_sets_match() {
    let repo_root = repository_root();
    let agents_dir = repo_root.join(".claude").join("agents");
    let agent_files: BTreeSet<String> = fs::read_dir(&agents_dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", agents_dir.display()))
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("md") {
                path.file_stem()?.to_str().map(|s| s.to_string())
            } else {
                None
            }
        })
        .collect();

    // Profiles live at <name>/agents/claude.yaml — collect the parent dir name.
    let mut profile_dirs: BTreeSet<String> = BTreeSet::new();
    for skill in manifest_skills(&repo_root) {
        let profile = repo_root.join(&skill).join("agents").join("claude.yaml");
        if profile.is_file() {
            profile_dirs.insert(skill);
        }
    }

    let agents_only: Vec<_> = agent_files.difference(&profile_dirs).collect();
    let profiles_only: Vec<_> = profile_dirs.difference(&agent_files).collect();
    assert!(
        agents_only.is_empty() && profiles_only.is_empty(),
        "agent⇄profile mismatch:\n  agents without a profile: {agents_only:?}\n  \
         profiles without an agent: {profiles_only:?}\n  \
         Every specialist must ship all three artifacts (SKILL.md + subagent + profile)."
    );
}

/// Core Web Vitals in the web specialist must not list FID as a current target.
/// INP replaced FID as a Core Web Vital (web.dev, March 2024).
#[test]
fn web_performance_reference_does_not_list_fid_as_current_cwv() {
    let path = repository_root()
        .join("web-development-life-cycle")
        .join("references")
        .join("50-performance-metrics-and-budgets.md");
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    assert!(
        !text.contains("FID Target:"),
        "FID is no longer a Core Web Vital target; use INP (≤200ms). File: {}",
        path.display()
    );
    assert!(
        text.contains("Interaction to Next Paint (INP)"),
        "must name INP as the current interactivity Core Web Vital: {}",
        path.display()
    );
    assert!(
        text.contains("≤ 200ms") || text.contains("< 200ms"),
        "must state the INP good threshold (200ms): {}",
        path.display()
    );
}

/// First-party SKILL.md files must not ship a top-level `paths:` frontmatter key.
/// Path-scoped skills are hidden from some hosts' Skill() catalog unless the
/// cwd matches the globs, which surfaces as `Error: Unknown skill: <name>` even
/// when `~/.claude/skills/<name>/SKILL.md` exists (seen with authentication-
/// and-identity). Routing must use description/when_to_use only so Skill(name)
/// always resolves after install. skill-lint warns on reintroduction; this test
/// fails closed so the pack cannot ship path-gated specialists again.
#[test]
fn first_party_skills_do_not_use_paths_frontmatter() {
    let repo_root = repository_root();
    let mut offenders: Vec<String> = Vec::new();
    for name in first_party_skill_dirs(&repo_root) {
        let path = repo_root.join(&name).join("SKILL.md");
        let text =
            fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        // Only the leading frontmatter block — a body mention of "paths:" is fine.
        let mut in_frontmatter = false;
        for line in text.lines() {
            let trimmed = line.trim();
            if !in_frontmatter {
                if trimmed == "---" {
                    in_frontmatter = true;
                }
                continue;
            }
            if trimmed == "---" {
                break;
            }
            // Top-level key only (not indented list items under something else).
            if !line.starts_with(char::is_whitespace) && trimmed.starts_with("paths:") {
                offenders.push(name.clone());
                break;
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "first-party skills must not use `paths:` frontmatter (breaks Skill() on some hosts): {offenders:?}. \
         Use description/when_to_use for routing; fall back to MCP skill_get if the host still reports Unknown skill."
    );
}

/// The committed plugin hook wiring must gate the Iron Law on ALL tools, not
/// only Bash. A `"Bash"` matcher on PreToolUse (as shipped before this guard)
/// means Edit/Write/NotebookEdit/Agent never reach the deny handler for plugin
/// installs, silently disabling the hard enforcement the native install has.
#[test]
fn plugin_hooks_pretooluse_matcher_is_unscoped() {
    let repo_root = repository_root();
    let path = repo_root.join(".claude").join("hooks.json");
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let doc: Value =
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()));
    let pre = doc
        .get("hooks")
        .and_then(|h| h.get("PreToolUse"))
        .and_then(Value::as_array)
        .expect(".claude/hooks.json has a PreToolUse array");
    assert!(!pre.is_empty(), "PreToolUse must have at least one entry");
    for entry in pre {
        let matcher = entry
            .get("matcher")
            .and_then(Value::as_str)
            .expect("each PreToolUse entry has a string matcher");
        assert_eq!(
            matcher, "",
            "PreToolUse matcher must be \"\" so the Iron Law gate fires on every \
             edit-class tool; a scoped matcher (e.g. \"Bash\") disables the gate \
             for Edit/Write/Agent on the plugin install path"
        );
    }
}
