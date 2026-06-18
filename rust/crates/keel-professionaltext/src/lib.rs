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
                "Avoid AI or tool wording unless the change is literally about Claude Code integration."
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
            "What Changed\n- Claude Code hook guidance now states current runtime limits.",
            LintOptions {
                allow_claude_code_integration: true,
            },
        );
        assert!(
            !has_blocking_findings(&findings),
            "unexpected blocking findings: {findings:?}"
        );
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
