//! Purpose: Deterministic strict-format validator for Agile/Jira user stories,
//!   backing `claude-skills user-story lint`. Parses a markdown story set and
//!   fails when a story is missing a Connextra clause (role/goal/benefit) or has
//!   no Gherkin acceptance scenario (Given/When/Then), and warns on INVEST risks.
//!   This is the structural gate behind the `writing-user-stories` skill — the
//!   same role `skill-lint` plays for SKILL.md: catch a malformed artifact before
//!   it is trusted, without invoking the live model.
//! Caller: commands.rs `user-story` dispatch arm.
//! Dependencies: std::fs/io, crate::args::FlagSet, serde_json (already a
//!   workspace dependency).
//! Main Functions: run_user_story_command, lint_stories, StoryReport.
//! Side Effects: Reads the stories file (or stdin); writes a report. No mutation.
//!
//! Determinism: parsing is line-based and case-insensitive on the keyword anchors
//! ("As a", "I want", "so that", "Given", "When", "Then"). A story block starts at
//! a heading (`#`/`##`/...), a numbered/bulleted list item, or an "As a" line, and
//! runs until the next such start. Two runs over the same input produce identical
//! findings.

use std::fs;
use std::io::{Read, Write};

use crate::args::FlagSet;

/// One parsed story and the findings against it.
#[derive(Debug, Default, PartialEq, Eq)]
struct StoryReport {
    /// 1-based ordinal in the file, for stable referencing in output.
    index: usize,
    /// The story's narrative line (the "As a ..." line) or its heading, trimmed.
    title: String,
    errors: Vec<String>,
    warnings: Vec<String>,
}

impl StoryReport {
    fn ok(&self) -> bool {
        self.errors.is_empty()
    }
}

pub fn run_user_story_command(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let action = arguments.first().map(String::as_str).unwrap_or("");
    if action.is_empty() || matches!(action, "help" | "--help" | "-h") {
        let _ = writeln!(
            standard_output,
            "Usage: claude-skills user-story lint [flags]\n\
             \n\
             lint   Validate a user-story set against the strict format\n\
             \x20      (Connextra role/goal/benefit + Gherkin Given/When/Then),\n\
             \x20      flagging INVEST risks.\n\
             \n\
             Flags:\n\
             \x20 --file <path>  Markdown file containing the stories.\n\
             \x20 --stdin        Read the stories from standard input instead.\n\
             \x20 --json         Machine-readable output."
        );
        return if action.is_empty() { 1 } else { 0 };
    }
    match action {
        "lint" => run_lint(&arguments[1..], standard_output, standard_error),
        other => {
            let _ = writeln!(standard_error, "user-story: unknown subcommand: {other}");
            1
        }
    }
}

fn run_lint(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flags = FlagSet::new("user-story lint");
    flags.string_flag("file", "");
    flags.bool_flag("stdin", false);
    flags.bool_flag("json", false);
    if let Err(error) = flags.parse(arguments) {
        let _ = writeln!(standard_error, "user-story lint: {}", error.message);
        return 1;
    }

    let file = flags.string_value("file");
    let use_stdin = flags.bool_value("stdin");
    if file.is_empty() && !use_stdin {
        let _ = writeln!(
            standard_error,
            "user-story lint: provide --file <path> or --stdin"
        );
        return 1;
    }
    let text = if use_stdin {
        let mut buffer = String::new();
        if let Err(error) = std::io::stdin().read_to_string(&mut buffer) {
            let _ = writeln!(standard_error, "user-story lint: read stdin: {error}");
            return 1;
        }
        buffer
    } else {
        match fs::read_to_string(file) {
            Ok(text) => text,
            Err(error) => {
                let _ = writeln!(standard_error, "user-story lint: read {file}: {error}");
                return 1;
            }
        }
    };

    let reports = lint_stories(&text);
    let json = flags.bool_value("json");

    if reports.is_empty() {
        if json {
            let _ = writeln!(
                standard_output,
                "{}",
                serde_json::json!({"stories": 0, "failed": 0, "warned": 0, "ok": false})
            );
        } else {
            let _ = writeln!(
                standard_error,
                "user-story lint: no user stories found (expected at least one \"As a <role>, I want <goal>, so that <benefit>\" line)"
            );
        }
        // No stories at all is a failure: the artifact does not contain the thing
        // it claims to. This keeps an empty/garbage file from passing silently.
        return 1;
    }

    let failed = reports.iter().filter(|report| !report.ok()).count();
    let warned = reports
        .iter()
        .filter(|report| report.ok() && !report.warnings.is_empty())
        .count();

    if json {
        let stories: Vec<serde_json::Value> = reports
            .iter()
            .map(|report| {
                serde_json::json!({
                    "index": report.index,
                    "title": report.title,
                    "errors": report.errors,
                    "warnings": report.warnings,
                })
            })
            .collect();
        let payload = serde_json::json!({
            "stories": reports.len(),
            "failed": failed,
            "warned": warned,
            "ok": failed == 0,
            "reports": stories,
        });
        match serde_json::to_string_pretty(&payload) {
            Ok(serialized) => {
                let _ = writeln!(standard_output, "{serialized}");
            }
            Err(error) => {
                let _ = writeln!(standard_error, "user-story lint: render json: {error}");
                return 1;
            }
        }
    } else {
        for report in &reports {
            let status = if report.ok() { "ok" } else { "FAIL" };
            let _ = writeln!(
                standard_output,
                "[{status}] story {}: {}",
                report.index, report.title
            );
            for error in &report.errors {
                let _ = writeln!(standard_output, "    error: {error}");
            }
            for warning in &report.warnings {
                let _ = writeln!(standard_output, "    warn:  {warning}");
            }
        }
        let _ = writeln!(
            standard_output,
            "\nuser-story lint: {} story(ies), {failed} failed, {warned} warned",
            reports.len()
        );
    }

    if failed == 0 {
        0
    } else {
        1
    }
}

/// A raw story block: its starting title line plus the lines belonging to it.
struct Block {
    title: String,
    lines: Vec<String>,
}

/// Split the document into story blocks. A block begins at the first line that is
/// a markdown heading, a list item, or an "As a" narrative line, and extends to
/// the line before the next such boundary. Preamble before the first boundary is
/// ignored. This tolerates the common shapes (numbered list of stories, each story
/// under its own heading, or bare "As a" lines).
fn split_blocks(text: &str) -> Vec<Block> {
    let mut blocks: Vec<Block> = Vec::new();
    for raw in text.lines() {
        let trimmed = raw.trim();
        if is_block_start(trimmed) {
            blocks.push(Block {
                title: strip_marker(trimmed),
                lines: vec![raw.to_string()],
            });
        } else if let Some(current) = blocks.last_mut() {
            current.lines.push(raw.to_string());
        }
    }
    blocks
}

/// Whether a (trimmed) line begins a new story block.
fn is_block_start(trimmed: &str) -> bool {
    if trimmed.is_empty() {
        return false;
    }
    if trimmed.starts_with('#') {
        return true;
    }
    if starts_with_ci(trimmed, "as a ") || starts_with_ci(trimmed, "as an ") {
        return true;
    }
    // Ordered list ("1." / "1)") or bullet ("-", "*", "+") item.
    list_item_body(trimmed).is_some()
}

/// Strip a leading heading/list marker for the displayed title.
fn strip_marker(trimmed: &str) -> String {
    let without_heading = trimmed.trim_start_matches('#').trim_start();
    if let Some(body) = list_item_body(without_heading) {
        return body.to_string();
    }
    without_heading.to_string()
}

/// If `trimmed` is a list item, return its body after the marker.
fn list_item_body(trimmed: &str) -> Option<&str> {
    for bullet in ["- ", "* ", "+ "] {
        if let Some(rest) = trimmed.strip_prefix(bullet) {
            return Some(rest.trim_start());
        }
    }
    // Ordered: digits then '.' or ')' then space.
    let bytes = trimmed.as_bytes();
    let mut index = 0;
    while index < bytes.len() && bytes[index].is_ascii_digit() {
        index += 1;
    }
    if index > 0 && index < bytes.len() && (bytes[index] == b'.' || bytes[index] == b')') {
        let rest = trimmed[index + 1..].trim_start();
        if !rest.is_empty() || index + 1 == trimmed.len() {
            return Some(rest);
        }
    }
    None
}

/// Validate every story block. Only blocks that contain an "As a" clause somewhere
/// in their body are treated as user stories; heading/list blocks that are clearly
/// not stories (no narrative anywhere) are skipped so section headers do not
/// produce spurious failures.
fn lint_stories(text: &str) -> Vec<StoryReport> {
    let blocks = split_blocks(text);
    let mut reports = Vec::new();
    let mut index = 0usize;
    for block in &blocks {
        let body = block.lines.join("\n");
        let lower = body.to_lowercase();
        // A story is any block whose body contains the Connextra opener. Blocks
        // without it (pure section headings, prose) are not stories.
        if !lower.contains("as a ") && !lower.contains("as an ") {
            continue;
        }
        index += 1;
        let mut report = StoryReport {
            index,
            title: first_nonempty_title(&block.title, &body),
            ..Default::default()
        };

        check_connextra(&lower, &mut report);
        check_gherkin(&lower, &mut report);
        check_invest(&lower, &mut report);

        reports.push(report);
    }
    reports
}

fn first_nonempty_title(title: &str, body: &str) -> String {
    let candidate = title.trim();
    if !candidate.is_empty() && !candidate.starts_with('#') {
        return truncate_title(candidate);
    }
    // Fall back to the first "As a" line in the body.
    for line in body.lines() {
        let trimmed = line.trim();
        if starts_with_ci(trimmed, "as a ") || starts_with_ci(trimmed, "as an ") {
            return truncate_title(trimmed);
        }
    }
    truncate_title(candidate)
}

fn truncate_title(title: &str) -> String {
    const MAX: usize = 80;
    if title.chars().count() <= MAX {
        return title.to_string();
    }
    let cut: String = title.chars().take(MAX).collect();
    format!("{cut}…")
}

/// Connextra: the body must contain "as a"/"as an", "i want", and "so that", and
/// each clause must have non-empty content after the keyword.
fn check_connextra(lower: &str, report: &mut StoryReport) {
    let has_role = clause_has_content(lower, "as a ") || clause_has_content(lower, "as an ");
    if !has_role {
        report
            .errors
            .push("missing or empty role clause (\"As a <role>\")".into());
    }
    if !clause_has_content(lower, "i want ") {
        report
            .errors
            .push("missing or empty goal clause (\"I want <goal>\")".into());
    }
    if !clause_has_content(lower, "so that ") {
        report
            .errors
            .push("missing or empty benefit clause (\"so that <benefit>\")".into());
    }
}

/// Gherkin: the body must contain at least one Given, one When, and one Then with
/// content. We do not enforce strict ordering (a Then-first scenario is unusual
/// but legal), only presence of the three testable anchors.
fn check_gherkin(lower: &str, report: &mut StoryReport) {
    let missing: Vec<&str> = ["given ", "when ", "then "]
        .iter()
        .filter(|keyword| !line_keyword_has_content(lower, keyword))
        .copied()
        .collect();
    if !missing.is_empty() {
        let names: Vec<String> = missing
            .iter()
            .map(|keyword| keyword.trim().to_uppercase())
            .collect();
        report.errors.push(format!(
            "missing Gherkin acceptance criteria: no {} clause (need Given/When/Then)",
            names.join("/")
        ));
    }
}

/// INVEST: deterministic, non-blocking heuristics. These warn rather than error —
/// INVEST is a judgement framework, and only the worst smells are mechanically
/// detectable.
fn check_invest(lower: &str, report: &mut StoryReport) {
    // Testable: if Gherkin is present this is satisfied; otherwise the Gherkin
    // error already fired, so do not double-report.
    // Valuable: a benefit clause that is filler ("so that it works") is not value.
    for filler in ["so that it works", "so that it is done", "so that we can"] {
        if lower.contains(filler) {
            report.warnings.push(format!(
                "INVEST(Valuable): benefit clause looks like filler ({filler:?}); state a concrete user-visible value"
            ));
            break;
        }
    }
    // Small: a story enumerating many "and"-joined goals is likely an epic.
    if let Some(goal) = clause_content(lower, "i want ") {
        let and_count = goal.matches(" and ").count();
        if and_count >= 2 {
            report.warnings.push(
                "INVEST(Small): goal chains multiple capabilities with \"and\"; consider splitting into separate stories"
                    .into(),
            );
        }
    }
}

/// True if, on some line, `keyword` appears and is followed by non-whitespace
/// content on the same line. Used for the line-anchored Gherkin keywords.
fn line_keyword_has_content(lower: &str, keyword: &str) -> bool {
    for line in lower.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix(keyword) {
            if !rest.trim().is_empty() {
                return true;
            }
        }
    }
    false
}

/// True if `keyword` appears anywhere in `lower` followed by non-empty content up
/// to the next clause boundary. Used for the Connextra clauses which often share
/// one line ("As a X, I want Y, so that Z").
fn clause_has_content(lower: &str, keyword: &str) -> bool {
    clause_content(lower, keyword).is_some_and(|content| !content.is_empty())
}

/// Content after `keyword` up to the next Connextra keyword or end of segment.
fn clause_content(lower: &str, keyword: &str) -> Option<String> {
    let start = lower.find(keyword)? + keyword.len();
    let rest = &lower[start..];
    // Cut at the next clause keyword or a sentence/line break.
    let mut end = rest.len();
    for boundary in [", i want ", " i want ", ", so that ", " so that ", "\n"] {
        if let Some(position) = rest.find(boundary) {
            if position < end {
                end = position;
            }
        }
    }
    Some(
        rest[..end]
            .trim()
            .trim_end_matches([',', '.'])
            .trim()
            .to_string(),
    )
}

fn starts_with_ci(haystack: &str, prefix: &str) -> bool {
    haystack.len() >= prefix.len() && haystack[..prefix.len()].eq_ignore_ascii_case(prefix)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = "\
# Stories

1. As a developer, I want command output compacted, so that my context window lasts longer.
   Given a noisy build command
   When I run it through the proxy
   Then only high-signal lines enter context.

2. As an admin, I want to revoke a token, so that a leaked credential stops working.
   Given an active token
   When the admin revokes it
   Then the next request with that token is rejected.
";

    #[test]
    fn well_formed_stories_pass() {
        let reports = lint_stories(GOOD);
        assert_eq!(reports.len(), 2, "two stories parsed");
        assert!(
            reports.iter().all(|r| r.ok()),
            "both well-formed: {reports:?}"
        );
    }

    #[test]
    fn missing_benefit_clause_fails() {
        let text = "As a user, I want to log in.\nGiven creds\nWhen I submit\nThen I am in.\n";
        let reports = lint_stories(text);
        assert_eq!(reports.len(), 1);
        assert!(!reports[0].ok());
        assert!(
            reports[0].errors.iter().any(|e| e.contains("benefit")),
            "missing 'so that' must error: {:?}",
            reports[0].errors
        );
    }

    #[test]
    fn missing_gherkin_fails() {
        let text = "As a user, I want to log in, so that I can see my dashboard.\n";
        let reports = lint_stories(text);
        assert_eq!(reports.len(), 1);
        assert!(!reports[0].ok());
        assert!(
            reports[0].errors.iter().any(|e| e.contains("Gherkin")),
            "no Given/When/Then must error: {:?}",
            reports[0].errors
        );
    }

    #[test]
    fn missing_goal_clause_fails() {
        let text = "As a user, so that I stay logged in.\nGiven x\nWhen y\nThen z.\n";
        let reports = lint_stories(text);
        assert_eq!(reports.len(), 1);
        assert!(reports[0].errors.iter().any(|e| e.contains("goal")));
    }

    #[test]
    fn empty_document_yields_no_stories() {
        assert!(lint_stories("").is_empty());
        assert!(lint_stories("# Heading\n\nSome prose with no stories.\n").is_empty());
    }

    #[test]
    fn section_headings_without_narrative_are_not_stories() {
        let text = "## Background\n\nThis explains context but has no story.\n\n## Story\n\nAs a dev, I want X, so that Y.\nGiven a\nWhen b\nThen c.\n";
        let reports = lint_stories(text);
        assert_eq!(reports.len(), 1, "only the block with an 'As a' is a story");
    }

    #[test]
    fn epic_goal_warns_small() {
        let text = "As a user, I want to log in and reset my password and update my profile and delete my account, so that I control my identity.\nGiven x\nWhen y\nThen z.\n";
        let reports = lint_stories(text);
        assert_eq!(reports.len(), 1);
        assert!(reports[0].ok(), "INVEST issues warn, not fail");
        assert!(
            reports[0].warnings.iter().any(|w| w.contains("Small")),
            "multi-and goal warns Small: {:?}",
            reports[0].warnings
        );
    }

    #[test]
    fn filler_benefit_warns_valuable() {
        let text = "As a user, I want a feature, so that it works.\nGiven x\nWhen y\nThen z.\n";
        let reports = lint_stories(text);
        assert_eq!(reports.len(), 1);
        assert!(
            reports[0].warnings.iter().any(|w| w.contains("Valuable")),
            "filler benefit warns Valuable: {:?}",
            reports[0].warnings
        );
    }

    #[test]
    fn lint_is_deterministic() {
        let first = format!("{:?}", lint_stories(GOOD));
        let second = format!("{:?}", lint_stories(GOOD));
        assert_eq!(first, second);
    }
}
