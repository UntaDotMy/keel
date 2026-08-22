use super::*;
use crate::runtime::{resolve_repository_root, write_text};
use std::fs;

pub(crate) fn run_review_hosted_command(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    if arguments.is_empty() || is_help_argument(&arguments[0]) {
        let _ = writeln!(
            standard_output,
            "Usage: keel review hosted [check|comment] [flags]"
        );
        return if arguments.is_empty() { 1 } else { 0 };
    }
    let hosted_kind = arguments[0].as_str();
    if hosted_kind != "check" && hosted_kind != "comment" {
        let _ = writeln!(
            standard_error,
            "Unknown review hosted command: {hosted_kind}"
        );
        return 1;
    }
    let mut flag_set = review_flag_set(&format!("review hosted {hosted_kind}"));
    flag_set.string_flag("provider", "generic");
    flag_set.string_flag("write-payload-file", "");
    flag_set.string_flag("write-body-file", "");
    if let Err(parse_error) = flag_set.parse(&arguments[1..]) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 1;
    }
    let body = hosted_body();
    if !flag_set.string_value("write-body-file").trim().is_empty() {
        if let Err(error) = write_text(Path::new(flag_set.string_value("write-body-file")), &body) {
            let _ = writeln!(standard_error, "{error}");
            return 1;
        }
    }
    // Hosted review is not wired to a real provider API: emit an honest
    let payload = Value::Object(vec![
        (
            "provider".into(),
            Value::String(flag_set.string_value("provider").to_string()),
        ),
        ("gate".into(), Value::String("skipped".into())),
        (
            "summary".into(),
            Value::String(
                "hosted review is not configured \
                 -- run `keel review pre-pr` for the local verdict."
                    .into(),
            ),
        ),
        ("body".into(), Value::String(body.clone())),
        ("conclusion".into(), Value::String("action_required".into())),
        (
            "title".into(),
            Value::String("the harness Native Review Report".into()),
        ),
    ]);
    if !flag_set
        .string_value("write-payload-file")
        .trim()
        .is_empty()
    {
        let mut buffer = Vec::new();
        if write_indented(&mut buffer, &payload).is_err() {
            let _ = writeln!(standard_error, "Unable to render hosted review payload");
            return 1;
        }
        if let Err(error) = fs::write(flag_set.string_value("write-payload-file"), buffer) {
            let _ = writeln!(
                standard_error,
                "write {}: {error}",
                flag_set.string_value("write-payload-file")
            );
            return 1;
        }
    }
    match flag_set.string_value("format") {
        "json" => {
            let _ = write_indented(standard_output, &payload);
        }
        "compact" => {
            let _ = writeln!(
                standard_output,
                "gate=skipped blocking=0 warnings=0 findings=0 note=hosted-review-not-configured"
            );
        }
        _ => {
            let _ = write!(standard_output, "{body}");
        }
    }
    0
}
/// Run `keel review comments`: lint added-comment style and report findings.
/// `--all` ignores the diff and scans the whole tracked tree (for cleanup work).
pub(crate) fn run_review_comments_command(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flag_set = FlagSet::new("review comments");
    flag_set.string_flag("repo-root", "");
    flag_set.string_flag("base-ref", "origin/main");
    flag_set.string_flag("format", "compact");
    flag_set.bool_flag("all", false);
    if let Err(parse_error) = flag_set.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 1;
    }
    let repository_root = match resolve_repository_root(flag_set.string_value("repo-root")) {
        Ok(path) => path,
        Err(error) => {
            let _ = writeln!(standard_error, "{error}");
            return 1;
        }
    };
    let mut findings = if flag_set.bool_value("all") {
        crate::comment_lint::lint_tracked_tree(&repository_root)
    } else {
        crate::comment_lint::lint_added_comments(
            &repository_root,
            flag_set.string_value("base-ref").trim(),
        )
    };
    // Also lint added prose (markdown/doc body text) for AI-slop, unless --all
    // (whole-tree prose scan is out of scope for the comments surface).
    if !flag_set.bool_value("all") {
        findings.extend(crate::comment_lint::lint_added_prose(
            &repository_root,
            flag_set.string_value("base-ref").trim(),
        ));
    }
    let blocking = crate::comment_lint::has_blocking(&findings)
        || crate::comment_lint::has_blocking_prose(&findings);
    if flag_set.string_value("format") == "json" {
        let items: Vec<Value> = findings
            .iter()
            .map(|f| {
                Value::Object(vec![
                    ("file".into(), Value::String(f.file.clone())),
                    ("line".into(), Value::Number(f.line.to_string())),
                    ("id".into(), Value::String(f.id.clone())),
                    ("severity".into(), Value::String(f.severity.clone())),
                    ("message".into(), Value::String(f.message.clone())),
                ])
            })
            .collect();
        let payload = Value::Object(vec![
            ("command".into(), Value::String("review comments".into())),
            ("passed".into(), Value::Bool(!blocking)),
            (
                "findingCount".into(),
                Value::Number(findings.len().to_string()),
            ),
            ("findings".into(), Value::Array(items)),
        ]);
        let _ = write_indented(standard_output, &payload);
        let _ = writeln!(standard_output);
    } else if findings.is_empty() {
        let _ = writeln!(standard_output, "comment style: no added-comment issues");
    } else {
        let _ = writeln!(
            standard_output,
            "{}",
            crate::comment_lint::format_findings(&findings)
        );
        let _ = writeln!(
            standard_output,
            "{} issue(s); {} blocking",
            findings.len(),
            if blocking { "has" } else { "no" }
        );
    }
    if blocking {
        1
    } else {
        0
    }
}

pub(crate) fn run_review_policy_command(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    if arguments.first().map(String::as_str) == Some("show") {
        let mut flag_set = FlagSet::new("review policy show");
        flag_set.string_flag("repo-root", "");
        flag_set.string_flag("format", "markdown");
        if let Err(parse_error) = flag_set.parse(&arguments[1..]) {
            let _ = writeln!(standard_error, "{}", parse_error.message);
            return 1;
        }
        let format = flag_set.string_value("format");
        if format == "compact" {
            let _ = writeln!(
                standard_output,
                "native_rules=rust,python,js,go,cpp language_gates=auto go_fallback=false"
            );
        } else if format == "json" {
            let payload = Value::Object(vec![
                ("runtime".into(), Value::String("rust-native".into())),
                (
                    "native_rules".into(),
                    Value::Array(
                        ["rust", "python", "js", "go", "cpp"]
                            .iter()
                            .map(|rule| Value::String((*rule).into()))
                            .collect(),
                    ),
                ),
                ("language_gates".into(), Value::String("auto".into())),
                (
                    "language_gates_detail".into(),
                    Value::String("auto-detect root markers (Cargo.toml, pyproject.toml/setup.py, package.json, go.mod, CMakeLists.txt / root C/C++ sources)".into()),
                ),
                (
                    "pre_commit".into(),
                    Value::String("rust fmt+clippy; python black+ruff; js prettier+eslint; go gofmt+vet; c/c++ clang-format (tools missing = non-blocking)".into()),
                ),
                (
                    "pre_pr".into(),
                    Value::String("above plus unit tests (cargo test / pytest / npm test / go test) and typecheck (mypy / tsc when present); pytest exit 5 (no tests) is non-blocking".into()),
                ),
                (
                    "python_checks".into(),
                    Value::Array(
                        ["black", "ruff", "mypy", "pytest", "circular_imports", "import_safety"]
                            .iter()
                            .map(|check| Value::String((*check).into()))
                            .collect(),
                    ),
                ),
                (
                    "js_checks".into(),
                    Value::Array(
                        ["prettier", "eslint", "tsc", "npm test"]
                            .iter()
                            .map(|check| Value::String((*check).into()))
                            .collect(),
                    ),
                ),
                (
                    "go_checks".into(),
                    Value::String("gofmt, go vet, go test".into()),
                ),
                ("cpp_checks".into(), Value::String("clang-format".into())),
                ("go_fallback".into(), Value::Bool(false)),
            ]);
            let _ = write_indented(standard_output, &payload);
        } else {
            let _ = writeln!(standard_output, "# Native Review Policy");
            let _ = writeln!(standard_output, "- runtime: rust-native");
            let _ = writeln!(
                standard_output,
                "- language_gates: auto-detect root markers (Cargo.toml, pyproject.toml/setup.py, package.json, go.mod, CMakeLists.txt / root C/C++ sources)"
            );
            let _ = writeln!(
                standard_output,
                "- pre-commit: rust fmt+clippy; python black+ruff; js prettier+eslint; go gofmt+vet; c/c++ clang-format (tools missing = non-blocking)"
            );
            let _ = writeln!(
                standard_output,
                "- pre-pr: above plus unit tests (cargo test / pytest / npm test / go test) and typecheck (mypy / tsc when present); pytest exit 5 (no tests) is non-blocking"
            );
            let _ = writeln!(
                standard_output,
                "- python_checks: black, ruff, mypy, pytest, circular_imports, import_safety"
            );
            let _ = writeln!(
                standard_output,
                "- js_checks: prettier, eslint, tsc, npm test"
            );
            let _ = writeln!(standard_output, "- go_checks: gofmt, go vet, go test");
            let _ = writeln!(standard_output, "- cpp_checks: clang-format");
            let _ = writeln!(standard_output, "- go_fallback: false");
        }
        return 0;
    }
    let _ = writeln!(standard_error, "Usage: keel review policy show [flags]");
    1
}
