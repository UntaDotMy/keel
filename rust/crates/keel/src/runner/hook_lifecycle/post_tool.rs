//! Hook lifecycle post_tool responsibility split.

use super::*;

pub(super) fn run_hook_post_tool_use(standard_error: &mut dyn Write) -> u8 {
    let input_text = match std::io::read_to_string(std::io::stdin()) {
        Ok(text) => text,

        // PostToolUse must never fail loudly: a non-zero exit teaches the harness
        Err(error) => {
            let _ = writeln!(
                standard_error,
                "keel post-tool-use: unable to read hook input: {error}"
            );

            return 0;
        }
    };

    let input: JsonDocument = match serde_json::from_str(&input_text) {
        Ok(value) => value,

        Err(error) => {
            let _ = writeln!(
                standard_error,
                "keel post-tool-use: unable to decode hook input: {error}"
            );

            return 0;
        }
    };

    let tool_name = input
        .get("tool_name")
        .and_then(JsonDocument::as_str)
        .unwrap_or_default();

    // Record `duration_ms` for every tool, not just edit-class ones. CC 2.1.119
    if let Err(error) = tool_timings::record_tool_timing("PostToolUse", &input) {
        let _ = writeln!(
            standard_error,
            "keel post-tool-use: tool-timings record failed: {error}"
        );
    }

    // Capture a behavioral observation for the autonomous learning loop. This
    match observation::record_observation(&input) {
        Ok(true) => {
            if let Ok(claude_home) = resolve_claude_home("") {
                learning::run_continuous_learning_if_due(&claude_home, standard_error);
            }
        }
        Ok(false) => {}
        Err(error) => {
            let _ = writeln!(
                standard_error,
                "keel post-tool-use: observation record failed: {error}"
            );
        }
    }

    // Iron Law evidence: mark session satisfied when a keel research tool
    // (or balanced-mode host research tool) completes successfully.
    maybe_mark_iron_law_from_tool_event(&input);

    if !is_edit_class_tool(tool_name) {
        return 0;
    }

    // Comment-style lint: catch long/chatty comments at write time, not just review.
    // Advisory only (env-gated, fail-open). See run_post_tool_comment_lint.
    if let Some(nudge) = run_post_tool_comment_lint(tool_name, &input) {
        let _ = writeln!(standard_error, "{nudge}");
        let payload = serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "PostToolUse",
                "additionalContext": nudge,
            },
            "suppressOutput": true,
        });
        if let Ok(rendered) = serde_json::to_string(&payload) {
            let _ = writeln!(std::io::stdout(), "{rendered}");
        }
    }

    // Graph context: after an edit, surface the blast radius (which files import
    // the edited file) so the next action is scoped by real edges, not grep.
    if let Some(context) = run_post_tool_graph_context(tool_name, &input) {
        let payload = serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "PostToolUse",
                "additionalContext": context,
            },
            "suppressOutput": true,
        });
        if let Ok(rendered) = serde_json::to_string(&payload) {
            let _ = writeln!(std::io::stdout(), "{rendered}");
        }
    }

    let threshold = system_map_refresh_threshold();

    if threshold == 0 {
        return 0;
    }

    let Some(counter_path) = system_map_edit_counter_path() else {
        return 0;
    };

    let next_count = match increment_counter_file(&counter_path) {
        Ok(value) => value,
        Err(error) => {
            let _ = writeln!(
                standard_error,
                "keel post-tool-use: counter update failed: {error}"
            );

            return 0;
        }
    };

    if next_count >= threshold {
        let _ = refresh_memory_scope_for_current_directory(standard_error);
        let _ = reset_counter_file(&counter_path);
    }

    0
}

/// Advisory comment-style lint for PostToolUse. Returns a nudge string when the
/// just-edited file introduced a blocking comment finding (over-length impl
/// comment, em/en dash, chatty/first-person wording), `None` otherwise.
///
/// Design constraints (it runs on every Edit/Write, a hot path):
/// - **Env-gated**: `CLAUDE_SKILLS_COMMENT_LINT_GATE=off` disables; anything
///   else (incl. unset) leaves it advisory-on. Matches the gate-mode convention.
/// - **Fail-open**: any error (no git repo, no cwd, parse failure) → `None`.
///   A comment lint must never break the PostToolUse hook.
/// - **Natural dedup**: the nudge stops firing once the comment is fixed
///   (findings clear from the working diff), so a repeated nudge means the
///   comment is still wrong, not spam.
/// - **Scoped to the edited file**: scans the working diff, filters findings to
///   the file just written so unrelated pre-existing comments are not flagged.
pub(super) fn run_post_tool_comment_lint(tool_name: &str, input: &JsonDocument) -> Option<String> {
    if std::env::var("CLAUDE_SKILLS_COMMENT_LINT_GATE").as_deref() == Ok("off") {
        return None;
    }
    // Only Edit/Write carry a file path the code can scope to. Other edit-class tools
    // (apply_patch, str_replace) have no single file, so skip them to avoid noise.
    let edited_path = if matches!(tool_name, "Edit" | "Write" | "MultiEdit") {
        input
            .get("tool_input")
            .and_then(|ti| ti.get("file_path"))
            .and_then(JsonDocument::as_str)
            .unwrap_or_default()
    } else {
        return None;
    };
    if edited_path.is_empty() {
        return None;
    }
    let repo_root = std::env::current_dir().ok()?;
    let findings = crate::comment_lint::lint_working_comments(&repo_root);
    if findings.is_empty() {
        return None;
    }
    // Scope to the file just edited (path may be absolute or repo-relative).
    let target = std::path::Path::new(edited_path);
    let target_str = target.to_string_lossy();
    let scoped: Vec<&crate::comment_lint::FileCommentFinding> = findings
        .iter()
        .filter(|f| target_str.ends_with(f.file.as_str()) || f.file.ends_with(target_str.as_ref()))
        .collect();
    if scoped.is_empty() {
        return None;
    }
    let blocking = crate::comment_lint::has_blocking(&findings);
    if !blocking {
        return None;
    }
    let rendered = crate::comment_lint::format_findings(
        &scoped.iter().map(|f| (*f).clone()).collect::<Vec<_>>(),
    );
    Some(format!(
        "keel comment-lint: blocking comment finding(s) in this edit — fix before moving on:\n{rendered}\nAdvisory; set CLAUDE_SKILLS_COMMENT_LINT_GATE=off to silence."
    ))
}

/// Graph context for PostToolUse: after an edit, report the edited file's blast
/// radius (the in-repo files that import it) so the agent's next step is scoped
/// by real dependency edges instead of a grep loop.
///
/// Design constraints (runs on every edit, a hot path):
/// - Env-gated: `CLAUDE_SKILLS_GRAPH_CONTEXT_GATE=off` disables; on by default.
/// - Fail-open: any error (no graph artifact, unreadable JSON, no cwd) -> `None`.
///   A context nudge must never break the PostToolUse hook.
/// - Cheap: reads the cached per-workspace code-graph artifact; it never builds
///   the graph here (building walks the whole tree, too slow for a hot path). If
///   no artifact exists yet, the nudge says how to build it once.
/// - Bounded: caps the dependent list so a wide blast radius cannot flood context.
pub(super) fn run_post_tool_graph_context(tool_name: &str, input: &JsonDocument) -> Option<String> {
    if std::env::var("CLAUDE_SKILLS_GRAPH_CONTEXT_GATE").as_deref() == Ok("off") {
        return None;
    }
    // Only Edit/Write/MultiEdit carry a single file path.
    let edited_path = if matches!(tool_name, "Edit" | "Write" | "MultiEdit") {
        input
            .get("tool_input")
            .and_then(|ti| ti.get("file_path"))
            .and_then(JsonDocument::as_str)
            .unwrap_or_default()
    } else {
        return None;
    };
    if edited_path.is_empty() {
        return None;
    }
    let repo_root = std::env::current_dir().ok()?;
    let artifact = crate::utility::code_graph::cached_artifact_path(&repo_root, "")?;
    // Staleness guard: a graph older than the file just edited is stale for this
    // edit and would mislead, so stay silent rather than inject wrong dependents.
    let artifact_mtime = file_mtime_ms(&artifact)?;
    let edited_mtime = file_mtime_ms(std::path::Path::new(edited_path))?;
    if artifact_mtime < edited_mtime {
        return None;
    }
    let graph = crate::utility::code_graph::CodeGraph::from_json_file(&artifact)?;
    // Normalize the edited path to the graph's workspace-relative forward-slash id.
    let relative = std::path::Path::new(edited_path)
        .strip_prefix(&repo_root)
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| edited_path.replace('\\', "/"));
    let impacted = graph.impact_of(std::slice::from_ref(&relative));
    if impacted.is_empty() {
        return None;
    }
    const MAX_LISTED: usize = 8;
    let listed: Vec<&str> = impacted
        .iter()
        .take(MAX_LISTED)
        .map(String::as_str)
        .collect();
    let more = impacted.len().saturating_sub(MAX_LISTED);
    let mut line = format!(
        "keel graph: `{relative}` is imported by {} file(s): {}",
        impacted.len(),
        listed.join(", ")
    );
    if more > 0 {
        line.push_str(&format!(" (+{more} more)"));
    }
    line.push_str(". Verify these still compile/behave before closeout.");
    Some(line)
}

/// PostToolUseFailure handler.
///
/// PostToolUseFailure (CC 2.1.119+) carries the same `duration_ms` field as
/// PostToolUse so we can see how long failing tool calls took before they
/// errored. The handler reads stdin, records the timing alongside the
/// success entries, and returns 0. No edit-counter touch — a failing tool
/// call did not change files, so nudging the SYSTEM_MAP refresh would be
/// noise.
///
/// Like PostToolUse, this handler must never fail the hook: any I/O or
/// parse error is logged to stderr and swallowed.
pub(super) fn run_hook_post_tool_use_failure(standard_error: &mut dyn Write) -> u8 {
    let input_text = match std::io::read_to_string(std::io::stdin()) {
        Ok(text) => text,
        Err(error) => {
            let _ = writeln!(
                standard_error,
                "keel post-tool-use-failure: unable to read hook input: {error}"
            );

            return 0;
        }
    };

    let input: JsonDocument = match serde_json::from_str(&input_text) {
        Ok(value) => value,
        Err(error) => {
            let _ = writeln!(
                standard_error,
                "keel post-tool-use-failure: unable to decode hook input: {error}"
            );

            return 0;
        }
    };

    if let Err(error) = tool_timings::record_tool_timing("PostToolUseFailure", &input) {
        let _ = writeln!(
            standard_error,
            "keel post-tool-use-failure: tool-timings record failed: {error}"
        );
    }

    // Capture the FAILURE as its own behavioral observation. A failing tool call
    match observation::record_failure_observation(&input) {
        Ok(true) => {
            if let Ok(claude_home) = resolve_claude_home("") {
                learning::run_continuous_learning_if_due(&claude_home, standard_error);
            }
        }
        Ok(false) => {}
        Err(error) => {
            let _ = writeln!(
                standard_error,
                "keel post-tool-use-failure: observation record failed: {error}"
            );
        }
    }

    0
}
