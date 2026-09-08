# Phase 4 Architecture Design Gate Evidence

## Delivered contract

Phase 4 inserts an explicit design stage into the compiled planner:

```text
specified -> researched -> designed -> tasked -> valid
```

`plan research` writes a pending fourteen-section `architecture.md` scaffold.
The operator completes that canonical artifact and runs `plan design`; task
generation then requires both current design validation and
`architectureStatus: complete`. Re-running research invalidates design and task
readiness. Brownfield pre-PR and closeout review report design defects through a
separate blocking `architecture_design` result beside research freshness.

## Requirement evidence

| Requirement | Evidence |
| --- | --- |
| Complete design before implementation | The shared validator requires `Status: complete`, the exact 14 ordered sections, and non-placeholder structured decisions. `plan tasks` fails before design succeeds. |
| Component traceability | Every `- Component:` record requires known REQ and AC IDs, and coverage checks reject any specification requirement or criterion with no component mapping. |
| Alternatives and failure semantics | Paired alternatives/tradeoffs and risks/mitigations are required, along with one policy owner, chosen option, constraint fit, compatibility, explicit failure status, fallback, and named visibility. |
| Infrastructure and host compatibility | Architecture must name reused Keel infrastructure. Host-impacting designs must preserve all 11 adapter contracts; Phase 4 reuses plan paths/status, atomic writes, diff scope, and review result tallying without adapter changes. |
| Bounded token impact | Planner and review read at most 65,537 bytes and reject architecture input above 65,536 bytes. The artifact remains plan-scoped rather than fixed injected context. |
| Verification and rollback | The design requires security/privacy, token measurement, verification with AC references, rollback, and complete REQ/AC/classified-claim references before tasks. |
| Review enforcement | `architecture_design` detects committed and uncommitted established-source changes, requires the same named plan as research, and lists actionable design findings without changing the greenfield exemption. |

The real branch plan is
`plan-1788906841924697800-15560-71a2dbee`. Its completed architecture maps all
five changed component groups to REQ-001 and AC-001, records two rejected
alternatives, visible no-fallback behavior, the measured token plan, and the
Phase 3 recovery boundary. The local binary passed
`design -> tasks -> check --rtm` with one requirement and one acceptance
criterion.

## TDD progression

The first outside-in run failed 0/4: task generation still passed without a
design and `plan design` was unknown. The initial review-gate test then failed
to compile because `architecture_design_gate` did not exist. RawStore:
`20260909-065225-5c364da1`. The first legacy planner run failed three tests
until the existing helper completed and validated architecture before tasks.
RawStore: `20260909-064901-d6a328b9`.

Final focused proof:

| Gate | Result | Evidence |
| --- | --- | --- |
| Architecture lifecycle | 6 passed, 0 failed | Required design, mappings, ordered sections, decisions, 65,536-byte bound, research invalidation, and correct pre-research stage; RawStore `20260909-071121-52cff103` |
| Planner regression | 14 passed, 0 failed | RawStore `20260909-071124-a8b629ff` |
| Review gates | 85 passed, 0 failed | Includes incomplete/complete architecture review proof; RawStore `20260909-071141-0f9aeed0` |
| Fixed-context budget | 5 passed, 0 failed | RawStore `20260909-071145-61aa1896` |
| Documentation parity | 27 passed, 0 failed | RawStore `20260909-071146-becfa801` |

## Token measurement

The runtime ledger reports `within_budget` for every ratified surface. The
planner pointer remains 9/10 tokens, the MCP catalog remains 2,902/3,193, and
the host instruction surfaces are unchanged. The generated architecture body
is stored only in the workspace plan lane and is subject to the explicit
65,536-byte read limit.

## Full validation

| Gate | Result | Evidence |
| --- | --- | --- |
| Anvil compile and dry-run | pass | One piece, three casts, one gate; frozen-prefix check passed before implementation. |
| Real planner lifecycle | pass | Phase 4 plan reached designed, tasked, and valid with a complete RTM. |
| Rust formatting | pass | `cargo fmt --all -- --check` |
| Rust lint | pass | `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` |
| Complete sibling scan | pass | Final changed-file set was scanned with focused planner lifecycle and review-gate queries. |
| Full workspace | 1,444 passed, 0 failed | RawStore `20260909-071605-9478c47d` after reviewer fixes. |
| Scoped pre-commit | pass, 0 warnings | Final diff evidence recorded after staging. |
| Researched and designed pre-PR | pass, 0 warnings | Both named-plan gates pass against the Phase 3 base. |
| Completion gate | pass | All six Phase 4 acceptance criteria carry focused, plan, review, and workspace proof. |

## Release ladder

| Rung | Verdict | Evidence |
| --- | --- | --- |
| Smoke | pass | The real local binary completed design, task generation, and full plan check. |
| Functional | pass | Six architecture lifecycle scenarios and fourteen planner regressions pass. |
| Integration | pass | Planner, review, docs, fixed-context, library, and workspace suites pass together. |
| UI | not applicable | No graphical surface changed. |
| Load | not applicable | No service workload was added; architecture input is deterministically bounded. |
| Stress | not applicable | No network client, retry loop, queue, or concurrent mutable service was added. |
| Security | pass | Safe plan IDs remain enforced, reads are plan-local and bounded, invalid UTF-8 fails, and no credentials or executable content are introduced. |

Hosted CI and push remain deferred under the program-wide no-push instruction.
The Phase 3 commit is the recovery boundary; Phase 4 is isolated on its own
stacked branch and will be committed as one local phase after final review.
