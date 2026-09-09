# Phase 2 Compiled Planner Evidence

## Delivered contract

Phase 2 adds the native sequence:

```text
keel plan specify -> research -> tasks -> check
```

Plans live under the canonical workspace memory lane. `specify` publishes all
six required artifacts; `research` grounds the current-workspace claim;
`tasks` compiles task and RTM mappings; `check` validates the entire graph and
updates status.

## Requirement evidence

| Requirement | Evidence |
| --- | --- |
| Five command forms | Native dispatch and help coverage in `commands.rs`, `help_operator.txt`, and `utility/plan.rs`. |
| Six versioned artifacts | Strict Markdown headers and JSON `schemaVersion`/artifact/plan identity checks. |
| Twenty-two specification sections | Runtime-generated `spec.md`; integration test enumerates every required section. |
| Structured acceptance criteria | Parser and validator require ID, REQ mapping, precondition, action, observable/negative outcomes, verification, evidence, and owner. |
| Vague request handling | Material vague terms write interpretations plus a clarification question and block task/check progression. |
| Claim classification | `verified`, `assumption`, and `derived` are the only accepted states; verified claims require sources. |
| Traceability | `tasks.json` and `rtm.json` map every REQ through AC, task, verification method, and evidence type. |
| Safe persistence | Plan IDs use one safe segment; initial bundles publish by staged-directory rename; later writes are atomic. |
| Planner pointer budget | Runtime measurement remains 9 tokens against the ratified 10-token budget. |

## TDD and validation

The first focused run failed 0/5 because `plan` had no native dispatch. After
implementation:

| Gate | Result | Evidence |
| --- | --- | --- |
| Planner workflow integration | 6 passed, 0 failed | `cargo test -p keel --test plan_workflow_test`; RawStore `20260909-045435-e6059ee7` |
| Planner unit contract | 4 passed, 0 failed | `cargo test -p keel --lib utility::plan::tests`; RawStore `20260909-045327-8fe8005d` |
| Keel library | 1,268 passed, 0 failed | `cargo test -p keel --lib`; RawStore `20260909-050147-93cdcb80` |
| Rust lint | pass | `cargo clippy -p keel --all-targets -- -D warnings` |
| Full workspace | 1,424 passed, 0 failed | `cargo test --workspace`; RawStore `20260909-050338-feef27d0` |
| Strict pre-commit review | pass, 0 warnings | Formatting, lint, prose, slop, flow, and completeness gates all passed. |

The negative fixture removes an AC, a REQ mapping, and a verification method.
Separate mutations remove a factual-claim classification, set an unsupported
schema, and attempt a traversal plan ID. Every mutation fails with an actionable
diagnostic.
