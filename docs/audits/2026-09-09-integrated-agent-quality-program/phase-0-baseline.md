# Phase 0 Baseline

- Baseline revision: `63281ddeb6cd40361c72dfc35446e5d8362ac8cf`
- Capture completed: 2026-09-09T00:57:37+08:00
- Branch during capture: `task/integrated-agent-quality-phase-0`
- Runtime behavior changed before capture: no
- Raw manifest: [raw/phase-0/manifest.json](raw/phase-0/manifest.json)
- Warning candidates: [raw/phase-0/warning-candidates.json](raw/phase-0/warning-candidates.json)
- Program working brief: `integrated-agent-quality-program-20260909`
- Phase working brief: `integrated-agent-quality-phase-0-20260909`

## Requirements and acceptance evidence

| ID | Requirement | Status | Evidence |
|---|---|---|---|
| `P0-R1` | Research all 12 required topics before runtime implementation | pass | [research record](../../research/2026-09-09-agent-quality-planning-verification.md) contains 18 dated, scoped source records |
| `P0-R2` | Execute and archive every required baseline command | pass | [manifest](raw/phase-0/manifest.json) records exact argv, streams, exit codes, durations, and hashes for all 14 requested commands |
| `P0-R3` | Record revision, environment, suites, warnings, context costs, reducer results, and host contracts | pass with explicit unmeasured fields | The sections below map each available measurement and name the fixed-context fields the current owner does not expose |
| `P0-R4` | Preserve non-green baseline states | pass | Completion-gate failure, Git warnings, doctor warnings, and unmeasured values remain non-pass |
| `P0-R5` | Make every numeric claim reproducible | pass | [reproduction commands](reproduction.md), capture script, raw streams, and manifest hashes are retained |
| `P0-R6` | Change no runtime behavior in the baseline phase | pass | Runtime owners and schemas are unchanged; the only Rust edit isolates three graph-validation test fixtures from the process-global Keel home |

The table is the Phase 0 traceability record. The compiled task-ticket and RTM owners do not exist until Phase 5, so their runtime validation is not applicable to this phase; `P0-R1` through `P0-R6` are the provisional source rows Phase 5 must import or supersede without losing evidence.

## Environment

| Surface | Captured value | Evidence |
|---|---|---|
| Operating system | Microsoft Windows 10.0.26200, X64 | [manifest](raw/phase-0/manifest.json) |
| PowerShell | 7.6.5 | [manifest](raw/phase-0/manifest.json) |
| Git | 2.50.1.windows.1 | [stdout](raw/phase-0/git-version.stdout.log) |
| Rust | rustc 1.98.0, commit `88d9e12ae`, LLVM 22.1.8, `x86_64-pc-windows-msvc` | [stdout](raw/phase-0/rustc-version.stdout.log) |
| Cargo | 1.98.0 | [stdout](raw/phase-0/cargo-version.stdout.log) |
| Bun | 1.4.0 | [stdout](raw/phase-0/bun-version.stdout.log) |
| Keel | development build, foundation `phase-1-foundation`, Windows amd64 standard feature set | [stdout](raw/phase-0/keel-version.stdout.log) |

## Required command results

Every row links to raw output. Exit code, duration, stream hashes, and exact argv are in the [manifest](raw/phase-0/manifest.json).

| Command ID | Status | Result |
|---|---|---|
| `cargo-fmt` | pass | Exit 0; no output |
| `cargo-clippy` | pass | Exit 0; `-D warnings` accepted; no diagnostic warnings |
| `cargo-build` | pass | Exit 0; no build warnings |
| `cargo-test` | pass with baseline warnings | 1,407 passed, 0 failed, 0 ignored across 17 Rust test and doc-test executables; two Git CRLF warnings were emitted from test-fixture working copies |
| `skill-lint` | pass | 51 skills checked; 0 failed; 0 warned |
| `skill-eval` | pass | 38 fixtures passed; 0 failed; 0 skipped |
| `reducer-eval` | pass | 7 fixtures; 2,218 raw tokens; 1,613 compact tokens; 605 measured tokens saved |
| `gain` | pass | Reproducible command-output metrics captured in [stdout](raw/phase-0/gain.stdout.log) |
| `config-audit` | pass | 0 high, 0 medium, 0 low findings |
| `review-pre-commit` | pass | Blocking count 0; warning count 0 |
| `review-pre-pr` | pass | Blocking count 0; warning count 0 |
| `review-gates` | pass | Blocking count 0; warning count 0 |
| `completion-gate` | fail | Exit 1 because the required prompt command omitted `--brief-id` and `--proof`; the current CLI lists brief IDs rather than selecting one silently |
| `host-contracts` | pass | 12 tests passed; 0 failed; 175 assertions across the 11 host-adapter contract surface |

The full Rust suite identities are retained in [cargo-test stderr](raw/phase-0/cargo-test.stderr.log), and the individual test outcomes are retained in [cargo-test stdout](raw/phase-0/cargo-test.stdout.log).

## Warning baseline

The capture's broad text scan produced seven warning candidates. Five are test names or completion-gate prose, not tool diagnostics. The two actual compiler/tool-stream warnings are:

1. `warning: in the working copy of 'README.md', LF will be replaced by CRLF the next time Git touches it`
2. `warning: in the working copy of 'src/main.rs', LF will be replaced by CRLF the next time Git touches it`

Both came from Git operations inside the Rust test run. They predate runtime implementation for this program and are recorded as baseline rather than passed, discarded, or treated as newly introduced diagnostics.

`keel doctor` also reported two warn-tagged operational observations in [doctor stdout](raw/phase-0/doctor.stdout.log): the Claude binary is present, and one stale executable sibling exists at the recorded local path. The stale sibling has the documented recovery command `keel doctor --fix`; Phase 0 does not perform cleanup.

## Existing fixed-context measurements

These values come from `keel stats --json` using the repository's `o200k_base` token meter. They are current-machine measurements, not budgets or cross-machine benchmarks.

| Surface | Tokens / count |
|---|---:|
| SessionStart context | 1,095 tokens |
| UserPromptSubmit base fixture | 102 tokens |
| UserPromptSubmit code-change fixture | 153 tokens |
| MCP serialized catalog | 2,885 tokens |
| MCP tool count | 37 tools |

Raw evidence: [stats JSON](raw/phase-0/stats-json.stdout.log).

The current binary does not separately report the required costs for `CLAUDE.md`, `AGENTS.md`, `WORKFLOW.md`, generated host files, inline skill catalogs versus full skill bodies, planner/research/ticket/warning/UI pointers, or the eager/deferred catalog split. Those values are `unmeasured` at this baseline and remain explicit Phase 1 work.

## Command-output compaction baseline

The exact `keel gain` capture reports 500 observed commands, 220 compacted commands, 1,675,416 tokens before reduction, 216,306 tokens after reduction, 1,459,612 gross tokens saved, 502 wrapper-overhead tokens, and 1,459,110 net tokens saved. These are event-log measurements for this machine and time window, not universal product performance.

Raw evidence: [gain stdout](raw/phase-0/gain.stdout.log).

## Baseline decision

Phase 0 establishes a reproducible green build/test/review baseline with three explicit non-green categories:

- the bare completion-gate command is invalid for a real check under the current CLI contract;
- two Git CRLF diagnostics are baseline warnings from test fixtures;
- several requested fixed-context surfaces are not exposed by the current measurement owner and are `unmeasured`.

None of these states is reported as pass. The first and third are inputs to later compiled-gate phases; the second is preserved for the Phase 6 warning lifecycle.

## Post-capture baseline stabilization

A strict pre-PR rerun exposed one nondeterministic failure in `utility::code_graph::tests::from_json_file_round_trips_and_impact_works`. The same test passed alone, and the next unmodified full-suite run passed all 1,407 tests. Source tracing showed that the failing fixture stored its artifact outside the canonical code-graph lane, so `CodeGraph::from_json_file` selected the process-global default Keel home while parallel tests could temporarily change that value. All three tests that reach graph validation now use the existing `cached_artifact_path` owner with unique temporary Keel homes. Production code and the artifact schema are unchanged.

Failure and control-run hashes are retained in [review-regression.md](review-regression.md). This finding does not rewrite the captured baseline: it records the instability found by the independent review and the test-only stabilization applied afterward.

## Safety checklist

| Check | Status | Reason / evidence |
|---|---|---|
| Working brief existed before source edits | pass | Program brief `integrated-agent-quality-program-20260909` was created before the Phase 0 files |
| `keel flow start` used for each established source file | pass | The post-capture test stabilization has current flow evidence for `rust/crates/keel/src/utility/code_graph.rs`; the baseline evidence files were new |
| Baseline outputs archived | pass | [raw/phase-0](raw/phase-0/) contains separate stdout and stderr for every command |
| Existing warnings retained | pass | Warning candidates and the classified warning baseline are preserved above |
| Metrics have reproduction commands | pass | [reproduction.md](reproduction.md) runs the retained capture script |

## Phase report

- Before/after runtime metric: not applicable because Phase 0 changes no runtime owner; the recorded values are the sole baseline for later comparisons.
- UI, load, and stress verification: not applicable because the phase changes no UI, service, or runtime path.
- Security review: limited to repository payload and secret hygiene; no credentials or secret-shaped values are expected in the audit bundle.
- Hosted CI: not yet available because the Phase 0 pull request has not been published.
- Human review: required before commit and push under the repository's reviewer gate.
- Rollback: revert the Phase 0 documentation commit; no runtime, schema, or persisted user data is changed.
