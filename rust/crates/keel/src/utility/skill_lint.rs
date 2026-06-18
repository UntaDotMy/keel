//! Purpose: Skill eval/lint harness — validates that every `<name>/SKILL.md`
//!   has the structural properties the harness skill matcher needs to TRIGGER
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
    /// Deterministic 0–100 quality score and its per-dimension breakdown. This
    /// is the graded analog of an LLM-Judge: instead of a model rating the
    /// skill's prose, we score the structural quality signals the matcher and a
    /// reader actually rely on (trigger language, description band, scoping,
    /// body structure, reference health). A skill can be lint-`ok` (no errors)
    /// yet score poorly — e.g. a terse passive description with no body
    /// structure — so the score surfaces quality the binary pass/fail cannot.
    score: u32,
    score_breakdown: Vec<(String, u32, u32)>,
}

impl SkillReport {
    fn ok(&self) -> bool {
        self.errors.is_empty()
    }
}

/// Per-dimension max points; they sum to 100. Weighted by how much each signal
/// affects whether the skill triggers and reads well: trigger language and a
/// healthy description band matter most (they drive matcher activation),
/// followed by explicit `when_to_use`, tool scoping, body structure, and clean
/// references. Tuned so a well-formed skill (the `well_formed_skill_scores_high`
/// test fixture) lands in the 90s and a bare-minimum one scores in the 40s.
const SCORE_TRIGGER: u32 = 25;
const SCORE_DESCRIPTION_BAND: u32 = 25;
const SCORE_WHEN_TO_USE: u32 = 15;
const SCORE_ALLOWED_TOOLS: u32 = 15;
const SCORE_BODY_STRUCTURE: u32 = 10;
const SCORE_REFERENCES: u32 = 10;

/// Healthy description length band (chars). Below `MIN` the matcher has too
/// little signal to activate reliably; the upper bound is the documented
/// 1536-char combined cap enforced as an error elsewhere. A description inside
/// the band earns full points; a too-short one earns a proportional share.
const HEALTHY_DESCRIPTION_MIN: usize = 60;

pub fn run_skill_lint_command(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flag_set = FlagSet::new("skill-lint");
    flag_set.string_flag("repo-root", "");
    flag_set.bool_flag("json", false);
    // --min-score <0-100>: fail closed when ANY skill scores below the floor,
    // turning the graded eval into a release gate (the analog of wshobson's
    // certify threshold). Default 0 keeps lint backward-compatible — scoring is
    // always computed and reported, but only gates when a floor is set.
    flag_set.string_flag("min-score", "0");
    if let Err(parse_error) = flag_set.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 1;
    }
    let min_score: u32 = match flag_set.string_value("min-score").parse() {
        Ok(value) if value <= 100 => value,
        _ => {
            let _ = writeln!(
                standard_error,
                "skill-lint: --min-score must be an integer in 0..=100"
            );
            return 1;
        }
    };
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
    // Skills that pass lint (no errors) but score below the floor. Only
    // meaningful when --min-score is set; these gate the exit code without
    // being lint "failures", so the two signals stay distinct.
    let below_floor: Vec<&SkillReport> = reports
        .iter()
        .filter(|report| report.ok() && report.score < min_score)
        .collect();
    let average_score = if reports.is_empty() {
        0
    } else {
        reports.iter().map(|r| r.score).sum::<u32>() / reports.len() as u32
    };

    if flag_set.bool_value("json") {
        let payload = Value::Object(vec![
            ("total".into(), Value::Number(reports.len().to_string())),
            ("failed".into(), Value::Number(failed.to_string())),
            ("warned".into(), Value::Number(warned.to_string())),
            ("minScore".into(), Value::Number(min_score.to_string())),
            (
                "belowMinScore".into(),
                Value::Number(below_floor.len().to_string()),
            ),
            (
                "averageScore".into(),
                Value::Number(average_score.to_string()),
            ),
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
        if failed > 0 {
            return 1;
        }
        return if below_floor.is_empty() { exit } else { 1 };
    }

    let _ = writeln!(
        standard_output,
        "skill-lint: {} skill(s), {failed} failed, {warned} warned, avg score {average_score}/100",
        reports.len()
    );
    for report in &reports {
        let status = if report.ok() { "ok" } else { "FAIL" };
        let _ = writeln!(
            standard_output,
            "  [{status}] {} (score {}/100)",
            report.name, report.score
        );
        for error in &report.errors {
            let _ = writeln!(standard_output, "    error: {error}");
        }
        for warning in &report.warnings {
            let _ = writeln!(standard_output, "    warn:  {warning}");
        }
        // Show the dimension breakdown only for skills that fell short of the
        // floor, so the operator sees WHY a gated skill failed without flooding
        // the common all-clean run with detail.
        if min_score > 0 && report.ok() && report.score < min_score {
            for (dimension, earned, max) in &report.score_breakdown {
                let _ = writeln!(standard_output, "    score: {dimension} {earned}/{max}");
            }
        }
    }
    if !below_floor.is_empty() {
        let _ = writeln!(
            standard_output,
            "skill-lint: {} skill(s) below the --min-score floor of {min_score}",
            below_floor.len()
        );
    }
    if failed > 0 {
        return 1;
    }
    if below_floor.is_empty() {
        0
    } else {
        1
    }
}

fn report_to_value(report: &SkillReport) -> Value {
    Value::Object(vec![
        ("name".into(), Value::String(report.name.clone())),
        ("path".into(), Value::String(report.path.clone())),
        ("ok".into(), Value::Bool(report.ok())),
        ("score".into(), Value::Number(report.score.to_string())),
        (
            "scoreBreakdown".into(),
            Value::Array(
                report
                    .score_breakdown
                    .iter()
                    .map(|(dimension, earned, max)| {
                        Value::Object(vec![
                            ("dimension".into(), Value::String(dimension.clone())),
                            ("earned".into(), Value::Number(earned.to_string())),
                            ("max".into(), Value::Number(max.to_string())),
                        ])
                    })
                    .collect(),
            ),
        ),
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

    // Trigger language: the matcher activates far more reliably when the text
    // states WHEN to use the skill, not just what it is. Warn when no trigger
    // phrase is present so a passive description gets tightened.
    if !description.trim().is_empty() && !has_trigger_language(&description, &when_to_use) {
        report.warnings.push(
            "description has no trigger phrase (e.g. \"Use when...\", \"Use for...\", \"Use to...\") — passive descriptions activate less reliably"
                .to_string(),
        );
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

    // Quality score: a deterministic, graded view that complements the binary
    // pass/fail above. Computed from the SAME signals the checks inspected, so
    // the grade can never disagree with the lint findings.
    score_skill(
        &mut report,
        &description,
        &when_to_use,
        field(&fields, "allowed-tools").is_some(),
        &body,
    );

    report
}

/// Compute the 0–100 quality score and its per-dimension breakdown, recording
/// both on the report. Pure scoring over already-extracted fields — the lint
/// checks decided pass/fail; this measures *how good*, the deterministic stand-in
/// for an LLM judge. Each dimension awards a share of its max points so a
/// partially-good skill scores partially, rather than the all-or-nothing a
/// boolean lint gives.
fn score_skill(
    report: &mut SkillReport,
    description: &str,
    when_to_use: &str,
    has_allowed_tools: bool,
    body: &str,
) {
    let mut breakdown: Vec<(String, u32, u32)> = Vec::new();

    // Trigger language: full points when a trigger phrase is present (drives
    // matcher activation), zero when the description is passive.
    let trigger = if has_trigger_language(description, when_to_use) {
        SCORE_TRIGGER
    } else {
        0
    };
    breakdown.push(("trigger-language".to_string(), trigger, SCORE_TRIGGER));

    // Description band: full points inside the healthy band, a proportional
    // share when too short (a 30-char description earns half), zero when empty.
    let desc_len = description.trim().chars().count();
    let band = if desc_len == 0 {
        0
    } else if desc_len >= HEALTHY_DESCRIPTION_MIN {
        SCORE_DESCRIPTION_BAND
    } else {
        // Linear ramp from 0 to full across the [1, MIN) range.
        ((desc_len as u32) * SCORE_DESCRIPTION_BAND) / (HEALTHY_DESCRIPTION_MIN as u32)
    };
    breakdown.push(("description-band".to_string(), band, SCORE_DESCRIPTION_BAND));

    // Explicit when_to_use sharpens activation beyond the description alone.
    let wtu = if when_to_use.trim().is_empty() {
        0
    } else {
        SCORE_WHEN_TO_USE
    };
    breakdown.push(("when-to-use".to_string(), wtu, SCORE_WHEN_TO_USE));

    // Tool scoping: a skill that declares allowed-tools made a deliberate
    // least-privilege choice rather than inheriting everything.
    let tools = if has_allowed_tools {
        SCORE_ALLOWED_TOOLS
    } else {
        0
    };
    breakdown.push(("allowed-tools".to_string(), tools, SCORE_ALLOWED_TOOLS));

    // Body structure: a skill with section headers is navigable and was written
    // as real guidance, not a one-liner stub.
    let structured = body.lines().any(|line| line.trim_start().starts_with('#'));
    let structure = if structured { SCORE_BODY_STRUCTURE } else { 0 };
    breakdown.push((
        "body-structure".to_string(),
        structure,
        SCORE_BODY_STRUCTURE,
    ));

    // References: full points when no reference is dangling. A dangling
    // reference is already an error; here it also costs quality points.
    let refs = if report
        .errors
        .iter()
        .any(|e| e.contains("references missing file"))
    {
        0
    } else {
        SCORE_REFERENCES
    };
    breakdown.push(("references".to_string(), refs, SCORE_REFERENCES));

    report.score = breakdown.iter().map(|(_, earned, _)| earned).sum();
    report.score_breakdown = breakdown;
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

/// True when the trigger text names a condition for loading the skill. Passive
/// "X is a Y specialist" descriptions activate less reliably than ones that say
/// when to use them, so the matcher signal lives in these phrases.
fn has_trigger_language(description: &str, when_to_use: &str) -> bool {
    let text = format!("{description} {when_to_use}").to_ascii_lowercase();
    const TRIGGERS: &[&str] = &[
        "use when",
        "use this",
        "use for",
        "use to",
        "use before",
        "use after",
        "use on",
        "use during",
        "use it",
        "use proactively",
        "when you",
        "when the",
        "when a",
        "when working",
        "when editing",
        "when adding",
        "invoke when",
        "call when",
        "trigger when",
        "for tasks",
        "whenever",
        "always",
    ];
    TRIGGERS.iter().any(|phrase| text.contains(phrase))
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
        let root = std::env::temp_dir().join(format!("keel-skilllint-{label}-{pid}-{unique}"));
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
    fn well_formed_skill_scores_high() {
        // A complete skill — trigger phrase, healthy description, when_to_use,
        // scoped tools, and a body header — should land in the 90s. This pins
        // the scorer's calibration so a future weighting change is caught.
        let (root, skill_path) = temp_skill(
            "reviewer",
            "---\nname: reviewer\ndescription: Reviews code for production readiness across security and correctness. Use after implementation before opening a PR.\nwhen_to_use: After implementation, before PR.\nallowed-tools: Read, Grep\n---\n# Reviewer\n\n## Process\nReview steps.\n",
        );
        let report = lint_skill(&skill_path);
        assert!(report.ok(), "errors: {:?}", report.errors);
        assert!(
            report.score >= 90,
            "well-formed skill should score >= 90, got {} ({:?})",
            report.score,
            report.score_breakdown
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn lint_ok_but_low_quality_skill_scores_poorly() {
        // The key value over binary lint: this skill has NO errors (it is
        // lint-ok) but is low quality — a terse passive description, no
        // when_to_use, no allowed-tools, no body structure. The score must
        // reflect that even though pass/fail says "ok".
        let (root, skill_path) = temp_skill(
            "thin",
            "---\nname: thin\ndescription: A backend helper.\n---\nstub\n",
        );
        let report = lint_skill(&skill_path);
        assert!(report.ok(), "should be lint-ok: {:?}", report.errors);
        assert!(
            report.score < 50,
            "thin lint-ok skill should score < 50, got {} ({:?})",
            report.score,
            report.score_breakdown
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn score_breakdown_sums_to_total() {
        let (root, skill_path) = temp_skill(
            "sumcheck",
            "---\nname: sumcheck\ndescription: Use when verifying the scorer. A description long enough to clear the band.\nwhen_to_use: Always.\nallowed-tools: Read\n---\n# Heading\nbody\n",
        );
        let report = lint_skill(&skill_path);
        let summed: u32 = report.score_breakdown.iter().map(|(_, e, _)| e).sum();
        assert_eq!(summed, report.score, "breakdown must sum to the score");
        // The max points across dimensions must total 100.
        let max_total: u32 = report.score_breakdown.iter().map(|(_, _, m)| m).sum();
        assert_eq!(max_total, 100, "dimension maxima must total 100");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn min_score_gate_fails_a_low_quality_skill() {
        // --min-score turns the graded eval into a release gate: a lint-ok but
        // low-quality skill must make the command exit non-zero when a floor is
        // set, and exit 0 when no floor is set (backward compatible).
        let (root, skill_path) = temp_skill(
            "thin",
            "---\nname: thin\ndescription: A backend helper.\n---\nstub\n",
        );
        let repo_root = skill_path
            .parent()
            .and_then(|p| p.parent())
            .unwrap()
            .to_string_lossy()
            .to_string();

        // No floor: exits 0 despite the low score (lint-ok).
        let mut out = Vec::new();
        let mut err = Vec::new();
        let exit = run_skill_lint_command(
            &["--repo-root".to_string(), repo_root.clone()],
            &mut out,
            &mut err,
        );
        assert_eq!(
            exit,
            0,
            "no floor should pass; stderr: {}",
            String::from_utf8_lossy(&err)
        );

        // With a floor of 60: the thin skill is below it, so the gate fails.
        let mut out2 = Vec::new();
        let mut err2 = Vec::new();
        let exit2 = run_skill_lint_command(
            &[
                "--repo-root".to_string(),
                repo_root,
                "--min-score".to_string(),
                "60".to_string(),
            ],
            &mut out2,
            &mut err2,
        );
        assert_eq!(exit2, 1, "floor of 60 should fail the thin skill");
        let rendered = String::from_utf8_lossy(&out2);
        assert!(
            rendered.contains("below the --min-score floor"),
            "should report the floor failure; rendered: {rendered}"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn min_score_rejects_out_of_range_value() {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let exit = run_skill_lint_command(
            &["--min-score".to_string(), "150".to_string()],
            &mut out,
            &mut err,
        );
        assert_eq!(exit, 1);
        assert!(String::from_utf8_lossy(&err).contains("0..=100"));
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

    #[test]
    fn passive_description_warns_about_trigger_language() {
        let (root, skill_path) = temp_skill(
            "passive",
            "---\nname: passive\ndescription: A backend specialist for APIs and databases.\nallowed-tools: Read\n---\nbody\n",
        );
        let report = lint_skill(&skill_path);
        assert!(
            report.ok(),
            "passive description is a warning, not an error"
        );
        assert!(
            report.warnings.iter().any(|w| w.contains("trigger phrase")),
            "expected a trigger-language warning, got: {:?}",
            report.warnings
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn trigger_phrase_description_has_no_warning() {
        let (root, skill_path) = temp_skill(
            "active",
            "---\nname: active\ndescription: Backend specialist. Use when designing APIs or database schemas.\nallowed-tools: Read\n---\nbody\n",
        );
        let report = lint_skill(&skill_path);
        assert!(
            !report.warnings.iter().any(|w| w.contains("trigger phrase")),
            "a description with a trigger phrase must not warn: {:?}",
            report.warnings
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn has_trigger_language_accepts_common_phrasings() {
        assert!(has_trigger_language("Use before adding columns.", ""));
        assert!(has_trigger_language(
            "Use on every requirement-bearing prompt.",
            ""
        ));
        assert!(has_trigger_language(
            "Bootstrap skill.",
            "Always. Auto-loaded."
        ));
        assert!(has_trigger_language(
            "Specialist.",
            "Use when editing existing code."
        ));
        assert!(!has_trigger_language(
            "A backend and data specialist for APIs and databases.",
            "Backend tasks."
        ));
    }
}
