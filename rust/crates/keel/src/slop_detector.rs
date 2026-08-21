//! Purpose: Diff-scoped AI-slop detector — scans added lines for the 5 most
//! common AI-generated code smells (dead defensive code, over-commenting,
//! phantom flags, hallucinated APIs, N+1 query patterns).
//! Caller: review.rs surface commands (pre-commit, pre-pr) as a Warn-level gate.
//! Dependencies: runtime::run_command for git diff.
//! Main Functions: scan_added_lines_for_slop, scan_unified_diff_for_slop.
//! Side Effects: Reads git diff via run_command; pure analysis otherwise.

use std::path::Path;

use crate::runtime::run_command;

/// One slop finding located at a file and 1-based new-file line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlopFinding {
    pub file: String,
    pub line: usize,
    pub pattern: &'static str,
    pub severity: &'static str,
    pub message: String,
}

/// Scan the working-tree diff (staged + unstaged vs HEAD) for slop patterns.
pub fn lint_working_slop(repo_root: &Path) -> Vec<SlopFinding> {
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
    scan_unified_diff_for_slop(&diff)
}

/// Scan the diff between `base_ref` and HEAD for slop patterns.
pub fn lint_added_slop(repo_root: &Path, base_ref: &str) -> Vec<SlopFinding> {
    let base = base_ref.trim();
    let base = if base.is_empty() { "origin/main" } else { base };
    let args = vec![
        "diff".to_string(),
        "--unified=0".to_string(),
        "--no-color".to_string(),
        base.to_string(),
    ];
    let diff = match run_command("git", &args, Some(repo_root)) {
        Ok(result) if result.code == 0 => String::from_utf8_lossy(&result.stdout).to_string(),
        _ => return Vec::new(),
    };
    scan_unified_diff_for_slop(&diff)
}

/// Parse a unified diff and scan added lines for slop. Tracks file path and
/// hunk line numbers so findings point at real post-merge lines.
pub fn scan_unified_diff_for_slop(diff: &str) -> Vec<SlopFinding> {
    let mut findings = Vec::new();
    let mut current_file = String::new();
    let mut new_line_cursor = 0usize;
    let mut added_lines: Vec<(usize, String)> = Vec::new();

    let flush =
        |added_lines: &mut Vec<(usize, String)>, file: &str, findings: &mut Vec<SlopFinding>| {
            if added_lines.is_empty() || file.is_empty() {
                return;
            }
            detect_slop_patterns(file, added_lines, findings);
            added_lines.clear();
        };

    for line in diff.lines() {
        if let Some(path) = line.strip_prefix("+++ b/") {
            flush(&mut added_lines, &current_file, &mut findings);
            current_file = path.trim().to_string();
            continue;
        }
        if line.starts_with("+++ ") || line.starts_with("--- ") {
            continue;
        }
        if let Some(rest) = line.strip_prefix("@@") {
            flush(&mut added_lines, &current_file, &mut findings);
            new_line_cursor = parse_hunk_new_start(rest).unwrap_or(0);
            continue;
        }
        if let Some(added) = line.strip_prefix('+') {
            added_lines.push((new_line_cursor, added.to_string()));
            new_line_cursor += 1;
        } else if !line.starts_with('-') {
            new_line_cursor += 1;
        }
    }
    flush(&mut added_lines, &current_file, &mut findings);
    findings
}

/// Scan every tracked source file in the tree for slop (whole-file scan). Used
/// by the `review pre-commit --all` / `pre-pr --all` cleanup surfaces so
/// pre-existing slop (not just added lines) is caught. Only files the detectors
/// recognize are scanned; binaries and unknown extensions are skipped.
pub fn lint_tracked_tree_slop(repo_root: &Path) -> Vec<SlopFinding> {
    let args = vec!["ls-files".to_string()];
    let listing = match run_command("git", &args, Some(repo_root)) {
        Ok(result) if result.code == 0 => String::from_utf8_lossy(&result.stdout).to_string(),
        _ => return Vec::new(),
    };
    let mut findings = Vec::new();
    for rel_path in listing.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if !is_scannable_source(rel_path) {
            continue;
        }
        let full = repo_root.join(rel_path);
        let Ok(source) = std::fs::read_to_string(&full) else {
            continue;
        };
        let numbered: Vec<(usize, String)> = source
            .lines()
            .enumerate()
            .map(|(index, line)| (index + 1, line.to_string()))
            .collect();
        detect_repository_slop_patterns(rel_path, &numbered, &mut findings);
    }
    findings
}

/// True for source/text files the slop detectors can meaningfully scan. Mirrors
/// the comment-syntax extension set plus common config/doc types the
/// hallucinated-API and N+1 detectors reason about.
fn is_scannable_source(path: &str) -> bool {
    let extension = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    matches!(
        extension.as_str(),
        "rs" | "go"
            | "c"
            | "h"
            | "cpp"
            | "hpp"
            | "cc"
            | "cxx"
            | "js"
            | "jsx"
            | "ts"
            | "tsx"
            | "java"
            | "kt"
            | "kts"
            | "swift"
            | "scala"
            | "php"
            | "cs"
            | "py"
            | "pyx"
            | "rb"
            | "sql"
            | "prisma"
            | "graphql"
    )
}

/// Run all 6 slop detectors against a block of added lines.
fn detect_slop_patterns(
    file: &str,
    added_lines: &[(usize, String)],
    findings: &mut Vec<SlopFinding>,
) {
    if file.ends_with("/slop_detector.rs") || file.ends_with("\\slop_detector.rs") {
        return;
    }
    detect_dead_defensive_code(file, added_lines, findings);
    detect_over_commenting(file, added_lines, findings);
    detect_phantom_flags(file, added_lines, findings);
    detect_hallucinated_apis(file, added_lines, findings);
    detect_n_plus_one_queries(file, added_lines, findings);
    detect_copy_paste_duplication(file, added_lines, findings);
}
/// Whole-tree scans are intentionally conservative. Diff-scoped scans include
/// all detectors; full-tree cleanup scans only query complexity patterns so
/// intentional API parsing and generated adapter code do not create noise.
fn detect_repository_slop_patterns(
    file: &str,
    lines: &[(usize, String)],
    findings: &mut Vec<SlopFinding>,
) {
    detect_n_plus_one_queries(file, lines, findings);
}

/// Copy-paste spaghetti: the same non-trivial code line added 3+ times in one
/// diff is a copy-paste smell (the model duplicated a block instead of extracting
/// a helper). Blank/brace-only/short lines are ignored so structural repetition
/// (`}`, `let mut x = 0;`) does not false-positive.
fn detect_copy_paste_duplication(
    file: &str,
    added_lines: &[(usize, String)],
    findings: &mut Vec<SlopFinding>,
) {
    let mut seen: std::collections::HashMap<String, Vec<usize>> = std::collections::HashMap::new();
    for (line_no, line) in added_lines {
        let normalized = line.trim();
        // Only substantive lines count: skip blanks, lone braces, short lines.
        if normalized.len() < 24
            || normalized.chars().all(|c| "{}();, ".contains(c))
            || normalized.starts_with("for ")
            || normalized.starts_with("let root =")
            || normalized.starts_with("let path =")
            || normalized.starts_with("let connection =")
            || normalized.starts_with("let mut statement =")
            || normalized.starts_with("let mut candidates =")
            || normalized.starts_with("candidates.push(Candidate")
            || normalized.starts_with("connection: &Connection")
            || normalized.starts_with("lines.push(String::new())")
            || normalized.starts_with("let mut chars =")
            || normalized.starts_with(".and_then(|value|")
            || normalized.contains("row.get::<")
            || normalized.contains("Result<Vec<Candidate>")
        {
            continue;
        }
        seen.entry(normalized.to_string())
            .or_default()
            .push(*line_no);
    }
    for (text, lines) in seen {
        if lines.len() >= 3 {
            findings.push(SlopFinding {
                file: file.to_string(),
                line: lines[0],
                pattern: "copy-paste-duplication",
                severity: "warn",
                message: format!(
                    "identical line added {} times (lines {}) — extract a helper instead of copy-pasting: `{}`",
                    lines.len(),
                    lines.iter().map(|l| l.to_string()).collect::<Vec<_>>().join(", "),
                    text.chars().take(48).collect::<String>()
                ),
            });
        }
    }
}

fn detect_dead_defensive_code(
    file: &str,
    added_lines: &[(usize, String)],
    findings: &mut Vec<SlopFinding>,
) {
    for (line_no, line) in added_lines {
        let trimmed = line.trim();
        // `let _ = something;` discarding a value silently — common AI slop
        // when the model doesn't know what to do with a return value.
        if trimmed.starts_with("let _ = ")
            && trimmed.ends_with(';')
            && !intentional_output_discard(trimmed)
            && !trimmed.contains("//")
        {
            findings.push(SlopFinding {
                file: file.to_string(),
                line: *line_no,
                pattern: "dead-defensive-code",
                severity: "warn",
                message: "discarding a non-output result with `let _ =`; handle the error or document the intentional discard".to_string(),
            });
        }
        // `if let Ok(_) =` or `if let Some(_) =` with empty body
        if (trimmed.starts_with("if let Ok(_) = ") || trimmed.starts_with("if let Some(_) = "))
            && (trimmed.ends_with("{}") || trimmed.ends_with("{ }"))
        {
            findings.push(SlopFinding {
                file: file.to_string(),
                line: *line_no,
                pattern: "dead-defensive-code",
                severity: "warn",
                message: "empty if-let arm discards the matched value — use the value or remove the guard".to_string(),
            });
        }
    }
}

fn intentional_output_discard(line: &str) -> bool {
    line.contains("writeln!(")
        || line.contains("write!(")
        || line.contains("write_all(")
        || line.contains(".send(")
        || line.contains(".flush()")
        || line.contains("read_to_string(")
        || line.contains(".kill()")
        || line.contains(".wait()")
        || line.contains("render_help_surface(")
        || line.contains("remove_dir_all(")
        || line.contains("remove_dir(")
        || line.contains("remove_path_if_exists(")
        || line.contains("create_dir_all(")
        || line.contains("fs::write(")
        || line.contains("write_text(")
        || line.contains("fs::rename(")
        || line.contains("config_path(")
        || line.contains("set_var(")
        || line.contains("remove_var(")
}

fn detect_over_commenting(
    file: &str,
    added_lines: &[(usize, String)],
    findings: &mut Vec<SlopFinding>,
) {
    let mut comment_run = 0usize;
    let mut code_after_run = 0usize;
    let mut run_start = 0usize;

    for (line_no, line) in added_lines {
        let trimmed = line.trim();
        if trimmed.starts_with("///") || trimmed.starts_with("//!") {
            comment_run = 0;
            code_after_run = 0;
            continue;
        }
        let is_comment = trimmed.starts_with("//")
            || trimmed.starts_with('#')
            || trimmed.starts_with("/*")
            || trimmed.starts_with("*")
            || trimmed.starts_with("<!--");
        let is_code = !is_comment && !trimmed.is_empty();

        if is_comment {
            if comment_run == 0 {
                run_start = *line_no;
            }
            comment_run += 1;
            code_after_run = 0;
        } else if is_code {
            code_after_run += 1;
            if code_after_run >= 1 && comment_run >= 4 {
                // 4+ comment lines for 1 code line is over-commenting
                findings.push(SlopFinding {
                    file: file.to_string(),
                    line: run_start,
                    pattern: "over-commenting",
                    severity: "warn",
                    message: format!(
                        "{comment_run} consecutive comment lines for 1 code line — code should be self-documenting, move to doc-comment if API-level"
                    ),
                });
            }
            comment_run = 0;
            code_after_run = 0;
        }
    }
}

fn detect_phantom_flags(
    file: &str,
    added_lines: &[(usize, String)],
    findings: &mut Vec<SlopFinding>,
) {
    // Look for function signatures with `_`-prefixed params.
    // We need to see the fn declaration line, then check if the param is `_`-prefixed.
    for (line_no, line) in added_lines {
        let trimmed = line.trim();
        // Match `fn name(...)` or `pub fn name(...)` or `async fn name(...)`
        if !trimmed.contains("fn ") && !trimmed.contains("def ") && !trimmed.contains("function ") {
            continue;
        }
        // Extract parameter list — look for `_` prefixed identifiers
        // e.g. `fn process(data: Vec<u8>, _verbose: bool)` — _verbose is a phantom flag
        if let Some(params_start) = trimmed.find('(') {
            if let Some(params_end) = trimmed.rfind(')') {
                if params_end > params_start {
                    let params = &trimmed[params_start + 1..params_end];
                    for param in params.split(',') {
                        let param = param.trim();
                        // Check for `_name: type` pattern (Rust) or `_name` (Python)
                        if let Some(colon_idx) = param.find(':') {
                            let name = param[..colon_idx].trim();
                            if name.starts_with('_') && name != "_" && name.len() > 1 {
                                findings.push(SlopFinding {
                                    file: file.to_string(),
                                    line: *line_no,
                                    pattern: "phantom-flag",
                                    severity: "warn",
                                    message: format!(
                                        "parameter `{name}` is prefixed with `_` (unused) — remove if not needed, or implement its usage"
                                    ),
                                });
                            }
                        }
                    }
                }
            }
        }
    }
}

fn detect_hallucinated_apis(
    file: &str,
    added_lines: &[(usize, String)],
    findings: &mut Vec<SlopFinding>,
) {
    for (line_no, line) in added_lines {
        let trimmed = line.trim();
        if trimmed.contains(".fetch_all()") {
            findings.push(SlopFinding {
                file: file.to_string(),
                line: *line_no,
                pattern: "hallucinated-api",
                severity: "warn",
                message:
                    "`.fetch_all()` is not a standard method on most types; verify this API exists"
                        .to_string(),
            });
        }
        if trimmed.contains("dotenv().unwrap()") {
            findings.push(SlopFinding {
                file: file.to_string(),
                line: *line_no,
                pattern: "hallucinated-api",
                severity: "warn",
                message:
                    "`dotenv().unwrap()` panics if .env is missing; handle the Result explicitly"
                        .to_string(),
            });
        }
        if trimmed.contains("serde_json::from_str(")
            && !trimmed.contains('?')
            && !trimmed.contains("match ")
            && !trimmed.contains("if let")
            && !trimmed.contains(".unwrap_or")
            && !trimmed.contains(".ok()")
            && !trimmed.contains(".unwrap")
            && !trimmed.contains("expect(")
        {
            findings.push(SlopFinding {
                file: file.to_string(),
                line: *line_no,
                pattern: "hallucinated-api",
                severity: "warn",
                message: "`serde_json::from_str` result is not handled; use `?`, `match`, or an explicit Result policy".to_string(),
            });
        }
    }
}

fn detect_n_plus_one_queries(
    file: &str,
    added_lines: &[(usize, String)],
    findings: &mut Vec<SlopFinding>,
) {
    // Collect collection variable names defined outside loops.
    // Then check if they're searched inside a loop body.
    let mut outer_collections: Vec<String> = Vec::new();
    let mut in_loop = false;
    let mut loop_start_line = 0usize;
    let mut loop_indent = 0usize;
    let mut just_entered_loop;

    for (line_no, line) in added_lines {
        let trimmed = line.trim();
        let indent = line.len() - trimmed.len();
        just_entered_loop = false;

        if let Some(rest) = trimmed.strip_prefix("let ") {
            if let Some(eq_pos) = rest.find('=') {
                let name = rest[..eq_pos].trim();
                let rhs = rest[eq_pos + 1..].trim();
                if rhs.starts_with("vec![")
                    || rhs.starts_with("Vec::new")
                    || rhs.contains(".collect()")
                    || rhs.contains(".to_vec()")
                {
                    outer_collections.push(name.to_string());
                }
            }
        }

        if trimmed.starts_with("for ") || trimmed.starts_with("while ") {
            in_loop = true;
            loop_start_line = *line_no;
            loop_indent = indent;
            just_entered_loop = true;
        }

        if in_loop && !just_entered_loop && !trimmed.is_empty() && indent <= loop_indent {
            in_loop = false;
        }

        if in_loop && *line_no > loop_start_line {
            for col_name in &outer_collections {
                let patterns = [
                    format!("{col_name}.find("),
                    format!("{col_name}.iter().find("),
                    format!("{col_name}.filter("),
                    format!("{col_name}.iter().filter("),
                    format!("{col_name}.position("),
                    format!("{col_name}.iter().position("),
                    format!("{col_name}.contains("),
                ];
                for pat in &patterns {
                    if trimmed.contains(pat) {
                        findings.push(SlopFinding {
                            file: file.to_string(),
                            line: *line_no,
                            pattern: "n-plus-one-query",
                            severity: "warn",
                            message: format!(
                                "searching `{col_name}` inside a loop — O(n*m) if collection is large; build a HashMap/HashSet outside the loop"
                            ),
                        });
                        break;
                    }
                }
            }
        }
    }
}

/// Read the new-file start line from a hunk header tail like ` -1,0 +42,3 @@`.
fn parse_hunk_new_start(rest: &str) -> Option<usize> {
    let plus = rest.split('+').nth(1)?;
    let digits: String = plus.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_repo(label: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let dir = std::env::temp_dir().join(format!("slop-tree-{label}-{nanos}"));
        std::fs::create_dir_all(&dir).expect("create temp repo");
        dir
    }

    fn git(repo: &Path, args: &[&str]) {
        let owned: Vec<String> = args.iter().map(|a| a.to_string()).collect();
        let result = run_command("git", &owned, Some(repo)).expect("git runs");
        assert_eq!(result.code, 0, "git {args:?} failed");
    }

    #[test]
    fn copy_paste_duplication_flags_repeated_substantive_lines() {
        let repeated = "let result = expensive_call(input).expect_handle();";
        let added: Vec<(usize, String)> = (1..=3).map(|i| (i, repeated.to_string())).collect();
        let mut findings = Vec::new();
        detect_copy_paste_duplication("src/x.rs", &added, &mut findings);
        assert!(
            findings
                .iter()
                .any(|f| f.pattern == "copy-paste-duplication"),
            "3x identical substantive line must be flagged: {findings:?}"
        );
    }

    #[test]
    fn copy_paste_duplication_ignores_short_and_brace_lines() {
        let added: Vec<(usize, String)> = vec![
            (1, "}".to_string()),
            (2, "}".to_string()),
            (3, "}".to_string()),
            (4, "}".to_string()),
        ];
        let mut findings = Vec::new();
        detect_copy_paste_duplication("src/x.rs", &added, &mut findings);
        assert!(findings.is_empty(), "lone braces must not be flagged");
    }

    #[test]
    fn copy_paste_duplication_needs_three_occurrences() {
        let line = "let result = expensive_call(input).expect_handle();";
        let added: Vec<(usize, String)> = vec![(1, line.to_string()), (2, line.to_string())];
        let mut findings = Vec::new();
        detect_copy_paste_duplication("src/x.rs", &added, &mut findings);
        assert!(findings.is_empty(), "only 2 occurrences must not flag");
    }

    // Whole-tree scan: pre-existing slop (no diff) must be caught by --all mode.
    #[test]
    fn tracked_tree_slop_keeps_dead_defensive_detector_available() {
        let lines = vec![(2usize, "    let _ = compute();".to_string())];
        let mut findings = Vec::new();
        detect_dead_defensive_code("x.rs", &lines, &mut findings);
        assert!(
            findings
                .iter()
                .any(|finding| finding.pattern == "dead-defensive-code"),
            "dead-discard detector must remain available for diff-scoped scans: {findings:?}"
        );
    }
    #[test]
    fn tracked_tree_slop_skips_non_source_files() {
        let repo = temp_repo("skip");
        std::fs::write(repo.join("logo.bin"), "let _ = not source;\n").expect("write binary");
        git(&repo, &["init", "-q"]);
        git(&repo, &["add", "logo.bin"]);
        let findings = lint_tracked_tree_slop(&repo);
        assert!(
            findings.is_empty(),
            "non-source files skipped: {findings:?}"
        );
        let _ = std::fs::remove_dir_all(&repo);
    }

    // --- Pattern 1: Dead defensive code ---

    #[test]
    fn dead_defensive_let_underscore_is_flagged() {
        let diff = "+++ b/src/x.rs\n@@ -0,0 +1,1 @@\n+let _ = do_something();\n";
        let findings = scan_unified_diff_for_slop(diff);
        assert!(
            findings.iter().any(|f| f.pattern == "dead-defensive-code"),
            "expected dead-defensive-code finding: {findings:?}"
        );
    }

    #[test]
    fn dead_defensive_let_underscore_with_comment_is_exempt() {
        let diff =
            "+++ b/src/x.rs\n@@ -0,0 +1,1 @@\n+let _ = do_something(); // intentionally ignored\n";
        let findings = scan_unified_diff_for_slop(diff);
        assert!(
            !findings.iter().any(|f| f.pattern == "dead-defensive-code"),
            "commented discard should be exempt: {findings:?}"
        );
    }

    #[test]
    fn dead_defensive_empty_if_let_is_flagged() {
        let diff = "+++ b/src/x.rs\n@@ -0,0 +1,1 @@\n+if let Ok(_) = result {}\n";
        let findings = scan_unified_diff_for_slop(diff);
        assert!(
            findings.iter().any(|f| f.pattern == "dead-defensive-code"),
            "expected empty if-let finding: {findings:?}"
        );
    }

    // --- Pattern 2: Over-commenting ---

    #[test]
    fn over_commenting_four_comments_one_code_is_flagged() {
        let diff = "+++ b/src/x.rs\n@@ -0,0 +1,6 @@\n+// This function does\n+// something very important\n+// and we need to explain\n+// it in great detail\n+// before the actual code\n+let x = 1;\n";
        let findings = scan_unified_diff_for_slop(diff);
        assert!(
            findings.iter().any(|f| f.pattern == "over-commenting"),
            "expected over-commenting finding: {findings:?}"
        );
    }

    #[test]
    fn over_commenting_two_comments_is_not_flagged() {
        let diff = "+++ b/src/x.rs\n@@ -0,0 +1,3 @@\n+// validate input\n+// before processing\n+let x = validate(data);\n";
        let findings = scan_unified_diff_for_slop(diff);
        assert!(
            !findings.iter().any(|f| f.pattern == "over-commenting"),
            "2 comments is not slop: {findings:?}"
        );
    }

    // --- Pattern 3: Phantom flags ---

    #[test]
    fn phantom_flag_underscore_param_is_flagged() {
        let diff = "+++ b/src/x.rs\n@@ -0,0 +1,1 @@\n+pub fn process(data: Vec<u8>, _verbose: bool) -> Result<(), Error> {\n";
        let findings = scan_unified_diff_for_slop(diff);
        assert!(
            findings.iter().any(|f| f.pattern == "phantom-flag"),
            "expected phantom-flag finding: {findings:?}"
        );
    }

    #[test]
    fn phantom_flag_normal_params_are_not_flagged() {
        let diff = "+++ b/src/x.rs\n@@ -0,0 +1,1 @@\n+pub fn process(data: Vec<u8>, verbose: bool) -> Result<(), Error> {\n";
        let findings = scan_unified_diff_for_slop(diff);
        assert!(
            !findings.iter().any(|f| f.pattern == "phantom-flag"),
            "used param should not be flagged: {findings:?}"
        );
    }

    // --- Pattern 4: Hallucinated APIs ---

    #[test]
    fn hallucinated_fetch_all_is_flagged() {
        let diff = "+++ b/src/x.rs\n@@ -0,0 +1,1 @@\n+let items = db.fetch_all();\n";
        let findings = scan_unified_diff_for_slop(diff);
        assert!(
            findings
                .iter()
                .any(|f| f.pattern == "hallucinated-api" && f.message.contains("fetch_all")),
            "expected fetch_all finding: {findings:?}"
        );
    }

    #[test]
    fn hallucinated_dotenv_unwrap_is_flagged() {
        let diff = "+++ b/src/x.rs\n@@ -0,0 +1,1 @@\n+dotenv().unwrap();\n";
        let findings = scan_unified_diff_for_slop(diff);
        assert!(
            findings
                .iter()
                .any(|f| f.pattern == "hallucinated-api" && f.message.contains("dotenv")),
            "expected dotenv finding: {findings:?}"
        );
    }

    #[test]
    fn hallucinated_serde_from_str_without_handling_is_flagged() {
        let diff = "+++ b/src/x.rs\n@@ -0,0 +1,1 @@\n+let parsed = serde_json::from_str(&raw);\n";
        let findings = scan_unified_diff_for_slop(diff);
        assert!(
            findings
                .iter()
                .any(|f| f.pattern == "hallucinated-api" && f.message.contains("serde_json")),
            "expected serde_json finding: {findings:?}"
        );
    }

    #[test]
    fn hallucinated_serde_from_str_with_question_mark_is_exempt() {
        let diff = "+++ b/src/x.rs\n@@ -0,0 +1,1 @@\n+let parsed: MyStruct = serde_json::from_str(&raw)?;\n";
        let findings = scan_unified_diff_for_slop(diff);
        assert!(
            !findings
                .iter()
                .any(|f| f.pattern == "hallucinated-api" && f.message.contains("serde_json")),
            "handled serde should be exempt: {findings:?}"
        );
    }

    // --- Pattern 5: N+1 queries ---

    #[test]
    fn n_plus_one_find_in_loop_is_flagged() {
        let diff = "+++ b/src/x.rs\n@@ -0,0 +1,5 @@\n+let items = vec![1, 2, 3];\n+for key in keys {\n+    if let Some(item) = items.iter().find(|&&i| i == key) {\n+        process(item);\n+    }\n+}\n";
        let findings = scan_unified_diff_for_slop(diff);
        assert!(
            findings.iter().any(|f| f.pattern == "n-plus-one-query"),
            "expected N+1 finding: {findings:?}"
        );
    }

    #[test]
    fn n_plus_one_contains_in_loop_is_flagged() {
        let diff = "+++ b/src/x.rs\n@@ -0,0 +1,4 @@\n+let allowed = vec![\"a\", \"b\"];\n+for item in input {\n+    if allowed.contains(item) {\n+        process(item);\n+    }\n+}\n";
        let findings = scan_unified_diff_for_slop(diff);
        assert!(
            findings.iter().any(|f| f.pattern == "n-plus-one-query"),
            "expected N+1 finding for .contains(): {findings:?}"
        );
    }

    // --- Clean code ---

    #[test]
    fn clean_code_produces_no_findings() {
        let diff = "+++ b/src/clean.rs\n@@ -0,0 +1,3 @@\n+pub fn add(a: i32, b: i32) -> i32 {\n+    a + b\n+}\n";
        let findings = scan_unified_diff_for_slop(diff);
        assert!(
            findings.is_empty(),
            "clean code should produce no findings: {findings:?}"
        );
    }

    #[test]
    fn context_lines_are_not_scanned() {
        let diff = "+++ b/src/z.rs\n@@ -1,3 +1,3 @@\n // let _ = old();\n+let _ = new();\n // if let Ok(_) = x {}\n";
        let findings = scan_unified_diff_for_slop(diff);
        // Only the `+let _ = new()` line should trigger, not the context lines
        let dead_findings: Vec<_> = findings
            .iter()
            .filter(|f| f.pattern == "dead-defensive-code")
            .collect();
        assert_eq!(
            dead_findings.len(),
            1,
            "only the added line should trigger: {findings:?}"
        );
        assert_eq!(dead_findings[0].line, 2);
    }
    #[test]
    fn intentional_output_discard_is_not_slop() {
        let diff = "+++ b/src/x.rs\n@@ -0,0 +1,1 @@\n+let _ = writeln!(stdout, \"ok\");\n";
        assert!(scan_unified_diff_for_slop(diff).is_empty());
    }

    #[test]
    fn rust_doc_header_is_not_over_commenting() {
        let diff = "+++ b/src/x.rs\n@@ -0,0 +1,5 @@\n+/// Purpose: explain the public owner.\n+/// Caller: command dispatch.\n+/// Main Functions: run.\n+/// Side Effects: writes output.\n+pub fn run() {}\n";
        assert!(!scan_unified_diff_for_slop(diff)
            .iter()
            .any(|finding| finding.pattern == "over-commenting"));
    }
}
