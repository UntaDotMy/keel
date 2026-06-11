<!--
Purpose: Capture native-command routing, hook transparent rewrite, and token-optimization rules previously inline in AGENTS.md.
Caller: AGENTS.md when shell-command routing, hook semantics, or compaction surfaces are in scope.
Dependencies: claude-skills run, claude-skills rewrite, claude-skills hook install, claude-skills code-search, claude-skills flow, claude-skills review, claude-skills git-workflow.
Main Functions: Define when to route through native commands, how the hook rewrite works, and what compaction surfaces exist.
Side Effects: None — this file is informational.
-->
# Native Command Routing, Hook Rewrite, and Token Optimization

## Native Command Routing — Must Follow First

When a native `claude-skills` command owns the job, use it instead of recreating the behavior with raw shell, generic search, or ad hoc instructions.

**Token-saving rule:** the goal is to prevent noisy raw command output from entering Claude Code context. Do not run a raw noisy command first and compact afterward; route through `claude-skills run -- <command>` or rely on the hook's transparent rewrite before noisy output is produced.

**Before noisy shell commands:**
- Prefer `claude-skills run -- <command>` for test, build, lint, log, status, search, Docker, Kubernetes, Terraform, package-manager, and CI-style commands.
- Use `claude-skills rewrite "<command>"` when unsure whether a command has native compaction.
- The hook transparently rewrites the command via `toolInputOverride`, wrapping it in `claude-skills run --`. No manual rerun needed.

**Before broad repository search:**
- Prefer `claude-skills code-search search --workspace-root "$PWD" --query "<query>"`.
- Use raw `rg`, `grep`, `find`, or `git grep` only after scoped search/map context is insufficient.
- For noisy search output, run it through `claude-skills run --`.

**Before editing existing source:**
- Run or validate Preserve Existing Flow evidence first.
- Use `claude-skills flow start`, `claude-skills flow check`, and `claude-skills flow finish`.
- Record target file/function, current behavior, entry point, producer, source of truth, state/storage/queue owner, side-effect owner, consumers, cleanup/recovery path, edit boundary, validation needed, and validation evidence in `~/.claude/memories/workspaces/<workspace-slug>/flow/flow-check.json`.
- Do not patch the first suspicious branch until the behavior owner is proven.

**Before commit, PR, or final response:**
- Use professional templates and linting.
- Use `claude-skills git-workflow commit-message --from-diff --test-result "<result>"`.
- Use `claude-skills git-workflow pr-body --from-diff --test-result "<result>"`.
- Use `claude-skills git-workflow lint-message <file>` against the rendered text.
- Run native review gates (`claude-skills review pre-pr`, `claude-skills review gates check`) before finalizing.

### Concrete before/after examples

Instead of:
```bash
cargo test --workspace
```
Prefer:
```bash
claude-skills run -- cargo test --workspace
```

Instead of:
```bash
rg "RunReview" .
```
Prefer:
```bash
claude-skills code-search search --workspace-root "$PWD" --query "RunReview owner path"
```
Then, if still needed:
```bash
claude-skills run -- rg "RunReview" internal
```

Instead of patching immediately:
Read the target file → trace the owner path (producer, source of truth, state/storage/queue, side-effect owner, consumer, recovery) → `claude-skills flow start` → `claude-skills flow check` → patch a small batch → re-read the touched surface → run the narrowest proving validation.

If the hook rewrites a command, it replaces the tool input transparently and execution proceeds with the wrapped command. No manual rerun is needed.

## Hook Transparent Rewrite

The managed hook may return `permissionDecision: "allow"` with a `toolInputOverride` that wraps the command in `claude-skills run --`. This is expected behavior, not a failure.

When that happens:
1. The hook replaces the original command's `tool_input.command` with the wrapped version.
2. Execution proceeds automatically with the wrapped command.
3. Continue from the compacted output produced by `claude-skills run --`.
4. Do not re-run the original raw command unless the wrapper itself fails.

Example:
- Raw command attempted: `cargo test --workspace`
- Hook response: `toolInputOverride.command` = `claude-skills run -- cargo test --workspace`
- Correct behavior: execution proceeds transparently with the wrapped command.

Do not re-run the original raw command unless the wrapper itself fails for a real reason (not because the wrapper exists).

### Compaction surface hierarchy

- **Level 1 — Direct native wrapper:** `claude-skills run -- <command>` is the most reliable transparent surface; it owns command execution, shell-aware parser/rewrite support, command-specific semantic reducers, high-signal error/warning extraction, noisy-output head/tail compaction, raw-output recovery, and native savings analytics in one step. Use `claude-skills run --stream -- <command>` only when bounded live progress is needed.
- **Level 2 — Rewrite helper:** `claude-skills rewrite "<command>"` returns the resolved wrapper for inspection or scripting. It recognizes common shell wrappers, environment prefixes, and pipelines, and routes shell syntax through `bash -lc`.
- **Level 3 — Hook guidance:** `claude-skills hook install` registers native Claude Code lifecycle hooks for `PreToolUse`, `PermissionRequest`, `PostToolUse`, `PreCompact`, `PostCompact`, `SessionStart`, `UserPromptSubmit`, and `Stop` in `~/.claude/hooks.json`. `PreToolUse` owns token-saving interception because it must run before noisy Bash output exists; the other lifecycle hooks are native no-op/checkpoint surfaces for memory and recovery wiring. The hook may return `permissionDecision: "allow"` with a `toolInputOverride` that transparently wraps the command (not a block-and-rerun).
- **Level 4 — Native install/update:** Use the installed Rust binary directly (`~/.claude/claude-skills` or `%USERPROFILE%\.claude\claude-skills.exe`) for update, verify, status, hooks, and compaction. Shell and PowerShell wrapper launchers are not supported runtime entrypoints.

For agent-facing instructions, `claude-skills hook instructions` prints the same usage contract in `markdown` (default) or `--format json`. The same contract is also tracked in [`docs/hook-usage.md`](../../docs/hook-usage.md).

## Token Optimization (Native Command Compaction)

claude_skills includes native command output compaction to reduce wasted CLI-output context on common development commands, benchmarked against external output-reduction and context-efficiency patterns without naming those tools in the managed prompt surface. External tools remain feature benchmarks, not runtime dependencies. The default implementation stays native because it is integrated with Claude Code hooks, flow, review, install/update, repository instructions, raw-output recovery, and persisted `gain` analytics. It can help users fit more useful work into the same Claude Code usage window; it does not increase hard usage limits or bypass rate limits.

### Auto-Install Hook

To enable automatic command output compaction, run:

```bash
claude-skills hook install
```

The one-line installer refreshes the managed hook set automatically, and `claude-skills hook install` can refresh it manually. The hook set points at the current claude-skills command surface. `PreToolUse` transparently rewrites supported shell commands via `toolInputOverride`; the other supported lifecycle events (`PermissionRequest`, `PostToolUse`, `PreCompact`, `PostCompact`, `SessionStart`, `UserPromptSubmit`, `MessageSend`, `MessageReceive`, `SubagentStop`, `TeammateIdle`, `SubagentStart`, `Resume`, `Stop`, and `Elicitation`) are native lifecycle/checkpoint surfaces. `FileChanged` is also supported (fires on watched file changes; its matcher doubles as a per-repo watch list, so it is not auto-installed when the matcher is empty). `MessageDisplay` fires on every assistant message and emits `hookSpecificOutput.displayContent` — not auto-installed to avoid silently rewriting on-screen text. Both `FileChanged` and `MessageDisplay` are still dispatchable via `claude-skills hook file-changed` and `claude-skills hook message-display` for ad-hoc invocations.

### Supported Command Wrapper

The Rust-native `run` command executes the requested command, emits command-specific semantic reducers plus high-signal error/warning context and compacted head/tail summaries for noisy or long output, records native savings analytics with reducer/family dimensions, and records a raw-output recovery log. Do not route through Go or third-party compaction tools to recover old behavior.

Use the wrapper for high-noise command categories such as tests, builds, lints, logs, status, search, Docker, Kubernetes, Terraform, package-manager, and CI-style commands. Product wording must stay honest: high-signal extraction, shell-aware rewrite, semantic reducers, bounded streaming, head/tail compaction, analytics, and raw-output recovery are implemented; broader savings claims require Rust proof before they are advertised.

### Manual Compaction

For commands not covered by the hook, use manual compaction:

```bash
claude-skills run -- cargo test --workspace
claude-skills run -- git status
claude-skills run -- cargo test
```

### Rewrite Command

To check if a command is supported for compaction:

```bash
claude-skills rewrite "cargo test --workspace"
# Output resolves through the current executable, for example: claude-skills run -- cargo test --workspace
```

### Token Savings Analytics

`gain` reads the Rust-native compaction event log from the Claude Code home and reports observed commands, compacted commands, saved bytes, savings percentage, and top commands:

```bash
claude-skills gain              # Show all-time dashboard
claude-skills gain --daily      # Today's stats
claude-skills gain --weekly     # Last 7 days
claude-skills gain --monthly    # Last 30 days
claude-skills gain --top 20     # Top 20 commands by savings
claude-skills gain --chart      # ASCII chart
claude-skills gain --json       # Machine-readable output
```

### Hook Management

```bash
claude-skills hook install        # Install managed lifecycle hooks
claude-skills hook uninstall      # Remove managed lifecycle hooks
claude-skills hook list           # List installed hooks
claude-skills hook show           # Show hooks.json content
claude-skills hook instructions   # Print agent-facing hook usage (markdown by default; --format json available)
```
