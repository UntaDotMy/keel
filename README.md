<!--
Purpose: Present the native claude_skills product surface, install paths, and proof-first workflow.
Caller: Contributors, operators, and AI agents onboarding to the managed skill pack.
Dependencies: Native CLI commands, workflow docs, memory surfaces, review gates, and release artifacts.
Main Functions: Explain what to run first, where to find each surface, and how closure proof works.
Side Effects: Sets contributor and operator expectations for the repo-managed native experience.
-->
[![Validate](https://github.com/UntaDotMy/claude_core/actions/workflows/validate.yml/badge.svg)](https://github.com/UntaDotMy/claude_core/actions/workflows/validate.yml)

# claude-core

**Discipline as code for Claude Code.** A single Rust binary that forces the agent to read the codebase before answering, restate the iron law on every prompt, refresh a structural project map across compactions, write a working brief before non-trivial work, and run a reviewer pass before closeout. No Node, no Python, no daemon.

## The Iron Law

Three rules are restated to the agent on every prompt. You cannot skip them.

- **Read first.** Read SYSTEM_MAP, CLAUDE.md, the owning module, and the existing implementation. Do not propose changes against an imagined version of the file.
- **Invoke relevant skills.** If there is even a 1% chance a skill applies, use the Skill tool *before* writing code or giving a final answer. The cost of skipping a skill that did apply is shipping a regression.
- **Find the root cause.** Take the symptom as a starting point, not the spec. The real problem is usually one layer below what was asked. Trace the symptom end-to-end against the running code with file:line evidence before changing anything.

## Install in One Paste

```bash
# macOS / Linux
curl -fsSL https://raw.githubusercontent.com/UntaDotMy/claude_core/main/install.sh | bash
```

```powershell
# Windows PowerShell
irm https://raw.githubusercontent.com/UntaDotMy/claude_core/main/install.ps1 | iex
```

```bat
:: Windows CMD
curl -fsSL https://raw.githubusercontent.com/UntaDotMy/claude_core/main/install.cmd -o install.cmd && install.cmd && del install.cmd
```

The installer detects your OS and architecture, pulls the matching prebuilt binary from [GitHub Releases](https://github.com/UntaDotMy/claude_core/releases/latest), runs `claude-skills install`, verifies `status`, and cleans up. No Rust toolchain required. Pin a version with `CLAUDE_SKILLS_VERSION=vX.Y.Z`.

## Demo

A 30-second walkthrough of `workflow start -> cockpit -> finish`:

```bash
asciinema play docs/demos/quickstart.cast
```

The cast file ships in this repo. Render to GIF with `agg docs/demos/quickstart.cast docs/demos/quickstart.gif` if your viewer prefers a static asset.

## What You Get

| Surface | What it gives you |
| --- | --- |
| Iron-law hooks | SessionStart loads the bootstrap skill, UserPromptSubmit restates the three rules, PostToolBatch nudges a reviewer pass, PreCompact refreshes SYSTEM_MAP. |
| Workflow CLI | `workflow start`, `workflow route`, `workflow cockpit`, `workflow finish` — proof-first delivery rails. |
| Review gates | Native `.claude-review.json`, `review pre-pr`, and CI-ready artifacts so non-trivial code never self-reviews. |
| Memory | Working briefs, completion ledgers, scoped `SYSTEM_MAP.md`, and durable recovery state under `~/.claude/memories/`. |
| Command compaction | `claude-skills run -- <cmd>` produces compact output for noisy test/build/lint/log/search commands without dropping diagnostic signal. |
| MCP server | `claude-skills mcp serve` is registered through the plugin manifest so Claude Code auto-discovers `recall`, `system_map`, `run_command`, and `recall_status` tools plus the system-map and recall-status resources. |
| Slash commands | `/claude-core:workflow`, `/claude-core:review`, `/claude-core:recall`, `/claude-core:gain` — discoverable `/`-menu wrappers over the implemented CLI surfaces. Shipped via the plugin manifest `commands` key. |
| Specialist skills | 18 managed specialist profiles synced into `~/.claude/agent-profiles/*.toml`, invokable via the Skill tool. |

## Use as a Claude Code Plugin

This repo ships a `.claude-plugin/plugin.json` manifest. From inside Claude Code:

```text
/plugin marketplace add UntaDotMy/claude_core
/plugin install claude-core@claude_core
```

That mounts the skills, agents, and hooks without running the native installer. Use the one-paste installer above when you want the full `claude-skills` CLI for workflow, memory, and command-compaction surfaces.

---

## Native Command Routing — Must Follow First

When a native `claude-skills` command owns the job, use it instead of recreating the behavior with raw shell, generic search, or ad hoc instructions.

**Token-saving rule:** the goal is to prevent noisy raw command output from entering Claude Code context. Do not run a raw noisy command first and compact afterward; route through `claude-skills run -- <command>` or the hook-provided `Rerun that as:` wrapper before noisy output is produced.

- **Noisy shell commands:** prefer `claude-skills run -- <command>` for test, build, lint, log, status, search, Docker, Kubernetes, Terraform, package-manager, and CI-style commands. Use `claude-skills rewrite "<command>"` when unsure whether a command has native compaction.
- **Hook block-and-rerun:** if the managed `PreToolUse` hook returns `Rerun that as: <command>`, immediately run that exact command. Do not ask the user, do not treat the hook block as a task failure, and do not repeat the raw command first.
- **Repository search:** prefer `claude-skills code-search search --workspace-root "$PWD" --query "<query>"`. Use raw `rg`, `grep`, `find`, or `git grep` only after scoped search is insufficient, and pipe noisy raw search through `claude-skills run --`.
- **Existing-source edits:** run or validate Preserve Existing Flow evidence first. Use `claude-skills flow start`, `claude-skills flow check`, and `claude-skills flow finish`, and record the owner path in `~/.claude/memories/workspaces/<workspace-slug>/flow/flow-check.json` before patching.
- **Commit/PR/final response text:** use `claude-skills git-workflow commit-message --from-diff`, `claude-skills git-workflow pr-body --from-diff`, and `claude-skills git-workflow lint-message <file>` against the templates in `templates/commit-body.md`, `templates/pr-body.md`, `templates/final-response.md`, and `templates/review-summary.md` before submitting. Run `claude-skills review pre-pr` and `claude-skills review gates check` before finalizing.

For agent-facing usage in markdown or JSON, run `claude-skills hook instructions` (see also [`docs/hook-usage.md`](docs/hook-usage.md)).

## Hook Retry Handling

The managed hook may return a Claude Code denial whose reason begins with `Rerun that as:`. This is expected behavior, not a failure.

When that happens:
1. Copy the command after `Rerun that as:`.
2. Run it exactly once.
3. Preserve the resulting exit code and output.
4. Continue from the compacted output.
5. Do not ask the user for permission unless the suggested command itself is destructive or outside the requested task.

Example: a raw `cargo test --workspace` may produce `Rerun that as: claude-skills run -- cargo test --workspace`; the correct next action is to run `claude-skills run -- cargo test --workspace` and continue from its compacted output.

## Start Here

| Need | Run | Why |
| --- | --- | --- |
| First install, no Rust required | Download a release, extract it, run `./claude-skills install` or `.\claude-skills.exe install` | Installs the native binary and managed skills into Claude Code home. |
| Check the install | `~/.claude/claude-skills status` or `%USERPROFILE%\.claude\claude-skills.exe status` | Confirms the managed Claude Code-home surface. |
| Start normal work | `claude-skills workflow start --request "..."` | The lowest-friction first run. |
| Route a broad request first | `claude-skills workflow route --request "..."` | Picks the recommended preset before starting. |
| See live state | `claude-skills workflow cockpit` | Shows stage, proof, blockers, and next command. |
| Close a branch | `claude-skills workflow finish` | The default closeout path. |

The default operator path is `workflow start -> workflow cockpit -> workflow finish`; the default closeout path is `claude-skills workflow finish`.

After install, the preferred global CLI path for agents on supported operating systems is:

- macOS or Linux: `~/.claude/claude-skills`
- Windows: `~/.claude/claude-skills.exe`

This matters because the install metadata remembers the source bundle or checkout so `status`, `update`, `verify`, `doctor`, and `menu` can still work when the installed binary is called from another project. For AI-agent or shell contexts where PATH resolution is not guaranteed, prefer the explicit installed path in the Claude Code home root. `--repo-root <path>` is an advanced override for CI, unusual layouts, or running the binary from a different folder than the extracted release/source checkout.

## Install Details

After running the one-paste installer above, verify with:

```bash
~/.claude/claude-skills status              # macOS / Linux
& "$env:USERPROFILE\.claude\claude-skills.exe" status   # Windows PowerShell
```

### Manual Release Install

Download the archive for your OS from GitHub Releases, extract it, open a terminal in the extracted folder, then run `./claude-skills install` or `.\claude-skills.exe install`. Archives are named like `claude-skills_<version>_<os>_<arch>`. The release bundle includes the native binary plus the managed skill files, so Rust/Cargo is not required for normal install.

### Contributors: install from source

```bash
git clone https://github.com/UntaDotMy/claude_core.git
cd claude_core
cargo run --bin claude-skills -- install
cargo run --bin claude-skills -- status
```

Use `--repo-root <path>` only when you intentionally run `claude-skills install` from outside the extracted release folder or source checkout.

### Native Update

```bash
~/.claude/claude-skills update
~/.claude/claude-skills verify
~/.claude/claude-skills status
```

```powershell
& "$env:USERPROFILE\.claude\claude-skills.exe" update
& "$env:USERPROFILE\.claude\claude-skills.exe" verify
& "$env:USERPROFILE\.claude\claude-skills.exe" status
```

The Rust manager remembers the source checkout in install metadata, fast-forwards that checkout on `update`, rebuilds the native CLI when needed, delta-syncs changed files, removes stale managed files, and preserves unrelated Claude Code-home files. Shell and PowerShell wrapper launchers are no longer shipped.

On Windows, install replaces the running `claude-skills.exe` synchronously via `MoveFileEx(MOVEFILE_REPLACE_EXISTING)` (the same trick rustup uses) instead of a detached `cmd /C copy`. Failures now surface as install errors instead of leaving a stale binary on disk. When the source and the deployed binary are byte-identical, the swap is skipped entirely so a no-op `update` does not touch the executable.

### After Install

Run these once after a fresh install or update:

```bash
~/.claude/claude-skills.exe verify     # confirms inventory + binary match the source
~/.claude/claude-skills.exe doctor     # probes hooks end-to-end, reports any drift
~/.claude/claude-skills.exe status     # version, repo SHA, install timestamp
```

Hooks are wired automatically by `install`. If you want to refresh `~/.claude/settings.json` without a full reinstall:

```bash
~/.claude/claude-skills.exe hook install
```

Optional environment variables:

| Variable | Default | Purpose |
| --- | --- | --- |
| `CLAUDE_SKILLS_RAW_RETENTION_DAYS` | `14` | Days of `~/.claude/raw-output/` runs kept on disk. SessionEnd hook prunes anything older. Set to `0` to disable auto-prune. |
| `CLAUDE_SKILLS_VERSION` | latest | Pin the bootstrap installer to a specific release tag. |

Manual prune (any time):

```bash
~/.claude/claude-skills.exe raw prune --older-than 30d
```

## Slash Commands

When installed as a plugin, claude-core registers four namespaced slash commands
(see the `commands` key in `.claude-plugin/plugin.json`). Each is a thin,
discoverable `/`-menu wrapper over an **implemented** `claude-skills` CLI surface
— none of them invoke planned-but-unimplemented commands.

| Command | Wraps | Use it for |
| --- | --- | --- |
| `/claude-core:workflow [route\|start\|cockpit\|finish] <args>` | `workflow` ledger | Drive a proof-first workstream. |
| `/claude-core:review [pre-commit\|pre-pr\|gates] [base-ref]` | `review` gates | Deterministic local quality gate on the diff. |
| `/claude-core:recall <terms>` | `memory recall` | FTS5 search over durable memory. |
| `/claude-core:gain [since]` | `gain` | Report command-output compaction savings. |

Command files live at the plugin root `commands/`. They ship through the plugin
install path; the native `claude-skills install` does not yet sync them into
`~/.claude/commands/` (tracked follow-up — the installer has `sync_skills` and
`sync_agents` but no `sync_commands` arm yet).

## Statusline (opt-in)

A cross-platform statusline script renders the active model, context usage, and a
**compaction-savings badge** sourced from `claude-skills gain --json` (the badge
is omitted when the binary or savings data is unavailable, and the line never
errors). It is opt-in — claude-core does not overwrite your `statusLine` setting.

`statusline/statusline-claude-core.sh` (macOS/Linux) and
`statusline/statusline-claude-core.ps1` (Windows). To enable, point your
`settings.json` `statusLine.command` at the script:

```json
{
  "statusLine": {
    "type": "command",
    "command": "~/.claude/statusline-claude-core.sh"
  }
}
```

```json
{
  "statusLine": {
    "type": "command",
    "command": "powershell -NoProfile -ExecutionPolicy Bypass -File %USERPROFILE%\\.claude\\statusline-claude-core.ps1"
  }
}
```

Both scripts read the documented statusline session JSON on stdin and print one
line such as `Opus | ctx 8% | saved 3800 tok`.


### Cache Hygiene and Token Economy

The hook lifecycle is tuned to preserve Claude Code's prompt cache and minimize per-prompt input tokens.

- **What stays cached:** the system prompt, tool definitions, `CLAUDE.md`, and the SessionStart context are read at the cache breakpoint. Reuse costs ~10% of normal input tokens for ~5 minutes after each write.
- **What gets paid every prompt:** `UserPromptSubmit` injects a short research-first iron-law restatement (~80 tokens) via `additionalContext`. The full bootstrap skill, Red Flags table, and skill catalog ride on `SessionStart` so per-prompt cost stays small while the iron law stays top-of-mind every turn.
- **What gets paid every turn end / tool call:** `Stop`, `SubagentStop`, `SessionEnd`, and `PostToolUse` are silent. `PostToolBatch` injects a short reviewer-on-close reminder before the next model turn. Earlier versions of the lifecycle emitted ~50 tokens of generic closeout text on every tool call; that overhead is gone.
- **Why this matters:** the per-prompt and per-batch hooks are sized to carry information the model genuinely uses. The system prompt, tool definitions, `CLAUDE.md`, and `SessionStart` context stay above the cache breakpoint so they reuse cleanly within the 5-minute cache window.

If you customize hooks downstream and want to see exactly what the lifecycle emits for an event:

```bash
echo '{}' | ~/.claude/claude-skills.exe hook stop
echo '{}' | ~/.claude/claude-skills.exe hook user-prompt-submit
```

Empty stdout means the hook is intentionally silent for that event.

## Find Fast

| Job | Commands |
| --- | --- |
| Route a broad request | `claude-skills workflow route --request "..."` |
| Start work | `claude-skills workflow start --preset autopilot --request "..."` |
| Watch live state | `claude-skills workflow cockpit`, `claude-skills workflow dashboard`, `claude-skills workflow watch` |
| Review locally | `claude-skills review pre-commit`, `claude-skills review pre-pr`, `claude-skills review gates check` |
| Finish a workstream | `claude-skills workflow finish --id <entry-id> --proof "..."`, `gh pr checks --watch` |
| Compact noisy commands | `claude-skills rewrite "cargo test --workspace"`, `claude-skills run -- grep -RIn TODO rust` |
| Refresh memory map | `claude-skills memory scope resolve --create-missing --refresh-system-map` |
| Advanced help | `claude-skills help advanced` |

External output-compaction tools are feature benchmarks for expected output reduction and recoverability, not runtime dependencies. The default path stays the native Rust implementation because it is integrated with Claude Code hooks, Preserve Existing Flow, review gates, install/update, repository instructions, raw-output recovery, and persisted `gain` analytics.

See [Native Gap Map](docs/native-gap-map.md) for the anonymized comparison between external output reducers, runtime-shell peers, and the current native implementation.

Route, start, watch, cockpit, and finish now share one operator-shell vocabulary: `stage`, `active_lane`, `proof_state`, `blocker`, `next_command`, and `recovery_path`. The workflow shell also keeps the active launch surface intact, so source-checkout runs use `cargo run --bin claude-skills -- ...` and installed runs keep using the installed executable.

Start with the preset-driven native CLI when the operator wants a top-layer product surface: Use `workflow route`, `workflow start --preset ...`, `workflow cockpit`, and `workflow finish` for most delivery work.

## Daily Paths

Quick labels: Feature work: Bug fixing: PR rescue: TDD-first implementation: Bounded parallel work:

### Feature work

```bash
claude-skills workflow route --request "Add the next feature and carry it to closure"
claude-skills workflow start --preset autopilot --request "Add the next feature and carry it to closure"
claude-skills workflow cockpit
claude-skills workflow finish --id <entry-id> --proof "tests green"
```

### Bug fixing

```bash
claude-skills workflow route --request "Trace the regression, fix the root cause, and prove it"
claude-skills workflow start --preset debug --request "Trace the regression, fix the root cause, and prove it"
claude-skills workflow cockpit
claude-skills workflow finish --id <entry-id> --proof "regression covered by tests"
```

### Review

```bash
claude-skills workflow route --request "Audit the current branch and call out the real gaps"
claude-skills workflow start --preset review --request "Audit the current branch and call out the real gaps"
claude-skills workflow cockpit
claude-skills workflow finish --id <entry-id> --proof "review notes recorded"
```

### TDD-first implementation

```bash
claude-skills workflow start --preset tdd --request "Write the failing test first, implement the smallest fix that makes it pass, and close with regression proof"
claude-skills workflow cockpit
claude-skills workflow finish --id <entry-id> --proof "failing -> fix -> regression proof"
```

### Common job shapes

Feature work, Bug fixing, PR rescue, TDD-first implementation, and Bounded parallel work all use the same visible loop: route, start, cockpit, prove, finish. Hosted-check failures are repaired on the same PR with `gh pr checks --watch` and another push.

### Native guidance tracks

Brainstorming:

```bash
claude-skills workflow route --request "Brainstorm the approach, compare the options, and recommend the right next lane"
```

Plan writing:

```bash
claude-skills workflow route --request "Write the implementation plan, file targets, proof steps, and recovery path before coding"
```

Plan execution:

```bash
claude-skills workflow start --preset autopilot --request "Carry the approved plan to closure"
```

Systematic debugging:

```bash
claude-skills workflow start --preset debug --request "Trace the regression, find the root cause, and prove the real fix"
```

Code review:

```bash
claude-skills workflow start --preset review --request "Review the branch, call out the real gaps, and decide if it is ready"
```

Workstream finish:

```bash
claude-skills workflow finish --id <entry-id> --proof "tests green; review pre-pr passed"
```

### Native plan surface

Use exact file targets, verification steps, and recovery checkpoints before coding.

- File targets: list every expected write target before the first edit.
- Verification steps: name the narrow proving checks first.
- Recovery checkpoints: if interrupted, reopen the workstream with `workflow status`, `workflow cockpit`, and `workflow resume --id <entry-id>` before changing the plan.

### Native engineering principles

The guide now teaches TDD, YAGNI, and DRY as native workflow prompts and examples instead of pushing operators back to a standalone prompt library:

- TDD:
- YAGNI:
- DRY:
- TDD operator check: keep the native three-stage proof contract visible in `workflow cockpit` and `workflow finish` instead of relying on prose reminders.

### Workflow presets versus lower-level primitives

Keep the native CLI as the primary surface instead of drifting back toward a prompt-library-only identity. The router prints a short "Start Now" command first, keeps a scoped variant available for traceable workstreams, and cockpit shows route, active lanes, proof state, a live proof board, blockers, and the next command in one place. The branch path keeps proof-board gate status visible.

Useful workflow command shelf: `claude-skills workflow route`, `claude-skills workflow start --preset <preset> --request "..."`, `claude-skills workflow cockpit`, `claude-skills workflow status`, `claude-skills workflow dashboard`, `claude-skills workflow watch`, `claude-skills workflow resume --id <entry-id>`, and `claude-skills workflow finish --id <entry-id> --proof "..."`.

The dashboard includes a synthesized runtime-state summary and team-health summary so operators do not have to reconstruct that picture from raw memory artifacts. Cockpit surfaces the same runtime-state summary and team-health summary alongside the proof board, with a lighter day-to-day shell summary. Finish starts with a lighter closeout summary and records the supplied proof against the workflow ledger entry.

## Presets

Each preset now says what it owns, what it does not own, and what done means at that stage, so the operator can see the boundary instead of inferring it.

| Preset | Use it for | Done means |
| --- | --- | --- |
| `autopilot` | Broad feature or maintenance work. | Working brief, completion gate, cockpit proof board, review pass, and native finish checks are current. |
| `debug` | Stateful bugs, failing checks, and root-cause repair. | Behavior mismatch, root cause, fix, and rerun proof are visible. |
| `tdd` | Test-first delivery. | Failing proof first, fix proof second, regression proof third. |
| `review` | Audit, production-readiness, and merge decisions. | Findings or approval are backed by current evidence. |
| `eco` | Bounded maintenance. | Narrowest honest proving validation passes. |
| `parallel` | Bounded multi-lane work. | Required lanes, proof board, and blockers are terminal. |

### Preset guide

`autopilot`: the default first-run preset.
When to use: broad feature or maintenance work where one owner should keep moving from alignment through closure.
Proof it expects: the working brief, completion gate, cockpit proof board, review pass, and native finish checks stay current before closeout.
If interrupted: reopen the workstream with `workflow status`, `workflow cockpit`, and `workflow resume --id <entry-id>`.

`debug`: the focused preset for stateful bugs.
`tdd`: the preset for test-first delivery.
Proof it expects: failing proof first, fix proof second, regression proof third, plus the normal review and finish checks.
`review`: the preset for audit, production-readiness, gap-finding, and final validation.
`eco`: the lighter preset for bounded maintenance.
`parallel`: the preset for bounded multi-lane work.
If interrupted: recover from `workflow status`, `workflow cockpit`, and `workflow resume --id <entry-id>`.

The lighter `autopilot` preset and `standard` tier power the default low-friction path.

## Proof Rules

The pack is strict on purpose:

- Work is not done just because implementation happened.
- Work is not done because one test passed or the first rerun turned green after a fix.
- Finished work must be re-audited against the user story, PRD or spec when one exists, explicit tasks, active plan items, tracked requirements, required lanes, and closure-ready evidence.
- The current job scope must be 100% complete for that scope.
- After a fix, rerun the narrow proving checks and re-audit the broader impacted system.
- Verify the relevant language, framework, runtime, and tooling release notes before non-trivial implementation.
- Use the right inspection tool: browser automation such as Playwright for web UI, live desktop runtime with screenshots or equivalent visual evidence for desktop UI, and runtime-native inspection for CLI, services, workflows, or devices.

## Native Review and CI

`.claude-review.json` is the tracked repo-level rule file.

- claude-skills review pre-commit is the local pre-commit surface.
- claude-skills review pre-pr is the local pre-PR surface.
- The cockpit proof view keeps a live proof board.

```bash
claude-skills review pre-commit --format compact
claude-skills review pre-pr --base-ref origin/main --format compact
claude-skills review gates check --surface pre-pr --base-ref origin/main --format compact
cargo test --workspace
```

For heavier Rust validation, run the release build after the workspace test proof.

```bash
cargo build --release --bin claude-skills
cargo fmt --all --check
```

```powershell
cargo build --release --bin claude-skills
cargo fmt --all --check
```

Hosted PR discipline:

1. Run local proof.
2. Push one cohesive feature branch.
3. Open the PR.
4. Wait at least 20 seconds for hosted checks to appear. In checklists this is written as: wait at least 20 seconds.
5. Watch `gh pr checks --watch`.
6. If a hosted lane fails, inspect the failing logs, fix the root cause on the same PR, push again, and rerun `gh pr checks --watch`.

Run `claude-skills git-workflow preflight --repo-root . --base-ref origin/main` before push or merge-request creation.

The validate workflow is fail-closed: repo-wide Rust proof, native review artifacts, cross-platform manager loops, and the summary must pass.

## Command Output Compaction

Use the Rust-native command proxy before noisy shell commands when you want `claude-skills` to prevent raw output from entering the agent transcript. The proxy executes the command, captures stdout/stderr outside context, saves raw recovery files under `~/.claude/raw-output/YYYY-MM-DD/<raw_id>/`, runs a command-specific semantic adapter when one matches, falls back to generic high-signal compaction only when needed, preserves the original exit code, and records exact `o200k_base` before/after token savings in the native JSONL event log.

```bash
claude-skills rewrite "cargo test --workspace"
claude-skills run -- cargo test --workspace
claude-skills run --json -- pytest tests -q
claude-skills run -- git status
claude-skills run -- rg "CompactResult" rust
claude-skills gain --since today
claude-skills gain discover --since today
claude-skills raw <raw_id>
claude-skills doctor
```

What is implemented today:

- `run` executes the requested command, saves `stdout.log`, `stderr.log`, `command.txt`, `meta.json`, and `compact.txt`, and returns compact output with `raw: claude-skills raw <raw_id>`.
- `run --json` returns `command`, `exit_code`, `adapter_name`, `compacted`, `raw_id`, `raw_path`, exact token fields, `summary`, `stdout`, and `stderr`.
- `run --full` and `run --no-compact` pass through raw output while still recording metadata; `--adapter <name>`, `--list-adapters`, `--max-lines <n>`, and `--recovery-dir <path>` are available for debugging and control.
- Built-in adapters cover `tests`, `git`, `search`, `files`, `build`, `lint`, `containers`, `cloud`, `database`, `logs`, and `generic` fallback. Test adapters handle cargo/pytest/go/JS-style failure signals; git/search/files adapters summarize diffs, matches, and large reads; the `containers` adapter compacts docker/kubectl/helm; the `cloud` adapter reduces aws/az/gcloud output (structure-only JSON, secret redaction, failure-first); the `database` adapter reduces psql/mysql/sqlite3/redis-cli/mongosh result sets (header + sampled rows, structure-only JSON, credential redaction).
- `raw <raw_id>`, `raw --path <raw_id>`, `raw list`, `raw prune --older-than 30d`, and `replay <raw_id>` provide local recovery and retention controls.
- `rewrite --json "<command>"` returns supported/reason/rewritten-command metadata and understands common shell wrappers, environment prefixes, and pipelines by routing them through `bash -lc` when needed.
- `hook install` writes the documented global Claude Code lifecycle hook set, with `PreToolUse` handling block-and-rerun command compaction.
- `hook instructions` prints the agent-facing rerun contract in markdown or JSON.
- `gain` reads native compaction events from the Claude Code home and reports observed commands, compacted/passthrough counts, exact tokens before/after/saved, savings percentage, adapter breakdowns, and top commands.
- `gain discover` reports missed-savings opportunities: commands that ran through the proxy but were not compacted (passthrough), grouped by command with the estimated uncompacted tokens that entered context. `gain` reports what was saved; `discover` reports what was left on the table.
- `doctor` checks the binary, raw store, event log, adapter registry, rewrite behavior, and hook/proxy setup with ok/warn/fix-style output.
- The runtime never shells out to Go for compaction, hooks, or command dispatch.

Example compact outputs:

```text
PASS cargo test --workspace
test result: ok. 42 passed; 0 failed; finished in 0.16s

raw: claude-skills raw 20260512-102221-303d93eb
saved: 912 tokens exact/o200k_base (91.8%)
```

```text
FAIL pytest tests -q
2 failed, 143 passed in 12.8s

failures:
tests/api/test_users.py::test_create_user FAILED
E AssertionError: expected 201, got 500
tests/api/test_users.py:88

raw: claude-skills raw <raw_id>
saved: <measured> tokens exact/o200k_base
```

Limitations and safety:

- Hooks may not intercept every host or shell path; explicit `claude-skills run -- <command>` is the reliable path.
- Token counts use `tiktoken-rs` with the `o200k_base` tokenizer; compatibility JSON fields may still be named `estimated_tokens_*`, but their values are exact tokenizer counts.
- Raw output stays local and is not uploaded, but it can contain secrets; manage retention with `claude-skills raw prune --older-than 30d`.
- Compaction redacts obvious secret-looking lines in compact output, but raw recovery preserves what the command printed locally.

### Hook path

The one-line installer refreshes the managed Claude Code hooks automatically, and `claude-skills hook install` can refresh them manually. The hook set is written to `~/.claude/hooks.json`. `PreToolUse` keeps the `Bash` matcher because command-output wrapping is scoped to shell commands; the other lifecycle events use native lifecycle handlers.

```json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Bash",
        "hooks": [
          {
            "type": "command",
            "command": "\"/path/to/claude-skills\" hook pre-tool-use",
            "statusMessage": "Checking native command compaction"
          }
        ]
      }
    ],
    "SessionStart": [
      {
        "matcher": "",
        "hooks": [
          {
            "type": "command",
            "command": "\"/path/to/claude-skills\" hook session-start",
            "statusMessage": "Preparing native session state"
          }
        ]
      }
    ]
  }
}
```

The hook contract is explicit rerun guidance rather than hidden command mutation. The Rust hook installer manages 28 of the 30 lifecycle events in the `HOOK_EVENTS` table (`rust/crates/claude-skills/src/hooks/claude.rs`), writing them to `~/.claude/settings.json`: `PreToolUse`, `PostToolUse`, `PostToolUseFailure`, `PostToolBatch`, `PermissionRequest`, `PermissionDenied`, `Notification`, `UserPromptSubmit`, `UserPromptExpansion`, `Stop`, `StopFailure`, `SubagentStart`, `SubagentStop`, `TaskCreated`, `TaskCompleted`, `TeammateIdle`, `WorktreeCreate`, `WorktreeRemove`, `CwdChanged`, `PreCompact`, `PostCompact`, `SessionStart`, `SessionEnd`, `Setup`, `InstructionsLoaded`, `ConfigChange`, `Elicitation`, and `ElicitationResult`. `PreToolUse` owns command compaction before noisy output exists. `SessionStart` delivers the bootstrap skill once per session, `UserPromptSubmit` injects a short research-first iron-law restatement per prompt, and `PostToolBatch` injects the reviewer-on-close reminder before each next turn. The remaining lifecycle hooks are silent checkpoint surfaces reserved for memory and recovery wiring without shell-profile wrappers. Two events are defined in the dispatch table but skipped on install: `FileChanged` (its `matcher` doubles as a per-repo file watch list, so an empty matcher would ship dead config) and `MessageDisplay` (no matcher, fires on every assistant message, and emits `hookSpecificOutput.displayContent` — auto-installing would be a no-op or would silently rewrite on-screen text, so it stays opt-in). Ad-hoc invocations like `claude-skills hook file-changed` and `claude-skills hook message-display` still work.

## Preserve Existing Flow Evidence

Existing source-file edits use a native preserve-flow gate before implementation. Docs-only, formatting-only, generated-only, and explicitly greenfield work are exempt; established source behavior needs owner-path evidence before review gates pass.

```bash
claude-skills flow start --target-file rust/crates/claude-skills/src/commands.rs --target-function Application::run
claude-skills flow check
claude-skills flow finish
```

The default artifact is `~/.claude/memories/workspaces/<workspace-slug>/flow/flow-check.json`. It records the target file or function, current behavior to preserve, entry point, producer, source of truth, storage/state/queue owner, side-effect owner, consumers, cleanup/recovery path, edit boundary, validation needed, and validation evidence. The schema is documented in `docs/flow-check-schema.md`, and native review blocks existing source edits when that artifact is missing or incomplete.

## Professional Text Templates

Commit bodies, PR bodies, final responses, and review summaries should stay professional, concise, and scoped to the actual diff. Central templates live in `templates/commit-body.md`, `templates/pr-body.md`, `templates/final-response.md`, and `templates/review-summary.md`.

```bash
claude-skills git-workflow commit-message --from-diff --test-result "cargo test --workspace passed"
claude-skills git-workflow pr-body --from-diff --test-result "cargo test --workspace passed"
claude-skills git-workflow lint-message .git/COMMIT_EDITMSG
```

The linter rejects chatty language, escaped newline PR bodies, unrelated AI/Claude Code wording, unsupported hype wording, and first-person phrasing. `git-workflow preflight --message-file <path>` and `review pre-pr --pr-body <text>` use the same professional text rules.

## Memory and System Map

### Global project system map

Use the scoped memory path first so the user workspace stays clean:

```bash
claude-skills memory scope resolve --create-missing --refresh-system-map
claude-skills memory system-map refresh
```

The project-scoped global `SYSTEM_MAP.md` target lives under Claude Code-managed memory, not inside the user repo. Use `claude-skills memory system-map refresh` when the map is missing, stale, or contradicted by current code. The generated map records visible top-level folders, files, direct child structure, applications, entrypoints, main flows, and key ownership hints. Use trace-by-function or trace-by-flow from the relevant entrypoint, mark unknown facts as `Not found`, respect generated artifact trees, handle a monorepo or multi-app workspace by app, and read the target file plus traced function or flow before editing. Modified files should keep file doc headers in the native comment style when the scoped rules require them.

Useful memory commands:

```bash
claude-skills memory working-brief write --request "Ship the native workflow layer" --constraints "no Go fallback" --acceptance-criteria "tests green"
claude-skills memory working-brief list
claude-skills memory completion-gate check --id <entry-id> --proof "tests green"
claude-skills memoriesv2 scope resolve --workspace-root "$PWD" --create-missing --refresh-system-map
claude-skills memoriesv2 working-brief list
```

Advanced memory and search surfaces:

- The Rust runtime implements `scope`, `system-map`, `working-brief`, `completion-gate check`, and `recall` for both `memory` and `memoriesv2` command groups, plus the family commands `research-cache` (record/lookup/stale/reward/list), `maintenance` (append-working-buffer/trim/recalibrate), `agent-registry` (register/list), `agent-packets` (build/show/list), `loop-guard` (record/check), `entity` (upsert/list/query), `graph` (add/list/query), `retrieve` (cross-family lexical search), and `status`. Each family persists flat-JSON records under `<claude-home>/<group>/<family>/`, isolated per command group.
- The `orchestration` group implements `runtime-preflight`, `resume-status`, `task` (begin/progress/complete/list), and `checkpoint`.
- Still not implemented (return non-zero with a "not implemented" message): `memory report`, `memory index`, `memory hook`, and `consolidate`.
- Code-search demo details live at [./docs/code-search-demo-and-gap-map.md](./docs/code-search-demo-and-gap-map.md).

## Manager and Operator Surfaces

The interactive manager now keeps five clear choices:

- Doctor: run a report-first diagnostic pass that combines manager state with deep verification and recommends the next command to run.
- Install: sync the managed skill pack into `~/.claude`.
- Update: refresh an existing install from the current checkout or release source.
- Verify: prove managed artifact health.
- Uninstall: remove the managed pack safely.

Release download overrides are available for controlled environments:

- CLAUDE_NATIVE_CLI_RELEASE_METADATA_URL
- CLAUDE_NATIVE_CLI_RELEASE_BASE_URL

## Managed Agent Profiles

The managed install mirrors these 18 specialist lanes into `~/.claude/agent-profiles/*.toml`:

`api-contract-design`, `backend-and-data-architecture`, `cloud-and-devops-expert`, `git-expert`, `memory-status-reporter`, `mobile-development-life-cycle`, `postgres-migration-safety`, `preserve-existing-flow`, `qa-and-automation-engineer`, `react-performance-audit`, `reviewer`, `security-and-compliance-auditor`, `software-development-life-cycle`, `stripe-integration`, `ui-design-systems-and-responsive-interfaces`, `ux-research-and-experience-strategy`, `web-development-life-cycle`, and `websocket-realtime-design`.

Routine work stays in the main lane. Specialist profiles are for the moments where domain ownership or independent verification is worth the extra context.

## Legacy Command Compatibility

The native CLI is the primary surface. Most subcommand shapes earlier docs referenced are now implemented: `orchestration task begin|progress|complete|list`, `orchestration checkpoint`, and `memory|memoriesv2 research-cache|maintenance|agent-registry|agent-packets|loop-guard|entity|graph|retrieve|status` all work today (see [Memory and System Map](#memory-and-system-map) above). A few remain **not yet implemented in the Rust runtime** and return non-zero with a "not implemented" message instead of silently succeeding: `memory working-brief record-summary`, `memory completion-gate record-requirement`, `memory report`, `memory index`, `memory hook`, and `consolidate`. The full working surface is listed above and in `claude-skills help advanced`.

## Documentation Map

| Topic | Link |
| --- | --- |
| First Success Path | [./docs/first-success-path.md](./docs/first-success-path.md) |
| Workflow rules | [./WORKFLOW.md](./WORKFLOW.md) |
| Agent rules | [./AGENTS.md](./AGENTS.md) |
| Compatibility matrix | [./docs/compatibility-matrix.md](./docs/compatibility-matrix.md) |
| Why `claude_skills` over native Claude Code, runtime-shell comparator, and workflow-teaching comparator | [./docs/why-claude-skills.md](./docs/why-claude-skills.md) |
| Competitive gap closure (named comparators + remaining work) | [./docs/competitive-gap-closure.md](./docs/competitive-gap-closure.md) |
| Release notes | [./docs/release-notes.md](./docs/release-notes.md) |
| Release proof bundle | [./docs/release-proof-bundle.md](./docs/release-proof-bundle.md) |
| Audit bundle format | [./docs/audit-bundle-format.md](./docs/audit-bundle-format.md) |
| Security audit status | [./docs/security-audit-status.md](./docs/security-audit-status.md) |
| Benchmark suite | [./docs/benchmark-suite.md](./docs/benchmark-suite.md) |
| Shared benchmark harness | [./docs/shared-benchmark-harness.md](./docs/shared-benchmark-harness.md), the shared benchmark harness contract and common evidence format |
| Benchmark comparison scorecard | [./docs/benchmark-comparison-scorecard.md](./docs/benchmark-comparison-scorecard.md) |
| Memory recall benchmark bundle | [./docs/memory-recall-benchmark-bundle.md](./docs/memory-recall-benchmark-bundle.md) |
| Memory recall audit | [./docs/audits/2026-04-11-memory-recall-benchmark/audit-summary.md](./docs/audits/2026-04-11-memory-recall-benchmark/audit-summary.md) |
| Benchmark posture audit | [./docs/audits/2026-04-09-benchmark-posture/audit-summary.md](./docs/audits/2026-04-09-benchmark-posture/audit-summary.md) |
| Competitive apples-to-apples audit | [./docs/audits/2026-04-09-competitive-apples-to-apples/audit-summary.md](./docs/audits/2026-04-09-competitive-apples-to-apples/audit-summary.md) |
| Demo: PR-fix flow | [./docs/demo-pr-fix-flow.md](./docs/demo-pr-fix-flow.md) |
| Demo: branch-closeout flow | [./docs/demo-branch-closeout-flow.md](./docs/demo-branch-closeout-flow.md) |
| Runtime guardrails and memory protocols | [./docs/runtime-guardrails-and-memory-protocols.md](./docs/runtime-guardrails-and-memory-protocols.md) |
| Open-source memory patterns | [./docs/open-source-memory-patterns.md](./docs/open-source-memory-patterns.md) |
| Context efficiency playbook | [./docs/context-efficiency-playbook.md](./docs/context-efficiency-playbook.md) |

Public claims stay source-backed. A durable audit artifact required before numeric security or governance claims are upgraded, and [./docs/security-audit-status.md](./docs/security-audit-status.md) defines the boundary between published artifacts and unproven claims. [./docs/release-proof-bundle.md](./docs/release-proof-bundle.md) is the durable proof artifact published with notable releases.

[./docs/audits/2026-04-09-competitive-apples-to-apples/audit-summary.md](./docs/audits/2026-04-09-competitive-apples-to-apples/audit-summary.md) is the current published source-backed competitive audit bundle for workflow, memory, and indexing peers.

The benchmark docs track real scenario evidence across 8 flows, including greenfield delivery, stateful fixes, hosted rescue, branch closeout, closure proof, Windows validation, docs governance, and regression hardening.

## Repository Layout

```text
claude_skills/
|- rust/crates/claude-skills     Native install, update, hook, review, flow, and compaction surfaces
|- rust/crates/claude-skills-*   Rust support crates for flow, platform, release assets, and text linting
|- .github/workflows/           native Rust CI and release pipelines
|- .claude-review.json           native review rules
|- AGENTS.md                    agent operating doctrine
|- WORKFLOW.md                  branch and completion rules
```

## Summary

Install `claude_skills` when Claude Code needs a clearer path from request to proof:

- Start work with the workflow shell.
- Keep state in memory and cockpit surfaces.
- Compact noisy command output before it fills context.
- Prove the branch locally and on hosted checks.
- Finish only when the evidence says the scope is actually done.
