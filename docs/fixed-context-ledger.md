# Fixed-context ledger

`keel stats` measures the context Keel can place in an agent session before
task-specific command output. The ledger uses the runtime `o200k_base`
`TokenMeter`; bytes and estimated ratios are not substitutes.

Run the ledger from a source checkout:

```powershell
cargo run --locked --bin keel -- stats --json --workspace-root <repo>
```

The text form prints the same 19 rows. Every row includes its current token
count, ratified budget, status, and reproduction command. A missing repository
source reports `source_missing`; a count above budget reports `exceeded`.
Rows are not summed: generated host files are alternatives, and the skill rows
compare inline frontmatter with on-demand full bodies rather than simultaneous
session injection.

## Phase 1 baseline

Budgets are `ceil(actual × 1.10)` from the first hermetic Phase 1 runtime
measurement. Actual values are recomputed on every invocation and in CI.

| Surface | Actual | Budget |
|---|---:|---:|
| `mcp.tools_list.catalog` | 2,885 | 3,174 |
| `hook.session_start.bootstrap` | 457 | 503 |
| `hook.user_prompt_submit.simple` | 102 | 113 |
| `hook.user_prompt_submit.code_change` | 153 | 169 |
| `repo.CLAUDE.md` | 9,906 | 10,897 |
| `repo.AGENTS.md` | 2,348 | 2,583 |
| `repo.WORKFLOW.md` | 4,884 | 5,373 |
| `generated.claude.CLAUDE.md` | 964 | 1,061 |
| `generated.codex.AGENTS.md` | 364 | 401 |
| `generated.omp.AGENTS.md` | 131 | 145 |
| `generated.zcode.AGENTS.md` | 130 | 143 |
| `generated.antigravity.GEMINI.md` | 131 | 145 |
| `skills.inline_catalog` | 10,146 | 11,161 |
| `skills.full_bodies` | 87,931 | 96,725 |
| `pointer.planner` | 9 | 10 |
| `pointer.research` | 12 | 14 |
| `pointer.ticket_checklist` | 11 | 13 |
| `pointer.warning_status` | 9 | 10 |
| `pointer.ui_verification` | 15 | 17 |

The MCP baseline is 37 total tools: 37 eager and 0 deferred. Phase 12 owns any
catalog-profile change. The skill baseline covers 51 parseable first-party
`SKILL.md` files.

## Measurement boundaries

- SessionStart measures the fixed bootstrap only. Workspace maps, briefs,
  instincts, and synthesis are dynamic and are not charged to a fixed budget.
- UserPromptSubmit uses `hello` and `fix the bug` as the simple and code-change
  fixtures.
- Repository rows read the current `CLAUDE.md`, `AGENTS.md`, and `WORKFLOW.md`.
  A missing file fails closed instead of appearing as a zero-token success.
- Generated-host rows call the same pure block builders used by installation;
  they do not measure mutable user content around a managed block.
- The inline skill row places parsed frontmatter content between canonical
  fences in stable skill-name order. The full-body row uses the complete
  matching files in that same order without writing the skill cache.
- Pointer strings are canonical constants reserved for later phases. The
  warning-status pointer also has a hard limit below 30 tokens.

## CI and accounting

`rust/crates/keel/tests/fixed_context_budget_test.rs` invokes the shipped CLI
and checks every runtime row. Its scratch-inflation case raises one row to
`budget + 1` and verifies that the failure names the surface, actual count,
budget, and reproduction command.

Fixed context is not command-output compaction. `keel gain` reports the
separation explicitly and never folds these costs into gross or net savings.

Any phase that changes a measured source must update the ledger in the same PR:
run the test, inspect the precise delta, document why the growth is necessary,
and ratify a new threshold only after review. Do not calculate a budget from the
new value inside the test; the checked-in threshold is the regression gate.
