use super::*;
use std::fs;

pub(crate) fn render_generated_message(
    message_kind: &str,
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flag_set = FlagSet::new(format!("git-workflow {message_kind}"));
    flag_set.bool_flag("from-diff", false);
    flag_set.string_flag("test-result", "");
    // `repo-root` scopes the git diff. `base-ref` selects the range the diff is
    // computed against (committed commits ahead of the base); when a base ref is
    // given, staging state is irrelevant. `format` selects the output rendering
    // (commit-message/pr-body default to markdown when unset).
    flag_set.string_flag("repo-root", "");
    flag_set.string_flag("base-ref", "");
    flag_set.string_flag("format", "");
    if let Err(parse_error) = flag_set.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 1;
    }
    let repo_root_value = flag_set.string_value("repo-root");
    let repo_root: Option<std::path::PathBuf> = if repo_root_value.trim().is_empty() {
        None
    } else {
        Some(std::path::PathBuf::from(repo_root_value))
    };
    let from_diff = flag_set.bool_value("from-diff");
    let base_ref = flag_set.string_value("base-ref").trim().to_string();
    let format = {
        let value = flag_set.string_value("format").trim().to_string();
        if value.is_empty() {
            "markdown".to_string()
        } else {
            value
        }
    };
    if format != "markdown" && format != "json" && format != "compact" {
        let _ = writeln!(
            standard_error,
            "git-workflow {message_kind}: unknown --format '{format}' (expected json, markdown, or compact)"
        );
        return 1;
    }
    let changed_files = if from_diff {
        if !base_ref.is_empty() {
            changed_files_against_base(repo_root.as_deref(), &base_ref).unwrap_or_default()
        } else {
            staged_files(repo_root.as_deref()).unwrap_or_default()
        }
    } else {
        Vec::new()
    };
    let diff_summary = if from_diff {
        if !base_ref.is_empty() {
            git_diff_stat_against_base(repo_root.as_deref(), &base_ref)
                .unwrap_or_else(|| "No diff summary available.".to_string())
        } else {
            git_diff_stat(repo_root.as_deref())
                .unwrap_or_else(|| "No diff summary available.".to_string())
        }
    } else {
        "No diff summary requested.".to_string()
    };
    if message_kind == "commit" {
        let subject = generate_commit_subject(from_diff, &changed_files);
        let body = commit_body_from_staged(&changed_files);
        if format == "json" {
            let payload = Value::Object(vec![
                (
                    "command".into(),
                    Value::String("git-workflow commit-message".into()),
                ),
                ("subject".into(), Value::String(subject)),
                ("body".into(), Value::String(body)),
                ("diff_summary".into(), Value::String(diff_summary)),
                (
                    "files".into(),
                    Value::Array(
                        changed_files
                            .iter()
                            .map(|file| Value::String(file.clone()))
                            .collect(),
                    ),
                ),
            ]);
            let _ = write_indented(standard_output, &payload);
        } else {
            let _ = writeln!(standard_output, "{subject}");
            let _ = writeln!(standard_output);
            let _ = writeln!(standard_output, "{body}");
            let _ = writeln!(standard_output);
            let _ = writeln!(standard_output, "{diff_summary}");
        }
    } else {
        let bullets = pr_summary_bullets(&changed_files);
        let test_result = flag_set.string_value("test-result");
        let test_plan = if test_result.trim().is_empty() {
            "Not provided".to_string()
        } else {
            test_result.to_string()
        };
        if format == "json" {
            let payload = Value::Object(vec![
                (
                    "command".into(),
                    Value::String("git-workflow pr-body".into()),
                ),
                (
                    "summary".into(),
                    Value::Array(bullets.iter().map(|b| Value::String(b.clone())).collect()),
                ),
                (
                    "test_plan".into(),
                    Value::Array(vec![Value::String(test_plan)]),
                ),
                (
                    "files".into(),
                    Value::Array(
                        changed_files
                            .iter()
                            .map(|file| Value::String(file.clone()))
                            .collect(),
                    ),
                ),
            ]);
            let _ = write_indented(standard_output, &payload);
        } else {
            let _ = writeln!(standard_output, "## Summary");
            for bullet in &bullets {
                let _ = writeln!(standard_output, "- {bullet}");
            }
            let _ = writeln!(standard_output);
            let _ = writeln!(standard_output, "## Test plan");
            let _ = writeln!(standard_output, "- {test_plan}");
        }
    }
    0
}

/// Paths changed between `base_ref` and HEAD, from `git diff --name-only`.
/// Unlike [`staged_files`], this is independent of the index/staging state.
pub(crate) fn changed_files_against_base(
    repo_root: Option<&std::path::Path>,
    base_ref: &str,
) -> Option<Vec<String>> {
    let result = run_command(
        "git",
        &[
            "diff".to_string(),
            "--name-only".to_string(),
            format!("{base_ref}..HEAD"),
        ],
        repo_root,
    )
    .ok()?;
    if result.code != 0 {
        return None;
    }
    let text = String::from_utf8_lossy(&result.stdout);
    Some(
        text.lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect(),
    )
}

/// `git diff --stat` of the committed range `base_ref..HEAD`, for message bodies.
pub(crate) fn git_diff_stat_against_base(
    repo_root: Option<&std::path::Path>,
    base_ref: &str,
) -> Option<String> {
    let result = run_command(
        "git",
        &[
            "diff".to_string(),
            "--stat".to_string(),
            format!("{base_ref}..HEAD"),
        ],
        repo_root,
    )
    .ok()?;
    if result.code != 0 {
        return None;
    }
    let text = String::from_utf8_lossy(&result.stdout).trim().to_string();
    Some(if text.is_empty() {
        "No diff against base.".to_string()
    } else {
        text
    })
}

pub(crate) fn staged_files(repo_root: Option<&std::path::Path>) -> Option<Vec<String>> {
    let result = run_command(
        "git",
        &[
            "diff".to_string(),
            "--cached".to_string(),
            "--name-only".to_string(),
        ],
        repo_root,
    )
    .ok()?;
    if result.code != 0 {
        return None;
    }
    let text = String::from_utf8_lossy(&result.stdout);
    let files: Vec<String> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect();
    Some(files)
}

/// Canonical commit categories (capitalized first letter). New generators emit
/// these; validators also accept the legacy lowercase form.
pub(crate) const COMMIT_CATEGORIES: [&str; 6] = ["Add", "Config", "Refactor", "Wip", "Fix", "Docs"];

/// Map a category token (any casing) to its canonical form, or None if unknown.
pub(crate) fn normalize_commit_category(raw: &str) -> Option<&'static str> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "add" => Some("Add"),
        "config" => Some("Config"),
        "refactor" => Some("Refactor"),
        "wip" => Some("Wip"),
        "fix" => Some("Fix"),
        "docs" => Some("Docs"),
        _ => None,
    }
}

/// Validate a commit subject against `Add : FEATURE : short information`.
///
/// The commit subject is COLON-separated (three parts), with spaces around
/// colons preferred. Distinct from branch names (`task/<task>`). Do not conflate.
///
/// - Category must be one of [`COMMIT_CATEGORIES`] (case-insensitive; legacy
///   lowercase still accepted so in-flight history keeps validating).
/// - FEATURE must be a non-empty uppercase component label (e.g. RGB, PROTOCOL).
/// - Short information must be non-empty.
pub(crate) fn validate_commit_subject(subject: &str) -> Result<(), String> {
    let trimmed = subject.trim();
    let parts: Vec<&str> = trimmed.splitn(3, ':').collect();
    if parts.len() < 3 {
        return Err(
            "expected three colon-separated parts: Add : FEATURE : short information".to_string(),
        );
    }
    let category = parts[0].trim();
    let feature = parts[1].trim();
    let information = parts[2].trim();
    if normalize_commit_category(category).is_none() {
        return Err(format!(
            "category '{category}' must be one of: {}",
            COMMIT_CATEGORIES.join(", ")
        ));
    }
    if feature.is_empty() {
        return Err("feature_category is required (e.g. RGB, LED, ARGB, SENSOR)".to_string());
    }
    if feature != feature.to_uppercase() {
        return Err(format!("feature_category '{feature}' must be uppercase"));
    }
    if information.is_empty() {
        return Err("short information is required after the feature category".to_string());
    }
    Ok(())
}

pub(crate) fn detect_category(paths: &[String]) -> &'static str {
    if paths.is_empty() {
        return "Wip";
    }
    let all_match = |predicate: fn(&str) -> bool| paths.iter().all(|path| predicate(path));

    if all_match(is_docs_path) {
        return "Docs";
    }
    if all_match(is_config_path) {
        return "Config";
    }
    "Wip"
}

pub(crate) fn is_docs_path(path: &str) -> bool {
    path.ends_with(".md") || path.starts_with("docs/")
}

pub(crate) fn is_ci_path(path: &str) -> bool {
    path.starts_with(".github/workflows/")
        || path.starts_with(".github/actions/")
        || path == ".gitlab-ci.yml"
}

pub(crate) fn is_config_path(path: &str) -> bool {
    is_ci_path(path)
        || path.ends_with(".toml")
        || path.ends_with(".yml")
        || path.ends_with(".yaml")
        || path.ends_with(".ini")
        || path.ends_with(".cfg")
        || path.ends_with(".conf")
        || path.ends_with(".json")
        || path.ends_with(".gitignore")
        || path.ends_with(".gitattributes")
}

pub(crate) fn is_test_path(path: &str) -> bool {
    path.contains("/tests/")
        || path.starts_with("tests/")
        || path.ends_with("_test.rs")
        || path.ends_with("_tests.rs")
        || path.contains("/test_")
        || path.contains("__tests__/")
}

pub(crate) fn is_source_path(path: &str) -> bool {
    !is_docs_path(path) && !is_ci_path(path) && !is_test_path(path)
}

pub(crate) fn derive_scope(paths: &[String]) -> Option<String> {
    if paths.is_empty() {
        return None;
    }
    let segments: Vec<Vec<&str>> = paths
        .iter()
        .map(|path| path.split('/').collect::<Vec<&str>>())
        .collect();

    let head: Vec<&str> = segments[0].clone();
    let mut prefix_len = head.len().saturating_sub(1);
    for path in &segments[1..] {
        let limit = std::cmp::min(prefix_len, path.len().saturating_sub(1));
        let mut shared = 0;
        while shared < limit && head[shared] == path[shared] {
            shared += 1;
        }
        prefix_len = shared;
        if prefix_len == 0 {
            break;
        }
    }
    if prefix_len == 0 {
        return None;
    }
    let is_generic = |segment: &str| {
        matches!(
            segment,
            "src" | "tests" | "test" | "lib" | "crates" | "packages"
        )
    };
    let mut idx = prefix_len.saturating_sub(1);
    while idx > 0 && is_generic(head[idx]) {
        idx -= 1;
    }
    let leaf = head[idx];
    let scope = leaf.trim_end_matches(".rs");
    if scope.is_empty() || is_generic(scope) {
        return None;
    }
    Some(scope.to_string())
}

pub(crate) fn generate_commit_subject(from_diff: bool, paths: &[String]) -> String {
    if !from_diff {
        return "Wip : GENERAL : update".to_string();
    }
    if paths.is_empty() {
        return "Wip : GENERAL : no staged changes".to_string();
    }
    let category = detect_category(paths);
    let summary = subject_summary(paths);
    let feature = derive_scope(paths)
        .map(|scope| scope.to_uppercase())
        .unwrap_or_else(|| "GENERAL".to_string());
    format!("{category} : {feature} : {summary}")
}

pub(crate) fn subject_summary(paths: &[String]) -> String {
    if paths.len() == 1 {
        let leaf = paths[0].rsplit('/').next().unwrap_or(&paths[0]);
        return format!("update {leaf}");
    }
    format!("update {} files", paths.len())
}

pub(crate) fn commit_body_from_staged(paths: &[String]) -> String {
    if paths.is_empty() {
        return "No staged changes.".to_string();
    }
    let mut lines = vec!["What Changed:".to_string()];
    for path in paths.iter().take(20) {
        lines.push(format!("- {path}"));
    }
    if paths.len() > 20 {
        lines.push(format!("- ... and {} more files", paths.len() - 20));
    }
    lines.join("\n")
}

pub(crate) fn pr_summary_bullets(paths: &[String]) -> Vec<String> {
    if paths.is_empty() {
        return vec!["No staged changes detected.".to_string()];
    }
    let mut bullets = Vec::new();
    let docs: Vec<&String> = paths.iter().filter(|p| is_docs_path(p)).collect();
    let ci: Vec<&String> = paths.iter().filter(|p| is_ci_path(p)).collect();
    let tests: Vec<&String> = paths.iter().filter(|p| is_test_path(p)).collect();
    let source: Vec<&String> = paths.iter().filter(|p| is_source_path(p)).collect();
    if !source.is_empty() {
        bullets.push(format!(
            "Source changes across {} file(s): {}",
            source.len(),
            preview_paths(&source, 3)
        ));
    }
    if !tests.is_empty() {
        bullets.push(format!(
            "Test changes across {} file(s): {}",
            tests.len(),
            preview_paths(&tests, 3)
        ));
    }
    if !docs.is_empty() {
        bullets.push(format!(
            "Docs changes across {} file(s): {}",
            docs.len(),
            preview_paths(&docs, 3)
        ));
    }
    if !ci.is_empty() {
        bullets.push(format!(
            "CI changes across {} file(s): {}",
            ci.len(),
            preview_paths(&ci, 3)
        ));
    }
    if bullets.is_empty() {
        bullets.push(format!("Updated {} file(s)", paths.len()));
    }
    bullets
}

pub(crate) fn preview_paths(paths: &[&String], limit: usize) -> String {
    let mut shown: Vec<String> = paths.iter().take(limit).map(|p| (*p).clone()).collect();
    if paths.len() > limit {
        shown.push(format!("(+{} more)", paths.len() - limit));
    }
    shown.join(", ")
}

pub(crate) fn lint_message(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    if arguments.len() != 1 {
        let _ = writeln!(
            standard_error,
            "Usage: keel git-workflow lint-message <file>"
        );
        return 1;
    }
    match fs::read_to_string(&arguments[0]) {
        Ok(text) => {
            let first_line = text.lines().next().unwrap_or("");
            if first_line.len() > 72 {
                let _ = writeln!(standard_error, "message subject exceeds 72 characters");
                return 1;
            }
            if let Err(reason) = validate_commit_subject(first_line) {
                let _ = writeln!(
                    standard_error,
                    "subject does not match <category>: <FEATURE>: <short information>: {reason}"
                );
                let _ = writeln!(
                    standard_error,
                    "  categories (lowercase): {}",
                    COMMIT_CATEGORIES.join(", ")
                );
                let _ = writeln!(
                    standard_error,
                    "  example: wip: RGB: Build light effect mode (multi color)"
                );
                return 1;
            }
            let _ = writeln!(standard_output, "message lint passed");
            0
        }
        Err(error) => {
            let _ = writeln!(standard_error, "read {}: {error}", arguments[0]);
            1
        }
    }
}

pub(crate) fn git_diff_stat(repo_root: Option<&std::path::Path>) -> Option<String> {
    let result = run_command(
        "git",
        &["diff".to_string(), "--stat".to_string()],
        repo_root,
    )
    .ok()?;
    if result.code != 0 {
        return None;
    }
    let text = String::from_utf8_lossy(&result.stdout).trim().to_string();
    Some(if text.is_empty() {
        "No local diff.".to_string()
    } else {
        text
    })
}
