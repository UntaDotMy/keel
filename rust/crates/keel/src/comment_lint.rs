//! Purpose: Diff-scoped code-comment style gate over added lines in a git range.
//! Caller: review.rs surface/gates commands and the `review comments` subcommand.
//! Dependencies: keel-professionaltext engine, runtime::run_command for git diff.
//! Main Functions: lint_added_comments, scan_unified_diff, format_findings.
//! Side Effects: Reads git diff via run_command; pure analysis otherwise.

use std::path::Path;

use keel_professionaltext::{
    comment_syntax_for_path, has_blocking_comment_findings, lint_code_comments, CommentFinding,
    CommentSyntax,
};

use crate::runtime::run_command;

/// One comment-style finding located at a file and 1-based new-file line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileCommentFinding {
    pub file: String,
    pub line: usize,
    pub id: String,
    pub severity: String,
    pub message: String,
}

/// Lint the comment style of lines added between `base_ref` and the work tree.
/// Returns findings only for added (`+`) lines, so pre-existing comments are
/// grandfathered. An unreadable diff yields no findings rather than a false gate.
pub fn lint_added_comments(repo_root: &Path, base_ref: &str) -> Vec<FileCommentFinding> {
    let range = format!("{base_ref}...HEAD");
    let args = vec![
        "diff".to_string(),
        "--unified=0".to_string(),
        "--no-color".to_string(),
        range,
    ];
    let diff = match run_command("git", &args, Some(repo_root)) {
        Ok(result) if result.code == 0 => String::from_utf8_lossy(&result.stdout).to_string(),
        _ => return Vec::new(),
    };
    scan_unified_diff(&diff)
}

/// Lint the comment style of currently staged changes (working diff). Used by the
/// pre-commit surface where there is no upstream ref to compare against.
pub fn lint_working_comments(repo_root: &Path) -> Vec<FileCommentFinding> {
    let args = vec![
        "diff".to_string(),
        "--unified=0".to_string(),
        "--no-color".to_string(),
        "HEAD".to_string(),
    ];
    let diff = match run_command("git", &args, Some(repo_root)) {
        Ok(result) if result.code == 0 => String::from_utf8_lossy(&result.stdout).to_string(),
        _ => return Vec::new(),
    };
    scan_unified_diff(&diff)
}

/// Lint every tracked source file in the tree (whole-file scan). Used by
/// `review comments --all` for cleanup work over pre-existing comments.
pub fn lint_tracked_tree(repo_root: &Path) -> Vec<FileCommentFinding> {
    let args = vec!["ls-files".to_string()];
    let listing = match run_command("git", &args, Some(repo_root)) {
        Ok(result) if result.code == 0 => String::from_utf8_lossy(&result.stdout).to_string(),
        _ => return Vec::new(),
    };
    let mut findings = Vec::new();
    for rel_path in listing.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let Some(syntax) = comment_syntax_for_path(rel_path) else {
            continue;
        };
        let full = repo_root.join(rel_path);
        let Ok(source) = std::fs::read_to_string(&full) else {
            continue;
        };
        for finding in lint_code_comments(&source, syntax, 1) {
            findings.push(promote(rel_path, finding));
        }
    }
    findings
}

/// Parse a unified diff and lint the added comment lines per file. Tracks the new
/// file path and hunk line numbers so findings point at real post-merge lines.
pub fn scan_unified_diff(diff: &str) -> Vec<FileCommentFinding> {
    let mut findings = Vec::new();
    let mut current_file = String::new();
    let mut current_syntax: Option<CommentSyntax> = None;
    let mut new_line_cursor = 0usize;
    // A contiguous run of added lines is linted as one block so the two-line cap
    // sees the whole comment, not a single line in isolation.
    let mut block_text = String::new();
    let mut block_start = 0usize;

    let flush = |block_text: &mut String,
                 block_start: usize,
                 file: &str,
                 syntax: Option<CommentSyntax>,
                 out: &mut Vec<FileCommentFinding>| {
        if block_text.is_empty() {
            return;
        }
        if let Some(syntax) = syntax {
            for finding in lint_code_comments(block_text, syntax, block_start) {
                out.push(promote(file, finding));
            }
        }
        block_text.clear();
    };

    for line in diff.lines() {
        if let Some(path) = line.strip_prefix("+++ b/") {
            flush(
                &mut block_text,
                block_start,
                &current_file,
                current_syntax,
                &mut findings,
            );
            current_file = path.trim().to_string();
            current_syntax = comment_syntax_for_path(&current_file);
            continue;
        }
        if line.starts_with("+++ ") || line.starts_with("--- ") {
            continue;
        }
        if let Some(rest) = line.strip_prefix("@@") {
            flush(
                &mut block_text,
                block_start,
                &current_file,
                current_syntax,
                &mut findings,
            );
            new_line_cursor = parse_hunk_new_start(rest).unwrap_or(0);
            continue;
        }
        if let Some(added) = line.strip_prefix('+') {
            if block_text.is_empty() {
                block_start = new_line_cursor;
            } else {
                block_text.push('\n');
            }
            block_text.push_str(added);
            new_line_cursor += 1;
        } else {
            flush(
                &mut block_text,
                block_start,
                &current_file,
                current_syntax,
                &mut findings,
            );
            block_start = new_line_cursor;
        }
    }
    flush(
        &mut block_text,
        block_start,
        &current_file,
        current_syntax,
        &mut findings,
    );
    findings
}

/// Attach a file path to an engine finding, mapping it into a located result.
fn promote(file: &str, finding: CommentFinding) -> FileCommentFinding {
    FileCommentFinding {
        file: file.to_string(),
        line: finding.line,
        id: finding.id,
        severity: finding.severity,
        message: finding.message,
    }
}

/// Read the new-file start line from a hunk header tail like ` -1,0 +42,3 @@`.
fn parse_hunk_new_start(rest: &str) -> Option<usize> {
    let plus = rest.split('+').nth(1)?;
    let digits: String = plus.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

/// True when any added-line finding is blocking (high severity).
pub fn has_blocking(findings: &[FileCommentFinding]) -> bool {
    let engine_shape: Vec<CommentFinding> = findings
        .iter()
        .map(|f| CommentFinding {
            line: f.line,
            id: f.id.clone(),
            severity: f.severity.clone(),
            message: f.message.clone(),
        })
        .collect();
    has_blocking_comment_findings(&engine_shape)
}

/// Render findings as one compact line each: `file:line [severity] message`.
pub fn format_findings(findings: &[FileCommentFinding]) -> String {
    findings
        .iter()
        .map(|f| format!("{}:{} [{}] {}", f.file, f.line, f.severity, f.message))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn added_three_line_impl_comment_is_blocking() {
        let diff = "diff --git a/src/x.rs b/src/x.rs\n--- a/src/x.rs\n+++ b/src/x.rs\n@@ -0,0 +1,4 @@\n+// one\n+// two\n+// three\n+let x = 1;\n";
        let findings = scan_unified_diff(diff);
        assert!(findings.iter().any(|f| f.id == "comment-too-long"));
        assert!(has_blocking(&findings));
        assert_eq!(findings[0].file, "src/x.rs");
        assert_eq!(findings[0].line, 1);
    }

    #[test]
    fn added_em_dash_comment_is_blocking() {
        let diff =
            "+++ b/src/y.rs\n@@ -0,0 +10,1 @@\n+// reject when expired \u{2014} see policy\n";
        let findings = scan_unified_diff(diff);
        assert!(findings.iter().any(|f| f.id == "comment-em-dash"));
        assert_eq!(findings[0].line, 10);
    }

    #[test]
    fn existing_context_lines_are_not_linted() {
        let diff = "+++ b/src/z.rs\n@@ -1,3 +1,3 @@\n // one\n // two\n // three\n";
        let findings = scan_unified_diff(diff);
        assert!(
            findings.is_empty(),
            "context lines must be ignored: {findings:?}"
        );
    }

    #[test]
    fn added_doc_header_is_not_capped() {
        let diff = "+++ b/src/d.rs\n@@ -0,0 +1,4 @@\n+/// line one\n+/// line two\n+/// line three\n+pub fn f() {}\n";
        let findings = scan_unified_diff(diff);
        assert!(
            !findings.iter().any(|f| f.id == "comment-too-long"),
            "doc headers exempt from cap: {findings:?}"
        );
    }

    #[test]
    fn non_source_file_is_skipped() {
        let diff = "+++ b/data.json\n@@ -0,0 +1,3 @@\n+// one\n+// two\n+// three\n";
        let findings = scan_unified_diff(diff);
        assert!(
            findings.is_empty(),
            "json has no comment syntax: {findings:?}"
        );
    }

    #[test]
    fn clean_two_line_added_comment_passes() {
        let diff = "+++ b/src/ok.rs\n@@ -0,0 +5,3 @@\n+// validate the token\n+// before the insert\n+foo();\n";
        let findings = scan_unified_diff(diff);
        assert!(findings.is_empty(), "expected clean, got {findings:?}");
    }
}
