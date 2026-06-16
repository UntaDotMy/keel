//! Purpose: Single source of truth for Claude Code hook event metadata.
//! Caller: hooks module, runner managed-hook payload, doctor checks.
//! Dependencies: Claude Code settings.json hooks schema.
//! Main Functions: pre_tool_matcher,
//! settings_file_name, event_by_name, event_by_slug, HOOK_EVENTS.
//! Side Effects: None.
//!
//! Design note: every per-event property (slug, matcher, status text, whether the event
//! supports hookSpecificOutput) lives on a single `HookEvent` row. Helpers below are thin
//! facades over `HOOK_EVENTS` so adding a new official event means appending one row —
//! no dispatch arm, no status table, no test array to keep in sync. That is the lesson
//! from the `PostToolUseFailure` regression where the dispatch arm was added but the
//! shipped binary still rejected the slug.

/// Claude Code stores hook configuration inside `settings.json` under a top-level `hooks` key.
pub const SETTINGS_FILE_NAME: &str = "settings.json";

/// One row per official Claude Code hook event.
///
/// `matcher`:
///   - `""` for events that aren't tool-scoped (Stop, SessionStart, ...)
///   - `"Bash"` for the two we narrow to shell invocations so the rewriter doesn't
///     spawn on every tool call
///
/// `supports_hook_specific_output`: events the official Claude Code schema
/// documents as accepting `hookSpecificOutput.additionalContext`. Per
/// code.claude.com/docs/en/hooks that set is SessionStart, Setup,
/// SubagentStart, UserPromptSubmit, UserPromptExpansion, PreToolUse,
/// PostToolUse, PostToolUseFailure, and PostToolBatch. Other events must use
/// top-level fields (`systemMessage`, `decision`, etc). Keeping the flag on
/// the row prevents `hook_lifecycle` from re-stating the rule in a parallel
/// `matches!`.
///
/// `installs_in_settings`: whether `claude-skills hook install` should write
/// a stanza for this event into `settings.json`. The dispatch table still
/// recognises every official event so ad-hoc invocations like
/// `claude-skills hook file-changed` work, but a stanza in settings.json
/// only makes sense when we have a meaningful default. FileChanged is the
/// known exception — per the official docs the matcher *is* the watch list,
/// so installing with `matcher: ""` produces a stanza Claude Code never
/// fires. We skip those rows on install rather than ship a dead stanza.
pub struct HookEvent {
    pub name: &'static str,
    pub slug: &'static str,
    pub matcher: &'static str,
    pub status: &'static str,
    pub supports_hook_specific_output: bool,
    pub installs_in_settings: bool,
}

/// Canonical Claude Code hook events. Order is stable so rendered settings.json entries
/// remain deterministic across installs. Matches the spec at code.claude.com/docs/en/hooks.
pub const HOOK_EVENTS: &[HookEvent] = &[
    HookEvent {
        name: "PreToolUse",
        slug: "pre-tool-use",
        matcher: "Bash",
        status: "Transparently rewriting noisy commands via claude-skills run",
        supports_hook_specific_output: true,
        installs_in_settings: true,
    },
    HookEvent {
        // PostToolUse fires after every tool call, not just Bash. Two
        // responsibilities depend on that breadth:
        //   1. `tool_timings::record_tool_timing` records `duration_ms` for
        //      every tool — Edit/Read/Write/Grep are not Bash and we want them
        //      sampled too so the per-session row count matches reality and the
        //      compression-discipline nudge fires when context fills with
        //      file reads, not only with shell commands.
        //   2. The edit-class counter that drives the every-N-edits SYSTEM_MAP
        //      refresh only ever fires when Edit/Write/MultiEdit/NotebookEdit
        //      run. With matcher="Bash", the handler was unreachable for those
        //      events — the counter directory never existed on disk and the
        //      auto-refresh path was dead code. The handler itself filters
        //      edit-class tools at runtime via `is_edit_class_tool`, so an
        //      empty matcher is the matcher that actually delivers the
        //      documented behavior.
        // Cost: spawning claude-skills on every tool call is microseconds.
        // Telemetry writes are append-only JSONL, daily-rotated, and pruned at
        // SessionEnd, so growth stays bounded.
        name: "PostToolUse",
        slug: "post-tool-use",
        matcher: "",
        status: "Recording post-tool lifecycle",
        supports_hook_specific_output: true,
        installs_in_settings: true,
    },
    HookEvent {
        name: "PostToolUseFailure",
        slug: "post-tool-use-failure",
        matcher: "",
        status: "Recording tool failure for routing and recovery",
        supports_hook_specific_output: true,
        installs_in_settings: true,
    },
    HookEvent {
        name: "PostToolBatch",
        slug: "post-tool-batch",
        matcher: "",
        status: "Injecting reviewer-on-close reminder",
        supports_hook_specific_output: true,
        installs_in_settings: true,
    },
    HookEvent {
        name: "PermissionRequest",
        slug: "permission-request",
        matcher: "",
        status: "Auto-approving claude-skills commands",
        supports_hook_specific_output: true,
        installs_in_settings: true,
    },
    HookEvent {
        name: "PermissionDenied",
        slug: "permission-denied",
        matcher: "",
        status: "Signaling retry on transient permission denial",
        supports_hook_specific_output: true,
        installs_in_settings: true,
    },
    HookEvent {
        name: "Notification",
        slug: "notification",
        matcher: "",
        status: "Emitting terminal bell on notification",
        supports_hook_specific_output: false,
        installs_in_settings: true,
    },
    HookEvent {
        name: "UserPromptSubmit",
        slug: "user-prompt-submit",
        matcher: "",
        status: "Routing prompt to the right skill",
        supports_hook_specific_output: true,
        installs_in_settings: true,
    },
    HookEvent {
        name: "UserPromptExpansion",
        slug: "user-prompt-expansion",
        matcher: "",
        status: "Prompt-expansion hook (reserved)",
        supports_hook_specific_output: true,
        installs_in_settings: true,
    },
    HookEvent {
        // Per code.claude.com/docs/en/hooks Stop supports both
        // hookSpecificOutput.additionalContext and decision:"block" (which
        // prevents stopping and continues the conversation). The handler
        // must still always exit 0 — a non-zero exit triggers a stop-loop
        // cascade. Context injection and decision control are JSON-level.
        name: "Stop",
        slug: "stop",
        matcher: "",
        status: "Injecting closeout context",
        supports_hook_specific_output: true,
        installs_in_settings: true,
    },
    HookEvent {
        name: "StopFailure",
        slug: "stop-failure",
        matcher: "",
        status: "Stop-failure hook (reserved; dispatch is currently a no-op)",
        supports_hook_specific_output: false,
        installs_in_settings: true,
    },
    HookEvent {
        name: "SubagentStart",
        slug: "subagent-start",
        matcher: "",
        status: "Injecting iron law context into spawned subagent",
        supports_hook_specific_output: true,
        installs_in_settings: true,
    },
    HookEvent {
        // Per code.claude.com/docs/en/hooks SubagentStop supports both
        // hookSpecificOutput.additionalContext and decision:"block".
        name: "SubagentStop",
        slug: "subagent-stop",
        matcher: "",
        status: "Closing subagent lifecycle",
        supports_hook_specific_output: true,
        installs_in_settings: true,
    },
    HookEvent {
        name: "TaskCreated",
        slug: "task-created",
        matcher: "",
        status: "Task lifecycle hook (reserved; dispatch is currently a no-op)",
        supports_hook_specific_output: false,
        installs_in_settings: true,
    },
    HookEvent {
        name: "TaskCompleted",
        slug: "task-completed",
        matcher: "",
        status: "Task lifecycle hook (reserved; dispatch is currently a no-op)",
        supports_hook_specific_output: false,
        installs_in_settings: true,
    },
    HookEvent {
        name: "TeammateIdle",
        slug: "teammate-idle",
        matcher: "",
        status: "Teammate-idle hook (reserved; dispatch is currently a no-op)",
        supports_hook_specific_output: false,
        installs_in_settings: true,
    },
    HookEvent {
        name: "WorktreeCreate",
        slug: "worktree-create",
        matcher: "",
        status: "Worktree lifecycle hook (reserved; dispatch is currently a no-op)",
        supports_hook_specific_output: false,
        installs_in_settings: true,
    },
    HookEvent {
        name: "WorktreeRemove",
        slug: "worktree-remove",
        matcher: "",
        status: "Worktree lifecycle hook (reserved; dispatch is currently a no-op)",
        supports_hook_specific_output: false,
        installs_in_settings: true,
    },
    HookEvent {
        name: "CwdChanged",
        slug: "cwd-changed",
        matcher: "",
        status: "Refreshing system map for new working directory",
        supports_hook_specific_output: false,
        installs_in_settings: true,
    },
    HookEvent {
        name: "PreCompact",
        slug: "pre-compact",
        matcher: "",
        status: "Checkpointing before compaction",
        supports_hook_specific_output: false,
        installs_in_settings: true,
    },
    HookEvent {
        name: "PostCompact",
        slug: "post-compact",
        matcher: "",
        status: "Resuming after compaction",
        supports_hook_specific_output: false,
        installs_in_settings: true,
    },
    HookEvent {
        name: "SessionStart",
        slug: "session-start",
        matcher: "",
        status: "Preparing native session state",
        supports_hook_specific_output: true,
        installs_in_settings: true,
    },
    HookEvent {
        name: "SessionEnd",
        slug: "session-end",
        matcher: "",
        status: "Recording session end",
        supports_hook_specific_output: false,
        installs_in_settings: true,
    },
    HookEvent {
        name: "Setup",
        slug: "setup",
        matcher: "",
        status: "Setup hook (reserved; dispatch is currently a no-op)",
        supports_hook_specific_output: true,
        installs_in_settings: true,
    },
    HookEvent {
        name: "InstructionsLoaded",
        slug: "instructions-loaded",
        matcher: "",
        status: "Instructions-loaded hook (reserved; dispatch is currently a no-op)",
        supports_hook_specific_output: false,
        installs_in_settings: true,
    },
    HookEvent {
        name: "ConfigChange",
        slug: "config-change",
        matcher: "",
        status: "Config-change hook (reserved; dispatch is currently a no-op)",
        supports_hook_specific_output: false,
        installs_in_settings: true,
    },
    // FileChanged: per code.claude.com/docs/en/hooks the `matcher` value
    // doubles as the watch list — segments split on `|` are registered as
    // literal filenames in the working directory. With `matcher: ""` no
    // file is watched, so installing this stanza ships dead config Claude
    // Code never fires. We keep the row so dispatch still works for ad-hoc
    // invocations like `claude-skills hook file-changed`, but skip it on
    // install until a per-repo watch list is meaningful.
    HookEvent {
        name: "FileChanged",
        slug: "file-changed",
        matcher: "",
        status: "File-change hook (reserved; dispatch is currently a no-op)",
        supports_hook_specific_output: false,
        installs_in_settings: false,
    },
    HookEvent {
        name: "Elicitation",
        slug: "elicitation",
        matcher: "",
        status: "Elicitation-prompt hook (reserved; dispatch is currently a no-op)",
        supports_hook_specific_output: false,
        installs_in_settings: true,
    },
    HookEvent {
        name: "ElicitationResult",
        slug: "elicitation-result",
        matcher: "",
        status: "Elicitation-result hook (reserved; dispatch is currently a no-op)",
        supports_hook_specific_output: false,
        installs_in_settings: true,
    },
    // MessageDisplay fires while assistant message text is displayed. Per the
    // official docs it has no matcher (always fires on every occurrence),
    // cannot block, and emits `hookSpecificOutput.displayContent` to rewrite
    // the on-screen text — NOT `additionalContext`, so `supports_hook_specific_output`
    // stays false (that flag gates the additionalContext payload shape in
    // `render_lifecycle_payload`). We keep the row so dispatch works for ad-hoc
    // `claude-skills hook message-display`, but skip it on install: a handler
    // with no displayContent is a no-op, and one that sets displayContent would
    // silently rewrite what the user sees on every message — not a safe default.
    HookEvent {
        name: "MessageDisplay",
        slug: "message-display",
        matcher: "",
        status: "Message-display hook (reserved; dispatch is currently a no-op)",
        supports_hook_specific_output: false,
        installs_in_settings: false,
    },
];

/// Find a row by Claude Code's PascalCase event name (`"PreToolUse"`).
pub fn event_by_name(name: &str) -> Option<&'static HookEvent> {
    HOOK_EVENTS.iter().find(|event| event.name == name)
}

/// Find a row by the kebab-case `claude-skills hook <slug>` subcommand.
pub fn event_by_slug(slug: &str) -> Option<&'static HookEvent> {
    HOOK_EVENTS.iter().find(|event| event.slug == slug)
}

/// PreToolUse matcher (`Bash`) so rewrites only fire on shell invocations.
pub fn pre_tool_matcher() -> &'static str {
    event_by_name("PreToolUse")
        .map(|event| event.matcher)
        .unwrap_or("Bash")
}

/// PostToolUse matcher (`""`) so the post-tool hook fires for every tool, not
/// just Bash. The handler gates edit-counter work on `is_edit_class_tool`, so
/// the empty matcher is what delivers the documented behavior.
#[cfg(test)]
pub fn post_tool_matcher() -> &'static str {
    event_by_name("PostToolUse")
        .map(|event| event.matcher)
        .unwrap_or("")
}

/// Map a Claude Code hook event name to the `claude-skills hook <subcommand>` kebab-case slug.
/// Kept as a facade over the table for tests and external callers; in-tree code reads
/// `event_by_name(name)?.slug` directly.
#[cfg(test)]
pub fn lifecycle_subcommand(event: &str) -> &'static str {
    event_by_name(event)
        .map(|row| row.slug)
        .unwrap_or("unknown")
}

/// Human-readable status message surfaced in Claude Code's hook feedback UI.
#[cfg(test)]
pub fn status_message(event: &str) -> &'static str {
    event_by_name(event)
        .map(|row| row.status)
        .unwrap_or("Native lifecycle hook")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Windows regression that motivated this refactor: a stale binary rejected
    /// `post-tool-use-failure` because the dispatch arm and the canonical list were
    /// kept as parallel arrays. With one table, the canonical list IS the dispatch
    /// list — divergence is structurally impossible.
    #[test]
    fn every_official_event_is_present() {
        for expected in [
            "PreToolUse",
            "PostToolUse",
            "PostToolUseFailure",
            "PostToolBatch",
            "PermissionRequest",
            "PermissionDenied",
            "Notification",
            "UserPromptSubmit",
            "UserPromptExpansion",
            "Stop",
            "StopFailure",
            "SubagentStart",
            "SubagentStop",
            "TaskCreated",
            "TaskCompleted",
            "TeammateIdle",
            "WorktreeCreate",
            "WorktreeRemove",
            "CwdChanged",
            "PreCompact",
            "PostCompact",
            "SessionStart",
            "SessionEnd",
            "Setup",
            "InstructionsLoaded",
            "ConfigChange",
            "FileChanged",
            "Elicitation",
            "ElicitationResult",
        ] {
            assert!(
                event_by_name(expected).is_some(),
                "missing canonical event {expected}"
            );
        }
    }

    #[test]
    fn slugs_are_kebab_lowercase_and_unique() {
        let mut seen = std::collections::BTreeSet::new();
        for event in HOOK_EVENTS {
            assert!(!event.slug.is_empty(), "{} has empty slug", event.name);
            assert!(
                event
                    .slug
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c == '-'),
                "{} has non-kebab slug `{}`",
                event.name,
                event.slug
            );
            assert!(
                seen.insert(event.slug),
                "slug `{}` duplicated across events",
                event.slug
            );
        }
    }

    #[test]
    fn names_are_pascal_case_and_unique() {
        let mut seen = std::collections::BTreeSet::new();
        for event in HOOK_EVENTS {
            assert!(!event.name.is_empty(), "empty event name");
            let first = event.name.chars().next().unwrap();
            assert!(
                first.is_ascii_uppercase(),
                "{} should be PascalCase",
                event.name
            );
            assert!(
                seen.insert(event.name),
                "event name `{}` duplicated",
                event.name
            );
        }
    }

    #[test]
    fn name_and_slug_lookups_are_inverses() {
        for event in HOOK_EVENTS {
            assert_eq!(
                event_by_name(event.name).map(|row| row.slug),
                Some(event.slug)
            );
            assert_eq!(
                event_by_slug(event.slug).map(|row| row.name),
                Some(event.name)
            );
        }
    }

    #[test]
    fn matcher_is_bash_only_for_tool_scoped_events() {
        for event in HOOK_EVENTS {
            match event.matcher {
                "" | "Bash" => {}
                other => panic!("{} has unexpected matcher `{}`", event.name, other),
            }
        }
        assert_eq!(event_by_name("PreToolUse").unwrap().matcher, "Bash");
        assert_eq!(event_by_name("PostToolUse").unwrap().matcher, "");
        assert_eq!(event_by_name("Stop").unwrap().matcher, "");
    }

    #[test]
    fn only_known_events_opt_out_of_install() {
        // Pins the install-allowlist invariant. Two rows opt out of
        // settings.json install, each for a distinct documented reason:
        //   - FileChanged: its matcher value IS the per-repo watch list per the
        //     official docs, so an empty matcher ships dead config.
        //   - MessageDisplay: it has no matcher and emits `displayContent` to
        //     rewrite on-screen text on every assistant message. A no-op handler
        //     is pointless and a displayContent handler would silently rewrite
        //     what the user sees, so it is opt-in, not auto-installed.
        // If you intentionally flip another row's `installs_in_settings` to false,
        // update this test deliberately so the regression is reviewed.
        let opt_outs: Vec<&'static str> = HOOK_EVENTS
            .iter()
            .filter(|event| !event.installs_in_settings)
            .map(|event| event.name)
            .collect();
        assert_eq!(
            opt_outs,
            ["FileChanged", "MessageDisplay"],
            "unexpected installs_in_settings=false rows; update this test if intentional"
        );
    }

    #[test]
    fn hook_specific_output_flag_matches_claude_code_schema() {
        // Per code.claude.com/docs/en/hooks, these events accept
        // `hookSpecificOutput.additionalContext`. Everything else must use
        // top-level fields (`systemMessage`, `decision`, etc).
        let allowed = [
            "SessionStart",
            "Setup",
            "SubagentStart",
            "SubagentStop",
            "UserPromptSubmit",
            "UserPromptExpansion",
            "PreToolUse",
            "PostToolUse",
            "PostToolUseFailure",
            "PostToolBatch",
            "Stop",
            "PermissionRequest",
            "PermissionDenied",
        ];
        for event in HOOK_EVENTS {
            let expected = allowed.contains(&event.name);
            assert_eq!(
                event.supports_hook_specific_output, expected,
                "{} hookSpecificOutput flag is wrong",
                event.name
            );
        }
    }

    #[test]
    fn legacy_facades_still_resolve() {
        // hook_lifecycle.rs and manager/doctor.rs still call these by name. Keep
        // them working so the dedup in Phase A doesn't ripple into every consumer.
        assert_eq!(lifecycle_subcommand("PreToolUse"), "pre-tool-use");
        assert_eq!(
            lifecycle_subcommand("PostToolUseFailure"),
            "post-tool-use-failure"
        );
        assert_eq!(lifecycle_subcommand("ZZZ"), "unknown");
        assert_eq!(status_message("Stop"), "Injecting closeout context");
        assert_eq!(status_message("ZZZ"), "Native lifecycle hook");
        assert_eq!(pre_tool_matcher(), "Bash");
        assert_eq!(post_tool_matcher(), "");
    }

    #[test]
    fn settings_file_name_is_claude_code_convention() {
        assert_eq!(SETTINGS_FILE_NAME, "settings.json");
    }
}
