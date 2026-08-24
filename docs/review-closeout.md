# Review closeout

`review closeout` is the final local reconciliation pass for a working brief and the repository state. It records findings and verification evidence; it does not automatically fix code.

## First run

Start with the repository and brief that define the work:

```text
keel review closeout --repo-root <path> --base-ref origin/main --brief-id <id> --proof "targeted checks passed" --format markdown
```

Use `--strict` when a working brief is required, and `--require-ci` when exact-head
CI proof is required before a pass. `--require-ci` also rejects a dirty worktree,
because CI cannot prove uncommitted files. The default base ref is `origin/main`;
the default output format is `json`.

Each run writes a ledger under `<claude-home>/state/review-closeout/<review-id>.json`. Without an explicit review id, the id is derived from the current `HEAD` (for example, `review-<12-char-sha>`). The JSON result includes the ledger path, status, findings, requirements, and gate snapshots.

### Criterion-bound proof

With a working brief, `--proof` must contain one non-empty evidence line for
each acceptance criterion. Use the requirement IDs shown in JSON output:

```text
requirement-<id>=cargo test --workspace --locked passed
requirement-<id>=hosted run https://github.com/org/repo/actions/runs/123
```

Closeout keeps each evidence line attached to its criterion. One generic
sentence is not sufficient and cannot close every requirement.

## Reviewed baseline

Repository-wide static findings can be carried in the tracked
`review-closeout-baseline.json` file without weakening dynamic gates. Baseline
entries use exact finding IDs, require a reviewer, reason, and RFC3339 expiry,
and only cover static `comment:`, `prose:`, `slop:`, and their aggregate
comment/prose gate findings. Gate, wiring, evidence, requirement, and CI
findings remain blocking.

Generate or refresh the baseline only from a clean tree:

```text
keel review closeout --repo-root <path> --brief-id <id> --proof "static findings reviewed" --write-baseline --baseline-reviewer "name" --baseline-reason "historical repository-wide static findings" --baseline-expires "2027-01-01T00:00:00Z"
```

Baseline files expire and must be renewed through a reviewed change. A changed
file, line, rule, or message produces a new finding ID and is not suppressed by
the old baseline.

## Findings and the fix loop

Finding statuses are `open`, `closed`, or `stale`. Findings use stable
rule/file/line/message identities: a matching finding remains open until a later
run no longer emits it, while a finding absent from the current full scan is
closed. Requirements remain open until proof is supplied **and** the current scan
has no unresolved findings. Requirements removed from the brief become stale.

1. Run closeout and read every open or stale finding.
2. Fix the source issue and run the targeted verification it needs.
3. Rerun closeout with the same scope, brief, and proof. Closeout refreshes flow
   and sibling evidence automatically. Repeat until the ledger reports `passed`
   and no unresolved requirements or findings remain.

Closeout reports status; it is not a claim that every possible bug in the repository has been mathematically proven absent.

## MCP asynchronous use

Closeout can exceed the MCP host deadline. Call the `review` tool with `{"action":"closeout","wait":false,...}`. The tool returns a `commandId` immediately. Poll it with `command_output` until `running:false`; use `command_kill` to terminate a run that should not continue. The MCP path uses the same CLI engine and command registry as `run_command`, so it does not duplicate review logic or synchronously run the closeout tests.
