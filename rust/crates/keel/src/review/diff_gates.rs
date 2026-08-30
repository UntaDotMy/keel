use super::*;
use crate::runtime::resolve_repository_root;
use std::fs;

pub(crate) fn collect_review_gate_results(
    repository_root: &Path,
    base_ref: &str,
    surface_name: &str,
    scan_all: bool,
    include_tests: bool,
    include_impact: bool,
) -> Vec<GateResult> {
    // Auto language gates (.githooks markers). Missing tools = non-blocking Blocked.
    let mut gate_results = run_rust_surface_gates(repository_root, include_tests);
    gate_results.extend(run_python_surface_gates(repository_root, include_tests));
    gate_results.extend(run_js_surface_gates(repository_root, include_tests));
    gate_results.extend(run_go_surface_gates(repository_root, include_tests));
    gate_results.extend(run_cpp_surface_gates(repository_root, include_tests));
    gate_results.push(comment_style_gate(
        repository_root,
        base_ref,
        surface_name,
        scan_all,
    ));
    gate_results.push(prose_style_gate(
        repository_root,
        base_ref,
        surface_name,
        scan_all,
    ));
    gate_results.push(slop_gate(repository_root, base_ref, surface_name, scan_all));
    gate_results.push(flow_check_gate(repository_root, base_ref, surface_name));
    gate_results.push(completeness_check_gate(
        repository_root,
        base_ref,
        surface_name,
    ));
    if include_impact {
        gate_results.push(impact_gate(repository_root, base_ref, surface_name));
    }
    if let Some(e2e_result) = check_e2e_config(repository_root) {
        gate_results.push(e2e_result);
    }
    gate_results
}

pub(crate) fn run_review_surface_command(
    surface_name: &str,
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flag_set = review_flag_set(&format!("review {surface_name}"));
    if let Err(parse_error) = flag_set.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 1;
    }

    // diff and init are informational surfaces ; keep the existing pass behavior.
    if surface_name == "diff" || surface_name == "init" {
        return render_gate_result("pass", 0, flag_set.string_value("format"), standard_output);
    }

    let repository_root = match resolve_repository_root(flag_set.string_value("repo-root")) {
        Ok(path) => path,
        Err(error) => {
            let _ = writeln!(standard_error, "{error}");
            return 1;
        }
    };

    // --all scans the whole tracked tree (cleanup mode) instead of only added
    // diff lines, so pre-existing slop/comments/prose are caught too.
    let scan_all = flag_set.bool_value("all");
    let gate_results = collect_review_gate_results(
        &repository_root,
        flag_set.string_value("base-ref"),
        surface_name,
        scan_all,
        surface_name == "pre-pr",
        flag_set.bool_value("impact"),
    );
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
/// Build the comment-style gate result for a review surface. Lints added comment
/// lines only (existing comments grandfathered). pre-commit scans the working
/// diff against HEAD; other surfaces scan against the base ref. Blocking only
/// when a high-severity finding (over-length impl comment or em/en dash) appears.
pub(crate) fn comment_style_gate(
    repository_root: &Path,
    base_ref: &str,
    surface_name: &str,
    scan_all: bool,
) -> GateResult {
    let findings = if scan_all {
        crate::comment_lint::lint_tracked_tree(repository_root)
    } else if surface_name == "pre-commit" {
        crate::comment_lint::lint_working_comments(repository_root)
    } else {
        let base = base_ref.trim();
        let base = if base.is_empty() { "origin/main" } else { base };
        crate::comment_lint::lint_added_comments(repository_root, base)
    };
    let blocking = crate::comment_lint::has_blocking(&findings);
    let status = if findings.is_empty() {
        GateStatus::Pass
    } else if blocking {
        GateStatus::Fail
    } else {
        GateStatus::Warn
    };
    let details = if findings.is_empty() {
        "no added-comment style issues".to_string()
    } else {
        let shown: Vec<String> = findings
            .iter()
            .take(5)
            .map(|f| format!("{}:{} {}", f.file, f.line, f.message))
            .collect();
        format!(
            "{} added-comment issue(s): {}",
            findings.len(),
            shown.join("; ")
        )
    };
    GateResult {
        name: "comment_style".to_string(),
        status,
        blocking,
        details: Some(details),
    }
}

/// Source extensions the brownfield gate treats as established behavior. Docs,
/// config, and data files carry no ownership flow to preserve.
pub(crate) const FLOW_SOURCE_EXTENSIONS: &[&str] = &[
    "rs", "go", "py", "js", "jsx", "ts", "tsx", "java", "kt", "kts", "swift", "c", "h", "cc",
    "cpp", "hpp", "cs", "rb", "php", "scala", "dart", "m", "mm", "sh", "ps1", "lua", "ex", "exs",
];

/// Path segments whose contents are generated or vendored, so they are exempt.
pub(crate) const FLOW_EXEMPT_SEGMENTS: &[&str] = &[
    "target/",
    "node_modules/",
    "vendor/",
    "dist/",
    "build/",
    ".git/",
    "generated/",
    "__pycache__/",
];

/// Existing source files modified in the reviewed diff, from `--name-status`.
/// Only `M` and `R` count: an added file is greenfield and has no prior owner to
/// preserve, which is the documented exemption.
///
/// `None` means the range could not be resolved (git missing, not a repository,
/// unknown base ref). The caller must not treat that as "nothing changed": an
/// unresolvable range once made this blocking gate report a clean pass over nine
/// modified files.
pub(crate) fn modified_existing_sources(
    repository_root: &Path,
    range: &[String],
) -> Option<Vec<String>> {
    let mut args = vec!["diff".to_string(), "--name-status".to_string()];
    args.extend(range.iter().cloned());
    let result = run_command("git", &args, Some(repository_root)).ok()?;
    if result.code != 0 {
        return None;
    }
    Some(
        String::from_utf8_lossy(&result.stdout)
            .lines()
            .filter_map(brownfield_source_from_name_status)
            .collect(),
    )
}

/// Classify one `git diff --name-status` line, returning the path when it is an
/// edit to established source. Split out from the git call so the exemption rules
/// (greenfield, docs, generated) are unit-testable without a repository.
pub(crate) fn brownfield_source_from_name_status(line: &str) -> Option<String> {
    let mut parts = line.split('\t');
    let status = parts.next()?.trim();
    let first_path = parts.next()?.trim();
    // a rename emits `R<score>\told\tnew`, and renaming while editing still
    // changes established behavior, so gate it against the destination path.
    let path = match status.chars().next()? {
        'M' => first_path,
        'R' => parts.next()?.trim(),
        _ => return None,
    };
    let normalized = path.replace('\\', "/");
    if FLOW_EXEMPT_SEGMENTS
        .iter()
        .any(|segment| normalized.contains(segment))
    {
        return None;
    }
    // case-fold the extension so a `Foo.RS` on a case-insensitive
    // filesystem cannot slip past the gate.
    let extension = normalized
        .rsplit('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if !FLOW_SOURCE_EXTENSIONS.contains(&extension.as_str()) {
        return None;
    }
    Some(normalized)
}

/// Blocking brownfield gate: modifying established source requires a complete
/// flow-check artifact recording the owner path. This is the enforcement half of
/// `preserve-existing-flow`; without it the contract was skill prose only.
/// Greenfield (added files), docs, and generated trees are exempt, and a diff
/// touching no existing source passes untouched.
pub(crate) fn flow_check_gate(
    repository_root: &Path,
    base_ref: &str,
    surface_name: &str,
) -> GateResult {
    let range: Vec<String> = if surface_name == "pre-commit" {
        vec!["HEAD".to_string()]
    } else {
        let base = base_ref.trim();
        let base = if base.is_empty() { "origin/main" } else { base };
        vec![format!("{base}...HEAD")]
    };

    let Some(touched) = modified_existing_sources(repository_root, &range) else {
        return GateResult {
            name: "flow_check".to_string(),
            status: GateStatus::Fail,
            blocking: true,
            details: Some(format!(
                "could not resolve the diff range ({}); brownfield evidence cannot be checked. \
                 Pass an existing --base-ref, or run the pre-commit surface.",
                range.join(" ")
            )),
        };
    };
    if touched.is_empty() {
        return GateResult {
            name: "flow_check".to_string(),
            status: GateStatus::Pass,
            blocking: true,
            details: Some(
                "no existing source modified; brownfield gate not applicable".to_string(),
            ),
        };
    }

    let artifact =
        keel_flow::resolve_artifact_path(repository_root, keel_flow::DEFAULT_ARTIFACT_PATH);
    let (errors, check) =
        match keel_flow::load_check(repository_root, keel_flow::DEFAULT_ARTIFACT_PATH) {
            Ok(check) => (
                keel_flow::validate_finished_check(check.clone()),
                Some(check),
            ),
            Err(load_error) => (vec![load_error.to_string()], None),
        };

    if errors.is_empty() {
        let check = check.expect("validated flow check must be present");
        if check.docs_only || check.formatting_only || check.generated_only || check.greenfield {
            return GateResult {
                name: "flow_check".to_string(),
                status: GateStatus::Fail,
                blocking: true,
                details: Some(format!(
                    "the flow-check artifact at {} claims an exemption, but the reviewed diff \
                     contains {} established source file(s) ({}); exemption claims cannot bypass \
                     existing-source ownership evidence",
                    artifact.display(),
                    touched.len(),
                    preview_touched_paths(&touched)
                )),
            };
        }
        if !artifact_targets_all_touched_files(&check.target_files, &touched) {
            return GateResult {
                name: "flow_check".to_string(),
                status: GateStatus::Fail,
                blocking: true,
                details: Some(format!(
                    "the flow-check artifact at {} does not trace every modified source file \
                     ({}). Re-run `keel flow start --target-file <path> --target-files <csv>` \
                     with the complete owner set, then `keel flow finish`.",
                    artifact.display(),
                    preview_touched_paths(&touched)
                )),
            };
        }
        match keel_flow::repository_state(repository_root) {
            Ok((head, fingerprint))
                if head == check.repository_head && fingerprint == check.diff_fingerprint => {}
            Ok(_) => {
                return GateResult {
                    name: "flow_check".to_string(),
                    status: GateStatus::Fail,
                    blocking: true,
                    details: Some(format!(
                        "the finalized flow-check artifact at {} is stale for the current HEAD/diff; \
                         re-run `keel flow finish` after the latest edit",
                        artifact.display()
                    )),
                };
            }
            Err(error) => {
                return GateResult {
                    name: "flow_check".to_string(),
                    status: GateStatus::Fail,
                    blocking: true,
                    details: Some(format!("could not verify finalized flow evidence: {error}")),
                };
            }
        }
        return GateResult {
            name: "flow_check".to_string(),
            status: GateStatus::Pass,
            blocking: true,
            details: Some(format!(
                "{} existing source file(s) modified; finalized flow-check covers every file and matches the current diff",
                touched.len()
            )),
        };
    }

    GateResult {
        name: "flow_check".to_string(),
        status: GateStatus::Fail,
        blocking: true,
        details: Some(format!(
            "{} existing source file(s) modified ({}) but the flow-check artifact at {} is missing or incomplete: {}. \
             Run `keel flow start --target-file <path>`, fill the owner path, then `keel flow finish`.",
            touched.len(),
            preview_touched_paths(&touched),
            artifact.display(),
            errors.join("; ")
        )),
    }
}

/// Blocking completeness gate: a source change without a fresh sibling scan
/// is a one-site close. Same marker `keel code-search siblings` writes.
/// Docs-only diffs pass; an unresolvable range warns (never a silent pass).
pub(crate) fn completeness_check_gate(
    repository_root: &Path,
    base_ref: &str,
    surface_name: &str,
) -> GateResult {
    let range: Vec<String> = if surface_name == "pre-commit" {
        vec!["HEAD".to_string()]
    } else {
        let base = base_ref.trim();
        let base = if base.is_empty() { "origin/main" } else { base };
        vec![format!("{base}...HEAD")]
    };

    let Some(touched) = completeness_touched_sources(repository_root, &range) else {
        return GateResult {
            name: "completeness_check".to_string(),
            status: GateStatus::Fail,
            blocking: true,
            details: Some(format!(
                "could not resolve the diff range ({}); sibling-scan evidence cannot be checked. \
                 Pass an existing --base-ref, or run the pre-commit surface.",
                range.join(" ")
            )),
        };
    };
    if touched.is_empty() {
        return GateResult {
            name: "completeness_check".to_string(),
            status: GateStatus::Pass,
            blocking: true,
            details: Some("no source files changed; completeness gate not applicable".to_string()),
        };
    }

    let after_ms = newest_source_mtime_ms(repository_root, &touched);
    let workspace = crate::runtime::display_path(repository_root);
    if crate::runner::hook_lifecycle::completeness_scan_satisfies(&workspace, after_ms) {
        return GateResult {
            name: "completeness_check".to_string(),
            status: GateStatus::Pass,
            blocking: true,
            details: Some(format!(
                "{} source file(s) changed; sibling scan is current",
                touched.len()
            )),
        };
    }

    GateResult {
        name: "completeness_check".to_string(),
        status: GateStatus::Fail,
        blocking: true,
        details: Some(format!(
            "{} source file(s) changed ({}) but `keel code-search siblings` has not run since those edits. \
             A one-site fix is unfinished. Run `keel code-search siblings --query \"<the bug shape>\"` \
             (or MCP code_search action=siblings) and handle every hit, or mark it out of scope.",
            touched.len(),
            preview_touched_paths(&touched)
        )),
    }
}

/// Union of the named range and the working tree vs HEAD. `gates check
/// --base-ref HEAD` uses `HEAD...HEAD` (empty); without the working-tree
/// half a dirty tree would pass completeness without a sibling scan.
pub(crate) fn completeness_touched_sources(
    repository_root: &Path,
    range: &[String],
) -> Option<Vec<String>> {
    let mut files = changed_sources_including_added(repository_root, range)?;
    if range != ["HEAD".to_string()] {
        if let Some(working_tree) =
            changed_sources_including_added(repository_root, &["HEAD".to_string()])
        {
            for path in working_tree {
                if !files.iter().any(|existing| existing == &path) {
                    files.push(path);
                }
            }
        }
    }
    Some(files)
}

pub(crate) fn changed_sources_including_added(
    repository_root: &Path,
    range: &[String],
) -> Option<Vec<String>> {
    let mut args = vec!["diff".to_string(), "--name-status".to_string()];
    args.extend(range.iter().cloned());
    let result = run_command("git", &args, Some(repository_root)).ok()?;
    if result.code != 0 {
        return None;
    }
    Some(
        String::from_utf8_lossy(&result.stdout)
            .lines()
            .filter_map(completeness_source_from_name_status)
            .collect(),
    )
}

/// Source files whose change requires a sibling scan (added, modified, renamed).
pub(crate) fn completeness_source_from_name_status(line: &str) -> Option<String> {
    let mut parts = line.split('\t');
    let status = parts.next()?.trim();
    let first_path = parts.next()?.trim();
    let path = match status.chars().next()? {
        'M' | 'A' => first_path,
        'R' => parts.next()?.trim(),
        _ => return None,
    };
    let normalized = path.replace('\\', "/");
    if FLOW_EXEMPT_SEGMENTS
        .iter()
        .any(|segment| normalized.contains(segment))
    {
        return None;
    }
    let extension = normalized
        .rsplit('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if !FLOW_SOURCE_EXTENSIONS.contains(&extension.as_str()) {
        return None;
    }
    Some(normalized)
}

pub(crate) fn newest_source_mtime_ms(repository_root: &Path, rel_paths: &[String]) -> u64 {
    let mut newest = 0u64;
    for rel in rel_paths {
        let path = repository_root.join(rel);
        let Ok(meta) = fs::metadata(&path) else {
            continue;
        };
        let Ok(modified) = meta.modified() else {
            continue;
        };
        let Ok(duration) = modified.duration_since(std::time::UNIX_EPOCH) else {
            continue;
        };
        newest = newest.max(duration.as_millis() as u64);
    }
    newest
}

/// First few touched paths, for a gate message that stays readable on a wide diff.
pub(crate) fn preview_touched_paths(paths: &[String]) -> String {
    paths
        .iter()
        .take(3)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Advisory blast-radius gate: reports which in-repo files transitively import
/// the changed files. Non-blocking and fail-open; a missing or unreadable graph
/// silently skips. Uses the cached artifact when present, builds fresh otherwise.
pub(crate) fn impact_gate(
    repository_root: &Path,
    base_ref: &str,
    surface_name: &str,
) -> GateResult {
    let range: Vec<String> = if surface_name == "pre-commit" {
        vec!["HEAD".to_string()]
    } else {
        let base = base_ref.trim();
        let base = if base.is_empty() { "origin/main" } else { base };
        vec![format!("{base}...HEAD")]
    };

    let Some(touched) = modified_existing_sources(repository_root, &range) else {
        return GateResult {
            name: "impact".to_string(),
            status: GateStatus::Blocked,
            blocking: false,
            details: Some("could not resolve diff range".to_string()),
        };
    };
    if touched.is_empty() {
        return GateResult {
            name: "impact".to_string(),
            status: GateStatus::Pass,
            blocking: false,
            details: Some("no existing source modified".to_string()),
        };
    }

    let graph = crate::utility::code_graph::cached_artifact_path(repository_root, "")
        .and_then(|p| crate::utility::code_graph::CodeGraph::from_json_file(&p))
        .unwrap_or_else(|| crate::utility::code_graph::build_graph(repository_root));

    let impacted = graph.impact_of(&touched);
    if impacted.is_empty() {
        return GateResult {
            name: "impact".to_string(),
            status: GateStatus::Pass,
            blocking: false,
            details: Some(format!("{} changed, no in-repo dependents", touched.len())),
        };
    }
    GateResult {
        name: "impact".to_string(),
        status: GateStatus::Pass,
        blocking: false,
        details: Some(format!(
            "{} changed, {} impacted: {}",
            touched.len(),
            impacted.len(),
            preview_touched_paths(&impacted)
        )),
    }
}

/// Whether the artifact's `target_file` names one of the files under review.
///
/// Suffix matching in both directions tolerates repo-relative vs absolute paths
/// and Windows separators, so a legitimate artifact is not rejected over path
/// formatting. An empty target never matches.
pub(crate) fn artifact_targets_a_touched_file(target_file: &str, touched: &[String]) -> bool {
    let target = target_file
        .replace('\\', "/")
        .trim()
        .trim_start_matches("./")
        .to_ascii_lowercase();
    if target.is_empty() {
        return false;
    }
    touched.iter().any(|path| {
        let candidate = path
            .replace('\\', "/")
            .trim_start_matches("./")
            .to_ascii_lowercase();
        candidate == target
            || candidate.ends_with(&format!("/{target}"))
            || target.ends_with(&format!("/{candidate}"))
    })
}

pub(crate) fn artifact_targets_all_touched_files(
    target_files: &[String],
    touched: &[String],
) -> bool {
    !touched.is_empty()
        && touched.iter().all(|touched_file| {
            target_files.iter().any(|target_file| {
                artifact_targets_a_touched_file(target_file, std::slice::from_ref(touched_file))
            })
        })
}

/// Build the prose-style gate result for a review surface. Lints added lines in
/// markdown/doc files for AI-slop vocabulary, em-dash, hype, first-person, and
/// chatty wording. Pre-existing prose is grandfathered (added lines only).
/// Blocking when a high-severity finding (AI-slop or em-dash) appears.
pub(crate) fn prose_style_gate(
    repository_root: &Path,
    base_ref: &str,
    surface_name: &str,
    scan_all: bool,
) -> GateResult {
    let findings = if scan_all {
        crate::comment_lint::lint_tracked_tree_prose(repository_root)
    } else if surface_name == "pre-commit" {
        crate::comment_lint::lint_working_prose(repository_root)
    } else {
        let base = base_ref.trim();
        let base = if base.is_empty() { "origin/main" } else { base };
        crate::comment_lint::lint_added_prose(repository_root, base)
    };
    let blocking = crate::comment_lint::has_blocking_prose(&findings);
    let status = if findings.is_empty() {
        GateStatus::Pass
    } else if blocking {
        GateStatus::Fail
    } else {
        GateStatus::Warn
    };
    let details = if findings.is_empty() {
        "no prose-style issues in added markdown/doc lines".to_string()
    } else {
        let shown: Vec<String> = findings
            .iter()
            .take(5)
            .map(|f| format!("{}:{} {}", f.file, f.line, f.message))
            .collect();
        format!(
            "{} prose-style issue(s) in markdown/doc: {}",
            findings.len(),
            shown.join("; ")
        )
    };
    GateResult {
        name: "prose_style".to_string(),
        status,
        blocking,
        details: Some(details),
    }
}

pub(crate) fn slop_gate(
    repository_root: &Path,
    base_ref: &str,
    surface_name: &str,
    scan_all: bool,
) -> GateResult {
    let findings = if scan_all {
        crate::slop_detector::lint_tracked_tree_slop(repository_root)
    } else if surface_name == "pre-commit" {
        crate::slop_detector::lint_working_slop(repository_root)
    } else {
        let mut findings = crate::slop_detector::lint_added_slop(repository_root, base_ref);
        findings.extend(crate::slop_detector::lint_working_slop(repository_root));
        findings.sort_by(|left, right| {
            left.file
                .cmp(&right.file)
                .then(left.line.cmp(&right.line))
                .then(left.pattern.cmp(right.pattern))
        });
        findings.dedup_by(|left, right| {
            left.file == right.file && left.line == right.line && left.pattern == right.pattern
        });
        findings
    };
    // Warn-level by design: heuristic findings must surface, never strand a
    // commit on a false positive.
    let status = if findings.is_empty() {
        GateStatus::Pass
    } else {
        GateStatus::Warn
    };
    let details = if findings.is_empty() {
        "no AI-slop patterns detected".to_string()
    } else {
        let shown: Vec<String> = findings
            .iter()
            .take(5)
            .map(|finding| {
                format!(
                    "{}:{} [{}] {}",
                    finding.file, finding.line, finding.pattern, finding.message
                )
            })
            .collect();
        format!("{} slop finding(s): {}", findings.len(), shown.join("; "))
    };
    GateResult {
        name: "slop_detector".to_string(),
        status,
        blocking: false,
        details: Some(details),
    }
}
