# Phase 5 Task Ticket and Evidence Traceability

## Delivered contract

`plan tasks` now writes one `task-<n>.json` ticket per requirement beside the
existing aggregate `tasks.json`. Each ticket retains the task, REQ, and AC
identity and contains the eleven base checklist layers. Request and architecture
scope add non-empty mandatory layers for source, public API, UI, dependency,
token/performance, data/memory, and host-integration work.

Existing task tickets are preserved on a task refresh. Keel validates them
before regenerating `tasks.json` and `rtm.json`; it does not overwrite progress
or remove existing fields. The RTM now records the exact chain:

```text
User request -> Requirement -> Acceptance criterion -> Task -> Checklist subtask -> Evidence
```

Brownfield pre-PR and closeout review add a blocking `task_evidence` gate beside
the research and architecture gates.

## Requirement evidence

| Requirement | Evidence |
| --- | --- |
| Ticket schema | Every ticket validates version, artifact and plan identity, task status, exact REQ/AC links, all base layers, and every required subtask field. |
| Scope-derived layers | Seven deterministic scope classes expand to the required implementation, compatibility, visual, dependency, measurement, data, and host layers. `design` remains mandatory so every AC has an evidence-producing subtask. |
| Immutable derived checklist | Missing derived layers, empty derived layers, deleted generated subtasks, or removed `derived=true` markers fail `plan tasks`, `plan check`, and named-plan review. |
| Honest status | `skipped`, `not_applicable`, and `needs_human` require a non-empty reason. `done` requires a resolvable evidence reference and RFC3339 verification timestamp. |
| Machine-resolvable evidence | Command, named-test, lint-diagnostic, source-hash, RawStore, screenshot, and benchmark payloads have type-specific validation. Ticket and evidence IDs, types, and timestamps must agree. |
| Safe evidence resolution | Evidence is a bounded regular JSON file inside the plan directory. Absolute paths, traversal, symlinks, oversized input, invalid UTF-8, failed results, and changed content fingerprints fail closed. |
| Complete traceability | Aggregate tasks and RTM entries must exactly match ticket REQ/AC links. Every ticket subtask has an RTM trace for each AC; unknown, missing, duplicate, or drifted task/subtask/evidence links fail. |
| Independent review | `task_evidence` loads the named plan and runs the same ticket, RTM, evidence, path, and fingerprint validator for established-source diffs. |

The real branch plan is
`plan-1788910986769913900-3276-0b8018db`. The branch binary preserved its
ticket, regenerated its aggregate artifacts, and passed `plan check --rtm` with
one requirement, one acceptance criterion, and nine source/public-CLI checklist
subtasks. RawStore: `20260909-083506-19493b0c`.

## TDD and reviewer progression

The first outside-in task-ticket run failed before the new schema existed, then
exposed implementation defects in ticket validation. RawStore:
`20260909-075206-18d5b092`. A later compile-only run caught the Rust 1.80 MSRV
incompatibility of `Option::is_none_or`; the implementation now uses the
supported equivalent. RawStore: `20260909-080204-7f6852ac`.

The reviewer pass found that legacy aggregate task rows and RTM entries could
carry an extra unknown AC even though tickets and per-subtask traces were exact.
Exact aggregate/RTM symmetry plus dangling-link regressions closed that class.

Final focused proof:

| Gate | Result | Evidence |
| --- | --- | --- |
| Task ticket lifecycle | 5 passed, 0 failed | Schema/layers, seven evidence types, completion rules, exact aggregate/RTM links, and tamper detection; RawStore `20260909-083336-6796f847` |
| Planner regression | 14 passed, 0 failed | RawStore `20260909-083347-8f98950b` |
| Architecture regression | 6 passed, 0 failed | RawStore `20260909-083422-c98651af` |
| Review gates | 86 passed, 0 failed | Includes unjustified-status and tampered-evidence review proof; RawStore `20260909-083412-1c25ab5e` |
| Documentation parity | 27 passed, 0 failed | RawStore `20260909-083424-85f2958c` |
| Fixed-context budget | 5 passed, 0 failed | RawStore `20260909-083429-1f7281cd` |

## Full validation

| Gate | Result | Evidence |
| --- | --- | --- |
| Anvil compile and dry-run | pass | One piece, three casts, one gate; frozen-prefix check passed before implementation. |
| Real planner lifecycle | pass | Phase 5 plan reached tasked and valid with nine evidence-producing RTM traces. |
| Rust formatting | pass | `cargo fmt --all -- --check` |
| Rust lint | pass | `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` |
| Complete sibling scan | pass | Final changed-file set has no unhandled `ticketFiles`, checklist-subtask, or evidence-reference siblings. |
| Full workspace | 1,453 passed, 0 failed | 21 test binaries; RawStore `20260909-084231-e2a5483b` |
| Scoped pre-commit | pass, 0 blocking findings | Format, clippy, comment, prose, flow, completeness, and impact passed. The staged added-file scan reported non-blocking repeated-line heuristics on idiomatic Rust signatures, value access, and test setup; manual review found no duplicated policy owner. |
| Researched, designed, ticketed pre-PR | pass, 0 warnings | `research_traceability`, `architecture_design`, and `task_evidence` all passed for the named plan and four established-source edits. |
| Completion gate | pass | All seven Phase 5 acceptance criteria have focused, plan, review, and workspace proof. |

## Release ladder

| Rung | Verdict | Evidence |
| --- | --- | --- |
| Smoke | pass | The real branch binary regenerated and checked the Phase 5 ticket and RTM. |
| Functional | pass | Five integration scenarios cover schema, every evidence type, status rules, trace links, and tampering. |
| Integration | pass | Planner, architecture, review, docs, fixed-context, and all 1,453 workspace tests pass together. |
| UI | not applicable | No graphical surface changed. |
| Load | not applicable | No service workload was added; ticket and evidence inputs are bounded to 65,536 bytes. |
| Stress | not applicable | No network client, retry loop, queue, or concurrent mutable service was added. |
| Security | pass | Evidence path confinement, regular-file checks, bounded reads, identity checks, traversal rejection, and content-tamper regressions pass. |

Hosted CI and push remain deferred under the program-wide no-push instruction.
The Phase 4 commit is the recovery boundary; Phase 5 remains isolated on its
own stacked branch until the final local phase commit.
