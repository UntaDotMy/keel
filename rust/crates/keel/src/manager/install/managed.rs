// Installer managed-region and inventory helpers.
use super::*;
use crate::runtime::{
    display_path, read_text_if_exists, state_directory, write_text, RepositoryLayout,
};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
pub(crate) const MANAGED_CLAUDE_MD_BEGIN: &str =
    "<!-- keel:begin (managed by keel install — edits inside this block are overwritten; edit outside it freely) -->";
pub(crate) const MANAGED_CLAUDE_MD_END: &str = "<!-- keel:end -->";

/// The always-on operating contract written into `~/.claude/CLAUDE.md`.
///
/// Why this exists: every other keel surface (the SessionStart bootstrap,
/// the per-prompt iron law, skill pointers, MCP-tool nudges) is delivered through
/// the harness's hook `additionalContext` channel. When that channel does not
/// reach the model — e.g. a gateway/proxy that drops injected context — the agent
/// sees none of keel. `~/.claude/CLAUDE.md` is loaded natively into every
/// session as user memory, the same hook-independent channel that carries the
/// base system prompt, so this block lands even when hooks do not. Kept compact
/// because it is paid on every session of every project.
pub(crate) const MANAGED_CLAUDE_MD_BODY: &str = r#"# keel operating contract (always-on)

Installed by keel into `~/.claude/CLAUDE.md` and loaded into **every** harness session as user memory — independent of hooks. Applies to every project you work in, not just keel.

## Iron Law — for any request that could touch code, config, or architecture
1. **Read first.** Read the workspace SYSTEM_MAP and the owning file before claiming behavior; never propose changes against an imagined version.
2. **Understand before building.** Restate what the request asks and research what is genuinely needed before writing code. No guessing, no building against an imagined spec.
3. **Request fidelity.** Implement only what the user asked. Do not invent features, APIs, files, refactors, or "nice extras" outside the request.
4. **Ask when unclear.** If the request is unclear, conflicting, incomplete, or you feel drift risk (multiple valid designs, unknown project conventions, scare that you will invent scope), **stop and ask the user** before coding. Do not decide silently. Do not "just pick one and go."
5. **Never trust knowledge-base alone.** Training data and generic patterns are not this project's structure, stories, or implementation path. Read SYSTEM_MAP, owning files, and user stories here. Each project has its own layout and conventions — nothing is hardcoded in your memory as truth for this repo.
6. **Invoke relevant skills.** If there is even a 1% chance a keel skill applies, use the Skill tool BEFORE writing code or giving a final answer.
7. **Find the root cause.** Trace the symptom end-to-end with file:line evidence and confirm the suspect is on that path before changing anything.
8. **Preserve existing data.** Never remove or replace an existing field, column, output, or record to fit a new format — ADD alongside, and ASK before dropping anything the user did not name. Data loss in an edit is destructive like `DROP TABLE`; if you would flag a removal *after* acting, ask *before* instead. Autonomy covers reversible choices, never data deletion.
9. **Memory-first navigation.** Prefer `system_map` + `recall` + working briefs over listing the whole tree. If context already names the path, open that path; do not rediscover the repo with blind `ls`/`find`/full-tree greps.
10. **Useful comments only.** Never write summary comments that restate the code. Prefer structured contracts (`@param`, `@returns`, `# Errors`, `// why:`) or no comment.

## keel MCP tools — always available, prefer over guessing
- `system_map` — call when you lack the workspace structural map this turn (e.g. "what is this project", "where does X live") instead of reading files blind. **Call it at most once per turn** — if you already called it this turn, the result is in your context; reuse it and read the owning files it points you at. Call it again only if you have since created, moved, or deleted files and the in-context map is now stale.
- `recall` — call when you need to surface a prior decision, working brief, or learning from durable memory instead of claiming from conversation. **Call it at most once per turn** — if you already called it this turn, reuse the result. Call it again only if you have since written new memory this turn and need to confirm it landed.
- `run_command` — run noisy shell commands (test, build, lint, logs, search) through it so compacted output enters context instead of the raw stream.

**No tool-call loops.** These tools answer "what is the structure" and "what do I remember". They do not change between calls within a single turn unless *you* changed something. Re-calling them with no intervening change is a loop — re-read the result already in your context instead.

**No blind exploration.** If SYSTEM_MAP or recall already points at the module, edit or read that file next. Broad directory listings and full-repo scans are last resorts after scoped memory fails.

## Skills
keel installs specialist skills under `~/.claude/skills/` (lifecycle, backend, cloud, security, reviewer, UI/UX, debugging, TDD, migrations, and more). Invoke by bare name with the Skill tool, e.g. `Skill("reviewer")`. The `using-keel` skill carries the full catalog and routing rules."#;

pub(crate) fn managed_claude_md_block() -> String {
    format!("{MANAGED_CLAUDE_MD_BEGIN}\n{MANAGED_CLAUDE_MD_BODY}\n{MANAGED_CLAUDE_MD_END}")
}

/// Splice `block` into `existing` CLAUDE.md content, preserving user content.
///
/// If a managed region already exists (both sentinels present, in order), its
/// contents are replaced in place. Otherwise the block is prepended (so the
/// contract is the first thing the model reads) and any pre-existing user
/// content is kept below it. Pure (no IO) so the splice logic is unit-testable.
pub(crate) fn merge_managed_claude_md(existing: &str, block: &str) -> String {
    if let (Some(start), Some(end_idx)) = (
        existing.find(MANAGED_CLAUDE_MD_BEGIN),
        existing.find(MANAGED_CLAUDE_MD_END),
    ) {
        if end_idx > start {
            let end_full = end_idx + MANAGED_CLAUDE_MD_END.len();
            let before = &existing[..start];
            let after = &existing[end_full..];
            return format!("{before}{block}{after}");
        }
    }
    if existing.trim().is_empty() {
        format!("{block}\n")
    } else {
        format!("{block}\n\n{existing}")
    }
}

/// Remove the managed region from `existing`, preserving user content. Pure
/// (no IO). Returns the content with the block (and the blank lines it
/// introduced) stripped; an all-managed file collapses to empty so the caller
/// can delete it.
pub(crate) fn strip_managed_claude_md(existing: &str) -> String {
    if let (Some(start), Some(end_idx)) = (
        existing.find(MANAGED_CLAUDE_MD_BEGIN),
        existing.find(MANAGED_CLAUDE_MD_END),
    ) {
        if end_idx > start {
            let end_full = end_idx + MANAGED_CLAUDE_MD_END.len();
            let before = existing[..start].trim_end();
            let after = existing[end_full..].trim_start();
            return match (before.is_empty(), after.is_empty()) {
                (true, true) => String::new(),
                (true, false) => format!("{after}\n"),
                (false, true) => format!("{before}\n"),
                (false, false) => format!("{before}\n\n{after}\n"),
            };
        }
    }
    existing.to_string()
}

/// Write/refresh the keel managed block in `~/.claude/CLAUDE.md`.
///
/// Guarded on the standard `.claude` home name for the same reason as
/// `maybe_register_mcp_server` / `maybe_install_hooks`: the integration suite
/// installs into throwaway `--claude-home` dirs and must stay hermetic. Real
/// installs into `~/.claude` always get the file. Best-effort: a failure is
/// reported in the summary but never fails the install.
pub(crate) fn maybe_sync_user_claude_md(claude_home: &Path) -> Option<String> {
    if !is_standard_home(claude_home) {
        return None;
    }
    let path = claude_home.join("CLAUDE.md");
    let existing = read_text_if_exists(&path).unwrap_or_default();
    let merged = merge_managed_claude_md(&existing, &managed_claude_md_block());
    if merged == existing {
        return Some("already current".to_string());
    }
    match write_text(&path, &merged) {
        Ok(()) => Some(format!("written to {}", display_path(&path))),
        Err(error) => Some(format!("skipped ({error})")),
    }
}

/// Strip the keel managed block from `~/.claude/CLAUDE.md` on uninstall,
/// preserving any user content outside it. Deletes the file only if it becomes
/// empty. Returns the number of paths changed/removed (0 or 1). A missing file
/// or a file with no managed block is a no-op.
pub(crate) fn remove_managed_user_claude_md(claude_home: &Path) -> Result<usize, String> {
    let path = claude_home.join("CLAUDE.md");
    let existing = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(_) => return Ok(0),
    };
    if !existing.contains(MANAGED_CLAUDE_MD_BEGIN) {
        return Ok(0);
    }
    let stripped = strip_managed_claude_md(&existing);
    if stripped.trim().is_empty() {
        remove_path_if_exists_counted(&path)
    } else {
        write_text(&path, &stripped)?;
        Ok(1)
    }
}

pub(crate) fn managed_files_inventory_path(claude_home: &Path) -> PathBuf {
    state_directory(claude_home).join("managed-files.txt")
}

pub(crate) fn managed_skills_inventory_path(claude_home: &Path) -> PathBuf {
    state_directory(claude_home).join("managed-skills.txt")
}

pub(crate) fn managed_agents_inventory_path(claude_home: &Path) -> PathBuf {
    state_directory(claude_home).join("managed-agents.txt")
}

/// Inventory of top-level shared-resource directory names (currently `_shared`)
/// that the installer has staged into `<claude_home>/skills/`. Tracked
/// separately from `managed-skills.txt` because shared resources have no
/// `SKILL.md` and follow directory-level orphan cleanup, not file-level.
pub(crate) fn managed_shared_resources_inventory_path(claude_home: &Path) -> PathBuf {
    state_directory(claude_home).join("managed-shared-resources.txt")
}

/// Top-level names under `<claude_home>` that hold **user / harness data**.
/// Install and orphan cleanup must never delete or rewrite these as "stale".
/// (Grok sessions live under `~/.grok/`, which install never touches.)
pub(crate) const PROTECTED_USER_DATA_TOP_LEVEL: &[&str] = &[
    "sessions",
    "projects",
    "file-history",
    "memories",
    "memory",
    "working-briefs",
    "orchestration",
    "workflow",
    "tasks",
    "teams",
    "backups",
    "cache",
    "raw-output",
    "session-env",
    "shell-snapshots",
    "history.jsonl",
    "settings.json",
    "recall-index.sqlite3",
    "command-compaction-events.jsonl",
    "statsig",
    "todos",
    "plugins",
    "ide",
    "stats-cache",
    "transcripts",
];

/// Relative paths install is allowed to remove as "stale managed" content.
/// Anything else is refused even if it appears in an old inventory (corruption
/// or a bug must never turn orphan cleanup into a home-directory wipe).
pub(crate) fn is_allowed_managed_orphan_relative(relative: &str) -> bool {
    let normalized = relative.replace('\\', "/");
    let rel = normalized.trim_start_matches("./");
    if rel.is_empty() || rel == "." {
        return false;
    }
    // Path traversal or absolute-like components to refuse.
    if rel.starts_with('/')
        || rel.starts_with("..")
        || rel.contains("/../")
        || rel.ends_with("/..")
        || rel.contains(':')
    {
        return false;
    }
    let first = rel.split('/').next().unwrap_or("");
    if PROTECTED_USER_DATA_TOP_LEVEL
        .iter()
        .any(|p| first.eq_ignore_ascii_case(p) || rel.eq_ignore_ascii_case(p))
    {
        return false;
    }
    // Never orphan-delete the always-on CLAUDE.md (merge-only surface).
    if rel.eq_ignore_ascii_case("CLAUDE.md") {
        return false;
    }
    // Never remove user-learned skills (not part of the ship pack).
    if rel.starts_with("skills/learned-") || rel == "skills/learned" {
        return false;
    }
    // Managed pack surfaces only.
    if matches!(
        first,
        "skills" | "agents" | "agent-profiles" | "commands" | "docs"
    ) {
        return true;
    }
    // Root guidance files from ROOT_GUIDANCE_RELATIVE_PATHS (file names only at root).
    if !rel.contains('/') && matches!(rel, "AGENTS.md" | "00-skill-routing-and-escalation.md") {
        return true;
    }
    false
}

/// Resolve `claude_home/relative` and ensure the result stays under `claude_home`.
pub(crate) fn resolve_managed_path_under_home(
    claude_home: &Path,
    relative: &str,
) -> Option<PathBuf> {
    if !is_allowed_managed_orphan_relative(relative) {
        return None;
    }
    let candidate = claude_home.join(relative);
    // Best-effort containment: after join, path must still start with claude_home.
    let home_canon = claude_home
        .canonicalize()
        .unwrap_or_else(|_| claude_home.to_path_buf());
    let cand_parent = candidate.parent().unwrap_or(&candidate);
    let parent_canon = cand_parent
        .canonicalize()
        .unwrap_or_else(|_| cand_parent.to_path_buf());
    if parent_canon.starts_with(&home_canon) || candidate.starts_with(claude_home) {
        Some(candidate)
    } else {
        None
    }
}

/// Whether orphan purge is enabled.
///
/// **Default is off** so a one-line reinstall never deletes user-adjacent data.
/// Opt in with `--purge-stale` or `KEEL_INSTALL_PURGE_STALE=1`. Even when on,
/// every delete still passes the protect/allowlist (sessions, projects, memories,
/// history, settings, learned skills, path traversal, etc. are never removed).
/// `--no-purge` always wins and disables deletes.
pub(crate) fn install_purge_stale_enabled(flag_no_purge: bool, flag_purge_stale: bool) -> bool {
    if flag_no_purge {
        return false;
    }
    if flag_purge_stale {
        return true;
    }
    match std::env::var("KEEL_INSTALL_PURGE_STALE") {
        Ok(v) => {
            let t = v.trim().to_ascii_lowercase();
            t == "1" || t == "true" || t == "yes" || t == "on"
        }
        Err(_) => false,
    }
}

pub(crate) fn remove_orphans(
    claude_home: &Path,
    previous_files: &BTreeSet<String>,
    previous_skills: &BTreeSet<String>,
    previous_shared_resources: &BTreeSet<String>,
    layout: &RepositoryLayout,
    tracker: &FileTracker,
    purge_stale: bool,
) -> Result<usize, String> {
    if !purge_stale {
        return Ok(0);
    }
    let mut removed = 0;
    for relative in previous_files.difference(&tracker.files) {
        let Some(absolute) = resolve_managed_path_under_home(claude_home, relative) else {
            // Refuse: protected user data, traversal, or non-managed surface.
            continue;
        };
        if absolute.is_file() {
            removed += remove_path_if_exists_counted(&absolute)?;
        }
    }
    let current_skills: BTreeSet<String> = layout.skills.iter().map(|s| s.name.clone()).collect();
    for orphan_skill in previous_skills.difference(&current_skills) {
        // Never purge user-learned skill directories.
        if orphan_skill.starts_with("learned-") {
            continue;
        }
        let rel = format!("skills/{orphan_skill}");
        let Some(skill_directory) = resolve_managed_path_under_home(claude_home, &rel) else {
            continue;
        };
        removed += remove_path_if_exists_counted(&skill_directory)?;
    }
    let current_shared_resources: BTreeSet<String> =
        layout.shared_resource_directories.iter().cloned().collect();
    for orphan_shared in previous_shared_resources.difference(&current_shared_resources) {
        if orphan_shared.contains("..")
            || orphan_shared.contains('/')
            || orphan_shared.contains('\\')
        {
            continue;
        }
        let rel = format!("skills/{orphan_shared}");
        let Some(shared_directory) = resolve_managed_path_under_home(claude_home, &rel) else {
            continue;
        };
        removed += remove_path_if_exists_counted(&shared_directory)?;
    }
    Ok(removed)
}
