//! Purpose: Skill eval/lint harness — validates that every `<name>/SKILL.md`
//!   has the structural properties the Claude Code skill matcher needs to TRIGGER
//!   the skill, not merely that the Rust compiles.
//! Caller: commands.rs `skill-lint` dispatch.
//! Dependencies: std::fs, std::path, crate::json, crate::runtime::display_path.
//! Main Functions: run_skill_lint_command, lint_skill, parse_frontmatter.
//! Side Effects: Reads SKILL.md files and their referenced `references/*.md`; writes a report.
//!
//! Why this exists: the matcher decides whether to load a skill from its
//! frontmatter `name` + `description` (and `when_to_use`). A skill that compiles
//! but has an empty description, an over-cap description, or a dangling
//! `references/` link silently fails to trigger or loads broken. This harness is
//! the superpowers-style "test that skills trigger" gate, expressed as the
//! checks we can verify deterministically without invoking the live model.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::args::FlagSet;
use crate::json::{write_indented, Value};
use crate::runtime::{display_path, resolve_repository_root};

/// Official cap: combined `description` + `when_to_use` text the matcher reads.
/// Source: code.claude.com/docs/en/skills (documented 1,536-char limit).
const DESCRIPTION_BUDGET_CHARS: usize = 1536;

#[derive(Debug, Default)]
struct SkillReport {
    name: String,
    path: String,
    errors: Vec<String>,
    warnings: Vec<String>,
}

impl SkillReport {
    fn ok(&self) -> bool {
        self.errors.is_empty()
    }
}

pub fn run_skill_lint_command(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flag_set = FlagSet::new("skill-lint");
    flag_set.string_flag("repo-root", "");
    flag_set.bool_flag("json", false);
    if let Err(parse_error) = flag_set.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 1;
    }
    let repository_root = match resolve_repository_root(flag_set.string_value("repo-root")) {
        Ok(path) => path,
        Err(error) => {
            let _ = writeln!(standard_error, "skill-lint: {error}");
            return 1;
        }
    };

    let skill_files = match discover_skill_files(&repository_root) {
        Ok(files) => files,
        Err(error) => {
            let _ = writeln!(standard_error, "skill-lint: {error}");
            return 1;
        }
    };
    if skill_files.is_empty() {
        let _ = writeln!(
            standard_error,
            "skill-lint: no SKILL.md files found under {}",
            display_path(&repository_root)
        );
        return 1;
    }

    let reports: Vec<SkillReport> = skill_files.iter().map(|path| lint_skill(path)).collect();
    let failed = reports.iter().filter(|report| !report.ok()).count();
    let warned = reports
        .iter()
        .filter(|report| report.ok() && !report.warnings.is_empty())
        .count();

    if flag_set.bool_value("json") {
        let payload = Value::Object(vec![
            ("total".into(), Value::Number(reports.len().to_string())),
            ("failed".into(), Value::Number(failed.to_string())),
            ("warned".into(), Value::Number(warned.to_string())),
            (
                "skills".into(),
                Value::Array(reports.iter().map(report_to_value).collect()),
            ),
        ]);
        let exit = if write_indented(standard_output, &payload).is_err() {
            1
        } else {
            0
        };
        return if failed > 0 { 1 } else { exit };
    }

    let _ = writeln!(
        standard_output,
        "skill-lint: {} skill(s), {failed} failed, {warned} warned",
        reports.len()
    );
    for report in &reports {
        let status = if report.ok() { "ok" } else { "FAIL" };
        let _ = writeln!(standard_output, "  [{status}] {}", report.name);
        for error in &report.errors {
            let _ = writeln!(standard_output, "    error: {error}");
        }
        for warning in &report.warnings {
            let _ = writeln!(standard_output, "    warn:  {warning}");
        }
    }
    if failed > 0 {
        1
    } else {
        0
    }
}

fn report_to_value(report: &SkillReport) -> Value {
    Value::Object(vec![
        ("name".into(), Value::String(report.name.clone())),
        ("path".into(), Value::String(report.path.clone())),
        ("ok".into(), Value::Bool(report.ok())),
        (
            "errors".into(),
            Value::Array(report.errors.iter().cloned().map(Value::String).collect()),
        ),
        (
            "warnings".into(),
            Value::Array(report.warnings.iter().cloned().map(Value::String).collect()),
        ),
    ])
}

/// Find every `<name>/SKILL.md` directly under the repository root. Skills live
/// one level down (the same layout `discover_repository_layout` expects), so a
/// shallow scan is sufficient and avoids walking `target/`, `.git/`, etc.
fn discover_skill_files(repository_root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut skill_files = Vec::new();
    let entries = fs::read_dir(repository_root)
        .map_err(|error| format!("read {}: {error}", display_path(repository_root)))?;
    for entry_result in entries {
        let entry = entry_result.map_err(|error| format!("read entry: {error}"))?;
        if !entry.path().is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let skill_md = entry.path().join("SKILL.md");
        if skill_md.is_file() {
            skill_files.push(skill_md);
        }
    }
    skill_files.sort();
    Ok(skill_files)
}

fn lint_skill(skill_path: &Path) -> SkillReport {
    let directory_name = skill_path
        .parent()
        .and_then(|parent| parent.file_name())
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();
    let mut report = SkillReport {
        name: directory_name.clone(),
        path: display_path(skill_path),
        ..SkillReport::default()
    };

    let text = match fs::read_to_string(skill_path) {
        Ok(text) => text,
        Err(error) => {
            report.errors.push(format!("unreadable: {error}"));
            return report;
        }
    };

    let Some((frontmatter, body)) = split_frontmatter(&text) else {
        report
            .errors
            .push("missing YAML frontmatter (no leading `---` block)".to_string());
        return report;
    };
    let fields = parse_frontmatter(&frontmatter);

    // name: recommended, and must match the directory so the installed path and
    // the matcher agree on the invocation name.
    match field(&fields, "name") {
        None => report
            .warnings
            .push("no `name` field (matcher uses the directory name)".to_string()),
        Some(name) if name != directory_name => report.errors.push(format!(
            "`name: {name}` does not match directory `{directory_name}`"
        )),
        Some(_) => {}
    }

    // description: the single field the matcher most relies on. Empty = the skill
    // effectively never triggers.
    let description = field(&fields, "description").unwrap_or_default();
    if description.trim().is_empty() {
        report.errors.push(
            "`description` is empty — the matcher cannot decide when to load this skill"
                .to_string(),
        );
    }

    // Official combined cap: description + when_to_use <= 1536 chars.
    let when_to_use = field(&fields, "when_to_use").unwrap_or_default();
    let combined = description.chars().count() + when_to_use.chars().count();
    if combined > DESCRIPTION_BUDGET_CHARS {
        report.errors.push(format!(
            "description + when_to_use is {combined} chars, over the {DESCRIPTION_BUDGET_CHARS}-char matcher budget"
        ));
    }

    // allowed-tools: not required, but a skill that runs Bash without scoping it
    // is a common footgun — warn so it is a deliberate choice.
    if field(&fields, "allowed-tools").is_none() {
        report
            .warnings
            .push("no `allowed-tools` — skill inherits all tools (consider scoping)".to_string());
    }

    // Dangling references: every `references/<file>` mentioned in the body must
    // exist on disk, or progressive disclosure loads a broken path.
    if let Some(parent) = skill_path.parent() {
        for referenced in referenced_files(&body) {
            let candidate = parent.join(&referenced);
            if !candidate.is_file() {
                report
                    .errors
                    .push(format!("references missing file `{referenced}`"));
            }
        }
    }

    report
}

/// Split a `---\n...\n---\n` leading frontmatter block from the body. Returns
/// `None` when the file does not start with a frontmatter fence.
fn split_frontmatter(text: &str) -> Option<(String, String)> {
    let trimmed_start = text.trim_start_matches(['\u{feff}', ' ', '\t']);
    if !trimmed_start.starts_with("---") {
        return None;
    }
    // Skip the opening fence line.
    let after_open = trimmed_start.split_once('\n').map(|(_, rest)| rest)?;
    // Find the closing fence at the start of a line.
    let mut frontmatter = String::new();
    let mut remaining_lines = after_open.lines();
    let mut closed = false;
    let mut consumed = 0usize;
    for line in remaining_lines.by_ref() {
        consumed += line.len() + 1;
        if line.trim() == "---" {
            closed = true;
            break;
        }
        frontmatter.push_str(line);
        frontmatter.push('\n');
    }
    if !closed {
        return None;
    }
    let body = after_open.get(consumed..).unwrap_or("").to_string();
    Some((frontmatter, body))
}

/// Parse `key: value` frontmatter lines into pairs. Simple line scanner matching
/// how the rest of the codebase reads its flat config — values keep their inline
/// text; nested YAML is not needed by the fields we validate.
fn parse_frontmatter(frontmatter: &str) -> Vec<(String, String)> {
    let mut fields = Vec::new();
    for line in frontmatter.lines() {
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        // Only treat top-level (non-indented) `key:` lines as fields so a nested
        // mapping value does not get misread as a new key.
        if line.starts_with(char::is_whitespace) {
            continue;
        }
        if let Some(colon) = line.find(':') {
            let key = line[..colon].trim().to_string();
            let value = line[colon + 1..].trim().to_string();
            if !key.is_empty() {
                fields.push((key, value));
            }
        }
    }
    fields
}

fn field(fields: &[(String, String)], key: &str) -> Option<String> {
    fields
        .iter()
        .find(|(field_key, _)| field_key == key)
        .map(|(_, value)| value.clone())
}

/// Extract `references/<file>` paths mentioned in the skill body. Matches the
/// `references/...md` token wherever it appears (backticked or bare) so the
/// dangling-link check covers the on-demand reference files.
fn referenced_files(body: &str) -> Vec<String> {
    let mut found = Vec::new();
    for token in
        body.split(|c: char| c.is_whitespace() || matches!(c, '`' | '(' | ')' | '"' | '\''))
    {
        let cleaned = token
            .trim_matches(|c: char| matches!(c, ',' | '.' | ':' | ';'))
            .trim();
        if cleaned.starts_with("references/")
            && cleaned.ends_with(".md")
            && !found.contains(&cleaned.to_string())
        {
            found.push(cleaned.to_string());
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_skill(label: &str, contents: &str) -> (PathBuf, PathBuf) {
        let unique: u128 = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let pid = std::process::id();
        let root =
            std::env::temp_dir().join(format!("claude-skills-skilllint-{label}-{pid}-{unique}"));
        let skill_dir = root.join(label);
        fs::create_dir_all(&skill_dir).expect("create skill dir");
        let skill_path = skill_dir.join("SKILL.md");
        fs::write(&skill_path, contents).expect("write skill");
        (root, skill_path)
    }

    #[test]
    fn well_formed_skill_passes() {
        let (root, skill_path) = temp_skill(
            "reviewer",
            "---\nname: reviewer\ndescription: Reviews code for production readiness.\nwhen_to_use: After implementation.\nallowed-tools: Read, Grep\n---\n# Reviewer\nbody\n",
        );
        let report = lint_skill(&skill_path);
        assert!(report.ok(), "errors: {:?}", report.errors);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn empty_description_is_an_error() {
        let (root, skill_path) = temp_skill(
            "broken",
            "---\nname: broken\ndescription: \nallowed-tools: Read\n---\nbody\n",
        );
        let report = lint_skill(&skill_path);
        assert!(!report.ok());
        assert!(report
            .errors
            .iter()
            .any(|e| e.contains("description` is empty")));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn name_mismatch_is_an_error() {
        let (root, skill_path) = temp_skill(
            "actual-dir",
            "---\nname: different-name\ndescription: ok\nallowed-tools: Read\n---\nbody\n",
        );
        let report = lint_skill(&skill_path);
        assert!(!report.ok());
        assert!(report
            .errors
            .iter()
            .any(|e| e.contains("does not match directory")));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn over_budget_description_is_an_error() {
        let long = "x".repeat(DESCRIPTION_BUDGET_CHARS + 10);
        let (root, skill_path) = temp_skill(
            "verbose",
            &format!("---\nname: verbose\ndescription: {long}\nallowed-tools: Read\n---\nbody\n"),
        );
        let report = lint_skill(&skill_path);
        assert!(!report.ok());
        assert!(report.errors.iter().any(|e| e.contains("matcher budget")));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn dangling_reference_is_an_error() {
        let (root, skill_path) = temp_skill(
            "refskill",
            "---\nname: refskill\ndescription: ok\nallowed-tools: Read\n---\nSee `references/10-missing.md` for details.\n",
        );
        let report = lint_skill(&skill_path);
        assert!(!report.ok());
        assert!(report
            .errors
            .iter()
            .any(|e| e.contains("references missing file")));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn present_reference_passes() {
        let (root, skill_path) = temp_skill(
            "refok",
            "---\nname: refok\ndescription: ok\nallowed-tools: Read\n---\nSee `references/10-present.md`.\n",
        );
        let references = skill_path.parent().unwrap().join("references");
        fs::create_dir_all(&references).unwrap();
        fs::write(references.join("10-present.md"), "content").unwrap();
        let report = lint_skill(&skill_path);
        assert!(report.ok(), "errors: {:?}", report.errors);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_frontmatter_is_an_error() {
        let (root, skill_path) = temp_skill("nofm", "# Just a heading\nno frontmatter\n");
        let report = lint_skill(&skill_path);
        assert!(!report.ok());
        assert!(report.errors.iter().any(|e| e.contains("frontmatter")));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn parse_frontmatter_ignores_indented_and_comment_lines() {
        let fm = "name: x\n# comment\ndescription: hi\n  nested: ignored\n";
        let fields = parse_frontmatter(fm);
        assert_eq!(field(&fields, "name").as_deref(), Some("x"));
        assert_eq!(field(&fields, "description").as_deref(), Some("hi"));
        assert!(field(&fields, "nested").is_none());
    }
}
