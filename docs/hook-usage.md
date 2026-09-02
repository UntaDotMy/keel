<!--
Purpose: Document the managed harness lifecycle hook contract for agents and operators.
Caller: README, AGENTS.md, and contributors looking for the hook usage and rerun handling rules.
Dependencies: ~/.claude/hooks.json layout, keel run wrapper, and keel rewrite.
Main Functions: Explain what the hook does, what it does not do, and how agents interact with transparent rewrite.
Side Effects: None; documentation only.
-->
# the harness Hook Usage

The managed hook set is installed at `~/.claude/settings.json` by the one-line installer and by `keel hook install`. It manages every supported the harness lifecycle event: `PreToolUse`, `PermissionRequest`, `PostToolUse`, `PreCompact`, `PostCompact`, `SessionStart`, `UserPromptSubmit`, and `Stop`. `PreToolUse` reroutes noisy Bash commands before output exists. Lifecycle hooks inject `additionalContext` reminders so the harness automatically sees the skill-routing, memory, flow, and review contract; `SessionStart` also creates or refreshes the workspace memory scope and `SYSTEM_MAP.md` when possible. This page is the agent-facing contract; `keel hook instructions` prints the same content (`--format markdown` is the default; `--format json` returns a structured payload).

## Token-saving rule

The goal is to prevent noisy raw command output from entering the harness context. Do not run a raw noisy command first and compact afterward; route through `keel run -- <command>` or rely on the hook's transparent rewrite before noisy output is produced.

## What the hook does

- Inspects supported Bash commands and transparently rewrites them via `toolInputOverride`.
- Wraps the original command in `keel run --` before it executes, preventing noisy output from entering context.
- Emits command-specific semantic reducers, high-signal error/warning context, and compacted head/tail summaries for noisy or long output while recording the full raw stream under the harness home raw-output recovery log.
- Records native savings analytics for `keel gain`, including command family and reducer dimensions.
- Injects `additionalContext` at `SessionStart`, `UserPromptSubmit`, `PostToolUse`, compaction, and closeout events so the harness is reminded to use skills, memory, Preserve Existing Flow, workflow proof, and review gates automatically.
- Refreshes the workspace memory scope and `SYSTEM_MAP.md` during `SessionStart` when the current working directory can be resolved.

## What the hook does not do

- Composite rewrite uses **Bash on Unix** and **PowerShell on Windows**. fish and zsh are not the rewrite runtime. Putting `keel` on PATH for those shells does not mean hook rewrite runs in those shells. That hole is separate from PATH honesty and is not this cycle.
- Existing-source ownership still requires Preserve Existing Flow and review gates (`keel review pre-pr`, `keel review gates check`). The Iron Law edit gate only proves research happened before the first edit class tool.
- It cannot force the harness to *think* well; it can deny edit-class tools until evidence exists, inject `additionalContext`, and feed-forward closeout requirements.
- Closeout gates (working-brief, review, and others) ride `PostToolBatch`. **Harder defaults:** `CLAUDE_SKILLS_BRIEF_GATE` and `CLAUDE_SKILLS_REVIEW_GATE` default to **`block`** mode (imperative feed-forward via `additionalContext` when code changed without a brief / reviewer marker; Block cap defaults to 3 fires per session, then advisory). Opt-down with `=nudge`, `=escalate`, or `=off`. They do not run the heavy gate commands themselves.

## Iron Law hard gate (PreToolUse) — how compliance is settled

Text reminders are ignoreable. **Settlement is a tool deny**, not hope:

- **Default `KEEL_IRON_LAW_GATE=strict`:** these tools are **denied** until the session used a **keel research tool** (`system_map` / `recall` / `context_brief` / `skill_*` / `code_search` or matching CLI):
  - edit-class (`Edit` / `Write` / `MultiEdit` / `apply_patch` / …)
  - **shell** (`Bash` / …) unless the command is itself a keel research command
  - **Agent / Task** fan-out
- **Still allowed** while blocked: `Read` / `Grep` / `Glob`, and shell `keel doctor` / `keel memory …` / etc.
- Plain `Read` alone does **not** clear STRICT mode.
- Marker: `~/.claude/state/iron-law-satisfied/<session>` — written only when research is observed (PostToolUse/observe), never on deny.
- Modes: unset/`strict` → strict; `balanced` → keel **or** host Read/Grep; `off` → disabled.
- **UserPromptSubmit** also **pushes** a bounded workspace map/brief dump every turn so the agent has keel data without choosing to call tools, plus an `ENFORCED THIS TURN` strip naming the deny.

## Transparent Rewrite Handling

When the hook intercepts a supported Bash command, it returns `permissionDecision: "allow"` with a `toolInputOverride` that wraps the command in `keel run -- ...`. The execution proceeds transparently — no manual rerun is needed.

Example: a raw `cargo test --workspace` is transparently rewritten to `keel run -- cargo test --workspace` and the compacted output is returned directly.

## Automatic lifecycle guidance

Lifecycle hooks return `hookSpecificOutput.additionalContext`. The harness adds that text to context as a system reminder at the hook firing point:

- `SessionStart`: injects the operating contract and refreshes the workspace memory scope/system map.
- `UserPromptSubmit`: **always** injects `FOLLOW THE IRON LAW. USE KEEL.` plus the research-first contract, keel MCP tools, and the memory loop (recall first, working brief, save durable learnings, learn loop, completion-gate / review before close).
- `PostToolUse`: reminds the harness to update proof state after tool results and save durable facts.
- `PreCompact`/`PostCompact`: preserve and restore workflow, memory, validation, and review continuity around compaction.
- `Stop`/`SubagentStop`/`SessionEnd`: enforce closeout reminders before final responses or session end.

## Compaction surface hierarchy

- **Level 1: Direct native wrapper:** `keel run -- <command>` is the reliable transparent surface. It owns command execution, explicit shell routing, command-specific reducers, high-signal extraction, head/tail compaction, raw-output recovery, and native savings analytics. Child processes are timeout-bounded and process-tree cleanup is reported instead of hanging.
- **Level 2: Rewrite helper:** `keel rewrite "<command>"` returns the resolved wrapper without executing it. It preserves direct argv where possible and routes composite syntax through the platform shell on Windows or Bash on Unix; explicit PowerShell/cmd/bash MCP scripts never change shells.
- **Level 3 — Hook guidance:** `keel hook install` registers the managed lifecycle hooks described above. The `PreToolUse` hook may transparently rewrite tool input via `toolInputOverride` (not a block-and-rerun).
- **Level 4 — Native install/update:** Use the installed Rust binary directly for update, verify, status, hooks, and compaction. Shell and PowerShell profile wrappers are not supported runtime entrypoints.

## Related commands

```bash
keel hook install        # Install managed lifecycle hooks in ~/.claude/hooks.json
keel hook uninstall      # Remove managed lifecycle hooks
keel hook list           # List installed hooks
keel hook show           # Show hooks.json content
keel hook instructions   # Print this contract (markdown by default)
keel hook instructions --format json   # Same contract as a structured payload
keel hook diagnose       # Verify installed executable, settings.json, and managed hook entries
keel hook diagnose --format json       # Same checks as a structured payload
```
