use super::*;
use crate::runtime::resolve_repository_root;
use std::fs;

pub(crate) fn run_review_gates_command(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    if arguments.is_empty() || is_help_argument(&arguments[0]) {
        let _ = writeln!(standard_output, "Usage: keel review gates check [flags]");
        return if arguments.is_empty() { 1 } else { 0 };
    }
    if arguments[0] != "check" {
        let _ = writeln!(
            standard_error,
            "Unknown review gates command: {}",
            arguments[0]
        );
        return 1;
    }
    let mut flag_set = review_flag_set("review gates check");
    // "run" preserves this surface's long-standing behavior; the old "skip"
    // default was never read, so the documented flag controlled nothing.
    flag_set.string_flag("repo-test-policy", "run");
    flag_set.bool_flag("python-checks", false);
    flag_set.bool_flag("js-checks", false);
    if let Err(parse_error) = flag_set.parse(&arguments[1..]) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 1;
    }
    // A typo'd --surface silently changed the flow-check range before; reject
    // unknown surfaces instead of guessing.
    let surface = flag_set.string_value("surface").trim().to_string();
    if !matches!(
        surface.as_str(),
        "gates" | "pre-pr" | "pre-commit" | "diff" | "init" | "hosted" | "policy"
    ) {
        let _ = writeln!(
            standard_error,
            "review gates check: unknown --surface {surface:?}; expected one of gates, pre-pr, pre-commit, diff, init, hosted, policy"
        );
        return 1;
    }
    let repository_root = match resolve_repository_root(flag_set.string_value("repo-root")) {
        Ok(path) => path,
        Err(error) => {
            let _ = writeln!(standard_error, "{error}");
            return 1;
        }
    };

    let mut gate_results = Vec::new();

    // Rust tests, unless the caller opts out with --repo-test-policy skip.
    let skip_repo_tests = flag_set.string_value("repo-test-policy").trim() == "skip";
    let has_rust = repository_root.join("Cargo.toml").exists();
    if has_rust && !skip_repo_tests {
        let test_result = run_command(
            "cargo",
            &["test".to_string(), "--workspace".to_string()],
            Some(&repository_root),
        );
        let test_passed = test_result.map(|r| r.code == 0).unwrap_or(false);
        gate_results.push(GateResult {
            name: "rust_tests".to_string(),
            status: if test_passed {
                GateStatus::Pass
            } else {
                GateStatus::Fail
            },
            blocking: true,
            details: if test_passed {
                Some("cargo test --workspace passed".to_string())
            } else {
                Some("cargo test --workspace failed".to_string())
            },
        });
    }

    // Language gates: root markers like .githooks; --python-checks/--js-checks force without markers.
    let force_python = flag_set.bool_value("python-checks");
    let force_js = flag_set.bool_value("js-checks");
    if force_python || has_python_project(&repository_root) {
        if force_python && !has_python_project(&repository_root) {
            // Force path: run tools when any .py exists, else report blocked.
            if has_python_files(&repository_root) {
                gate_results.push(check_black(&repository_root));
                gate_results.push(check_ruff(&repository_root));
                gate_results.push(check_mypy(&repository_root));
                gate_results.push(check_python_tests(&repository_root));
            }
        } else {
            gate_results.extend(run_python_surface_gates(&repository_root, true));
        }
        gate_results.push(check_circular_imports(&repository_root));
        gate_results.push(check_import_safety(&repository_root));
    }
    if force_js || has_js_project(&repository_root) {
        if force_js && !has_js_project(&repository_root) {
            if has_js_files(&repository_root) {
                gate_results.push(check_prettier(&repository_root));
                gate_results.push(check_eslint(&repository_root));
            }
        } else {
            gate_results.extend(run_js_surface_gates(&repository_root, true));
        }
    }
    if has_go_project(&repository_root) {
        gate_results.extend(run_go_surface_gates(&repository_root, true));
    }

    // without this, `gates check` yields a green verdict without the
    // owner-path evidence pre-commit and pre-pr require.
    gate_results.push(flow_check_gate(
        &repository_root,
        flag_set.string_value("base-ref"),
        flag_set.string_value("surface"),
    ));
    gate_results.push(completeness_check_gate(
        &repository_root,
        flag_set.string_value("base-ref"),
        flag_set.string_value("surface"),
    ));

    // E2E verification awareness (informational, non-blocking)
    if let Some(e2e_result) = check_e2e_config(&repository_root) {
        gate_results.push(e2e_result);
    }

    let (blocking_findings, warnings) = tally_gate_results(&gate_results);

    render_gate_results(
        &gate_results,
        blocking_findings,
        warnings,
        flag_set.string_value("format"),
        standard_output,
    );

    if blocking_findings > 0 {
        1
    } else {
        0
    }
}

/// Tally blocking failures and non-blocking warnings from a slice of gate results.
/// Each gate is counted at most once — blocking failures take precedence over warning status.
pub(crate) fn tally_gate_results(gate_results: &[GateResult]) -> (i32, i32) {
    let mut blocking_findings = 0;
    let mut warnings = 0;
    for result in gate_results {
        if result.blocking && result.status == GateStatus::Fail {
            blocking_findings += 1;
        } else if result.status == GateStatus::Warn {
            warnings += 1;
        }
    }
    (blocking_findings, warnings)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum GateStatus {
    Pass,
    Fail,
    Warn,
    /// Reserved for gates that intentionally no-op; matched in status renderers.
    #[allow(dead_code)]
    Skipped,
    Blocked,
}

pub(crate) struct GateResult {
    pub(crate) name: String,
    pub(crate) status: GateStatus,
    pub(crate) blocking: bool,
    pub(crate) details: Option<String>,
}

#[allow(dead_code)]
#[derive(Clone, Copy)]
enum MissingToolBehavior {
    Blocked(&'static str),
    Failed(&'static str),
}

#[allow(dead_code)]
#[derive(Clone, Copy)]
enum ToolTestPolicy {
    Run,
    Skip(&'static str),
}

#[derive(Clone, Copy)]
enum ToolEvaluation {
    ExitCode {
        passed: &'static str,
        failed: &'static str,
    },
    StdoutLines {
        clean: &'static str,
        failed_prefix: &'static str,
    },
}

#[derive(Clone, Copy)]
struct ToolGateOptions {
    blocking: bool,
    missing: MissingToolBehavior,
    test_policy: ToolTestPolicy,
    evaluation: ToolEvaluation,
}

fn run_tool_gate(
    name: &str,
    executable: &str,
    args: &[String],
    repository_root: &Path,
    options: ToolGateOptions,
) -> GateResult {
    if let ToolTestPolicy::Skip(details) = options.test_policy {
        return GateResult {
            name: name.to_string(),
            status: GateStatus::Skipped,
            blocking: false,
            details: Some(details.to_string()),
        };
    }
    let missing = |details: &'static str| match options.missing {
        MissingToolBehavior::Blocked(_) => GateResult {
            name: name.to_string(),
            status: GateStatus::Blocked,
            blocking: false,
            details: Some(details.to_string()),
        },
        MissingToolBehavior::Failed(_) => GateResult {
            name: name.to_string(),
            status: GateStatus::Fail,
            blocking: options.blocking,
            details: Some(details.to_string()),
        },
    };
    let result = match run_command(executable, args, Some(repository_root)) {
        Ok(result) => result,
        Err(_) => {
            return match options.missing {
                MissingToolBehavior::Blocked(details) | MissingToolBehavior::Failed(details) => {
                    missing(details)
                }
            }
        }
    };
    let (status, details) = match options.evaluation {
        ToolEvaluation::ExitCode { passed, failed } => {
            if result.code == 0 {
                (GateStatus::Pass, passed.to_string())
            } else {
                (GateStatus::Fail, failed.to_string())
            }
        }
        ToolEvaluation::StdoutLines {
            clean,
            failed_prefix,
        } => {
            let output = String::from_utf8_lossy(&result.stdout);
            let dirty = output.lines().any(|line| !line.trim().is_empty());
            if dirty {
                (
                    GateStatus::Fail,
                    format!(
                        "{failed_prefix}: {}",
                        output.lines().take(5).collect::<Vec<_>>().join(", ")
                    ),
                )
            } else {
                (GateStatus::Pass, clean.to_string())
            }
        }
    };
    GateResult {
        name: name.to_string(),
        status,
        blocking: options.blocking,
        details: Some(details),
    }
}

fn run_tool_command(
    executable: &str,
    args: &[String],
    repository_root: &Path,
) -> Result<crate::runtime::ProcessResult, String> {
    run_command(executable, args, Some(repository_root))
}

pub(crate) fn has_python_files(repository_root: &Path) -> bool {
    let extensions = ["py", "pyx", "pxd"];
    check_for_extensions(repository_root, &extensions)
}

/// Python root markers (aligned with `.githooks/pre-commit`). Root only avoids monorepo false positives.
pub(crate) fn has_python_project(repository_root: &Path) -> bool {
    repository_root.join("pyproject.toml").exists()
        || repository_root.join("setup.py").exists()
        || repository_root.join("setup.cfg").exists()
}

pub(crate) fn has_js_files(repository_root: &Path) -> bool {
    let extensions = ["js", "jsx", "ts", "tsx", "css", "scss", "less"];
    check_for_extensions(repository_root, &extensions)
}

/// JS/TS project markers aligned with `.githooks/pre-commit` (root package.json only).
pub(crate) fn has_js_project(repository_root: &Path) -> bool {
    repository_root.join("package.json").exists()
}

/// Go project markers aligned with `.githooks/pre-commit` (root go.mod only).
pub(crate) fn has_go_project(repository_root: &Path) -> bool {
    repository_root.join("go.mod").exists()
}

/// C/C++ project markers aligned with `.githooks/pre-commit` (CMakeLists or root sources).
pub(crate) fn has_cpp_project(repository_root: &Path) -> bool {
    if repository_root.join("CMakeLists.txt").exists() {
        return true;
    }
    if let Ok(entries) = fs::read_dir(repository_root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                if matches!(ext, "c" | "cc" | "cpp" | "cxx" | "h" | "hpp" | "hxx") {
                    return true;
                }
            }
        }
    }
    false
}

pub(crate) fn check_for_extensions(repository_root: &Path, extensions: &[&str]) -> bool {
    let mut found = false;
    if let Ok(entries) = fs::read_dir(repository_root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if matches!(
                    name,
                    "node_modules" | "target" | ".git" | "venv" | ".venv" | "__pycache__"
                ) {
                    continue;
                }
                if check_for_extensions(&path, extensions) {
                    found = true;
                    break;
                }
            } else if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                if extensions.contains(&ext) {
                    found = true;
                    break;
                }
            }
        }
    }
    found
}

pub(crate) fn check_black(repository_root: &Path) -> GateResult {
    run_tool_gate(
        "black",
        "black",
        &["--check".to_string(), ".".to_string()],
        repository_root,
        ToolGateOptions {
            blocking: true,
            missing: MissingToolBehavior::Blocked("black not found or not applicable"),
            test_policy: ToolTestPolicy::Run,
            evaluation: ToolEvaluation::ExitCode {
                passed: "black --check passed",
                failed: "black --check found formatting issues",
            },
        },
    )
}

pub(crate) fn check_ruff(repository_root: &Path) -> GateResult {
    run_tool_gate(
        "ruff",
        "ruff",
        &["check".to_string(), ".".to_string()],
        repository_root,
        ToolGateOptions {
            blocking: true,
            missing: MissingToolBehavior::Blocked("ruff not found or not applicable"),
            test_policy: ToolTestPolicy::Run,
            evaluation: ToolEvaluation::ExitCode {
                passed: "ruff check passed",
                failed: "ruff check found issues",
            },
        },
    )
}

pub(crate) fn check_mypy(repository_root: &Path) -> GateResult {
    run_tool_gate(
        "mypy",
        "mypy",
        &[],
        repository_root,
        ToolGateOptions {
            blocking: true,
            missing: MissingToolBehavior::Blocked("mypy not found or not applicable"),
            test_policy: ToolTestPolicy::Run,
            evaluation: ToolEvaluation::ExitCode {
                passed: "mypy passed",
                failed: "mypy found type errors",
            },
        },
    )
}

pub(crate) fn check_circular_imports(repository_root: &Path) -> GateResult {
    // Detect real circular imports: build a local-module import graph and run
    let check_script = r#"
import ast
import sys
from collections import defaultdict
from pathlib import Path

def find_python_files(directory):
    for path in Path(directory).rglob("*.py"):
        s = str(path)
        if "__pycache__" not in s and "venv" not in s and ".tox" not in s and "site-packages" not in s:
            yield path

def module_name_for(path):
    rel = Path(path).with_suffix("")
    parts = [p for p in rel.parts if p not in (".", "..")]
    if parts and parts[-1] == "__init__":
        parts = parts[:-1]
    return ".".join(parts)

def imports_of(path):
    try:
        with open(path, encoding="utf-8", errors="replace") as f:
            tree = ast.parse(f.read(), filename=str(path))
    except Exception:
        return []
    names = []
    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            for alias in node.names:
                names.append(alias.name)
        elif isinstance(node, ast.ImportFrom):
            if node.module:
                names.append(node.module)
    return names

files = list(find_python_files("."))

# Pass 1: collect local module names so we only track local edges.
local_modules = set()
for pyfile in files:
    mod = module_name_for(pyfile)
    if mod:
        local_modules.add(mod)

# Pass 2: build graph of local-module -> local-module edges.
graph = defaultdict(set)
for pyfile in files:
    mod = module_name_for(pyfile)
    if not mod:
        continue
    for imp in imports_of(pyfile):
        top = imp.split(".")[0]
        target = imp if imp in local_modules else (top if top in local_modules else None)
        if target and target != mod:
            graph[mod].add(target)

# DFS cycle detection with a recursion stack (GRAY = on current path).
WHITE, GRAY, BLACK = 0, 1, 2
color = {m: WHITE for m in local_modules}
cycles = []
sys.setrecursionlimit(10000)

def dfs(node, stack):
    color[node] = GRAY
    stack.append(node)
    for neighbor in graph.get(node, set()):
        c = color.get(neighbor, WHITE)
        if c == GRAY:
            if neighbor in stack:
                idx = stack.index(neighbor)
                cycles.append(stack[idx:] + [neighbor])
        elif c == WHITE:
            dfs(neighbor, stack)
    stack.pop()
    color[node] = BLACK

for mod in list(graph.keys()):
    if color.get(mod, WHITE) == WHITE:
        dfs(mod, [])

if cycles:
    seen = set()
    for c in cycles:
        key = tuple(sorted(set(c[:-1])))
        if key in seen:
            continue
        seen.add(key)
        print("circular import: " + " -> ".join(c))
    sys.exit(1)
sys.exit(0)
"#;
    let result = run_command(
        "python",
        &["-c".to_string(), check_script.to_string()],
        Some(repository_root),
    );
    match result {
        Ok(r) => GateResult {
            name: "circular_imports".to_string(),
            status: if r.code == 0 {
                GateStatus::Pass
            } else {
                GateStatus::Fail
            },
            blocking: false,
            details: Some(if r.code == 0 {
                "no circular imports detected".to_string()
            } else {
                "circular imports detected".to_string()
            }),
        },
        Err(_) => GateResult {
            name: "circular_imports".to_string(),
            status: GateStatus::Blocked,
            blocking: false,
            details: Some("circular import check not available".to_string()),
        },
    }
}

pub(crate) fn check_import_safety(repository_root: &Path) -> GateResult {
    // Scan every .py for dangerous top-level imports (eval/exec/__import__/compile)
    let check_script = r#"
import ast
import sys
from pathlib import Path

DANGEROUS_IMPORTS = {"eval", "exec", "__import__", "compile"}

def find_python_files(directory):
    for path in Path(directory).rglob("*.py"):
        s = str(path)
        if "__pycache__" not in s and "venv" not in s and ".tox" not in s and "site-packages" not in s:
            yield path

def check_file(path):
    findings = []
    try:
        with open(path, encoding="utf-8", errors="replace") as f:
            tree = ast.parse(f.read(), filename=str(path))
    except Exception:
        return findings
    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            for alias in node.names:
                top = alias.name.split(".")[0]
                if top in DANGEROUS_IMPORTS:
                    findings.append((str(path), node.lineno, top))
        elif isinstance(node, ast.ImportFrom):
            if node.module:
                top = node.module.split(".")[0]
                if top in DANGEROUS_IMPORTS:
                    findings.append((str(path), node.lineno, top))
    return findings

all_findings = []
for pyfile in find_python_files("."):
    all_findings.extend(check_file(pyfile))

if all_findings:
    for path, line, name in all_findings[:20]:
        print(f"{path}:{line}: dangerous import '{name}'")
    sys.exit(1)
sys.exit(0)
"#;
    let result = run_command(
        "python",
        &["-c".to_string(), check_script.to_string()],
        Some(repository_root),
    );
    match result {
        Ok(r) => GateResult {
            name: "import_safety".to_string(),
            status: if r.code == 0 {
                GateStatus::Pass
            } else {
                GateStatus::Fail
            },
            blocking: false,
            details: Some(if r.code == 0 {
                "no dangerous imports detected".to_string()
            } else {
                "potential dangerous imports found".to_string()
            }),
        },
        Err(_) => GateResult {
            name: "import_safety".to_string(),
            status: GateStatus::Blocked,
            blocking: false,
            details: Some("import safety check not available".to_string()),
        },
    }
}

/// E2E config filenames to detect. When found at the repository root, the review
/// gate reports their presence as an informational (non-blocking) note so the
/// operator knows E2E verification is available.
pub(crate) const E2E_CONFIG_FILENAMES: &[&str] = &[
    "playwright.config.ts",
    "playwright.config.js",
    "playwright.config.mjs",
    "cypress.config.ts",
    "cypress.config.js",
];

/// Detect E2E test configuration at the repository root. Returns an
/// informational (non-blocking) `GateResult` when a known config file exists,
/// or `None` to skip silently when no E2E config is found.
pub(crate) fn check_e2e_config(repository_root: &Path) -> Option<GateResult> {
    for name in E2E_CONFIG_FILENAMES {
        let path = repository_root.join(name);
        if path.exists() {
            let kind = if name.starts_with("playwright") {
                "Playwright"
            } else {
                "Cypress"
            };
            let run_cmd = if kind == "Playwright" {
                "npx playwright test"
            } else {
                "npx cypress run"
            };
            return Some(GateResult {
                name: "e2e_verification".to_string(),
                status: GateStatus::Pass,
                blocking: false,
                details: Some(format!(
                    "E2E: {kind} config detected at {name}. Run `{run_cmd}` before merge."
                )),
            });
        }
    }
    None
}

pub(crate) fn check_prettier(repository_root: &Path) -> GateResult {
    let args = &[
        "prettier".to_string(),
        "--check".to_string(),
        ".".to_string(),
    ];
    let npx_result = run_tool_gate(
        "prettier",
        "npx",
        args,
        repository_root,
        ToolGateOptions {
            blocking: true,
            missing: MissingToolBehavior::Blocked("prettier not found or not applicable"),
            test_policy: ToolTestPolicy::Run,
            evaluation: ToolEvaluation::ExitCode {
                passed: "prettier --check passed",
                failed: "prettier --check found formatting issues",
            },
        },
    );
    if npx_result.status == GateStatus::Pass {
        return npx_result;
    }
    let direct_args = &["--check".to_string(), ".".to_string()];
    let direct_result = run_tool_gate(
        "prettier",
        "prettier",
        direct_args,
        repository_root,
        ToolGateOptions {
            blocking: true,
            missing: MissingToolBehavior::Blocked("prettier not found or not applicable"),
            test_policy: ToolTestPolicy::Run,
            evaluation: ToolEvaluation::ExitCode {
                passed: "prettier --check passed",
                failed: "prettier --check found formatting issues",
            },
        },
    );
    if direct_result.status == GateStatus::Blocked {
        npx_result
    } else {
        direct_result
    }
}
pub(crate) fn render_gate_results(
    results: &[GateResult],
    blocking: i32,
    warnings: i32,
    format: &str,
    standard_output: &mut dyn Write,
) {
    match format {
        "json" => {
            let payload = Value::Object(vec![
                (
                    "gate".into(),
                    Value::String(if blocking > 0 { "block" } else { "pass" }.into()),
                ),
                (
                    "blockingFindings".into(),
                    Value::Number(blocking.to_string()),
                ),
                (
                    "warningFindings".into(),
                    Value::Number(warnings.to_string()),
                ),
                (
                    "gates".into(),
                    Value::Array(
                        results
                            .iter()
                            .map(|r| {
                                Value::Object(vec![
                                    ("name".into(), Value::String(r.name.clone())),
                                    (
                                        "status".into(),
                                        Value::String(
                                            match r.status {
                                                GateStatus::Pass => "pass",
                                                GateStatus::Fail => "fail",
                                                GateStatus::Warn => "warn",
                                                GateStatus::Skipped => "skipped",
                                                GateStatus::Blocked => "blocked",
                                            }
                                            .into(),
                                        ),
                                    ),
                                    ("blocking".into(), Value::Bool(r.blocking)),
                                    (
                                        "details".into(),
                                        Value::String(r.details.clone().unwrap_or_default()),
                                    ),
                                ])
                            })
                            .collect(),
                    ),
                ),
                (
                    "summary".into(),
                    Value::String(format!("{blocking} blocking findings, {warnings} warnings")),
                ),
            ]);
            let _ = write_indented(standard_output, &payload);
        }
        "markdown" => {
            let _ = writeln!(standard_output, "# Native Review Gate Results");
            let _ = writeln!(standard_output);
            let _ = writeln!(standard_output, "## Summary");
            let _ = writeln!(
                standard_output,
                "- gate: {}",
                if blocking > 0 { "FAIL" } else { "PASS" }
            );
            let _ = writeln!(standard_output, "- blocking_findings: {blocking}");
            let _ = writeln!(standard_output, "- warnings: {warnings}");
            let _ = writeln!(standard_output);
            let _ = writeln!(standard_output, "## Gate Results");
            for result in results {
                let status_icon = match result.status {
                    GateStatus::Pass => "[PASS]",
                    GateStatus::Fail => "[FAIL]",
                    GateStatus::Warn => "[WARN]",
                    GateStatus::Skipped => "[SKIP]",
                    GateStatus::Blocked => "[BLK]",
                };
                let _ = writeln!(
                    standard_output,
                    "- {} {}: {}",
                    status_icon,
                    result.name,
                    result.details.clone().unwrap_or_default()
                );
            }
        }
        _ => {
            let _ = writeln!(
                standard_output,
                "gate={} blocking={blocking} warnings={warnings}",
                if blocking > 0 { "fail" } else { "pass" }
            );
            for result in results {
                let status_str = match result.status {
                    GateStatus::Pass => "pass",
                    GateStatus::Fail => "fail",
                    GateStatus::Warn => "warn",
                    GateStatus::Skipped => "skipped",
                    GateStatus::Blocked => "blocked",
                };
                let _ = writeln!(
                    standard_output,
                    "  {}={} blocking={} details={}",
                    result.name,
                    status_str,
                    result.blocking,
                    result
                        .details
                        .clone()
                        .unwrap_or_else(|| "no details".to_string())
                );
            }
        }
    }
}
/// Run the developer-facing Rust gate set for review surfaces.
/// pre-commit gets fmt + clippy (fast); pre-pr also runs the test suite.
/// Skipped entirely when no Cargo.toml exists at the repository root.
pub(crate) fn run_rust_surface_gates(
    repository_root: &Path,
    include_tests: bool,
) -> Vec<GateResult> {
    let mut gate_results = Vec::new();
    if !repository_root.join("Cargo.toml").exists() {
        return gate_results;
    }

    let fmt_result = run_command(
        "cargo",
        &[
            "fmt".to_string(),
            "--all".to_string(),
            "--".to_string(),
            "--check".to_string(),
        ],
        Some(repository_root),
    );
    let fmt_passed = fmt_result.map(|r| r.code == 0).unwrap_or(false);
    gate_results.push(GateResult {
        name: "cargo_fmt".to_string(),
        status: if fmt_passed {
            GateStatus::Pass
        } else {
            GateStatus::Fail
        },
        blocking: true,
        details: Some(
            if fmt_passed {
                "cargo fmt --check passed"
            } else {
                "cargo fmt --check found formatting issues"
            }
            .to_string(),
        ),
    });

    let clippy_result = run_command(
        "cargo",
        &[
            "clippy".to_string(),
            "--all-targets".to_string(),
            "--".to_string(),
            "-D".to_string(),
            "warnings".to_string(),
        ],
        Some(repository_root),
    );
    let clippy_passed = clippy_result.map(|r| r.code == 0).unwrap_or(false);
    gate_results.push(GateResult {
        name: "cargo_clippy".to_string(),
        status: if clippy_passed {
            GateStatus::Pass
        } else {
            GateStatus::Fail
        },
        blocking: true,
        details: Some(
            if clippy_passed {
                "cargo clippy --all-targets -- -D warnings passed"
            } else {
                "cargo clippy --all-targets -- -D warnings found issues"
            }
            .to_string(),
        ),
    });

    if include_tests {
        let test_result = run_command(
            "cargo",
            &["test".to_string(), "--workspace".to_string()],
            Some(repository_root),
        );
        let test_passed = test_result.map(|r| r.code == 0).unwrap_or(false);
        gate_results.push(GateResult {
            name: "cargo_test".to_string(),
            status: if test_passed {
                GateStatus::Pass
            } else {
                GateStatus::Fail
            },
            blocking: true,
            details: Some(
                if test_passed {
                    "cargo test --workspace passed"
                } else {
                    "cargo test --workspace failed"
                }
                .to_string(),
            ),
        });
    }

    gate_results
}

/// Python fmt/lint (and on pre-pr: mypy + pytest). Skipped when no Python project.
/// Tool missing → Blocked non-blocking; tool present and failing → Fail blocking.
pub(crate) fn run_python_surface_gates(
    repository_root: &Path,
    include_tests: bool,
) -> Vec<GateResult> {
    if !has_python_project(repository_root) {
        return Vec::new();
    }
    let mut gate_results = vec![check_black(repository_root), check_ruff(repository_root)];
    if include_tests {
        gate_results.push(check_mypy(repository_root));
        gate_results.push(check_python_tests(repository_root));
    }
    gate_results
}

/// JS/TS fmt/lint (and on pre-pr: tsc + npm test when present). Skipped when no JS project.
pub(crate) fn run_js_surface_gates(repository_root: &Path, include_tests: bool) -> Vec<GateResult> {
    if !has_js_project(repository_root) {
        return Vec::new();
    }
    let mut gate_results = vec![
        check_prettier(repository_root),
        check_eslint(repository_root),
    ];
    if include_tests {
        if repository_root.join("tsconfig.json").exists() {
            gate_results.push(check_tsc(repository_root));
        }
        gate_results.push(check_npm_test(repository_root));
    }
    gate_results
}

/// Go fmt/vet (and on pre-pr: go test). Skipped when no go.mod / .go sources.
pub(crate) fn run_go_surface_gates(repository_root: &Path, include_tests: bool) -> Vec<GateResult> {
    if !has_go_project(repository_root) {
        return Vec::new();
    }
    let mut gate_results = vec![check_gofmt(repository_root), check_go_vet(repository_root)];
    if include_tests {
        gate_results.push(check_go_test(repository_root));
    }
    gate_results
}

/// C/C++ format check via clang-format (aligned with `.githooks/pre-commit`).
/// No portable unit-test auto-runner; pre-pr still reports format gate only.
pub(crate) fn run_cpp_surface_gates(
    repository_root: &Path,
    _include_tests: bool,
) -> Vec<GateResult> {
    if !has_cpp_project(repository_root) {
        return Vec::new();
    }
    vec![check_clang_format(repository_root)]
}

pub(crate) fn collect_cpp_source_files(
    repository_root: &Path,
    out: &mut Vec<std::path::PathBuf>,
    depth: usize,
) {
    if depth > 4 || out.len() >= 50 {
        return;
    }
    let Ok(entries) = fs::read_dir(repository_root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if matches!(
                name,
                "node_modules" | "target" | ".git" | "build" | "dist" | "out" | "venv" | ".venv"
            ) {
                continue;
            }
            collect_cpp_source_files(&path, out, depth + 1);
        } else if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            if matches!(ext, "c" | "cc" | "cpp" | "cxx" | "h" | "hpp" | "hxx") {
                out.push(path);
                if out.len() >= 50 {
                    return;
                }
            }
        }
    }
}

pub(crate) fn check_clang_format(repository_root: &Path) -> GateResult {
    let mut files = Vec::new();
    collect_cpp_source_files(repository_root, &mut files, 0);
    if files.is_empty() {
        return GateResult {
            name: "clang_format".to_string(),
            status: GateStatus::Blocked,
            blocking: false,
            details: Some("no C/C++ source files found for clang-format".to_string()),
        };
    }
    let probe = run_tool_gate(
        "clang_format",
        "clang-format",
        &["--version".to_string()],
        repository_root,
        ToolGateOptions {
            blocking: true,
            missing: MissingToolBehavior::Blocked("clang-format not found or not applicable"),
            test_policy: ToolTestPolicy::Run,
            evaluation: ToolEvaluation::ExitCode {
                passed: "clang-format available",
                failed: "clang-format unavailable",
            },
        },
    );
    if probe.status == GateStatus::Blocked {
        return probe;
    }
    let mut dirty = 0usize;
    for file in &files {
        let Some(path_str) = file.to_str() else {
            continue;
        };
        let result = run_tool_gate(
            "clang_format",
            "clang-format",
            &[
                "--dry-run".to_string(),
                "--Werror".to_string(),
                path_str.to_string(),
            ],
            repository_root,
            ToolGateOptions {
                blocking: true,
                missing: MissingToolBehavior::Blocked("clang-format not found or not applicable"),
                test_policy: ToolTestPolicy::Run,
                evaluation: ToolEvaluation::ExitCode {
                    passed: "clang-format clean",
                    failed: "clang-format found unformatted files",
                },
            },
        );
        match result.status {
            GateStatus::Fail => dirty += 1,
            GateStatus::Blocked => return result,
            _ => {}
        }
    }
    GateResult {
        name: "clang_format".to_string(),
        status: if dirty == 0 {
            GateStatus::Pass
        } else {
            GateStatus::Fail
        },
        blocking: true,
        details: Some(if dirty == 0 {
            format!("clang-format --dry-run clean ({} file(s))", files.len())
        } else {
            format!("clang-format found {dirty} unformatted C/C++ file(s)")
        }),
    }
}

pub(crate) fn check_gofmt(repository_root: &Path) -> GateResult {
    run_tool_gate(
        "gofmt",
        "gofmt",
        &["-l".to_string(), ".".to_string()],
        repository_root,
        ToolGateOptions {
            blocking: true,
            missing: MissingToolBehavior::Blocked("gofmt not found or not applicable"),
            test_policy: ToolTestPolicy::Run,
            evaluation: ToolEvaluation::StdoutLines {
                clean: "gofmt -l . clean",
                failed_prefix: "gofmt found unformatted files",
            },
        },
    )
}

pub(crate) fn check_go_vet(repository_root: &Path) -> GateResult {
    run_tool_gate(
        "go_vet",
        "go",
        &["vet".to_string(), "./...".to_string()],
        repository_root,
        ToolGateOptions {
            blocking: true,
            missing: MissingToolBehavior::Blocked("go not found or not applicable"),
            test_policy: ToolTestPolicy::Run,
            evaluation: ToolEvaluation::ExitCode {
                passed: "go vet ./... passed",
                failed: "go vet ./... found issues",
            },
        },
    )
}

pub(crate) fn check_go_test(repository_root: &Path) -> GateResult {
    run_tool_gate(
        "go_test",
        "go",
        &["test".to_string(), "./...".to_string()],
        repository_root,
        ToolGateOptions {
            blocking: true,
            missing: MissingToolBehavior::Blocked("go not found or not applicable"),
            test_policy: ToolTestPolicy::Run,
            evaluation: ToolEvaluation::ExitCode {
                passed: "go test ./... passed",
                failed: "go test ./... failed",
            },
        },
    )
}

pub(crate) fn check_eslint(repository_root: &Path) -> GateResult {
    // Preserve npx's not-found distinction before falling back to the direct binary.
    let npx_args = &[
        "--no-install".to_string(),
        "eslint".to_string(),
        ".".to_string(),
    ];
    match run_tool_command("npx", npx_args, repository_root) {
        Ok(output) if output.code == 0 => {
            return GateResult {
                name: "eslint".to_string(),
                status: GateStatus::Pass,
                blocking: true,
                details: Some("eslint passed".to_string()),
            };
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
            let not_found = stderr.contains("not found")
                || stderr.contains("could not determine")
                || stderr.contains("enoent");
            if !not_found {
                return GateResult {
                    name: "eslint".to_string(),
                    status: GateStatus::Fail,
                    blocking: true,
                    details: Some("eslint found issues".to_string()),
                };
            }
        }
        Err(_) => {}
    }
    run_tool_gate(
        "eslint",
        "eslint",
        &[".".to_string()],
        repository_root,
        ToolGateOptions {
            blocking: true,
            missing: MissingToolBehavior::Blocked("eslint not found or not applicable"),
            test_policy: ToolTestPolicy::Run,
            evaluation: ToolEvaluation::ExitCode {
                passed: "eslint passed",
                failed: "eslint found issues",
            },
        },
    )
}

pub(crate) fn check_tsc(repository_root: &Path) -> GateResult {
    let npx_result = run_tool_gate(
        "tsc",
        "npx",
        &[
            "--no-install".to_string(),
            "tsc".to_string(),
            "--noEmit".to_string(),
        ],
        repository_root,
        ToolGateOptions {
            blocking: true,
            missing: MissingToolBehavior::Blocked("tsc not found or not applicable"),
            test_policy: ToolTestPolicy::Run,
            evaluation: ToolEvaluation::ExitCode {
                passed: "tsc --noEmit passed",
                failed: "tsc --noEmit found type errors",
            },
        },
    );
    if npx_result.status != GateStatus::Blocked {
        return npx_result;
    }
    run_tool_gate(
        "tsc",
        "tsc",
        &["--noEmit".to_string()],
        repository_root,
        ToolGateOptions {
            blocking: true,
            missing: MissingToolBehavior::Blocked("tsc not found or not applicable"),
            test_policy: ToolTestPolicy::Run,
            evaluation: ToolEvaluation::ExitCode {
                passed: "tsc --noEmit passed",
                failed: "tsc --noEmit found type errors",
            },
        },
    )
}

pub(crate) fn check_npm_test(repository_root: &Path) -> GateResult {
    if !repository_root.join("package.json").exists() {
        return GateResult {
            name: "npm_test".to_string(),
            status: GateStatus::Blocked,
            blocking: false,
            details: Some("no package.json — npm test not applicable".to_string()),
        };
    }
    // --if-present: exit 0 when no test script is defined.
    let result = run_command(
        "npm",
        &["test".to_string(), "--if-present".to_string()],
        Some(repository_root),
    );
    match result {
        Ok(output) => GateResult {
            name: "npm_test".to_string(),
            status: if output.code == 0 {
                GateStatus::Pass
            } else {
                GateStatus::Fail
            },
            blocking: true,
            details: Some(if output.code == 0 {
                "npm test --if-present passed".to_string()
            } else {
                "npm test failed".to_string()
            }),
        },
        Err(_) => GateResult {
            name: "npm_test".to_string(),
            status: GateStatus::Blocked,
            blocking: false,
            details: Some("npm not found or not applicable".to_string()),
        },
    }
}

/// Classify pytest/unittest exit codes for review closeout.
/// Exit 5 = no tests collected/ran for both pytest and unittest discover
/// (not a failure of product code; empty trees must not fail pre-pr).
pub(crate) fn classify_python_test_exit(tool: &str, code: i32) -> GateResult {
    if code == 0 {
        return GateResult {
            name: "python_tests".to_string(),
            status: GateStatus::Pass,
            blocking: true,
            details: Some(format!("{tool} passed")),
        };
    }
    // pytest and unittest discover both use exit 5 for "no tests".
    if code == 5 {
        return GateResult {
            name: "python_tests".to_string(),
            status: GateStatus::Blocked,
            blocking: false,
            details: Some(format!(
                "{tool} exit 5: no tests collected/ran (not applicable)"
            )),
        };
    }
    GateResult {
        name: "python_tests".to_string(),
        status: GateStatus::Fail,
        blocking: true,
        details: Some(format!("{tool} failed (exit {code})")),
    }
}

pub(crate) fn check_python_tests(repository_root: &Path) -> GateResult {
    // Prefer pytest; fall back to unittest discover.
    if let Ok(output) = run_command("pytest", &["-q".to_string()], Some(repository_root)) {
        return classify_python_test_exit("pytest", output.code);
    }
    match run_command(
        "python",
        &[
            "-m".to_string(),
            "unittest".to_string(),
            "discover".to_string(),
            "-q".to_string(),
        ],
        Some(repository_root),
    ) {
        Ok(output) => classify_python_test_exit("python -m unittest discover", output.code),
        Err(_) => GateResult {
            name: "python_tests".to_string(),
            status: GateStatus::Blocked,
            blocking: false,
            details: Some("pytest/unittest not found or not applicable".to_string()),
        },
    }
}
