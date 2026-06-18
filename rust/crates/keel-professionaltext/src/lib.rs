//! Purpose: Professional wording lint for committed text and final responses.
//! Caller: keel `git-workflow lint-message` and review surfaces.
//! Dependencies: std::sync::OnceLock; regex (workspace dep) for case-insensitive trigger matching.
//! Main Functions: lint_message, has_blocking_findings.
//! Side Effects: Pure analysis; no I/O. Rust-native professional text linting.

use std::sync::OnceLock;

use regex::Regex;

#[derive(Debug, Clone, Default, Copy)]
pub struct LintOptions {
    pub allow_claude_code_integration: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub id: String,
    pub severity: String,
    pub message: String,
}

fn first_person_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(r"(?i)\b(i|i'm|i've|we|we're|we've|our)\b")
            .expect("first_person pattern compiles")
    })
}

fn ai_tool_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(r"(?i)\b(ai|assistant|llm|model)\b").expect("ai_tool pattern compiles")
    })
}

fn chatty_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(r"(?i)\b(thanks|thank you|please review|hope this helps|happy to|let me know)\b")
            .expect("chatty pattern compiles")
    })
}

fn hype_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(
            r"(?i)\b(robust|comprehensive|seamless|powerful|world-class|best-in-class|magic)\b",
        )
        .expect("hype pattern compiles")
    })
}

pub fn lint_message(message_text: &str, options: LintOptions) -> Vec<Finding> {
    let mut findings: Vec<Finding> = Vec::new();
    let trimmed_message = message_text.trim();
    if trimmed_message.is_empty() {
        return findings;
    }
    if trimmed_message.contains(r"\n") || trimmed_message.contains(r"\r") {
        findings.push(Finding {
            id: "escaped-newlines".into(),
            severity: "high".into(),
            message: "Use real multiline text instead of escaped newline sequences.".into(),
        });
    }
    if first_person_pattern().is_match(trimmed_message) {
        findings.push(Finding {
            id: "first-person".into(),
            severity: "medium".into(),
            message: "Avoid first-person wording in commit, PR, review, and final text.".into(),
        });
    }
    if chatty_pattern().is_match(trimmed_message) {
        findings.push(Finding {
            id: "chatty-language".into(),
            severity: "medium".into(),
            message: "Avoid chatty wording; keep the message professional and diff-focused.".into(),
        });
    }
    if !options.allow_claude_code_integration && ai_tool_pattern().is_match(trimmed_message) {
        findings.push(Finding {
            id: "unrelated-ai-wording".into(),
            severity: "high".into(),
            message:
                "Avoid AI or tool wording unless the change is literally about the harness integration."
                    .into(),
        });
    }
    if hype_pattern().is_match(trimmed_message) {
        findings.push(Finding {
            id: "hype-wording".into(),
            severity: "medium".into(),
            message: "Avoid hype wording unless the diff provides specific evidence for the claim."
                .into(),
        });
    }
    findings
}

pub fn has_blocking_findings(findings: &[Finding]) -> bool {
    findings.iter().any(|finding| finding.severity == "high")
}

/// Comment marker styles keel knows how to lint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommentSyntax {
    /// `//`, `///`, `//!` (Rust, Go, C-family, JS/TS, Java, Kotlin, Swift).
    SlashSlash,
    /// `#` (Python, shell, TOML, YAML, Ruby).
    Hash,
    /// `<!-- -->` (Markdown, HTML, XML).
    Html,
}

/// One linted comment-style finding tied to a 1-based source line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommentFinding {
    pub line: usize,
    pub id: String,
    pub severity: String,
    pub message: String,
}

/// Pick a comment syntax from a file extension. Returns None for unknown types.
pub fn comment_syntax_for_path(path: &str) -> Option<CommentSyntax> {
    let extension = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match extension.as_str() {
        "rs" | "go" | "c" | "h" | "cpp" | "hpp" | "cc" | "cxx" | "js" | "jsx" | "ts" | "tsx"
        | "java" | "kt" | "kts" | "swift" | "scala" | "php" | "cs" | "rust" => {
            Some(CommentSyntax::SlashSlash)
        }
        "py" | "pyx" | "pxd" | "toml" | "yaml" | "yml" | "sh" | "bash" | "zsh" | "rb" | "pl"
        | "r" => Some(CommentSyntax::Hash),
        "md" | "markdown" | "html" | "htm" | "xml" => Some(CommentSyntax::Html),
        _ => None,
    }
}

/// True for structured doc headers that are exempt from the two-line cap.
fn is_doc_marker(trimmed: &str, syntax: CommentSyntax) -> bool {
    match syntax {
        CommentSyntax::SlashSlash => trimmed.starts_with("///") || trimmed.starts_with("//!"),
        CommentSyntax::Html => true,
        CommentSyntax::Hash => false,
    }
}

/// Strip the leading comment marker from a standalone comment line.
fn strip_marker(trimmed: &str, syntax: CommentSyntax) -> &str {
    match syntax {
        CommentSyntax::SlashSlash => trimmed
            .trim_start_matches('/')
            .trim_start_matches('!')
            .trim(),
        CommentSyntax::Hash => trimmed.trim_start_matches('#').trim(),
        CommentSyntax::Html => trimmed
            .trim_start_matches("<!--")
            .trim_end_matches("-->")
            .trim(),
    }
}

/// True when a trimmed line opens a standalone comment for the given syntax.
fn is_comment_line(trimmed: &str, syntax: CommentSyntax) -> bool {
    match syntax {
        CommentSyntax::SlashSlash => trimmed.starts_with("//"),
        // `#!` shebang and `#[derive(..)]` attributes are not prose comments.
        CommentSyntax::Hash => {
            trimmed.starts_with('#') && !trimmed.starts_with("#!") && !trimmed.starts_with("#[")
        }
        CommentSyntax::Html => trimmed.starts_with("<!--"),
    }
}

fn dangling_dash_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(r"[A-Za-z]\s+-\s+[A-Za-z]").expect("dangling_dash pattern compiles")
    })
}

/// Apply professional-wording rules to a single comment's joined text. Shared by
/// implementation comments and doc headers so wording stays consistent.
fn comment_wording_findings(line: usize, text: &str) -> Vec<CommentFinding> {
    let mut findings = Vec::new();
    if text.contains('\u{2014}') || text.contains('\u{2013}') {
        findings.push(CommentFinding {
            line,
            id: "comment-em-dash".into(),
            severity: "high".into(),
            message: "Replace em/en dash with a period or comma; it reads as AI-generated.".into(),
        });
    }
    if dangling_dash_pattern().is_match(text) {
        findings.push(CommentFinding {
            line,
            id: "comment-dangling-dash".into(),
            severity: "medium".into(),
            message: "Avoid ' - ' asides; split into a short sentence.".into(),
        });
    }
    if first_person_pattern().is_match(text) {
        findings.push(CommentFinding {
            line,
            id: "comment-first-person".into(),
            severity: "medium".into(),
            message: "Drop first-person wording; state what the code does.".into(),
        });
    }
    if chatty_pattern().is_match(text) {
        findings.push(CommentFinding {
            line,
            id: "comment-chatty".into(),
            severity: "medium".into(),
            message: "Drop chatty filler; keep the comment factual.".into(),
        });
    }
    if hype_pattern().is_match(text) {
        findings.push(CommentFinding {
            line,
            id: "comment-hype".into(),
            severity: "medium".into(),
            message: "Drop hype words; describe the behavior plainly.".into(),
        });
    }
    findings
}

/// Lint source comments for keel's house style. Implementation comments (`//`,
/// `#`) are capped at two lines; structured doc headers (`///`, `//!`, `<!--`)
/// are exempt from the cap. Both get the professional-wording rules. The `source`
/// is whole-file or diff-added text; `line_base` offsets reported line numbers.
pub fn lint_code_comments(
    source: &str,
    syntax: CommentSyntax,
    line_base: usize,
) -> Vec<CommentFinding> {
    let mut findings = Vec::new();
    let lines: Vec<&str> = source.lines().collect();
    let mut index = 0;
    while index < lines.len() {
        let trimmed = lines[index].trim_start();
        if !is_comment_line(trimmed, syntax) {
            index += 1;
            continue;
        }
        let is_doc = is_doc_marker(trimmed, syntax);
        let block_start = index;
        let mut block_text = String::new();
        // Group contiguous standalone comment lines of the same doc/impl class.
        while index < lines.len() {
            let line_trimmed = lines[index].trim_start();
            if !is_comment_line(line_trimmed, syntax)
                || is_doc_marker(line_trimmed, syntax) != is_doc
            {
                break;
            }
            if !block_text.is_empty() {
                block_text.push(' ');
            }
            block_text.push_str(strip_marker(line_trimmed, syntax));
            index += 1;
        }
        let report_line = line_base + block_start;
        let block_len = index - block_start;
        if !is_doc && block_len > 2 {
            findings.push(CommentFinding {
                line: report_line,
                id: "comment-too-long".into(),
                severity: "high".into(),
                message: format!(
                    "Implementation comment is {block_len} lines; keep it to two at most."
                ),
            });
        }
        findings.extend(comment_wording_findings(report_line, &block_text));
    }
    findings
}

/// True when any comment finding is blocking (high severity).
pub fn has_blocking_comment_findings(findings: &[CommentFinding]) -> bool {
    findings.iter().any(|finding| finding.severity == "high")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lint_message_rejects_chatty_and_ai_markers() {
        let findings = lint_message(
            "I made a robust fix. Thanks, please review. The AI did this.\\nTests passed.",
            LintOptions::default(),
        );
        let seen: std::collections::HashSet<&str> =
            findings.iter().map(|finding| finding.id.as_str()).collect();
        for expected in [
            "escaped-newlines",
            "first-person",
            "chatty-language",
            "unrelated-ai-wording",
            "hype-wording",
        ] {
            assert!(
                seen.contains(expected),
                "missing finding {expected} in {findings:?}"
            );
        }
    }

    #[test]
    fn lint_message_allows_empty_text() {
        let findings = lint_message("   ", LintOptions::default());
        assert!(findings.is_empty());
    }

    #[test]
    fn lint_message_accepts_concise_professional_body() {
        let message = "Problem\nHook install output overstated automatic command mutation.\n\nSolution\nState the PreToolUse Bash guidance limits and direct transparent compaction to shell profile wrappers.\n\nTest Result\ncargo test --workspace passed.";
        let findings = lint_message(message, LintOptions::default());
        assert!(findings.is_empty(), "expected clean, got {findings:?}");
    }

    #[test]
    fn lint_message_allows_claude_code_when_integration_is_explicit() {
        let findings = lint_message(
            "What Changed\n- the harness hook guidance now states current runtime limits.",
            LintOptions {
                allow_claude_code_integration: true,
            },
        );
        assert!(
            !has_blocking_findings(&findings),
            "unexpected blocking findings: {findings:?}"
        );
    }

    fn ids(findings: &[CommentFinding]) -> std::collections::HashSet<&str> {
        findings.iter().map(|f| f.id.as_str()).collect()
    }

    #[test]
    fn impl_comment_over_two_lines_is_blocking() {
        let source = "// line one\n// line two\n// line three\nlet x = 1;";
        let findings = lint_code_comments(source, CommentSyntax::SlashSlash, 1);
        assert!(ids(&findings).contains("comment-too-long"));
        assert!(has_blocking_comment_findings(&findings));
        assert_eq!(findings[0].line, 1);
    }

    #[test]
    fn two_line_impl_comment_is_clean() {
        let source = "// validate the token\n// before the database insert\nfoo();";
        let findings = lint_code_comments(source, CommentSyntax::SlashSlash, 1);
        assert!(findings.is_empty(), "expected clean, got {findings:?}");
    }

    #[test]
    fn doc_header_is_exempt_from_line_cap() {
        let source =
            "//! Purpose: a\n//! Caller: b\n//! Dependencies: c\n//! Main Functions: d\n//! Side Effects: e";
        let findings = lint_code_comments(source, CommentSyntax::SlashSlash, 1);
        assert!(
            !ids(&findings).contains("comment-too-long"),
            "doc headers must not trip the cap: {findings:?}"
        );
    }

    #[test]
    fn em_dash_in_comment_is_blocking() {
        let source = "// validate the token \u{2014} reject when expired\nfoo();";
        let findings = lint_code_comments(source, CommentSyntax::SlashSlash, 1);
        assert!(ids(&findings).contains("comment-em-dash"));
        assert!(has_blocking_comment_findings(&findings));
    }

    #[test]
    fn doc_header_em_dash_is_still_flagged() {
        let source = "/// Strip the marker \u{2014} return the inner text.";
        let findings = lint_code_comments(source, CommentSyntax::SlashSlash, 1);
        assert!(ids(&findings).contains("comment-em-dash"));
    }

    #[test]
    fn chatty_and_first_person_comment_flagged() {
        let source = "// Now we just quickly handle the edge case, hope this helps\nfoo();";
        let findings = lint_code_comments(source, CommentSyntax::SlashSlash, 1);
        let seen = ids(&findings);
        assert!(seen.contains("comment-first-person"));
        assert!(seen.contains("comment-chatty"));
    }

    #[test]
    fn hash_comment_cap_applies_and_shebang_attr_ignored() {
        let source = "#!/usr/bin/env python\n# one\n# two\n# three\nx = 1";
        let findings = lint_code_comments(source, CommentSyntax::Hash, 1);
        assert!(ids(&findings).contains("comment-too-long"));
        let shebang_only = lint_code_comments("#!/bin/sh\necho hi", CommentSyntax::Hash, 1);
        assert!(
            shebang_only.is_empty(),
            "shebang is not prose: {shebang_only:?}"
        );
    }

    #[test]
    fn line_base_offsets_reported_line() {
        let source = "// a\n// b\n// c";
        let findings = lint_code_comments(source, CommentSyntax::SlashSlash, 100);
        assert_eq!(findings[0].line, 100);
    }

    #[test]
    fn comment_syntax_resolves_from_extension() {
        assert_eq!(
            comment_syntax_for_path("src/x.rs"),
            Some(CommentSyntax::SlashSlash)
        );
        assert_eq!(comment_syntax_for_path("a/b.py"), Some(CommentSyntax::Hash));
        assert_eq!(
            comment_syntax_for_path("README.md"),
            Some(CommentSyntax::Html)
        );
        assert_eq!(comment_syntax_for_path("data.json"), None);
    }

    #[test]
    fn has_blocking_findings_classifies_high_severity_only() {
        assert!(!has_blocking_findings(&[Finding {
            id: "first-person".into(),
            severity: "medium".into(),
            message: "m".into(),
        }]));
        assert!(has_blocking_findings(&[Finding {
            id: "escaped-newlines".into(),
            severity: "high".into(),
            message: "m".into(),
        }]));
    }
}
