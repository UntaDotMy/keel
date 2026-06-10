<!--
Purpose: Document the managed Claude Code lifecycle hook contract for agents and operators.
Caller: README, AGENTS.md, and contributors looking for the hook usage and rerun handling rules.
Dependencies: ~/.claude/hooks.json layout, claude-skills run wrapper, and claude-skills rewrite.
Main Functions: Explain what the hook does, what it does not do, and how agents interact with transparent rewrite.
Side Effects: None; documentation only.
-->
# Claude Code Hook Usage

The managed hook set is installed at `~/.claude/settings.json` by the one-line installer and by `claude-skills hook install`. It manages every supported Claude Code lifecycle event: `PreToolUse`, `PermissionRequest`, `PostToolUse`, `PreCompact`, `PostCompact`, `SessionStart`, `UserPromptSubmit`, and `Stop`. `PreToolUse` reroutes noisy Bash commands before output exists. Lifecycle hooks inject `additionalContext` reminders so Claude Code automatically sees the skill-routing, memory, flow, and review contract; `SessionStart` also creates or refreshes the workspace memory scope and `SYSTEM_MAP.md` when possible. This page is the agent-facing contract; `claude-skills hook instructions` prints the same content (`--format markdown` is the default; `--format json` returns a structured payload).

## Token-saving rule

The goal is to prevent noisy raw command output from entering Claude Code context. Do not run a raw noisy command first and compact afterward; route through `claude-skills run -- <command>` or rely on the hook's transparent rewrite before noisy output is produced.

## What the hook does

- Inspects supported Bash commands and transparently rewrites them via `toolInputOverride`.
- Wraps the original command in `claude-skills run --` before it executes, preventing noisy output from entering context.
- Emits command-specific semantic reducers, high-signal error/warning context, and compacted head/tail summaries for noisy or long output while recording the full raw stream under the Claude Code home raw-output recovery log.
- Records native savings analytics for `claude-skills gain`, including command family and reducer dimensions.
- Injects `additionalContext` at `SessionStart`, `UserPromptSubmit`, `PostToolUse`, compaction, and closeout events so Claude Code is reminded to use skills, memory, Preserve Existing Flow, workflow proof, and review gates automatically.
- Refreshes the workspace memory scope and `SYSTEM_MAP.md` during `SessionStart` when the current working directory can be resolved.

## What the hook does not do

- It does not cover `apply_patch` or other file-edit tool surfaces. Existing-source edits stay governed by Preserve Existing Flow and review gates (`~/.claude/memories/workspaces/<workspace-slug>/flow/flow-check.json`, `claude-skills review pre-pr`, `claude-skills review gates check`).
- It cannot force Claude Code to execute a tool; it injects authoritative `additionalContext` reminders and refreshes lightweight memory artifacts, while Claude Code still owns reasoning and tool selection.
- It does not run expensive review gates automatically on every stop event; closeout hooks remind Claude Code to run the gates before claiming completion. Two PostToolBatch gates are on by default, each fires at most once per session, and each defaults to a **non-blocking nudge** (the gate's reminder is injected via `hookSpecificOutput.additionalContext`, so the agent is told to act but the turn is never halted) — the working-brief gate (`CLAUDE_SKILLS_BRIEF_GATE`, nudges once when a session edits code with no working brief written this session) and the review gate (`CLAUDE_SKILLS_REVIEW_GATE`, nudges once when a session edits code with no reviewer pass since the last edit). Each env var takes three values: unset/anything else → the non-blocking nudge (default); `block` → restore the opt-in hard stop (`decision: "block"`, refuses closeout until the artifact exists); `off` (or `…_MAX_BLOCKS=0`) → disabled. Whichever mode is selected, each gate is bounded to at most `…_MAX_BLOCKS` fires per session (default 1) by a monotonic counter so it can neither loop nor spam, and it fails open to the generic advisory on any telemetry error. They do not run the heavy gate commands themselves; they surface (nudge) or enforce (block) the requirement that the artifact — a brief, a review marker — exist.

## Transparent Rewrite Handling

When the hook intercepts a supported Bash command, it returns `permissionDecision: "allow"` with a `toolInputOverride` that wraps the command in `claude-skills run -- ...`. The execution proceeds transparently — no manual rerun is needed.

Example: a raw `cargo test --workspace` is transparently rewritten to `claude-skills run -- cargo test --workspace` and the compacted output is returned directly.

## Automatic lifecycle guidance

Lifecycle hooks return `hookSpecificOutput.additionalContext`. Claude Code adds that text to context as a system reminder at the hook firing point:

- `SessionStart`: injects the operating contract and refreshes the workspace memory scope/system map.
- `UserPromptSubmit`: reminds Claude Code to route work through skills, consult/save memory, maintain workflow proof, and run review closeout.
- `PostToolUse`: reminds Claude Code to update proof state after tool results and save durable facts.
- `PreCompact`/`PostCompact`: preserve and restore workflow, memory, validation, and review continuity around compaction.
- `Stop`/`SubagentStop`/`SessionEnd`: enforce closeout reminders before final responses or session end.

## Compaction surface hierarchy

- **Level 1 — Direct native wrapper:** `claude-skills run -- <command>` is the most reliable transparent surface; it owns command execution, shell-aware parser/rewrite support, command-specific reducers, high-signal extraction, head/tail compaction, raw-output recovery, and native savings analytics in one step. Add `--stream` before `--` when bounded live progress is more important than keeping the terminal silent until final compaction.
- **Level 2 — Rewrite helper:** `claude-skills rewrite "<command>"` returns the resolved wrapper without executing it. It recognizes common shell wrappers, environment-prefix commands, and pipelines; shell syntax is rerouted through `bash -lc` so the wrapper executes the intended command rather than a partial token.
- **Level 3 — Hook guidance:** `claude-skills hook install` registers the managed lifecycle hooks described above. The `PreToolUse` hook may transparently rewrite tool input via `toolInputOverride` (not a block-and-rerun).
- **Level 4 — Native install/update:** Use the installed Rust binary directly for update, verify, status, hooks, and compaction. Shell and PowerShell profile wrappers are not supported runtime entrypoints.

## Related commands

```bash
claude-skills hook install        # Install managed lifecycle hooks in ~/.claude/hooks.json
claude-skills hook uninstall      # Remove managed lifecycle hooks
claude-skills hook list           # List installed hooks
claude-skills hook show           # Show hooks.json content
claude-skills hook instructions   # Print this contract (markdown by default)
claude-skills hook instructions --format json   # Same contract as a structured payload
claude-skills hook diagnose       # Verify installed executable, settings.json, and managed hook entries
claude-skills hook diagnose --format json       # Same checks as a structured payload
```
