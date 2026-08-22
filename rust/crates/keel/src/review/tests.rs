use super::*;

// ---- await-ci fail-closed (offline; no provider CLI invoked) ----

/// The whole point of the fix: a provider ERROR or an explicitly-requested
/// but unavailable provider must block (exit 1), never pass with no signal.
#[test]
fn await_ci_error_outcome_blocks_merge() {
    assert_eq!(AwaitCiOutcome::Error.exit_code(), 1);
    assert_eq!(AwaitCiOutcome::Red.exit_code(), 1);
    assert_eq!(AwaitCiOutcome::Pending.exit_code(), 1);
    assert_eq!(AwaitCiOutcome::Timeout.exit_code(), 1);
    // Only a real green, or a genuine no-CI repo, may proceed.
    assert_eq!(AwaitCiOutcome::Green.exit_code(), 0);
    assert_eq!(AwaitCiOutcome::NoCi.exit_code(), 0);
}

/// An explicit `--provider gh`/`glab` that is not installed must resolve to
/// ExplicitUnavailable (which the caller maps to Error/block), NOT to the
/// NoneDetected pass path.
#[test]
fn explicit_provider_unavailable_is_not_treated_as_no_ci() {
    // A provider name that cannot be on PATH in the test environment.
    match resolve_provider("definitely-not-a-real-provider", None) {
        ProviderResolution::ExplicitUnavailable(_) => {}
        other => panic!("explicit unknown provider must be ExplicitUnavailable, got {other:?}"),
    }
}

/// The no-PR message from gh maps to a genuine no-checks (NoCi) result while
/// an unrecognized non-zero is an error; this is the discrimination the
/// fail-open bug lacked, asserted via the outcome mapping without spawning gh.
#[test]
fn gh_no_pr_message_is_no_ci_not_error() {
    // parse_gh_checks on empty output yields no checks (genuine no-CI), and
    // evaluate_checks maps that to NoChecks (which the loop renders as NoCi).
    assert!(parse_gh_checks("").is_none());
    assert!(matches!(evaluate_checks(&[]), CiVerdict::NoChecks));
    // A populated table parses to checks.
    let checks = parse_gh_checks("NAME  STATUS\nci  success\n").expect("one check");
    assert!(matches!(evaluate_checks(&checks), CiVerdict::Green));
}

/// `gh pr checks` exits 8 when checks are pending while still printing the
/// table. A non-zero exit carrying parseable rows is signal (pending), not
/// an error; the gate must read the table, not fail closed on the code.
#[test]
fn gh_pending_exit_code_still_reads_the_check_table() {
    let pending = parse_gh_checks("NAME  STATUS\nci  pending\nbuild  pass\n")
        .expect("two checks despite a pending exit code");
    assert!(matches!(evaluate_checks(&pending), CiVerdict::Pending));
}

/// Regression: only a PASSING review is a reviewer pass. A failed review or
/// the informational diff/init surfaces must not clear the review gate.
#[test]
fn review_pass_clears_gate_only_on_passing_real_surface() {
    // Passing real reviewer surfaces clear the gate.
    assert!(review_pass_clears_gate("gates", 0));
    assert!(review_pass_clears_gate("pre-pr", 0));
    assert!(review_pass_clears_gate("pre-commit", 0));
    // Failing (non-zero) review must NOT clear the gate.
    assert!(!review_pass_clears_gate("gates", 1));
    assert!(!review_pass_clears_gate("pre-pr", 2));
    assert!(!review_pass_clears_gate("pre-commit", 1));
    // Informational surfaces review nothing and never clear the gate.
    assert!(!review_pass_clears_gate("diff", 0));
    assert!(!review_pass_clears_gate("init", 0));
}

// ---- brownfield flow gate classification (offline; no git invocation) ----

/// Modifying established source is what the gate exists to catch.
#[test]
fn brownfield_gate_flags_modified_source_files() {
    for path in [
        "rust/crates/keel/src/review.rs",
        "app/main.py",
        "web/src/App.tsx",
        "cmd/server/main.go",
    ] {
        assert_eq!(
            brownfield_source_from_name_status(&format!("M\t{path}")),
            Some(path.to_string()),
            "{path} should require flow evidence"
        );
    }
}

/// Greenfield, docs, and generated trees are the documented exemptions. An
/// added file has no prior owner, so requiring an owner trace would be wrong.
#[test]
fn brownfield_gate_exempts_added_docs_and_generated_paths() {
    // Added and deleted files carry no established behavior to preserve.
    assert_eq!(
        brownfield_source_from_name_status("A\trust/crates/keel/src/new_module.rs"),
        None
    );
    assert_eq!(brownfield_source_from_name_status("D\tapp/old.py"), None);

    // Docs and config have no ownership flow.
    for path in ["README.md", "CLAUDE.md", "Cargo.toml", ".github/x.yml"] {
        assert_eq!(
            brownfield_source_from_name_status(&format!("M\t{path}")),
            None,
            "{path} should be exempt"
        );
    }

    // Generated and vendored trees are exempt even with a source extension.
    for path in [
        "target/debug/build/x.rs",
        "node_modules/pkg/index.js",
        "vendor/lib/thing.go",
        "app/generated/schema.py",
    ] {
        assert_eq!(
            brownfield_source_from_name_status(&format!("M\t{path}")),
            None,
            "{path} should be exempt"
        );
    }
}

/// Renaming while editing still changes established behavior. Verified against
/// git: `git mv old.rs new.rs` plus an edit reports `R050\told.rs\tnew.rs`, so
/// matching only `M` let a rename slip past the gate entirely.
#[test]
fn brownfield_gate_flags_renamed_source_using_destination_path() {
    assert_eq!(
        brownfield_source_from_name_status("R050\told.rs\tsrc/new.rs"),
        Some("src/new.rs".to_string())
    );
    assert_eq!(
        brownfield_source_from_name_status("R100\tsrc/a.rs\tsrc/b.rs"),
        Some("src/b.rs".to_string())
    );
    assert_eq!(
        brownfield_source_from_name_status("R050\tsrc/a.rs\tvendor/b.rs"),
        None
    );
    assert_eq!(
        brownfield_source_from_name_status("R050\tsrc/a.rs\tdocs/b.md"),
        None
    );
    assert_eq!(brownfield_source_from_name_status("R050\tonly-one"), None);
}

#[test]
fn completeness_touched_sources_includes_working_tree_when_range_is_empty() {
    let root = std::env::current_dir().expect("cwd");
    let from_head = changed_sources_including_added(&root, &["HEAD".to_string()]);
    let from_empty_range = completeness_touched_sources(&root, &["HEAD...HEAD".to_string()]);
    assert!(from_head.is_some() && from_empty_range.is_some());
    let head = from_head.unwrap();
    let combined = from_empty_range.unwrap();
    for path in &head {
        assert!(
            combined.iter().any(|item| item == path),
            "working-tree path {path} missing from empty-range union"
        );
    }
}

#[test]
fn completeness_gate_includes_added_source_unlike_flow() {
    assert_eq!(
        completeness_source_from_name_status("A\trust/crates/keel/src/new_module.rs"),
        Some("rust/crates/keel/src/new_module.rs".to_string())
    );
    assert_eq!(
        completeness_source_from_name_status("M\trust/crates/keel/src/review.rs"),
        Some("rust/crates/keel/src/review.rs".to_string())
    );
    assert_eq!(
        completeness_source_from_name_status("R050\told.rs\tsrc/new.rs"),
        Some("src/new.rs".to_string())
    );
    assert_eq!(completeness_source_from_name_status("M\tREADME.md"), None);
}

#[test]
fn completeness_scan_satisfies_after_marker() {
    let _guard = crate::test_support::ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = std::env::temp_dir().join(format!(
        "keel-completeness-review-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let previous = std::env::var("CLAUDE_TARGET_OVERRIDE").ok();
    std::env::set_var("CLAUDE_TARGET_OVERRIDE", &home);
    let workspace = home.join("ws");
    std::fs::create_dir_all(&workspace).expect("ws");
    let cwd = crate::runtime::display_path(&workspace);
    assert!(!crate::runner::hook_lifecycle::completeness_scan_satisfies(
        &cwd, 1
    ));
    crate::runner::hook_lifecycle::record_completeness_gate_clear_for(&workspace);
    assert!(crate::runner::hook_lifecycle::completeness_scan_satisfies(
        &cwd, 0
    ));
    match previous {
        Some(value) => std::env::set_var("CLAUDE_TARGET_OVERRIDE", value),
        None => std::env::remove_var("CLAUDE_TARGET_OVERRIDE"),
    }
    let _ = std::fs::remove_dir_all(&home);
}

/// The artifact is workspace-global, so relevance is what stops one filled
/// artifact from satisfying the gate forever regardless of what changed next.
#[test]
fn artifact_relevance_matches_touched_paths_tolerantly() {
    let touched = vec![
        "rust/crates/keel/src/review.rs".to_string(),
        "app/main.py".to_string(),
    ];
    // Exact repo-relative match.
    assert!(artifact_targets_a_touched_file(
        "rust/crates/keel/src/review.rs",
        &touched
    ));
    // Windows separators and a leading ./ must not cause a false stale verdict.
    assert!(artifact_targets_a_touched_file(
        ".\\rust\\crates\\keel\\src\\review.rs",
        &touched
    ));
    // An absolute path still resolves by suffix.
    assert!(artifact_targets_a_touched_file(
        "D:/Nasri/Project/keel/app/main.py",
        &touched
    ));
    // A stale artifact tracing an untouched file is rejected.
    assert!(!artifact_targets_a_touched_file(
        "rust/crates/keel/src/commands.rs",
        &touched
    ));
    // An empty target never counts as evidence.
    assert!(!artifact_targets_a_touched_file("", &touched));
    // A bare filename must not match a different directory's same-named file.
    assert!(!artifact_targets_a_touched_file(
        "other/review.rs",
        &touched
    ));
}

/// Case-insensitive filesystems allow `Foo.RS`; a case-sensitive extension
/// check would let that edit bypass the gate entirely.
#[test]
fn brownfield_gate_matches_extensions_case_insensitively() {
    assert_eq!(
        brownfield_source_from_name_status("M\tsrc/Foo.RS"),
        Some("src/Foo.RS".to_string())
    );
    assert_eq!(
        brownfield_source_from_name_status("M\tsrc/App.TSX"),
        Some("src/App.TSX".to_string())
    );
    // Still not a source extension regardless of case.
    assert_eq!(brownfield_source_from_name_status("M\tREADME.MD"), None);
}

/// Windows checkouts report backslash paths; the exemption match is on `/`.
#[test]
fn brownfield_gate_normalizes_windows_separators() {
    assert_eq!(
        brownfield_source_from_name_status("M\trust\\crates\\keel\\src\\review.rs"),
        Some("rust/crates/keel/src/review.rs".to_string())
    );
    assert_eq!(
        brownfield_source_from_name_status("M\tnode_modules\\pkg\\index.js"),
        None
    );
}

// ---- await-ci pure logic (offline-safe; no gh/glab invocation) ----

#[test]
fn classify_check_state_maps_statuses() {
    assert_eq!(classify_check_state("success"), CheckState::Green);
    assert_eq!(classify_check_state("passed"), CheckState::Green);
    assert_eq!(classify_check_state("SUCCESS"), CheckState::Green);
    assert_eq!(classify_check_state("running"), CheckState::Pending);
    assert_eq!(classify_check_state("in_progress"), CheckState::Pending);
    assert_eq!(classify_check_state("queued"), CheckState::Pending);
    assert_eq!(classify_check_state(""), CheckState::Pending);
    // Unknown / failure conclusions fail CLOSED to red so merge never proceeds blind.
    assert_eq!(classify_check_state("failure"), CheckState::Red);
    assert_eq!(classify_check_state("failed"), CheckState::Red);
    assert_eq!(classify_check_state("cancelled"), CheckState::Red);
    assert_eq!(classify_check_state("action_required"), CheckState::Red);
    assert_eq!(classify_check_state("something-weird"), CheckState::Red);
}

#[test]
fn evaluate_checks_blocks_on_any_red() {
    let checks = vec![
        CiCheck {
            name: "build".into(),
            state: CheckState::Green,
        },
        CiCheck {
            name: "test".into(),
            state: CheckState::Red,
        },
    ];
    assert!(matches!(evaluate_checks(&checks), CiVerdict::Red));
}

#[test]
fn evaluate_checks_pending_when_any_running() {
    let checks = vec![
        CiCheck {
            name: "build".into(),
            state: CheckState::Green,
        },
        CiCheck {
            name: "deploy".into(),
            state: CheckState::Pending,
        },
    ];
    assert!(matches!(evaluate_checks(&checks), CiVerdict::Pending));
}

#[test]
fn evaluate_checks_green_only_when_all_green() {
    let checks = vec![
        CiCheck {
            name: "build".into(),
            state: CheckState::Green,
        },
        CiCheck {
            name: "test".into(),
            state: CheckState::Green,
        },
    ];
    assert!(matches!(evaluate_checks(&checks), CiVerdict::Green));
}

#[test]
fn evaluate_checks_empty_is_no_checks() {
    assert!(matches!(evaluate_checks(&[]), CiVerdict::NoChecks));
}

#[test]
fn await_ci_exit_code_blocks_everything_except_green_or_no_ci() {
    assert_eq!(AwaitCiOutcome::Green.exit_code(), 0);
    assert_eq!(AwaitCiOutcome::NoCi.exit_code(), 0);
    assert_eq!(AwaitCiOutcome::Red.exit_code(), 1);
    assert_eq!(AwaitCiOutcome::Pending.exit_code(), 1);
    assert_eq!(AwaitCiOutcome::Timeout.exit_code(), 1);
}

#[test]
fn parse_gh_checks_reads_columns_and_skips_header() {
    let stdout = "NAME\tSTATUS\tCONCLUSION\nbuild\tpass\t\nlint\tfail\t\n";
    let checks = parse_gh_checks(stdout).expect("parse");
    assert_eq!(checks.len(), 2);
    assert_eq!(checks[0].name, "build");
    assert_eq!(checks[0].state, CheckState::Green);
    assert_eq!(checks[1].name, "lint");
    assert_eq!(checks[1].state, CheckState::Red);
}

#[test]
fn parse_gh_checks_empty_is_none() {
    assert!(parse_gh_checks("").is_none());
    assert!(parse_gh_checks("NAME\tSTATUS\n").is_none());
}

#[test]
fn parse_glab_status_reads_name_status_pairs() {
    let stdout = "build: success\ntest: running\n";
    let checks = parse_glab_status(stdout).expect("parse");
    assert_eq!(checks.len(), 2);
    assert_eq!(checks[0].name, "build");
    assert_eq!(checks[0].state, CheckState::Green);
    assert_eq!(checks[1].state, CheckState::Pending);
}

#[test]
fn workflow_slug_is_safe_and_lowercase() {
    assert_eq!(
        workflow_slug("D:\\Nasri\\Project\\keel"),
        "d-nasri-project-keel"
    );
    assert!(!workflow_slug("").is_empty());
}

#[test]
fn review_policy_show_succeeds_with_no_extra_args() {
    // The handler accepts the documented one-argument `review policy show` form.
    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let code = run_review_policy_command(&["show".to_string()], &mut stdout, &mut stderr);
    assert_eq!(code, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
    assert!(String::from_utf8_lossy(&stdout).contains("Native Review Policy"));
}

#[test]
fn review_policy_show_honors_compact_format() {
    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let code = run_review_policy_command(
        &[
            "show".to_string(),
            "--format".to_string(),
            "compact".to_string(),
        ],
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(code, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
    let out = String::from_utf8_lossy(&stdout);
    assert!(
        out.contains("native_rules=rust,python,js,go,cpp"),
        "compact policy should list multi-lang rules, got: {out}"
    );
    assert!(out.contains("language_gates=auto"));
}

#[test]
fn classify_python_test_exit_five_is_non_blocking() {
    for tool in ["pytest", "python -m unittest discover"] {
        let no_tests = classify_python_test_exit(tool, 5);
        assert_eq!(
            no_tests.status,
            GateStatus::Blocked,
            "{tool} exit 5 must be Blocked"
        );
        assert!(!no_tests.blocking, "{tool} exit 5 must be non-blocking");
        let details = no_tests.details.as_deref().unwrap_or("");
        assert!(
            details.contains("no tests") && details.contains(tool),
            "{tool} exit 5 must explain no-tests with tool name: {details}"
        );
    }

    let pass = classify_python_test_exit("pytest", 0);
    assert_eq!(pass.status, GateStatus::Pass);
    assert!(pass.blocking);

    let fail = classify_python_test_exit("pytest", 1);
    assert_eq!(fail.status, GateStatus::Fail);
    assert!(fail.blocking);

    let unittest_fail = classify_python_test_exit("python -m unittest discover", 1);
    assert_eq!(unittest_fail.status, GateStatus::Fail);
    assert!(unittest_fail.blocking);
}

#[test]
fn language_project_markers_are_root_only() {
    let temp = std::env::temp_dir().join(format!("keel-review-markers-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&temp);
    std::fs::create_dir_all(temp.join("nested")).unwrap();
    // Nested sources must not trigger root-marker project detection.
    std::fs::write(temp.join("nested").join("x.py"), "print(1)").unwrap();
    std::fs::write(temp.join("nested").join("x.go"), "package main").unwrap();
    std::fs::write(temp.join("nested").join("x.js"), "console.log(1)").unwrap();
    assert!(!has_python_project(&temp));
    assert!(!has_go_project(&temp));
    assert!(!has_js_project(&temp));
    assert!(!has_cpp_project(&temp));
    assert!(has_python_files(&temp));
    assert!(has_js_files(&temp));

    std::fs::write(temp.join("go.mod"), "module example\n").unwrap();
    assert!(has_go_project(&temp));
    std::fs::write(temp.join("package.json"), "{}").unwrap();
    assert!(has_js_project(&temp));
    std::fs::write(temp.join("pyproject.toml"), "[project]\nname='t'\n").unwrap();
    assert!(has_python_project(&temp));
    std::fs::write(temp.join("main.c"), "int main(void){return 0;}\n").unwrap();
    assert!(has_cpp_project(&temp));
    assert!(!run_cpp_surface_gates(&temp, false).is_empty());

    // Surface gates return empty when markers absent (no cargo/go/py/js/cpp root).
    let empty = std::env::temp_dir().join(format!("keel-review-empty-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&empty);
    std::fs::create_dir_all(&empty).unwrap();
    assert!(run_python_surface_gates(&empty, true).is_empty());
    assert!(run_js_surface_gates(&empty, true).is_empty());
    assert!(run_go_surface_gates(&empty, true).is_empty());
    assert!(run_cpp_surface_gates(&empty, true).is_empty());
    assert!(run_rust_surface_gates(&empty, true).is_empty());

    let _ = std::fs::remove_dir_all(&temp);
    let _ = std::fs::remove_dir_all(&empty);
}

#[test]
fn review_policy_unknown_subcommand_errors() {
    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let code = run_review_policy_command(&["bogus".to_string()], &mut stdout, &mut stderr);
    assert_eq!(code, 1);
}

#[test]
fn gate_result_status_mapping() {
    let pass = GateResult {
        name: "test".to_string(),
        status: GateStatus::Pass,
        blocking: true,
        details: Some("ok".to_string()),
    };
    assert_eq!(pass.status, GateStatus::Pass);

    let fail = GateResult {
        name: "test".to_string(),
        status: GateStatus::Fail,
        blocking: true,
        details: Some("fail".to_string()),
    };
    assert_eq!(fail.status, GateStatus::Fail);
}

#[test]
fn has_python_files_detection() {
    let temp = std::env::temp_dir().join("keel-review-test");
    std::fs::create_dir_all(&temp).unwrap();

    // Create a Python file
    std::fs::write(temp.join("test.py"), "print('hello')").unwrap();

    let result = has_python_files(&temp);
    assert!(result);

    // Cleanup
    std::fs::remove_dir_all(&temp).unwrap();
}

#[test]
fn has_js_files_detection() {
    let temp = std::env::temp_dir().join("keel-review-js-test");
    std::fs::create_dir_all(&temp).unwrap();

    // Create a JS file
    std::fs::write(temp.join("test.js"), "console.log('hello')").unwrap();

    let result = has_js_files(&temp);
    assert!(result);

    // Cleanup
    std::fs::remove_dir_all(&temp).unwrap();
}

#[test]
fn tally_counts_each_blocking_failure_once() {
    let gate_results = vec![
        GateResult {
            name: "rust_tests".to_string(),
            status: GateStatus::Fail,
            blocking: true,
            details: None,
        },
        GateResult {
            name: "ruff".to_string(),
            status: GateStatus::Pass,
            blocking: true,
            details: None,
        },
        GateResult {
            name: "prettier".to_string(),
            status: GateStatus::Warn,
            blocking: false,
            details: None,
        },
    ];

    let (blocking, warnings) = tally_gate_results(&gate_results);

    assert_eq!(
        blocking, 1,
        "exactly one blocking failure should produce blocking_findings=1, not 2 (regression guard for prior double-count bug)"
    );
    assert_eq!(warnings, 1);
}

#[test]
fn tally_handles_empty_and_all_pass() {
    let (blocking, warnings) = tally_gate_results(&[]);
    assert_eq!(blocking, 0);
    assert_eq!(warnings, 0);

    let all_pass = vec![GateResult {
        name: "fmt".to_string(),
        status: GateStatus::Pass,
        blocking: true,
        details: None,
    }];
    let (blocking, warnings) = tally_gate_results(&all_pass);
    assert_eq!(blocking, 0);
    assert_eq!(warnings, 0);
}

fn paths(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

#[test]
fn detect_category_classifies_docs_only() {
    let staged = paths(&["README.md", "docs/architecture.md"]);
    assert_eq!(detect_category(&staged), "Docs");
}

#[test]
fn detect_category_classifies_ci_as_config() {
    let staged = paths(&[".github/workflows/release.yml"]);
    assert_eq!(detect_category(&staged), "Config");
}

#[test]
fn detect_category_classifies_config_files() {
    let staged = paths(&["Cargo.toml", "rustfmt.toml"]);
    assert_eq!(detect_category(&staged), "Config");
}

#[test]
fn detect_category_falls_back_to_wip_for_source() {
    let staged = paths(&["src/lib.rs", "src/main.rs"]);
    assert_eq!(detect_category(&staged), "Wip");
}

#[test]
fn detect_category_empty_is_wip() {
    assert_eq!(detect_category(&[]), "Wip");
}

#[test]
fn derive_scope_returns_common_directory() {
    let staged = paths(&[
        "rust/crates/keel/src/review.rs",
        "rust/crates/keel/src/runner/mod.rs",
    ]);
    assert_eq!(
        derive_scope(&staged),
        Some("keel".to_string()),
        "scope should be the deepest shared directory above the leaf files"
    );
}

#[test]
fn derive_scope_returns_none_when_no_common_prefix() {
    let staged = paths(&["src/lib.rs", "tests/it.rs"]);
    assert_eq!(derive_scope(&staged), None);
}

#[test]
fn derive_scope_skips_bare_src_prefix() {
    let staged = paths(&["src/foo.rs", "src/bar.rs"]);
    assert_eq!(
        derive_scope(&staged),
        None,
        "src/ alone is not a meaningful scope label"
    );
}

#[test]
fn generate_commit_subject_without_diff_uses_placeholder() {
    assert_eq!(
        generate_commit_subject(false, &[]),
        "Wip : GENERAL : update"
    );
}

#[test]
fn generate_commit_subject_with_diff_but_no_staged_signals_empty() {
    assert_eq!(
        generate_commit_subject(true, &[]),
        "Wip : GENERAL : no staged changes"
    );
}

#[test]
fn generate_commit_subject_combines_category_feature_and_summary() {
    let staged = paths(&[
        "rust/crates/keel/src/review.rs",
        "rust/crates/keel/src/lib.rs",
    ]);
    assert_eq!(
        generate_commit_subject(true, &staged),
        "Wip : KEEL : update 2 files"
    );
}

#[test]
fn generate_commit_subject_single_file_uses_leaf_name() {
    let staged = paths(&["docs/architecture.md"]);
    let subject = generate_commit_subject(true, &staged);
    assert!(
        subject.starts_with("Docs : "),
        "expected Docs category, got {subject}"
    );
    assert!(
        subject.ends_with("update architecture.md"),
        "expected leaf summary, got {subject}"
    );
    assert!(
        validate_commit_subject(&subject).is_ok(),
        "generated subject must satisfy the strict validator, got {subject}"
    );
}

#[test]
fn generated_subjects_always_pass_strict_validation() {
    let cases: Vec<Vec<String>> = vec![
        paths(&["docs/readme.md"]),
        paths(&["Cargo.toml"]),
        paths(&["rust/crates/keel/src/review.rs"]),
        paths(&["a.rs", "b.rs"]),
    ];
    for staged in cases {
        let subject = generate_commit_subject(true, &staged);
        assert!(
            validate_commit_subject(&subject).is_ok(),
            "generated subject {subject:?} failed strict validation"
        );
    }
}

#[test]
fn validate_commit_subject_accepts_canonical_form() {
    // Preferred form: Capitalized category, spaces around colons.
    assert!(validate_commit_subject("Wip : RGB : Build light effect mode (multi color)").is_ok());
    assert!(validate_commit_subject("Fix : SENSOR : Correct I2C read timeout").is_ok());
    assert!(validate_commit_subject("Add : ARGB : Add rainbow cycle preset").is_ok());
    assert!(validate_commit_subject("Config : LED : Set default brightness").is_ok());
    assert!(validate_commit_subject("Refactor : RGB : Extract blend helper").is_ok());
    assert!(validate_commit_subject("Docs : SENSOR : Document calibration").is_ok());
    // Legacy lowercase / no-space form still accepted for in-flight history.
    assert!(validate_commit_subject("wip: RGB: Build light effect mode (multi color)").is_ok());
    assert!(validate_commit_subject("fix: SENSOR: Correct I2C read timeout").is_ok());
    assert!(validate_commit_subject("add: ARGB: Add rainbow cycle preset").is_ok());
}

#[test]
fn validate_commit_subject_rejects_unknown_category() {
    let error = validate_commit_subject("feat: RGB: do a thing").unwrap_err();
    assert!(error.contains("category"), "got {error}");
}

#[test]
fn validate_commit_subject_rejects_lowercase_feature() {
    let error = validate_commit_subject("Wip : rgb : do a thing").unwrap_err();
    assert!(error.contains("uppercase"), "got {error}");
}

#[test]
fn validate_commit_subject_rejects_missing_parts() {
    assert!(validate_commit_subject("wip: RGB").is_err());
    assert!(validate_commit_subject("just a message").is_err());
    assert!(validate_commit_subject("wip: RGB: ").is_err());
    assert!(validate_commit_subject("wip: : info").is_err());
}

#[test]
fn commit_body_lists_staged_paths_under_what_changed() {
    let staged = paths(&["a.rs", "b.rs"]);
    let body = commit_body_from_staged(&staged);
    assert!(body.starts_with("What Changed:"));
    assert!(body.contains("- a.rs"));
    assert!(body.contains("- b.rs"));
}

#[test]
fn commit_body_truncates_after_twenty_paths() {
    let many: Vec<String> = (0..25).map(|i| format!("file{i}.rs")).collect();
    let body = commit_body_from_staged(&many);
    assert!(body.contains("... and 5 more files"));
}

#[test]
fn commit_body_handles_empty() {
    assert_eq!(commit_body_from_staged(&[]), "No staged changes.");
}

#[test]
fn pr_summary_bullets_groups_by_change_kind() {
    let staged = paths(&[
        "src/lib.rs",
        "tests/it.rs",
        "README.md",
        ".github/workflows/ci.yml",
    ]);
    let bullets = pr_summary_bullets(&staged);
    assert_eq!(bullets.len(), 4);
    assert!(bullets[0].starts_with("Source changes"));
    assert!(bullets.iter().any(|b| b.starts_with("Test changes")));
    assert!(bullets.iter().any(|b| b.starts_with("Docs changes")));
    assert!(bullets.iter().any(|b| b.starts_with("CI changes")));
}

#[test]
fn pr_summary_bullets_empty_returns_no_changes_message() {
    assert_eq!(
        pr_summary_bullets(&[]),
        vec!["No staged changes detected.".to_string()]
    );
}

#[test]
fn rust_surface_gates_skip_when_no_cargo_toml() {
    let temp = std::env::temp_dir().join("keel-no-cargo-test");
    std::fs::create_dir_all(&temp).unwrap();
    let gates = run_rust_surface_gates(&temp, true);
    assert!(
        gates.is_empty(),
        "non-Rust repos should skip cargo gates, got {gates:?}",
        gates = gates.iter().map(|g| &g.name).collect::<Vec<_>>()
    );
    std::fs::remove_dir_all(&temp).unwrap();
}

// ---- git-workflow preflight ----

/// Run a git command in `dir`, asserting success, for test setup.
fn git_in(dir: &std::path::Path, args: &[&str]) {
    let owned: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    let result = crate::runtime::run_command("git", &owned, Some(dir))
        .unwrap_or_else(|error| panic!("git {args:?} spawn failed: {error}"));
    assert_eq!(
        result.code,
        0,
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
}

/// Create an initialized temp git repo with one commit on `main` and a
/// deterministic identity/branch so preflight checks are reproducible.
fn init_temp_repo(label: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let dir = std::env::temp_dir().join(format!(
        "keel-preflight-{label}-{}-{nanos}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("create temp repo dir");
    git_in(&dir, &["init", "-q"]);
    git_in(&dir, &["config", "user.email", "test@example.com"]);
    git_in(&dir, &["config", "user.name", "Test"]);
    git_in(&dir, &["checkout", "-q", "-B", "main"]);
    std::fs::write(dir.join("README.md"), "base\n").unwrap();
    git_in(&dir, &["add", "."]);
    git_in(&dir, &["commit", "-q", "-m", "chore: base commit"]);
    dir
}

fn run_preflight(repo: &std::path::Path, base_ref: &str) -> (u8, String) {
    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let code = run_git_workflow_preflight(
        &[
            "--repo-root".to_string(),
            repo.to_string_lossy().to_string(),
            "--base-ref".to_string(),
            base_ref.to_string(),
        ],
        &mut stdout,
        &mut stderr,
    );
    (code, String::from_utf8_lossy(&stdout).to_string())
}

#[test]
fn preflight_passes_on_clean_task_branch_ahead_of_base() {
    let repo = init_temp_repo("pass");
    // Preferred work branch: task/<task>
    git_in(&repo, &["checkout", "-q", "-b", "task/widget"]);
    std::fs::write(repo.join("widget.txt"), "feature\n").unwrap();
    git_in(&repo, &["add", "."]);
    git_in(&repo, &["commit", "-q", "-m", "Add : WIDGET : add widget"]);

    let (code, stdout) = run_preflight(&repo, "main");
    assert_eq!(code, 0, "stdout: {stdout}");
    assert!(stdout.contains("PASS"), "stdout: {stdout}");
    assert!(
        !stdout.to_lowercase().contains("legacy"),
        "preferred task/ branch must not warn as legacy: {stdout}"
    );

    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn preflight_allows_legacy_branch_with_warning() {
    let repo = init_temp_repo("legacy");
    git_in(&repo, &["checkout", "-q", "-b", "add/WIDGET"]);
    std::fs::write(repo.join("widget.txt"), "feature\n").unwrap();
    git_in(&repo, &["add", "."]);
    git_in(&repo, &["commit", "-q", "-m", "add: WIDGET: add widget"]);

    let (code, stdout) = run_preflight(&repo, "main");
    assert_eq!(code, 0, "legacy must still pass: {stdout}");
    assert!(
        stdout.to_lowercase().contains("legacy"),
        "legacy prefix should warn: {stdout}"
    );

    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn preflight_blocks_on_protected_branch() {
    let repo = init_temp_repo("protected");
    // Still on main (final-stable; never pushed from directly).
    let (code, stdout) = run_preflight(&repo, "main");
    assert_eq!(code, 1, "stdout: {stdout}");
    assert!(stdout.contains("final-stable branch"), "stdout: {stdout}");

    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn preflight_blocks_on_unsanctioned_branch_name() {
    let repo = init_temp_repo("badname");
    git_in(&repo, &["checkout", "-q", "-b", "random-branch"]);
    std::fs::write(repo.join("x.txt"), "x\n").unwrap();
    git_in(&repo, &["add", "."]);
    git_in(&repo, &["commit", "-q", "-m", "add: X: x"]);

    let (code, stdout) = run_preflight(&repo, "main");
    assert_eq!(code, 1, "stdout: {stdout}");
    assert!(stdout.contains("sanctioned"), "stdout: {stdout}");

    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn preflight_warns_on_integration_tier_branch() {
    // Standing on `feat` is valid only for promotion; preflight allows it
    let repo = init_temp_repo("tier");
    git_in(&repo, &["checkout", "-q", "-b", "feat"]);
    std::fs::write(repo.join("f.txt"), "f\n").unwrap();
    git_in(&repo, &["add", "."]);
    git_in(&repo, &["commit", "-q", "-m", "add: FEAT: integration"]);

    let (code, stdout) = run_preflight(&repo, "main");
    assert_eq!(
        code, 0,
        "integration tier is a warning, not a block: {stdout}"
    );
    assert!(stdout.contains("integration tier"), "stdout: {stdout}");

    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn preflight_blocks_on_dirty_worktree() {
    let repo = init_temp_repo("dirty");
    git_in(&repo, &["checkout", "-q", "-b", "fix/THING"]);
    std::fs::write(repo.join("thing.txt"), "committed\n").unwrap();
    git_in(&repo, &["add", "."]);
    git_in(&repo, &["commit", "-q", "-m", "fix: THING: thing"]);
    // Now leave an uncommitted change in the worktree.
    std::fs::write(repo.join("thing.txt"), "dirty edit\n").unwrap();

    let (code, stdout) = run_preflight(&repo, "main");
    assert_eq!(code, 1, "stdout: {stdout}");
    assert!(stdout.contains("uncommitted change"), "stdout: {stdout}");

    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn preflight_blocks_when_no_commits_ahead_of_base() {
    let repo = init_temp_repo("nocommits");
    // A sanctioned, clean work branch with NO commits beyond main.
    git_in(&repo, &["checkout", "-q", "-b", "add/EMPTY"]);
    let (code, stdout) = run_preflight(&repo, "main");
    assert_eq!(code, 1, "stdout: {stdout}");
    assert!(
        stdout.contains("no commits on HEAD ahead"),
        "stdout: {stdout}"
    );

    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn preflight_blocks_when_base_ref_missing() {
    let repo = init_temp_repo("nobase");
    git_in(&repo, &["checkout", "-q", "-b", "fix/THING"]);
    std::fs::write(repo.join("a.txt"), "a\n").unwrap();
    git_in(&repo, &["add", "."]);
    git_in(&repo, &["commit", "-q", "-m", "fix: THING: a"]);

    let (code, stdout) = run_preflight(&repo, "origin/does-not-exist");
    assert_eq!(code, 1, "stdout: {stdout}");
    assert!(stdout.contains("not found"), "stdout: {stdout}");

    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn preflight_blocks_on_non_git_directory() {
    let dir = std::env::temp_dir().join(format!("keel-preflight-nongit-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (code, _stdout) = run_preflight(&dir, "main");
    assert_eq!(code, 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn preflight_warns_on_commit_subject_prefix_drift() {
    let repo = init_temp_repo("drift");
    git_in(&repo, &["checkout", "-q", "-b", "add/MSG"]);
    std::fs::write(repo.join("m.txt"), "m\n").unwrap();
    git_in(&repo, &["add", "."]);
    // Non-conventional subject to should produce a [warn], not a block.
    git_in(&repo, &["commit", "-q", "-m", "random message no prefix"]);

    let (code, stdout) = run_preflight(&repo, "main");
    assert_eq!(code, 0, "drift is a warning, not a block: {stdout}");
    assert!(stdout.contains("conventional prefix"), "stdout: {stdout}");

    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn preflight_json_format_emits_structured_payload() {
    let repo = init_temp_repo("json");
    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let code = run_git_workflow_preflight(
        &[
            "--repo-root".to_string(),
            repo.to_string_lossy().to_string(),
            "--base-ref".to_string(),
            "main".to_string(),
            "--format".to_string(),
            "json".to_string(),
        ],
        &mut stdout,
        &mut stderr,
    );
    // Main with no commits ahead is blocked.
    assert_eq!(code, 1);
    let text = String::from_utf8_lossy(&stdout);
    assert!(text.contains("\"passed\""), "stdout: {text}");
    assert!(text.contains("\"blocking\""), "stdout: {text}");
    assert!(text.contains("\"branch\""), "stdout: {text}");
    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn e2e_config_detected_when_playwright_exists() {
    let temp = std::env::temp_dir().join(format!(
        "keel-e2e-pw-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ));
    std::fs::create_dir_all(&temp).unwrap();
    std::fs::write(temp.join("playwright.config.ts"), "export default {}").unwrap();

    let result = check_e2e_config(&temp);
    assert!(result.is_some(), "should detect playwright.config.ts");
    let gate = result.unwrap();
    assert_eq!(gate.name, "e2e_verification");
    assert_eq!(gate.status, GateStatus::Pass);
    assert!(!gate.blocking);
    let details = gate.details.unwrap();
    assert!(details.contains("Playwright"));
    assert!(details.contains("npx playwright test"));

    let _ = std::fs::remove_dir_all(&temp);
}

#[test]
fn e2e_config_detected_when_cypress_exists() {
    let temp = std::env::temp_dir().join(format!(
        "keel-e2e-cy-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ));
    std::fs::create_dir_all(&temp).unwrap();
    std::fs::write(temp.join("cypress.config.js"), "module.exports={}").unwrap();

    let result = check_e2e_config(&temp);
    assert!(result.is_some(), "should detect cypress.config.js");
    let gate = result.unwrap();
    assert_eq!(gate.name, "e2e_verification");
    let details = gate.details.unwrap();
    assert!(details.contains("Cypress"));
    assert!(details.contains("npx cypress run"));

    let _ = std::fs::remove_dir_all(&temp);
}

#[test]
fn e2e_config_absent_returns_none() {
    let temp = std::env::temp_dir().join(format!(
        "keel-e2e-none-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ));
    std::fs::create_dir_all(&temp).unwrap();

    let result = check_e2e_config(&temp);
    assert!(result.is_none(), "no E2E config means no gate result");

    let _ = std::fs::remove_dir_all(&temp);
}

#[test]
fn e2e_config_not_blocking_in_tally() {
    let e2e_gate = GateResult {
        name: "e2e_verification".to_string(),
        status: GateStatus::Pass,
        blocking: false,
        details: Some("Playwright detected".to_string()),
    };
    let results = vec![
        GateResult {
            name: "rust_tests".to_string(),
            status: GateStatus::Fail,
            blocking: true,
            details: None,
        },
        e2e_gate,
    ];
    let (blocking, warnings) = tally_gate_results(&results);
    assert_eq!(blocking, 1, "E2E should not add blocking findings");
    assert_eq!(warnings, 0);
}

#[test]
fn impact_flag_defaults_to_false() {
    let flag_set = review_flag_set("review pre-pr");
    assert!(
        !flag_set.bool_value("impact"),
        "impact gate must be opt-in to keep default review fast"
    );
}

#[test]
fn impact_gate_result_is_never_blocking() {
    let gate = GateResult {
        name: "impact".to_string(),
        status: GateStatus::Pass,
        blocking: false,
        details: Some("3 changed, 2 impacted: a.ts, b.ts".to_string()),
    };
    let results = vec![gate];
    let (blocking, _) = tally_gate_results(&results);
    assert_eq!(blocking, 0, "impact gate must never block review");
}
#[test]
fn compact_gate_output_includes_actionable_details() {
    let results = vec![GateResult {
        name: "rust_tests".to_string(),
        status: GateStatus::Fail,
        blocking: true,
        details: Some("cargo test failed: see stderr; rerun cargo test --workspace".to_string()),
    }];
    let mut output = Vec::new();
    render_gate_results(&results, 1, 0, "compact", &mut output);
    let rendered = String::from_utf8(output).expect("utf8");
    assert!(rendered.contains("rust_tests=fail"));
    assert!(rendered.contains("cargo test failed"));
    assert!(rendered.contains("rerun cargo test"));
}
