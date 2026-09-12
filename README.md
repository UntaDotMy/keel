<!--
Purpose: Introduce the native keel CLI and the managed harness pack.
Caller: Operators, contributors, and agents working from a checkout or install.
Dependencies: The Rust CLI, the plugin manifest, and the linked operator docs.
Main Functions: Show the shortest honest path from request to verified proof.
Side Effects: None; this document only describes repository-owned surfaces.
-->
[![Validate](https://github.com/UntaDotMy/keel/actions/workflows/validate.yml/badge.svg)](https://github.com/UntaDotMy/keel/actions/workflows/validate.yml)

# keel

**Read. Route. Prove.**

Keel is a Rust CLI and installable harness pack for evidence-backed software
work. It provides the delivery loop, brownfield ownership trace, review gates,
durable memory, deterministic workspace retrieval, command-output compaction,
and an MCP server. The repository also ships host adapters, specialist skills,
subagents, hooks, and slash commands.

Keel is a workflow and evidence boundary. It is not an LLM, a web researcher,
or a replacement for the host's editor, test runner, CI service, or deployment
system. Those systems remain responsible for the work they own; Keel records
and checks the evidence that crosses its boundary.

## Install

For a released binary, use the installer for the current shell:

```bash
# macOS, Linux, or WSL
curl -fsSL https://raw.githubusercontent.com/UntaDotMy/keel/main/install.sh | bash
```

```powershell
# Windows PowerShell
irm https://raw.githubusercontent.com/UntaDotMy/keel/main/install.ps1 | iex
```

The release installer downloads the matching archive, runs the native
`keel install`, and verifies the result. A release bundle can also be extracted
and run directly with `./keel install` or `.\keel.exe install`. A Rust
toolchain is only needed when developing from a source checkout.

Verify a fresh install in a new shell, or use the explicit installed path when
PATH has not refreshed:

```bash
keel status
keel doctor
```

```powershell
& "$env:USERPROFILE\.keel\keel.exe" status
& "$env:USERPROFILE\.keel\keel.exe" doctor
```

For source checkout development:

```bash
git clone https://github.com/UntaDotMy/keel.git
cd keel
cargo run --bin keel -- install
cargo run --bin keel -- status
```

The Claude Code plugin can be installed separately from the repository
manifest:

```text
/plugin marketplace add UntaDotMy/keel
/plugin install keel@keel
```

The plugin path publishes the manifest-owned skills, agents, hooks, commands,
and MCP registration. The native installer additionally provides the Rust CLI
and its managed state. See the [compatibility matrix](docs/compatibility-matrix.md)
for source, installed, hosted, and host-specific entry points.

## First success path

Use this sequence for a non-trivial change. Adapt the named quality command and
owned files to the actual request.

```bash
# 1. Establish the scoped map and capture the request.
keel memory scope resolve --create-missing --refresh-system-map
keel memory working-brief write \
  --request "Describe the requested change" \
  --acceptance-criteria "Describe the observable proof"

# 2. For established source, trace ownership before editing.
keel flow start --target-file rust/crates/keel/src/target.rs \
  --target-function target_function
keel flow check

# 3. Compile and run the bounded delivery loop.
keel anvil compile \
  --goal "Describe the bounded change" \
  --bar "cargo test --workspace --locked" \
  --files "rust/crates/keel/src/target.rs"
keel anvil run

# 4. Reconcile the diff and close with evidence.
keel code-search siblings
keel review pre-pr --base-ref origin/main
keel memory completion-gate check --brief-id <brief-id> \
  --proof "Named verification command and result"
```

For a plan-driven change, use `keel plan specify`, add current source or
official research with `keel plan research`, complete the generated design,
then run `keel plan design`, `keel plan tasks`, and `keel plan check --rtm`
before Anvil. The [compiled planner guide](docs/planner.md) defines the
artifacts and evidence contract.

`flow` is the brownfield gate: it records the current owner path and the
behavior that an edit must preserve. `code-search siblings` is the completeness
scan after an implementation or fix. `review pre-pr` reports the local gates;
it does not replace hosted CI or a human decision to publish or merge.

## Core surfaces

| Surface | Use it for | Entry points |
| --- | --- | --- |
| Anvil | A frozen, bounded delivery loop with named gates and evidence | `keel anvil compile`, `keel anvil run`, `keel anvil sieve`, `keel anvil loop` |
| Planner | Versioned requirements, current research, architecture, task tickets, and RTM checks | `keel plan specify|research|design|tasks|check` |
| Flow | Ownership and behavior trace before editing established source | `keel flow start|check|finish` |
| Review | Diff, policy, CI, hosted, and closeout gates | `keel review pre-commit`, `keel review pre-pr`, `keel review closeout` |
| Memory | Scoped maps, working briefs, recall, family records, and completion evidence | `keel memory ...` |
| Retrieval | Incremental file/symbol index, ranked search, impact graph, and sibling scan | `keel code-index ...`, `keel code-search ...`, `keel code-graph ...` |
| Command proxy | Bounded execution, compact diagnostics, raw recovery, replay, and savings reports | `keel run -- ...`, `keel rewrite`, `keel raw`, `keel replay`, `keel gain` |
| Host checks | Declared capability rows and bounded native conformance evidence | `keel host matrix --json`, `keel host conformance --json` |
| Manager | Install, update, status, diagnostics, verification, repair, and uninstall | `keel install`, `keel update`, `keel status`, `keel doctor`, `keel verify` |
| MCP | Stdio or loopback Streamable HTTP access to the native tools | `keel mcp serve`, `keel mcp serve-http` |

Run `keel help` for the operator surface and `keel help advanced` for the full
command inventory. When PATH is uncertain, call the installed binary directly
from the Keel home or use `cargo run --bin keel -- ...` in a checkout.

## Command proxy and recovery

Route noisy commands through the native proxy:

```bash
keel run -- cargo test --workspace --locked
keel run --json -- cargo clippy --workspace --all-targets -- -D warnings
keel rewrite "cargo test --workspace"
keel raw list
keel gain --since today
```

The proxy preserves the requested command's exit status, writes local raw and
compact artifacts, and returns a bounded summary when a semantic adapter
matches. `raw <raw-id>` and `replay <raw-id>` recover the original local run.
Raw output can contain secrets; it stays local and should be pruned according
to the retention policy for the installed Keel home.

## MCP and host adapters

The native server supports stdio (`keel mcp serve`) and loopback Streamable HTTP
(`keel mcp serve-http`). `keel mcp discover <capability>` exposes capability
metadata, while the MCP tools call the same native command owners used by the
CLI. HTTP binding and unsafe command execution are policy-controlled; consult
the [security audit status](docs/security-audit-status.md) and
[compatibility matrix](docs/compatibility-matrix.md) before changing deployment
defaults.

The canonical MCP tool names are `anvil`, `brief_create`, `brief_get`,
`brief_list`, `cli`, `code_graph`, `code_index`, `code_search`,
`command_kill`, `command_output`, `config_audit`, `context_brief`,
`design_intelligence`, `doctor`, `gain`, `git_workflow`, `learn`, `memory`,
`memory_status`, `observe`, `raw`, `recall`, `recall_status`, `review`,
`rewrite`, `run_command`, `session`, `skill_eval`, `skill_get`, `skill_lint`,
`skill_list`, `skill_route`, `stats`, `system_map`, `system_map_refresh`, and
`telemetry`.

The repository contains adapter surfaces for Claude Code, Codex CLI, OpenCode,
Cursor, Pi Agent, Oh My Pi, Command Code, Grok CLI, ZCode, Google Antigravity,
and Claude Desktop/Cowork. Integration depth is host-specific: some hosts
provide lifecycle hooks, some provide MCP only, and some require a bundled
runtime bridge. Use `keel host matrix --json` and `keel doctor` for the current
declared and installed state; use `keel host conformance` for bounded native
proxy evidence. A conformance result is not a claim that a live third-party
host process was exercised.

## Skills, agents, and commands

The `.claude-plugin/plugin.json` manifest is the source of truth for the
Claude Code plugin's skills, agents, hooks, commands, output styles, and MCP
server. `using-keel` is the small bootstrap gateway; specialist skills remain
separate so routing can stay explicit. `requesting-code-review` is retained as
a manifest-listed compatibility alias for `reviewer`.

Useful checks for the managed pack are:

```bash
keel skill-lint --repo-root .
keel skill-eval --repo-root .
keel config-audit --repo-root .
keel doctor
```

The native install mirrors the managed files into the host locations it owns.
It preserves unrelated user files and reports host limitations instead of
turning an unavailable host path into a success claim.

## Repository layout

```text
rust/crates/keel          Rust CLI and native runtime surfaces
rust/crates/keel-*        Supporting Rust crates
.claude-plugin/           Claude Code plugin manifest
.claude/                  Plugin-shipped agents and hook source
commands/                 Namespaced slash-command wrappers
<host adapter dirs>/      Codex, Cursor, Pi, OpenCode, Cowork, and other bridges
<skill dirs>/              Specialist skill packs
AGENTS.md                 Managed operating contract and routing index
WORKFLOW.md               Branch and delivery rules
docs/                      User guides, contracts, audits, and release evidence
tests/                     Cross-host contract fixtures and tests
bench/                     Benchmark inputs and comparison artifacts
```

## Validation and contribution

The standard local Rust checks are:

```bash
cargo fmt --all --check
cargo check --locked --workspace --all-targets
cargo test --locked --workspace
cargo clippy --locked --workspace --all-targets -- -D warnings
```

Use `keel run -- ...` when output is noisy. Run the native review gates and the
relevant host or release checks for the surface being changed. Hosted CI remains
the authority for hosted proof. Read [CONTRIBUTING.md](CONTRIBUTING.md),
[WORKFLOW.md](WORKFLOW.md), and [AGENTS.md](AGENTS.md) before opening a change.

## Documentation map

- [First success path](docs/first-success-path.md): one end-to-end operator run.
- [Compatibility matrix](docs/compatibility-matrix.md): supported contexts,
  host adapters, PATH behavior, and conformance boundaries.
- [Compiled planner](docs/planner.md): research, design, task, and RTM contracts.
- [Review closeout](docs/review-closeout.md): local reconciliation and evidence.
- [Flow-check schema](docs/flow-check-schema.md): brownfield ownership artifact.
- [Memory families](docs/memory-families-usage.md): durable memory surfaces.
- [Release proof bundle](docs/release-proof-bundle.md): release evidence format.
- [Security audit status](docs/security-audit-status.md): published security
  evidence and open boundaries.
- [Benchmark suite](docs/benchmark-suite.md): scenario and measurement contracts.
- [Plan traceability](docs/plan-traceability.md): phase status against the
  attached operating-system plan; it is not a completion badge.

Keel is licensed under the MIT License. See [LICENSE](LICENSE).
