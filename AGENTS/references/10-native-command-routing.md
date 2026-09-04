<!--
Purpose: Capture native-command routing, hook transparent rewrite, and token-optimization rules previously inline in AGENTS.md.
Caller: AGENTS.md when shell-command routing, hook semantics, or compaction surfaces are in scope.
Dependencies: keel run, keel rewrite, keel hook install, keel code-search, keel flow, keel review, keel git-workflow.
Main Functions: Define when to route through native commands, how the hook rewrite works, and what compaction surfaces exist.
Side Effects: None — this file is informational.
-->
# Native Command Routing, Hook Rewrite, and Token Optimization

## Native Command Routing — Must Follow First

When a native `keel` command owns the job, use it instead of recreating the behavior with raw shell, generic search, or ad hoc instructions.

**Token-saving rule:** the goal is to prevent noisy raw command output from entering the harness context. Do not run a raw noisy command first and compact afterward; route through `keel run -- <command>` or rely on the hook's transparent rewrite before noisy output is produced.

**Before noisy shell commands:**
- Prefer `keel run -- <command>` for test, build, lint, log, status, search, Docker, Kubernetes, Terraform, package-manager, and CI-style commands.
- Use `keel rewrite "<command>"` when unsure whether a command has native compaction.
- The hook transparently rewrites the command via `toolInputOverride`, wrapping it in `keel run --`. No manual rerun needed.

**Before broad repository search:**
- Prefer `keel code-search search --workspace-root "$PWD" --query "<query>"`.
- After a fix or implement, run `keel code-search siblings` (optional `--query` of the bug shape) and handle every hit. A one-site change is unfinished.
- Use raw `rg`, `grep`, `find`, or `git grep` only after scoped search/map context is insufficient.
- For noisy search output, run it through `keel run --`.

**Before editing existing source:**
- Run or validate Preserve Existing Flow evidence first.
- Use `keel flow start`, `keel flow check`, and `keel flow finish`.
- Record target file/function, current behavior, entry point, producer, source of truth, state/storage/queue owner, side-effect owner, consumers, cleanup/recovery path, edit boundary, validation needed, and validation evidence in `~/.keel/memories/workspaces/<workspace-key>/flow/flow-check.json`.
- Do not patch the first suspicious branch until the behavior owner is proven.

**Before commit, PR, or final response:**
- Use professional templates and linting.
- Use `keel git-workflow commit-message --from-diff --test-result "<result>"`.
- Use `keel git-workflow pr-body --from-diff --test-result "<result>"`.
- Use `keel git-workflow lint-message <file>` against the rendered text.
- Run native review gates (`keel review pre-pr`, `keel review gates check`) before finalizing.

### Concrete before/after examples

Instead of:
```bash
cargo test --workspace
```
Prefer:
```bash
keel run -- cargo test --workspace
```

Instead of:
```bash
rg "RunReview" .
```
Prefer:
```bash
keel code-search search --workspace-root "$PWD" --query "RunReview owner path"
```
Then, if still needed:
```bash
keel run -- rg "RunReview" internal
```

Instead of patching immediately:
Read the target file → trace the owner path (producer, source of truth, state/storage/queue, side-effect owner, consumer, recovery) → `keel flow start` → `keel flow check` → patch a small batch → re-read the touched surface → run the narrowest proving validation.

If the hook rewrites a command, it replaces the tool input transparently and execution proceeds with the wrapped command. No manual rerun is needed.

## Hook Transparent Rewrite

The managed hook may return `permissionDecision: "allow"` with a `toolInputOverride` that wraps the command in `keel run --`. This is expected behavior, not a failure.

When that happens:
1. The hook replaces the original command's `tool_input.command` with the wrapped version.
2. Execution proceeds automatically with the wrapped command.
3. Continue from the compacted output produced by `keel run --`.
4. Do not re-run the original raw command unless the wrapper itself fails.

Example:
- Raw command attempted: `cargo test --workspace`
- Hook response: `toolInputOverride.command` = `keel run -- cargo test --workspace`
- Correct behavior: execution proceeds transparently with the wrapped command.

Do not re-run the original raw command unless the wrapper itself fails for a real reason (not because the wrapper exists).

### Compaction surface hierarchy

- **Level 1: Direct native wrapper:** `keel run -- <command>` is the most reliable transparent surface; it owns command execution, shell-aware parser/rewrite support, command-specific semantic reducers, high-signal error/warning extraction, noisy-output head/tail compaction, raw-output recovery, and native savings analytics in one step. Use `keel run --stream -- <command>` only when bounded live progress is needed.
- **Level 2 — Rewrite helper:** `keel rewrite "<command>"` returns the resolved wrapper for inspection or scripting. It recognizes common shell wrappers, environment prefixes, and pipelines, and routes shell syntax through `bash -lc`.
- **Level 3 — Hook guidance:** `keel hook install` registers native harness lifecycle hooks for `PreToolUse`, `PermissionRequest`, `PostToolUse`, `PreCompact`, `PostCompact`, `SessionStart`, `UserPromptSubmit`, and `Stop` in `~/.claude/hooks.json`. `PreToolUse` owns token-saving interception because it must run before noisy Bash output exists; the other lifecycle hooks are native no-op/checkpoint surfaces for memory and recovery wiring. The hook may return `permissionDecision: "allow"` with a `toolInputOverride` that transparently wraps the command (not a block-and-rerun).
- **Level 4: Native install/update:** Use the installed Rust binary directly (`~/.keel/keel` or `%USERPROFILE%\.keel\keel.exe`) for update, verify, status, hooks, and compaction. Shell and PowerShell wrapper launchers are not supported runtime entrypoints.

For agent-facing instructions, `keel hook instructions` prints the same usage contract in `markdown` (default) or `--format json`. The same contract is also tracked in [`docs/hook-usage.md`](../../docs/hook-usage.md).

## Token Optimization (Native Command Compaction)

keel includes native command output compaction to reduce wasted CLI-output context on common development commands, benchmarked against external output-reduction and context-efficiency patterns without naming those tools in the managed prompt surface. External tools remain feature benchmarks, not runtime dependencies. The default implementation stays native because it is integrated with the harness hooks, flow, review, install/update, repository instructions, raw-output recovery, and persisted `gain` analytics. It can help users fit more useful work into the same the harness usage window; it does not increase hard usage limits or bypass rate limits.

### Auto-Install Hook

To enable automatic command output compaction, run:

```bash
keel hook install
```

The one-line installer refreshes the managed hook set automatically, and `keel hook install` can refresh it manually. The hook set points at the current keel command surface. `PreToolUse` transparently rewrites supported shell commands via `updatedInput`; the other supported lifecycle events (`PermissionRequest`, `PostToolUse`, `PostToolUseFailure`, `PostToolBatch`, `PreCompact`, `PostCompact`, `SessionStart`, `UserPromptSubmit`, `UserPromptExpansion`, `Stop`, `StopFailure`, `SubagentStart`, `SubagentStop`, `CwdChanged`, `Notification`, `PermissionDenied`, `SessionEnd`, and `WorktreeDiscard`) are native lifecycle/checkpoint surfaces. `FileChanged` is also supported (fires on watched file changes; its matcher doubles as a per-repo watch list, so it is not auto-installed when the matcher is empty). `MessageDisplay` fires on every assistant message.

### Supported Command Wrapper

The Rust-native `run` command executes the requested command, emits command-specific semantic reducers plus high-signal error/warning context and compacted head/tail summaries for noisy or long output, records native savings analytics with reducer/family dimensions, and records a raw-output recovery log. Do not route through Go or third-party compaction tools to recover old behavior.

Use the wrapper for high-noise command categories such as tests, builds, lints, logs, status, search, Docker, Kubernetes, Terraform, package-manager, and CI-style commands. Product wording must stay honest: high-signal extraction, shell-aware rewrite, semantic reducers, bounded streaming, head/tail compaction, analytics, and raw-output recovery are implemented; broader savings claims require Rust proof before they are advertised.

### Manual Compaction

For commands not covered by the hook, use manual compaction:

```bash
keel run -- cargo test --workspace
keel run -- git status
keel run -- cargo test
```

### Rewrite Command

To check if a command is supported for compaction:

```bash
keel rewrite "cargo test --workspace"
# Output resolves through the current executable, for example: keel run -- cargo test --workspace
```

### Token Savings Analytics

`gain` reads the Rust-native compaction event log from the harness home and reports observed commands, compacted commands, saved bytes, savings percentage, and top commands:

```bash
keel gain                   # Show summary (since today by default)
keel gain --since all       # All-time stats
keel gain --since week      # Last 7 days
keel gain --since month     # Last 30 days
keel gain --top 20          # Top 20 commands by savings
keel gain --adapter tests   # Filter by adapter
keel gain --json            # Machine-readable output
keel gain discover          # Discover uncompacted command opportunities
```

### Hook Management

```bash
keel hook install        # Install managed lifecycle hooks
keel hook uninstall      # Remove managed lifecycle hooks
keel hook list           # List installed hooks
keel hook show           # Show hooks.json content
keel hook instructions   # Print agent-facing hook usage (markdown by default; --format json available)
```
