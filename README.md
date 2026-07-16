<!--
Purpose: Present the native keel product surface, install paths, and proof-first workflow.
Caller: Contributors, operators, and AI agents onboarding to the managed skill pack.
Dependencies: Native CLI commands, workflow docs, memory surfaces, review gates, and release artifacts.
Main Functions: Explain what to run first, where to find each surface, and how closure proof works.
Side Effects: Sets contributor and operator expectations for the repo-managed native experience.
-->
[![Validate](https://github.com/UntaDotMy/keel/actions/workflows/validate.yml/badge.svg)](https://github.com/UntaDotMy/keel/actions/workflows/validate.yml)

# keel

**Discipline as code for the harness.** A single Rust binary that forces the agent to read the codebase before answering, restate the iron law on every prompt, refresh a structural project map across compactions, write a working brief before non-trivial work, and run a reviewer pass before closeout. No Node, no Python, no daemon.

## The Iron Law

Four rules are restated to the agent on every prompt. You cannot skip them.

- **Read first.** Read SYSTEM_MAP, CLAUDE.md, the owning module, and the existing implementation. Do not propose changes against an imagined version of the file.
- **Understand before building.** Restate what the request actually asks, confirm the user story, and research what is genuinely needed before writing code. No guessing, no assuming, no building against an imagined spec. Correct code that solved the wrong problem still gets thrown away ,  the research that prevents it is always cheaper than the rebuild.
- **Invoke relevant skills.** If there is even a 1% chance a skill applies, use the Skill tool *before* writing code or giving a final answer. The cost of skipping a skill that did apply is shipping a regression.
- **Find the root cause.** Take the symptom as a starting point, not the spec. The real problem is usually one layer below what was asked. Trace the symptom end-to-end against the running code with file:line evidence before changing anything.

## Install in One Paste

Works on macOS, Linux (incl. WSL), and Windows, on x86_64 and arm64 (Apple Silicon, Graviton, Pi). Pick the line for your shell:

```bash
# macOS / Linux / WSL
curl -fsSL https://raw.githubusercontent.com/UntaDotMy/keel/main/install.sh | bash
```

```powershell
# Windows PowerShell
irm https://raw.githubusercontent.com/UntaDotMy/keel/main/install.ps1 | iex
```

```bat
:: Windows CMD
curl -fsSL https://raw.githubusercontent.com/UntaDotMy/keel/main/install.cmd -o install.cmd && install.cmd && del install.cmd
```

The installer detects your OS and architecture, pulls the matching prebuilt binary from [GitHub Releases](https://github.com/UntaDotMy/keel/releases/latest), runs `keel install`, verifies `status`, and cleans up. No Rust toolchain required. Pin a version with `CLAUDE_SKILLS_VERSION=vX.Y.Z`.

**Semantic (vector-recall) build.** The default binary is lexical-only (FTS5). To install the variant with built-in vector semantic recall (sqlite-vec + a 33MB BERT model baked in), pass `--semantic`:

```bash
# macOS / Linux
curl -fsSL https://raw.githubusercontent.com/UntaDotMy/keel/main/install.sh | bash -s -- --semantic
```
```powershell
# Windows PowerShell
& ([scriptblock]::Create((irm https://raw.githubusercontent.com/UntaDotMy/keel/main/install.ps1))) -Semantic
```
```bat
:: Windows CMD
curl -fsSL https://raw.githubusercontent.com/UntaDotMy/keel/main/install.cmd -o install.cmd && install.cmd --semantic && del install.cmd
```

The `--semantic` variant is published for `linux/amd64`, `darwin/arm64`, and `windows/amd64`; other platforms fall back to the lexical-only binary or build from source with `cargo build --release --features semantic`.

## Demo

A 30-second walkthrough of `workflow start -> cockpit -> finish`:

```bash
asciinema play docs/demos/quickstart.cast
```

The cast file ships in this repo. Render to GIF with `agg docs/demos/quickstart.cast docs/demos/quickstart.gif` if your viewer prefers a static asset.

## What You Get

| Surface | What it gives you |
| --- | --- |
| Brownfield gate (unique) | `preserve-existing-flow` forces owner-path evidence before editing established source. Review gates block edits when the flow-check artifact is missing. No other harness has this. |
| Iron-law hooks | SessionStart loads the bootstrap skill, UserPromptSubmit restates the four rules, PostToolBatch nudges a reviewer pass, PreCompact refreshes SYSTEM_MAP. |
| Workflow CLI | `workflow start`, `workflow route`, `workflow cockpit`, `workflow finish` ,  proof-first delivery rails. |
| Review gates | `review pre-pr` / `review pre-commit`, review strictness via plugin `userConfig.review_strictness`, and CI-ready artifacts so non-trivial code never self-reviews. |
| Memory | Working briefs, completion ledgers, scoped `SYSTEM_MAP.md`, and durable recovery state under `~/.claude/memories/`. |
| Command compaction | `keel run -- <cmd>` produces compact output for noisy test/build/lint/log/search commands without dropping diagnostic signal. |
| MCP server | `keel mcp serve` (stdio: **one process per host session**; concurrent in-flight tools via `KEEL_MCP_MAX_INFLIGHT`, default 64; shared recall DB uses SQLite WAL + busy_timeout) and `keel mcp serve-http` (Streamable HTTP multi-client on `127.0.0.1:3920` by default). Registered through the plugin manifest so the harness auto-discovers the tool surface (count asserted by `tests/doc_parity_test.rs` via `"inputSchema":` in `mcp/tools.rs`) ,  `recall`, `system_map`, `run_command`, `recall_status`, `skill_route`, `skill_get`, `skill_list`, `memory_status`, `brief_list`, `brief_get`, `brief_create`, `system_map_refresh`, `context_brief`, `cli`, `sprint`, `user_story_lint`, `review`, `workflow`, `git_workflow`, `memory`, `gain`, `raw`, `config_audit`, `skill_lint`, `telemetry`, `orchestration`, `checkpoint`, `session`, `doctor`, `code_search`, `user_story` ,  plus system-map and recall-status resources. |
| Slash commands | `/keel:workflow`, `/keel:review`, `/keel:recall`, `/keel:gain`, `/keel:sprint`, `/keel:user-story` ,  six discoverable `/`-menu wrappers over implemented CLI surfaces. Shipped via the plugin manifest `commands` key. |
| Specialist skills | Manifest-driven specialist profiles synced into `~/.claude/agent-profiles/*.toml`, invokable via the Skill tool. Run `keel skill-lint` for the live verified count. |

## Use as a harness Plugin

This repo ships a `.claude-plugin/plugin.json` manifest. From inside the harness:

```text
/plugin marketplace add UntaDotMy/keel
/plugin install keel@keel
```

That mounts the skills, agents, and hooks without running the native installer. Use the one-paste installer above when you want the full `keel` CLI for workflow, memory, and command-compaction surfaces.

---

## Native Command Routing ,  Must Follow First

When a native `keel` command owns the job, use it instead of recreating the behavior with raw shell, generic search, or ad hoc instructions.

**Token-saving rule:** the goal is to prevent noisy raw command output from entering the harness context. Do not run a raw noisy command first and compact afterward; route through `keel run -- <command>` or the hook-provided `Rerun that as:` wrapper before noisy output is produced.

- **Noisy shell commands:** prefer `keel run -- <command>` for test, build, lint, log, status, search, Docker, Kubernetes, Terraform, package-manager, and CI-style commands. Use `keel rewrite "<command>"` when unsure whether a command has native compaction.
- **Hook block-and-rerun:** if the managed `PreToolUse` hook returns `Rerun that as: <command>`, immediately run that exact command. Do not ask the user, do not treat the hook block as a task failure, and do not repeat the raw command first.
- **Repository search:** prefer `keel code-search search --workspace-root "$PWD" --query "<query>"`. Use raw `rg`, `grep`, `find`, or `git grep` only after scoped search is insufficient, and pipe noisy raw search through `keel run --`.
- **Existing-source edits:** run or validate Preserve Existing Flow evidence first. Use `keel flow start`, `keel flow check`, and `keel flow finish`, and record the owner path in `~/.claude/memories/workspaces/<workspace-slug>/flow/flow-check.json` before patching.
- **Commit/PR/final response text:** use `keel git-workflow commit-message --from-diff`, `keel git-workflow pr-body --from-diff`, and `keel git-workflow lint-message <file>` against the templates in `templates/commit-body.md`, `templates/pr-body.md`, `templates/final-response.md`, and `templates/review-summary.md` before submitting. Run `keel review pre-pr` and `keel review gates check` before finalizing.

For agent-facing usage in markdown or JSON, run `keel hook instructions` (see also [`docs/hook-usage.md`](docs/hook-usage.md)).

## Hook Retry Handling

The managed hook may return a harness denial whose reason begins with `Rerun that as:`. This is expected behavior, not a failure.

When that happens:
1. Copy the command after `Rerun that as:`.
2. Run it exactly once.
3. Preserve the resulting exit code and output.
4. Continue from the compacted output.
5. Do not ask the user for permission unless the suggested command itself is destructive or outside the requested task.

Example: a raw `cargo test --workspace` may produce `Rerun that as: keel run -- cargo test --workspace`; the correct next action is to run `keel run -- cargo test --workspace` and continue from its compacted output.

## Start Here

| Need | Run | Why |
| --- | --- | --- |
| First install, no Rust required | Download a release, extract it, run `./keel install` or `.\keel.exe install` | Installs the native binary and managed skills into the harness home. |
| Check the install | `~/.claude/keel status` or `%USERPROFILE%\.claude\keel.exe status` | Confirms the managed harness-home surface. |
| Start normal work | `keel workflow start --request "..."` | The lowest-friction first run. |
| Route a broad request first | `keel workflow route --request "..."` | Picks the recommended preset before starting. |
| See live state | `keel workflow cockpit` | Shows stage, proof, blockers, and next command. |
| Close a branch | `keel workflow finish` | The default closeout path. |

The default operator path is `workflow start -> workflow cockpit -> workflow finish`; the default closeout path is `keel workflow finish`.

After install, the preferred global CLI path for agents on supported operating systems is:

- macOS or Linux: `~/.claude/keel`
- Windows: `~/.claude/keel.exe`

This matters because the install metadata remembers the source bundle or checkout so `status`, `update`, `verify`, `doctor`, and `menu` can still work when the installed binary is called from another project. For AI-agent or shell contexts where PATH resolution is not guaranteed, prefer the explicit installed path in the harness home root. `--repo-root <path>` is an advanced override for CI, unusual layouts, or running the binary from a different folder than the extracted release/source checkout.

## Install Details

After running the one-paste installer above, verify with:

```bash
~/.claude/keel status              # macOS / Linux
& "$env:USERPROFILE\.claude\keel.exe" status   # Windows PowerShell
```

### Manual Release Install

Download the archive for your OS from GitHub Releases, extract it, open a terminal in the extracted folder, then run `./keel install` or `.\keel.exe install`. Archives are named like `keel_<version>_<os>_<arch>`. The release bundle includes the native binary plus the managed skill files, so Rust/Cargo is not required for normal install.

### Contributors: install from source

```bash
git clone https://github.com/UntaDotMy/keel.git
cd keel
cargo run --bin keel -- install
cargo run --bin keel -- status
```

Use `--repo-root <path>` only when you intentionally run `keel install` from outside the extracted release folder or source checkout.

### Native Update

```bash
~/.claude/keel update
~/.claude/keel verify
~/.claude/keel status
```

```powershell
& "$env:USERPROFILE\.claude\keel.exe" update
& "$env:USERPROFILE\.claude\keel.exe" verify
& "$env:USERPROFILE\.claude\keel.exe" status
```

The Rust manager remembers the source checkout in install metadata, fast-forwards that checkout on `update`, rebuilds the native CLI when needed, delta-syncs changed files, removes stale managed files, and preserves unrelated harness-home files. Shell and PowerShell wrapper launchers are no longer shipped.

On Windows, install replaces the running `keel.exe` synchronously via `MoveFileEx(MOVEFILE_REPLACE_EXISTING)` (the same trick rustup uses) instead of a detached `cmd /C copy`. Failures now surface as install errors instead of leaving a stale binary on disk. When the source and the deployed binary are byte-identical, the swap is skipped entirely so a no-op `update` does not touch the executable.

### After Install

Run these once after a fresh install or update:

```bash
~/.claude/keel.exe verify     # confirms inventory + binary match the source
~/.claude/keel.exe doctor     # probes hooks end-to-end, reports any drift
~/.claude/keel.exe status     # version, repo SHA, install timestamp
```

Hooks are wired automatically by `install`. If you want to refresh `~/.claude/settings.json` without a full reinstall:

```bash
~/.claude/keel.exe hook install
```

Optional environment variables:

| Variable | Default | Purpose |
| --- | --- | --- |
| `CLAUDE_SKILLS_RAW_RETENTION_DAYS` | `14` | Days of `~/.claude/raw-output/` runs kept on disk. SessionEnd hook prunes anything older. Set to `0` to disable auto-prune. |
| `CLAUDE_SKILLS_VERSION` | latest | Pin the bootstrap installer to a specific release tag. |

Manual prune (any time):

```bash
~/.claude/keel.exe raw prune --older-than 30d
```

## Slash Commands

When installed as a plugin, keel registers six namespaced slash commands
(see the `commands` key in `.claude-plugin/plugin.json`). Each is a thin,
discoverable `/`-menu wrapper over an **implemented** `keel` CLI surface
,  none of them invoke planned-but-unimplemented commands.

| Command | Wraps | Use it for |
| --- | --- | --- |
| `/keel:workflow [route\|start\|cockpit\|finish] <args>` | `workflow` ledger | Drive a proof-first workstream. |
| `/keel:review [pre-commit\|pre-pr\|gates] [base-ref]` | `review` gates | Deterministic local quality gate on the diff. |
| `/keel:recall <terms>` | `memory recall` | FTS5 search over durable memory. |
| `/keel:gain [since]` | `gain` | Report command-output compaction savings. |
| `/keel:sprint [plan\|status\|advance\|review\|list] <args>` | `sprint` ledger | Drive a Scrum-style sprint over confirmed stories. |
| `/keel:user-story [lint] <args>` | `user-story lint` | Validate Connextra + Gherkin + INVEST story format. |

Command files live at the plugin root `commands/`. They ship through the plugin
install path, and the native `keel install` also syncs them into
`~/.claude/commands/` via its `sync_commands` arm (alongside `sync_skills` and
`sync_agents`), so they work whether installed through the plugin path or the
native installer.

## Statusline (opt-in)

A cross-platform statusline script renders the active model, context usage, and a
**compaction-savings badge** sourced from `keel gain --json` (the badge
is omitted when the binary or savings data is unavailable, and the line never
errors). It is opt-in ,  keel does not overwrite your `statusLine` setting.

`statusline/statusline-keel.sh` (macOS/Linux) and
`statusline/statusline-keel.ps1` (Windows). To enable, point your
`settings.json` `statusLine.command` at the script:

```json
{
  "statusLine": {
    "type": "command",
    "command": "~/.claude/statusline-keel.sh"
  }
}
```

```json
{
  "statusLine": {
    "type": "command",
    "command": "powershell -NoProfile -ExecutionPolicy Bypass -File %USERPROFILE%\\.claude\\statusline-keel.ps1"
  }
}
```

Both scripts read the documented statusline session JSON on stdin and print one
line such as `Opus | ctx 8% | saved 3800 tok`.


### Cache Hygiene and Token Economy

The hook lifecycle is tuned to preserve the harness's prompt cache and minimize per-prompt input tokens.

- **What stays cached:** the system prompt, tool definitions, `CLAUDE.md`, and the SessionStart context are read at the cache breakpoint. Reuse costs ~10% of normal input tokens for ~5 minutes after each write.
- **What gets paid every prompt:** `UserPromptSubmit` injects a short research-first iron-law restatement (~80 tokens) via `additionalContext`. The full bootstrap skill, Red Flags table, and skill catalog ride on `SessionStart` so per-prompt cost stays small while the iron law stays top-of-mind every turn.
- **What gets paid every turn end / tool call:** `Stop`, `SubagentStop`, `SessionEnd`, and `PostToolUse` are silent. `PostToolBatch` injects a short reviewer-on-close reminder before the next model turn. Earlier versions of the lifecycle emitted ~50 tokens of generic closeout text on every tool call; that overhead is gone.
- **Why this matters:** the per-prompt and per-batch hooks are sized to carry information the model genuinely uses. The system prompt, tool definitions, `CLAUDE.md`, and `SessionStart` context stay above the cache breakpoint so they reuse cleanly within the 5-minute cache window.

If you customize hooks downstream and want to see exactly what the lifecycle emits for an event:

```bash
echo '{}' | ~/.claude/keel.exe hook stop
echo '{}' | ~/.claude/keel.exe hook user-prompt-submit
```

Empty stdout means the hook is intentionally silent for that event.

## Find Fast

| Job | Commands |
| --- | --- |
| Route a broad request | `keel workflow route --request "..."` |
| Start work | `keel workflow start --preset autopilot --request "..."` |
| Watch live state | `keel workflow cockpit`, `keel workflow dashboard`, `keel workflow watch` |
| Review locally | `keel review pre-commit`, `keel review pre-pr`, `keel review gates check` |
| Finish a workstream | `keel workflow finish --id <entry-id> --proof "..."`, `gh pr checks --watch` |
| Compact noisy commands | `keel rewrite "cargo test --workspace"`, `keel run -- grep -RIn TODO rust` |
| Refresh memory map | `keel memory scope resolve --create-missing --refresh-system-map` |
| Advanced help | `keel help advanced` |

External output-compaction tools are feature benchmarks for expected output reduction and recoverability, not runtime dependencies. The default path stays the native Rust implementation because it is integrated with the harness hooks, Preserve Existing Flow, review gates, install/update, repository instructions, raw-output recovery, and persisted `gain` analytics.

See [Native Gap Map](docs/native-gap-map.md) for the anonymized comparison between external output reducers, runtime-shell peers, and the current native implementation.

Route, start, watch, cockpit, and finish now share one operator-shell vocabulary: `stage`, `active_lane`, `proof_state`, `blocker`, `next_command`, and `recovery_path`. The workflow shell also keeps the active launch surface intact, so source-checkout runs use `cargo run --bin keel -- ...` and installed runs keep using the installed executable.

Start with the preset-driven native CLI when the operator wants a top-layer product surface: Use `workflow route`, `workflow start --preset ...`, `workflow cockpit`, and `workflow finish` for most delivery work.

## Daily Paths

Quick labels: Feature work: Bug fixing: PR rescue: TDD-first implementation: Bounded parallel work:

### Feature work

```bash
keel workflow route --request "Add the next feature and carry it to closure"
keel workflow start --preset autopilot --request "Add the next feature and carry it to closure"
keel workflow cockpit
keel workflow finish --id <entry-id> --proof "tests green"
```

### Bug fixing

```bash
keel workflow route --request "Trace the regression, fix the root cause, and prove it"
keel workflow start --preset debug --request "Trace the regression, fix the root cause, and prove it"
keel workflow cockpit
keel workflow finish --id <entry-id> --proof "regression covered by tests"
```

### Review

```bash
keel workflow route --request "Audit the current branch and call out the real gaps"
keel workflow start --preset review --request "Audit the current branch and call out the real gaps"
keel workflow cockpit
keel workflow finish --id <entry-id> --proof "review notes recorded"
```

### TDD-first implementation

```bash
keel workflow start --preset tdd --request "Write the failing test first, implement the smallest fix that makes it pass, and close with regression proof"
keel workflow cockpit
keel workflow finish --id <entry-id> --proof "failing -> fix -> regression proof"
```

### Common job shapes

Feature work, Bug fixing, PR rescue, TDD-first implementation, and Bounded parallel work all use the same visible loop: route, start, cockpit, prove, finish. Hosted-check failures are repaired on the same PR with `gh pr checks --watch` and another push.

### Native guidance tracks

Brainstorming:

```bash
keel workflow route --request "Brainstorm the approach, compare the options, and recommend the right next lane"
```

Plan writing:

```bash
keel workflow route --request "Write the implementation plan, file targets, proof steps, and recovery path before coding"
```

Plan execution:

```bash
keel workflow start --preset autopilot --request "Carry the approved plan to closure"
```

Systematic debugging:

```bash
keel workflow start --preset debug --request "Trace the regression, find the root cause, and prove the real fix"
```

Code review:

```bash
keel workflow start --preset review --request "Review the branch, call out the real gaps, and decide if it is ready"
```

Workstream finish:

```bash
keel workflow finish --id <entry-id> --proof "tests green; review pre-pr passed"
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

Useful workflow command shelf: `keel workflow route`, `keel workflow start --preset <preset> --request "..."`, `keel workflow cockpit`, `keel workflow status`, `keel workflow dashboard`, `keel workflow watch`, `keel workflow resume --id <entry-id>`, and `keel workflow finish --id <entry-id> --proof "..."`.

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

`.claude/review.json` is the tracked repo-level rule file.

- keel review pre-commit is the local pre-commit surface.
- keel review pre-pr is the local pre-PR surface.
- The cockpit proof view keeps a live proof board.

```bash
keel review pre-commit --format compact
keel review pre-pr --base-ref origin/feat --format compact
keel review gates check --surface pre-pr --base-ref origin/feat --format compact
cargo test --workspace
```

For heavier Rust validation, run the release build after the workspace test proof.

```bash
cargo build --release --bin keel
cargo fmt --all --check
```

```powershell
cargo build --release --bin keel
cargo fmt --all --check
```

Hosted PR discipline:

1. Run local proof.
2. Push one cohesive `<category>/<FEATURE>` work branch (branched off `feat`). Never delete the branch after push or merge.
3. Open the PR against `feat`.
4. Wait at least 20 seconds for hosted checks to appear. In checklists this is written as: wait at least 20 seconds.
5. Watch `gh pr checks --watch`.
6. If a hosted lane fails, inspect the failing logs, fix the root cause on the same PR, push again, and rerun `gh pr checks --watch`.

Branch model: `main` (final stable, verified) ← `dev` (staging verification) ← `feat` (feature integration) ← `<category>/<FEATURE>` work branch (all hands-on commits; branch off `feat`). Fixes for in-flight work stay on the same work branch, never a new branch. Commit subjects strictly follow `<category>: <FEATURE>: <short information>` (categories lowercase: add, config, refactor, wip, fix, docs; FEATURE uppercase, e.g. `wip: RGB: Build light effect mode (multi color)`).

Run `keel git-workflow preflight --repo-root . --base-ref origin/feat` before push or merge-request creation (`origin/dev` when promoting `feat` to `dev`; `origin/main` only when promoting `dev` to `main`).

The validate workflow is fail-closed: repo-wide Rust proof, native review artifacts, cross-platform manager loops, and the summary must pass.

## Command Output Compaction

Use the Rust-native command proxy before noisy shell commands when you want `keel` to prevent raw output from entering the agent transcript. The proxy executes the command, captures stdout/stderr outside context, saves raw recovery files under `~/.claude/raw-output/YYYY-MM-DD/<raw_id>/`, runs a command-specific semantic adapter when one matches, falls back to generic high-signal compaction only when needed, preserves the original exit code, and records exact `o200k_base` before/after token savings in the native JSONL event log.

```bash
keel rewrite "cargo test --workspace"
keel run -- cargo test --workspace
keel run --json -- pytest tests -q
keel run -- git status
keel run -- rg "CompactResult" rust
keel gain --since today
keel gain discover --since today
keel raw <raw_id>
keel doctor
```

What is implemented today:

- `run` executes the requested command, saves `stdout.log`, `stderr.log`, `command.txt`, `meta.json`, and `compact.txt`, and returns compact output with `raw: keel raw <raw_id>`.
- `run --json` returns `command`, `exit_code`, `adapter_name`, `compacted`, `raw_id`, `raw_path`, exact token fields, `summary`, `stdout`, and `stderr`.
- `run --full` and `run --no-compact` pass through raw output while still recording metadata; `--adapter <name>`, `--list-adapters`, `--max-lines <n>`, and `--recovery-dir <path>` are available for debugging and control. `--errors-only` keeps only error/failure-class lines from any command (adapter-agnostic). `--ultra` uses a short failure-first body and a compact raw pointer.
- Built-in adapters cover `tests`, `git`, `search`, `files`, `build`, `lint`, `containers`, `cloud`, `database`, `logs`, and `generic` fallback. Test adapters handle cargo/pytest/go/JS-style failure signals; git/search/files adapters summarize diffs, matches, and large reads; the `containers` adapter compacts docker/kubectl/helm; the `cloud` adapter reduces aws/az/gcloud output (structure-only JSON, secret redaction, failure-first); the `database` adapter reduces psql/mysql/sqlite3/redis-cli/mongosh result sets (header + sampled rows, structure-only JSON, credential redaction).
- `raw <raw_id>`, `raw --path <raw_id>`, `raw list`, `raw prune --older-than 30d`, and `replay <raw_id>` provide local recovery and retention controls.
- `rewrite --json "<command>"` returns supported/reason/rewritten-command metadata and understands common shell wrappers, environment prefixes, and pipelines by routing them through `bash -lc` when needed.
- `hook install` writes the documented global the harness lifecycle hook set, with `PreToolUse` handling block-and-rerun command compaction.
- `hook instructions` prints the agent-facing rerun contract in markdown or JSON.
- `gain` reads native compaction events from the harness home and reports observed commands, compacted/passthrough counts, exact tokens before/after/saved, savings percentage, adapter breakdowns, and top commands.
- `gain discover` reports missed-savings opportunities: commands that ran through the proxy but were not compacted (passthrough), grouped by command with the estimated uncompacted tokens that entered context. `gain` reports what was saved; `discover` reports what was left on the table.
- `doctor` checks the binary, raw store, event log, adapter registry, rewrite behavior, and hook/proxy setup with ok/warn/fix-style output.
- The runtime never shells out to Go for compaction, hooks, or command dispatch.

Example compact outputs:

```text
PASS cargo test --workspace
test result: ok. 42 passed; 0 failed; finished in 0.16s

raw: keel raw 20260512-102221-303d93eb
saved: 912 tokens exact/o200k_base (91.8%)
```

```text
FAIL pytest tests -q
2 failed, 143 passed in 12.8s

failures:
tests/api/test_users.py::test_create_user FAILED
E AssertionError: expected 201, got 500
tests/api/test_users.py:88

raw: keel raw <raw_id>
saved: <measured> tokens exact/o200k_base
```

Limitations and safety:

- Hooks may not intercept every host or shell path; explicit `keel run -- <command>` is the reliable path.
- Token counts use `tiktoken-rs` with the `o200k_base` tokenizer; compatibility JSON fields may still be named `estimated_tokens_*`, but their values are exact tokenizer counts.
- Raw output stays local and is not uploaded, but it can contain secrets; manage retention with `keel raw prune --older-than 30d`.
- Compaction redacts obvious secret-looking lines in compact output, but raw recovery preserves what the command printed locally.

### Hook path

The one-line installer refreshes the managed harness hooks automatically, and `keel hook install` can refresh them manually. The hook set is written to `~/.claude/hooks.json`. `PreToolUse` keeps the `Bash` matcher because command-output wrapping is scoped to shell commands; the other lifecycle events use native lifecycle handlers.

```json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Bash",
        "hooks": [
          {
            "type": "command",
            "command": "\"/path/to/keel\" hook pre-tool-use",
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
            "command": "\"/path/to/keel\" hook session-start",
            "statusMessage": "Preparing native session state"
          }
        ]
      }
    ]
  }
}
```

The hook contract is explicit rerun guidance rather than hidden command mutation. The Rust hook installer manages **18 of the 30** lifecycle events in the `HOOK_EVENTS` table (`rust/crates/keel/src/hooks/claude.rs`), writing them to `~/.claude/settings.json`: `PreToolUse`, `PostToolUse`, `PostToolUseFailure`, `PostToolBatch`, `PermissionRequest`, `PermissionDenied`, `Notification`, `UserPromptSubmit`, `UserPromptExpansion`, `Stop`, `StopFailure`, `SubagentStart`, `SubagentStop`, `CwdChanged`, `PreCompact`, `PostCompact`, `SessionStart`, and `SessionEnd`. `PreToolUse` owns Iron Law edit-gating plus command compaction before noisy output exists. `SessionStart` delivers the bootstrap skill once per session, `UserPromptSubmit` injects a short research-first iron-law restatement per prompt, and `PostToolBatch` injects the reviewer-on-close reminder before each next turn. Twelve events stay dispatchable but are not auto-installed: reserved no-ops (`TaskCreated`, `TaskCompleted`, `TeammateIdle`, `WorktreeCreate`, `WorktreeRemove`, `Setup`, `InstructionsLoaded`, `ConfigChange`, `Elicitation`, `ElicitationResult`) and structural opt-outs (`FileChanged` ,  matcher is the watch list; `MessageDisplay` ,  would rewrite on-screen text). Ad-hoc invocations like `keel hook file-changed` and `keel hook message-display` still work.

## Preserve Existing Flow ,  The Brownfield Gate (Unique to keel)

This is keel's headline differentiator: **no other harness in the market forces owner-path evidence before editing established source code.** When an agent touches an existing file, `preserve-existing-flow` requires tracing who owns the current behavior, what the source of truth is, and what consumers depend on it ,  before any edit is made. Review gates block the edit when the flow-check artifact is missing or incomplete.

Docs-only, formatting-only, generated-only, and explicitly greenfield work are exempt; established source behavior needs owner-path evidence before review gates pass.

```bash
keel flow start --target-file rust/crates/keel/src/commands.rs --target-function Application::run
keel flow check
keel flow finish
```

The default artifact is `~/.claude/memories/workspaces/<workspace-slug>/flow/flow-check.json`. It records the target file or function, current behavior to preserve, entry point, producer, source of truth, storage/state/queue owner, side-effect owner, consumers, cleanup/recovery path, edit boundary, validation needed, and validation evidence. The schema is documented in `docs/flow-check-schema.md`, and native review blocks existing source edits when that artifact is missing or incomplete.

## Professional Text Templates

Commit bodies, PR bodies, final responses, and review summaries should stay professional, concise, and scoped to the actual diff. Central templates live in `templates/commit-body.md`, `templates/pr-body.md`, `templates/final-response.md`, and `templates/review-summary.md`.

```bash
keel git-workflow commit-message --from-diff --test-result "cargo test --workspace passed"
keel git-workflow pr-body --from-diff --test-result "cargo test --workspace passed"
keel git-workflow lint-message .git/COMMIT_EDITMSG
```

The linter rejects chatty language, escaped newline PR bodies, unrelated AI/the harness wording, unsupported hype wording, and first-person phrasing. `git-workflow preflight --message-file <path>` and `review pre-pr --pr-body <text>` use the same professional text rules.

## Memory and System Map

### Global project system map

Use the scoped memory path first so the user workspace stays clean:

```bash
keel memory scope resolve --create-missing --refresh-system-map
keel memory system-map refresh
```

The project-scoped global `SYSTEM_MAP.md` target lives under the harness-managed memory, not inside the user repo. Use `keel memory system-map refresh` when the map is missing, stale, or contradicted by current code. The generated map records visible top-level folders, files, direct child structure, applications, entrypoints, main flows, and key ownership hints. Use trace-by-function or trace-by-flow from the relevant entrypoint, mark unknown facts as `Not found`, respect generated artifact trees, handle a monorepo or multi-app workspace by app, and read the target file plus traced function or flow before editing. Modified files should keep file doc headers in the native comment style when the scoped rules require them.

Useful memory commands:

```bash
keel memory scope resolve --workspace-root "$PWD" --create-missing --refresh-system-map
keel memory working-brief write --request "Ship the native workflow layer" --constraints "no Go fallback" --acceptance-criteria "tests green"
keel memory working-brief list
keel memory completion-gate check --id <entry-id> --proof "tests green"
```

Advanced memory and search surfaces:

- The unified `keel memory` group implements `scope`, `system-map`, `working-brief`, `completion-gate check`, and `recall`, plus family commands `research-cache` (record/lookup/stale/reward/list), `maintenance` (append-working-buffer/trim/recalibrate), `agent-registry` (register/list), `agent-packets` (build/show/list), `loop-guard` (record/check), `entity` (upsert/list/query), `graph` (add/list/query), `retrieve` (cross-family lexical search), and `status`. Family records live under the unified memory layout (see `docs/memory-families-usage.md`).
- The `orchestration` group implements `runtime-preflight`, `resume-status`, `task` (begin/progress/complete/list), and `checkpoint`.
- `memory report` (alias for `status`), `memory index` (rebuilds the recall index), and `instincts` are also implemented. `memory hook` is intentionally not a memory subcommand; it points to `keel hook ...`. `memory working-brief record-summary`, `memory completion-gate record-requirement`, and `consolidate` are also implemented.
- Code-search details: [./docs/code-search-demo-and-gap-map.md](./docs/code-search-demo-and-gap-map.md).

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

## Cross-Agent Adapters

keel works with multiple AI coding agents through dedicated adapters. Each adapter injects keel's iron law, skill catalog, and operating instructions into the target agent.

| Agent | Adapter Type | Mechanism | Files |
| --- | --- | --- | --- |
| **Claude Code** (native) | Plugin manifest + hooks | `.claude-plugin/plugin.json` + `~/.claude/settings.json` hooks ,  automatic via `keel install` | `.claude-plugin/` |
| **Claude Desktop** (Cowork) | TypeScript plugin | `cowork/keel.ts` ,  lifecycle bridge with `bridge` subcommands per event | `cowork/` |
| **OpenCode** | TypeScript plugin | `opencode/keel.ts` ,  lifecycle bridge with `bridge` subcommands per event | `opencode/` |
| **Codex CLI** | Plugin + hooks + script | `codex/.codex-plugin/plugin.json` + `hooks/hooks.json` + `keel-codex.ts` | `codex/` |
| **Cursor IDE** | Rules + hooks + MCP | `cursor/.cursorrules` + `cursor/hooks/` + `cursor/mcp.json`: iron law, lifecycle bridge (`keel bridge`), MCP tools. Install with `keel install --with cursor` (Cursor is not always auto-detected) | `cursor/` |
| **Pi Agent** | Rules + hooks + MCP | `pi/AGENTS.md` + `pi/hooks.json` + `pi/keel-pi.ts` + `pi/.mcp.json`: iron law, lifecycle bridge, MCP tools | `pi/` |

Claude Code is the primary target (native hooks, full lifecycle). OpenCode, Codex, Cowork, Cursor, and Pi ship runtime bridges that map host events to `keel bridge` (edit gate, rewrite, observe, session-end learn). Cursor often needs `--with cursor` because desktop IDEs are not always detected.

`keel install` auto-detects which AI CLIs are installed (via config dirs, env vars, and binary-on-PATH) and wires only the matching adapters. Use `--with <name>` to force an adapter even when not detected (e.g. `--with cursor`), and `--without <name>` to skip a detected adapter (e.g. `--without opencode`). Names: `opencode`, `codex`, `pi`, `cursor`, `cowork`. Manual file copying is no longer required.

## Managed Agent Profiles

The managed install mirrors the specialist lanes (one profile per specialist, roster asserted by `tests/doc_parity_test.rs`) into `~/.claude/agent-profiles/*.toml`:

`api-contract-design`, `authentication-and-identity`, `backend-and-data-architecture`, `cloud-and-devops-expert`, `cloud-cost-and-finops`, `dart-and-flutter-expert`, `data-and-ml-engineering`, `dependency-and-supply-chain`, `git-expert`, `internationalization-and-localization`, `memory-status-reporter`, `mobile-development-life-cycle`, `observability-and-incident-response`, `postgres-migration-safety`, `preserve-existing-flow`, `qa-and-automation-engineer`, `react-performance-audit`, `reviewer`, `security-and-compliance-auditor`, `software-development-life-cycle`, `stripe-integration`, `ui-design-systems-and-responsive-interfaces`, `ux-research-and-experience-strategy`, `web-development-life-cycle`, and `websocket-realtime-design`.

Routine work stays in the main lane. Specialist profiles are for the moments where domain ownership or independent verification is worth the extra context.

## Legacy Command Compatibility

The native CLI is the primary surface. Most subcommand shapes earlier docs referenced are now implemented: `orchestration task begin|progress|complete|list`, `orchestration checkpoint`, and unified `memory` family verbs (`research-cache|maintenance|agent-registry|agent-packets|loop-guard|entity|graph|retrieve|status|instincts`) all work today, as do `memory report` (alias for `status`) and `memory index`. `memory working-brief record-summary`, `memory completion-gate record-requirement`, and `consolidate` are also implemented. `memory hook` points you to `keel hook ...`. The full working surface is listed above and in `keel help advanced`.

## Documentation Map

| Topic | Link |
| --- | --- |
| First Success Path | [./docs/first-success-path.md](./docs/first-success-path.md) |
| Workflow rules | [./WORKFLOW.md](./WORKFLOW.md) |
| Agent rules | [./AGENTS.md](./AGENTS.md) |
| Compatibility matrix | [./docs/compatibility-matrix.md](./docs/compatibility-matrix.md) |
| Why `keel` over native harness, runtime-shell comparator, and workflow-teaching comparator | [./docs/why-keel.md](./docs/why-keel.md) |
| Competitive gap closure (named comparators + remaining work) | [./docs/competitive-gap-closure.md](./docs/competitive-gap-closure.md) |
| Release notes | [./docs/release-notes.md](./docs/release-notes.md) |
| Release proof bundle | [./docs/release-proof-bundle.md](./docs/release-proof-bundle.md) |
| Audit bundle format | [./docs/audit-bundle-format.md](./docs/audit-bundle-format.md) |
| Security audit status | [./docs/security-audit-status.md](./docs/security-audit-status.md) |
| Benchmark suite | [./docs/benchmark-suite.md](./docs/benchmark-suite.md) |
| Shared benchmark harness | [./docs/shared-benchmark-harness.md](./docs/shared-benchmark-harness.md), the shared benchmark harness contract and common evidence format |
| Benchmark comparison scorecard | [./docs/benchmark-comparison-scorecard.md](./docs/benchmark-comparison-scorecard.md) |
| Memory families inventory | [./docs/memory-families-usage.md](./docs/memory-families-usage.md) |
| Memory recall audit (historical) | [./docs/audits/2026-04-11-memory-recall-benchmark/audit-summary.md](./docs/audits/2026-04-11-memory-recall-benchmark/audit-summary.md) |
| Benchmark posture audit (historical) | [./docs/audits/2026-04-09-benchmark-posture/audit-summary.md](./docs/audits/2026-04-09-benchmark-posture/audit-summary.md) |
| Competitive apples-to-apples audit (historical) | [./docs/audits/2026-04-09-competitive-apples-to-apples/audit-summary.md](./docs/audits/2026-04-09-competitive-apples-to-apples/audit-summary.md) |
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
keel/
|- rust/crates/keel          Native install, update, hook, review, flow, and compaction surfaces
|- rust/crates/keel-*        Rust support crates for flow, platform, release assets, and text linting
|- cowork/                    Claude Desktop (Cowork) adapter (TypeScript plugin with lifecycle bridge)
|- opencode/                 OpenCode adapter (TypeScript plugin with lifecycle bridge)
|- codex/                    Codex CLI adapter (plugin + hooks + TypeScript bridge)
|- cursor/                   Cursor IDE adapter (rules + hooks + MCP)
|- pi/                       Pi Agent adapter (static AGENTS.md + MCP config)
|- .claude-plugin/           Native Claude Code plugin manifest
|- .github/workflows/        Native Rust CI and release pipelines
|- .claude/review.json       Native review rules
|- AGENTS.md                 Agent operating doctrine
|- WORKFLOW.md               Branch and completion rules
```

## Summary

Install `keel` when the harness needs a clearer path from request to proof:

- Start work with the workflow shell.
- Keep state in memory and cockpit surfaces.
- Compact noisy command output before it fills context.
- Prove the branch locally and on hosted checks.
- Finish only when the evidence says the scope is actually done.
