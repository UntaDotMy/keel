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

/// AI-slop vocabulary: the strongest tells that text was model-generated. These
/// words/phrases rarely appear in senior-engineer prose but dominate LLM output.
/// Matched as whole words, case-insensitive. Add terms conservatively: a term
/// belongs here only if its presence in technical prose is almost always AI slop.
fn slop_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(
            r"(?i)\b(delve|delves|delving|leverage|leveraging|streamline|streamlining\
            |embark|embarking|navigate|navigating|realm|tapestry|intricate|nuanced\
            |it's worth noting|worth noting|it is worth noting\
            |in today's|in the world of|in the realm of\
            |ever-evolving|ever-evolve|game-changer|game-changing\
            |unlock|unlocking|unleash|unleashing\
            |cutting-edge|bleeding-edge|state-of-the-art\
            |holistic|holistically|synergy|synergies\
            |paving the way|paves the way\
            |when it comes to|at the end of the day\
            |landscape|paradigm|ecosystem)\b",
        )
        .expect("slop pattern compiles")
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

/// Summary-style / restating comments: AI-vibecoding tells that restate what the
/// code already says instead of a contract (why, @param, invariant, safety).
/// High severity so pre-commit and PostToolUse treat them as blockers.
fn summary_comment_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        // (?x) ignores whitespace/comments so the alternation stays readable.
        Regex::new(
            r"(?ix)^(
                this\s+(function|method|class|file|code|block|module|struct|helper|utility|section|component|type|enum|trait)\b
                |this\s+(ensures|allows|enables|makes\s+sure|is\s+used|handles|checks|validates|creates|initializes|sets\s+up)\b
                |handles\s+the\b
                |parses\s+the\b
                |returns\s+the\b
                |gets\s+the\b
                |sets\s+the\b
                |used\s+to\s+\w+
                |responsible\s+for\s+\w+
                |the\s+following\s+\w+
                |here\s+we\s+\w+
                |we\s+(then|now|just|simply)\s+\w+
                |simple\s+helper\b
                |helper\s+function\b
                |utility\s+function\b
                |main\s+entry\s+point\b
                |entry\s+point\s+for\b
                |wrapper\s+around\b
                |thin\s+wrapper\b
            )",
        )
        .expect("summary_comment pattern compiles")
    })
}

/// Structured contract markers that exempt a doc/impl comment from summary lint.
fn has_contract_marker(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("@param")
        || lower.contains("@returns")
        || lower.contains("@return")
        || lower.contains("@throws")
        || lower.contains("@remarks")
        || lower.contains("# errors")
        || lower.contains("# panics")
        || lower.contains("# safety")
        || lower.contains("why:")
        || lower.contains("purpose:")
        || lower.contains("caller:")
        || lower.contains("invariant")
        || lower.contains("safety:")
        || lower.contains("must not")
        || lower.contains("must be")
        || lower.contains("otherwise")
        || lower.contains("avoids ")
        || lower.contains("required by")
        // The comment points at a non-obvious constraint, edge case, or rationale
        // the code cannot express. These words signal "why", not "what".
        || lower.contains("because")
        || lower.contains("because ")
        || lower.contains("so that")
        || lower.contains("instead of")
        || lower.contains("rather than")
        || lower.contains("workaround")
        || lower.contains("regression")
        || lower.contains("edge case")
        || lower.contains("fallback")
        || lower.contains("default")
        || lower.contains("not obvious")
        || lower.contains("non-obvious")
}

/// Words that carry no information in a comment (articles, filler, code keywords
/// that just narrate). Used to strip a comment down to its information content.
fn is_stop_word(word: &str) -> bool {
    matches!(
        word,
        "the"
            | "a"
            | "an"
            | "this"
            | "that"
            | "these"
            | "those"
            | "we"
            | "it"
            | "to"
            | "of"
            | "and"
            | "or"
            | "is"
            | "are"
            | "was"
            | "be"
            | "been"
            | "will"
            | "would"
            | "here"
            | "now"
            | "then"
            | "just"
            | "simply"
            | "also"
            | "so"
            | "for"
            | "with"
            | "in"
            | "on"
            | "at"
            | "by"
            | "as"
            | "from"
            | "into"
            | "return"
            | "returns"
            | "returning"
            | "set"
            | "sets"
            | "get"
            | "gets"
            | "create"
            | "creates"
            | "make"
            | "makes"
            | "call"
            | "calls"
            | "use"
            | "uses"
            | "using"
            | "used"
            | "new"
            | "value"
            | "result"
            | "data"
            | "variable"
            | "function"
            | "method"
            | "loop"
            | "iterate"
            | "iterates"
            | "through"
            | "each"
            | "every"
            | "all"
            | "if"
            | "else"
            | "when"
            | "check"
            | "checks"
            | "handle"
            | "handles"
            | "process"
            | "processes"
            | "add"
            | "adds"
            | "added"
    )
}

/// Split an identifier or prose into lowercase word tokens (snake_case, camelCase,
/// and spaces all split). Used to compare a comment against the code it describes.
fn word_tokens(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        if ch.is_alphanumeric() {
            // Split camelCase boundaries (lower->upper) into separate tokens.
            if ch.is_uppercase() && !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
            current.push(ch.to_ascii_lowercase());
        } else if !current.is_empty() {
            tokens.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// A comment restates the code when every informative word in it already appears
/// as an identifier/keyword on the very next code line. Such a comment adds zero
/// information (`// increment the counter` over `counter += 1`). Returns true only
/// when the comment has at least one informative word AND all of them are covered
/// by the next code line, AND the comment carries no contract/why marker.
/// A comment word that just narrates the OPERATION on the next line (not a
/// literal identifier) still restates the code: `// increment the counter` over
/// `counter += 1`, `// loop through the items` over `for item in items`. Maps an
/// action word to the operator/keyword it narrates.
fn narrates_operation(word: &str, code_line: &str) -> bool {
    let code = code_line;
    let has = |needle: &str| code.contains(needle);
    match word {
        // arithmetic / assignment verbs
        "increment" | "increments" | "add" | "adds" | "adding" | "sum" | "sums" => {
            has("+") || has("push") || has("append") || has("add")
        }
        "decrement" | "decrements" | "subtract" | "subtracts" => has("-"),
        "multiply" | "multiplies" => has("*"),
        "divide" | "divides" => has("/"),
        "assign" | "assigns" | "set" | "sets" | "store" | "stores" => has("="),
        // iteration verbs
        "loop" | "loops" | "iterate" | "iterates" | "iterating" | "traverse" | "traverses" => {
            has("for ") || has("while ") || has(".iter()") || has(".map(") || has("loop")
        }
        // condition verbs
        "check" | "checks" | "checking" | "test" | "tests" | "validate" | "validates" => {
            has("if ") || has("assert") || has("match ") || has("==")
        }
        // return verbs
        "return" | "returns" | "returning" | "yield" | "yields" => {
            has("return") || has("->") || has("Ok(") || has("Some(")
        }
        _ => false,
    }
}

fn comment_restates_code(comment: &str, next_code_line: &str) -> bool {
    if has_contract_marker(comment) {
        return false;
    }
    let informative: Vec<String> = word_tokens(comment)
        .into_iter()
        .filter(|w| w.len() >= 3 && !is_stop_word(w))
        .collect();
    if informative.is_empty() {
        return false; // too short to judge; length/wording rules handle it
    }
    let code_tokens: std::collections::HashSet<String> = word_tokens(next_code_line)
        .into_iter()
        .filter(|w| w.len() >= 2)
        .collect();
    if code_tokens.is_empty() {
        return false;
    }
    // Every informative word in the comment is already named by the code, either
    // as a literal identifier or as the operation it narrates.
    informative
        .iter()
        .all(|w| code_tokens.contains(w) || narrates_operation(w, next_code_line))
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
    // Summary-style restatement (vibecoding): "This function does X" with no
    // contract marker. Prefer `@param` / `# Errors` / `// why:` or delete.
    if !has_contract_marker(text) && summary_comment_pattern().is_match(text.trim()) {
        findings.push(CommentFinding {
            line,
            id: "comment-summary".into(),
            severity: "high".into(),
            message: "Summary-style comment restates the code; use a contract (@param/# Errors/why:) or delete.".into(),
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
        // The next non-comment line after the block is the code the comment sits on.
        let next_code = lines[index..]
            .iter()
            .map(|l| l.trim())
            .find(|l| !l.is_empty() && !is_comment_line(l, syntax))
            .unwrap_or("");
        if !is_doc && comment_restates_code(&block_text, next_code) {
            findings.push(CommentFinding {
                line: report_line,
                id: "comment-restates-code".into(),
                severity: "high".into(),
                message: "Comment only restates the next line's identifiers; delete it or explain WHY/constraint instead.".into(),
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

/// One prose-wording finding located at a 1-based line in a markdown/doc file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProseFinding {
    pub line: usize,
    pub id: String,
    pub severity: String,
    pub message: String,
}

/// Lint prose (markdown, docs, any generated text) for AI-slop and unprofessional
/// wording. Unlike `lint_code_comments` (which only lints comment markers), this
/// lints the body text itself: every non-empty line is checked against the em-dash,
/// dangling-dash, first-person, chatty, hype, and AI-slop-vocabulary rules.
///
/// `line_base` offsets the reported line numbers (use 1 for whole-file scans; the
/// diff-scoped caller passes the hunk's starting line so findings point at real
/// post-merge lines).
pub fn lint_prose(source: &str, line_base: usize) -> Vec<ProseFinding> {
    let mut findings = Vec::new();
    for (offset, raw) in source.lines().enumerate() {
        let text = raw.trim();
        if text.is_empty() {
            continue;
        }
        let report_line = line_base + offset;
        if text.contains('\u{2014}') || text.contains('\u{2013}') {
            findings.push(ProseFinding {
                line: report_line,
                id: "prose-em-dash".into(),
                severity: "high".into(),
                message: "Replace em/en dash with a period or comma; it reads as AI-generated."
                    .into(),
            });
        }
        if dangling_dash_pattern().is_match(text) {
            findings.push(ProseFinding {
                line: report_line,
                id: "prose-dangling-dash".into(),
                severity: "medium".into(),
                message: "Avoid ' - ' asides; split into a short sentence.".into(),
            });
        }
        if first_person_pattern().is_match(text) {
            findings.push(ProseFinding {
                line: report_line,
                id: "prose-first-person".into(),
                severity: "medium".into(),
                message: "Drop first-person wording; state the facts.".into(),
            });
        }
        if chatty_pattern().is_match(text) {
            findings.push(ProseFinding {
                line: report_line,
                id: "prose-chatty".into(),
                severity: "medium".into(),
                message: "Drop chatty filler; keep the prose factual.".into(),
            });
        }
        if hype_pattern().is_match(text) {
            findings.push(ProseFinding {
                line: report_line,
                id: "prose-hype".into(),
                severity: "medium".into(),
                message: "Drop hype words; describe the thing plainly.".into(),
            });
        }
        if slop_pattern().is_match(text) {
            findings.push(ProseFinding {
                line: report_line,
                id: "prose-ai-slop".into(),
                severity: "high".into(),
                message: "Drop AI-slop vocabulary; rewrite in plain technical prose.".into(),
            });
        }
    }
    findings
}

/// True when any prose finding is blocking (high severity).
pub fn has_blocking_prose_findings(findings: &[ProseFinding]) -> bool {
    findings.iter().any(|finding| finding.severity == "high")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn has_id(findings: &[CommentFinding], id: &str) -> bool {
        findings.iter().any(|f| f.id == id)
    }

    #[test]
    fn restating_comment_is_flagged() {
        // The comment names only identifiers already on the next line: zero info.
        let src = "// increment the counter\nlet counter = counter + 1;\n";
        let findings = lint_code_comments(src, CommentSyntax::SlashSlash, 0);
        assert!(
            has_id(&findings, "comment-restates-code"),
            "restatement must be flagged: {findings:?}"
        );
    }

    #[test]
    fn restating_comment_camelcase_is_flagged() {
        let src = "// gets the user name\nfn getUserName() -> String {\n";
        let findings = lint_code_comments(src, CommentSyntax::SlashSlash, 0);
        assert!(has_id(&findings, "comment-restates-code"));
    }

    #[test]
    fn why_comment_is_not_flagged_as_restatement() {
        // A constraint/rationale the code cannot say is never a restatement.
        let src = "// retry because the registry closes idle sockets after 30s\nlet conn = reconnect(conn);\n";
        let findings = lint_code_comments(src, CommentSyntax::SlashSlash, 0);
        assert!(
            !has_id(&findings, "comment-restates-code"),
            "why-comment must not be flagged: {findings:?}"
        );
    }

    #[test]
    fn comment_with_new_information_is_not_flagged() {
        // Mentions a concept (timeout, registry) absent from the next line's code.
        let src = "// the remote registry enforces a 30s idle timeout\nlet conn = connect(host);\n";
        let findings = lint_code_comments(src, CommentSyntax::SlashSlash, 0);
        assert!(!has_id(&findings, "comment-restates-code"));
    }

    #[test]
    fn contract_marked_comment_is_not_flagged() {
        let src = "// why: avoids a stale read when the cache is cold\nlet v = cache.get(k);\n";
        let findings = lint_code_comments(src, CommentSyntax::SlashSlash, 0);
        assert!(!has_id(&findings, "comment-restates-code"));
    }

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
    fn summary_style_comment_is_blocking() {
        let source = "// This function parses the config and returns a map\nfoo();";
        let findings = lint_code_comments(source, CommentSyntax::SlashSlash, 1);
        assert!(
            ids(&findings).contains("comment-summary"),
            "summary restatement not caught: {findings:?}"
        );
        assert!(has_blocking_comment_findings(&findings));
    }

    #[test]
    fn contract_comment_is_not_summary() {
        let source = "/// # Errors\n/// Returns Io when the path is missing.\nfn load() {}";
        let findings = lint_code_comments(source, CommentSyntax::SlashSlash, 1);
        assert!(
            !ids(&findings).contains("comment-summary"),
            "contract doc should pass: {findings:?}"
        );
    }

    #[test]
    fn why_comment_is_not_summary() {
        let source = "// why: kernel requires page-aligned buffers on this path\nfoo();";
        let findings = lint_code_comments(source, CommentSyntax::SlashSlash, 1);
        assert!(
            !ids(&findings).contains("comment-summary"),
            "why-comment should pass: {findings:?}"
        );
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

    #[test]
    fn prose_lint_catches_ai_slop_in_markdown_body() {
        let md = "# Guide\n\nLet's delve into how we leverage this robust ecosystem.\n";
        let findings = lint_prose(md, 1);
        let ids: Vec<&str> = findings.iter().map(|f| f.id.as_str()).collect();
        assert!(
            ids.contains(&"prose-ai-slop"),
            "slop not caught: {findings:?}"
        );
        assert!(ids.contains(&"prose-hype"), "hype not caught: {findings:?}");
        assert!(
            ids.contains(&"prose-first-person"),
            "first-person not caught: {findings:?}"
        );
        assert!(
            has_blocking_prose_findings(&findings),
            "slop must be blocking"
        );
    }

    #[test]
    fn prose_lint_catches_em_dash_in_body_text() {
        let md = "This is a feature — it does the thing.\n";
        let findings = lint_prose(md, 1);
        assert!(
            findings.iter().any(|f| f.id == "prose-em-dash"),
            "em-dash not caught: {findings:?}"
        );
        assert!(has_blocking_prose_findings(&findings));
    }

    #[test]
    fn prose_lint_leaves_clean_technical_prose_untouched() {
        let md = "# API\n\nThe endpoint returns JSON. Set the timeout to 30 seconds.\n\n## Errors\n\nReturns 404 when the resource is absent.\n";
        let findings = lint_prose(md, 1);
        assert!(findings.is_empty(), "clean prose flagged: {findings:?}");
    }

    #[test]
    fn slop_pattern_catches_common_ai_tells() {
        for word in [
            "delve",
            "leverage",
            "streamline",
            "embark",
            "navigate",
            "realm",
            "tapestry",
            "intricate",
            "paradigm",
            "landscape",
            "ecosystem",
        ] {
            let findings = lint_prose(&format!("We {word} the system."), 1);
            assert!(
                findings.iter().any(|f| f.id == "prose-ai-slop"),
                "slop word {word:?} not caught"
            );
        }
    }
}
