//! Purpose: Detect callerless learning-path functions so the sweep runs in CI and in `keel doctor`.
//! Caller: manager::doctor (repo section) and tests/no_callerless_learning_path_test.rs.
//! Dependencies: std::fs, std::io, std::path.
//! Main Functions: callerless_learning_path_functions, report_learning_path_callerless.
//! Side Effects: Reads repository sources; writes doctor check lines.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// One module whose presence marks a keel source checkout, shared by the
/// module list and the doctor guard.
const KEEL_CHECKOUT_MARKER: &str = "rust/crates/keel/src/utility/decision.rs";

/// The learning-path modules the sweep owns: routing, calibration, decisions,
/// memory families, and the learning cycle.
pub const LEARNING_PATH_MODULES: &[&str] = &[
    "rust/crates/keel/src/utility/calibration.rs",
    "rust/crates/keel/src/utility/skill_match.rs",
    "rust/crates/keel/src/utility/skill_usage.rs",
    KEEL_CHECKOUT_MARKER,
    "rust/crates/keel/src/utility/memory_families.rs",
    "rust/crates/keel/src/runner/learning.rs",
];

/// Functions kept without a production caller on purpose, each with its reason
/// so the exemption is a recorded decision, not an omission.
pub const CALLERLESS_ALLOWLIST: &[(&str, &str)] = &[
    (
        "clear_skill_routing_cache",
        "test hook: resets the routing cache so a test can prove fingerprint invalidation",
    ),
    (
        "record_skill_semantic_learning",
        "online semantic-SGD entry point, staged for the MoE decision-model block to wire or delete",
    ),
    (
        "predict_semantic_skill_confidence",
        "online semantic-SGD entry point, staged for the MoE decision-model block to wire or delete",
    ),
];

/// One callerless function found by the sweep.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallerlessFunction {
    pub file: String,
    pub name: String,
}

/// Every learning-path `pub fn` with no reference outside its own definition in
/// non-test source. A reference counts only outside a trailing `#[cfg(test)]
/// mod` block, so a unit test alone cannot make a function look used. The scan
/// matches identifiers rather than the call graph: it is a screen, and a
/// doc-comment mention of the name also counts as a reference.
pub fn callerless_learning_path_functions(repo_root: &Path) -> Vec<CallerlessFunction> {
    let sources = production_sources(repo_root);
    let mut findings = Vec::new();
    for module in LEARNING_PATH_MODULES {
        let Ok(text) = fs::read_to_string(repo_root.join(module)) else {
            continue;
        };
        for name in public_function_names(&text) {
            if CALLERLESS_ALLOWLIST
                .iter()
                .any(|(exempt, _)| *exempt == name.as_str())
            {
                continue;
            }
            let referenced = sources.iter().any(|(path, body)| {
                body.lines().any(|line| {
                    !(path == module && is_definition_line(line, &name))
                        && count_word_occurrences(line, &name) > 0
                })
            });
            if !referenced {
                findings.push(CallerlessFunction {
                    file: (*module).to_string(),
                    name,
                });
            }
        }
    }
    findings
}

/// Doctor line: run the sweep when `repository_root` holds the keel sources,
/// and say nothing anywhere else. One line per finding so doctor stays
/// greppable.
pub(crate) fn report_learning_path_callerless(
    repository_root: &Path,
    standard_output: &mut dyn Write,
) {
    if !repository_root.join(KEEL_CHECKOUT_MARKER).is_file() {
        return;
    }
    let findings = callerless_learning_path_functions(repository_root);
    if findings.is_empty() {
        let _ = writeln!(
            standard_output,
            "[ok] learning path: every function has a caller"
        );
        return;
    }
    for finding in findings {
        let _ = writeln!(
            standard_output,
            "[warn] learning path: {}::{} has no caller",
            finding.file, finding.name
        );
    }
}

/// Every rust source under `rust/crates` with its `#[cfg(test)] mod` blocks
/// removed, so the sweep sees production code exactly.
fn production_sources(repo_root: &Path) -> Vec<(String, String)> {
    let mut files: Vec<PathBuf> = Vec::new();
    collect_rust_files(&repo_root.join("rust").join("crates"), &mut files);
    let mut sources = Vec::new();
    for path in files {
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        let key = path
            .strip_prefix(repo_root)
            .map(|relative| relative.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| path.to_string_lossy().replace('\\', "/"));
        sources.push((key, production_body(&text)));
    }
    sources
}

fn collect_rust_files(directory: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rust_files(&path, files);
        } else if path.extension().and_then(|extension| extension.to_str()) == Some("rs") {
            files.push(path);
        }
    }
}

/// A file's text without its `#[cfg(test)] mod` blocks.
fn production_body(text: &str) -> String {
    let ranges = test_module_ranges(text);
    let mut body = String::new();
    for (index, line) in text.lines().enumerate() {
        if ranges
            .iter()
            .any(|(start, end)| (*start..=*end).contains(&index))
        {
            continue;
        }
        body.push_str(line);
        body.push('\n');
    }
    body
}

/// Line ranges of the `#[cfg(test)] mod` blocks in one file. A module counts
/// only when the attribute is followed by a `mod` declaration, so a single
/// attribute-gated item keeps the rest of the file production code. The block
/// ends at the module's top-level closing brace, which rustfmt keeps at column
/// zero for a file-level module.
fn test_module_ranges(text: &str) -> Vec<(usize, usize)> {
    let lines: Vec<&str> = text.lines().collect();
    let mut ranges = Vec::new();
    let mut index = 0usize;
    while index < lines.len() {
        if lines[index].trim() != "#[cfg(test)]" {
            index += 1;
            continue;
        }
        let mut cursor = index + 1;
        while cursor < lines.len() && lines[cursor].trim().is_empty() {
            cursor += 1;
        }
        if !lines
            .get(cursor)
            .is_some_and(|line| line.trim_start().starts_with("mod "))
        {
            index += 1;
            continue;
        }
        let mut end = cursor;
        for (offset, line) in lines.iter().enumerate().skip(cursor) {
            if line.starts_with('}') {
                end = offset;
                break;
            }
        }
        ranges.push((index, end));
        index = end + 1;
    }
    ranges
}

/// `pub fn`/`pub(crate) fn` names declared in one source text, in file order.
fn public_function_names(text: &str) -> Vec<String> {
    let mut names = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        let rest = if let Some(rest) = trimmed.strip_prefix("pub fn ") {
            rest
        } else if let Some(rest) = trimmed.strip_prefix("pub(crate) fn ") {
            rest
        } else {
            continue;
        };
        let name: String = rest
            .chars()
            .take_while(|character| character.is_ascii_alphanumeric() || *character == '_')
            .collect();
        if !name.is_empty() {
            names.push(name);
        }
    }
    names
}

fn is_definition_line(line: &str, name: &str) -> bool {
    line.contains(&format!("fn {name}(")) || line.contains(&format!("fn {name}<"))
}

fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// Occurrences of `word` with identifier boundaries, so `blend` never counts
/// inside `hierarchical_blend`.
fn count_word_occurrences(text: &str, word: &str) -> usize {
    let bytes = text.as_bytes();
    let mut count = 0usize;
    let mut start = 0usize;
    while let Some(offset) = text[start..].find(word) {
        let index = start + offset;
        let before_ok = index == 0 || !is_word_byte(bytes[index - 1]);
        let after = index + word.len();
        let after_ok = after >= bytes.len() || !is_word_byte(bytes[after]);
        if before_ok && after_ok {
            count += 1;
        }
        start = index + word.len();
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_fixture(root: &Path, relative: &str, text: &str) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().expect("fixture parent")).expect("create fixture dir");
        fs::write(path, text).expect("write fixture");
    }

    fn fixture_root(label: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("keel-callerless-{label}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        root
    }

    fn drop_fixture(root: &Path) {
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn flags_only_the_function_without_a_caller() {
        let root = fixture_root("sweep");
        write_fixture(
            &root,
            LEARNING_PATH_MODULES[0],
            "pub fn orphan() {}\npub fn called() {}\n",
        );
        write_fixture(
            &root,
            LEARNING_PATH_MODULES[5],
            "fn drive() {\n    called();\n}\n",
        );
        assert_eq!(
            callerless_learning_path_functions(&root),
            vec![CallerlessFunction {
                file: LEARNING_PATH_MODULES[0].to_string(),
                name: "orphan".to_string(),
            }]
        );
        drop_fixture(&root);
    }

    #[test]
    fn a_reference_inside_a_test_module_does_not_count() {
        let root = fixture_root("tests-only");
        write_fixture(
            &root,
            LEARNING_PATH_MODULES[0],
            "pub fn only_tests() {}\n\n#[cfg(test)]\nmod tests {\n    fn call() { only_tests(); }\n}\n",
        );
        let findings = callerless_learning_path_functions(&root);
        assert_eq!(findings.len(), 1, "findings: {findings:?}");
        assert_eq!(findings[0].name, "only_tests");
        drop_fixture(&root);
    }

    #[test]
    fn an_allowlisted_hook_is_exempt() {
        let root = fixture_root("allowlist");
        write_fixture(
            &root,
            LEARNING_PATH_MODULES[1],
            "pub fn clear_skill_routing_cache() {}\n",
        );
        assert!(callerless_learning_path_functions(&root).is_empty());
        drop_fixture(&root);
    }

    #[test]
    fn doctor_line_reports_findings_and_skips_foreign_repos() {
        let root = fixture_root("report");
        write_fixture(&root, KEEL_CHECKOUT_MARKER, "pub fn orphan() {}\n");
        let mut output = Vec::new();
        report_learning_path_callerless(&root, &mut output);
        let rendered = String::from_utf8_lossy(&output);
        assert!(
            rendered.contains(&format!(
                "[warn] learning path: {KEEL_CHECKOUT_MARKER}::orphan"
            )),
            "output: {rendered}"
        );

        let foreign = fixture_root("report-foreign");
        fs::create_dir_all(&foreign).expect("foreign root");
        let mut output = Vec::new();
        report_learning_path_callerless(&foreign, &mut output);
        assert!(
            output.is_empty(),
            "a repository without keel sources must stay untouched"
        );
        drop_fixture(&root);
        drop_fixture(&foreign);
    }

    /// The D7 invariant: no learning-path function may be callerless. This test
    /// is the sweep's enforcement; `keel doctor` prints the same screen. A
    /// checkout that is not the keel repository scans nothing and passes.
    #[test]
    fn repository_has_no_callerless_learning_path_functions() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(3)
            .expect("workspace repository root")
            .to_path_buf();
        let findings = callerless_learning_path_functions(&root);
        assert!(
            findings.is_empty(),
            "callerless learning-path functions must be wired, deleted, or allowlisted with a reason: {findings:#?}"
        );
    }
}
