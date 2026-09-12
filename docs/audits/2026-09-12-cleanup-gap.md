<!--
Purpose: Record evidence for the Phase 10 repository-hygiene cleanup pass.
Caller: Reviewers reconciling the final operating-system plan against this checkout.
Dependencies: Cargo checks, indexed search, the plugin manifest, and plan §§47-50, 56-58.
Main Functions: Separate confirmed cleanup from deferred debt and tool limitations.
Side Effects: None; this is a dated audit record.
-->
# Repository cleanup-gap audit: 2026-09-12

## Scope

This pass covers the Phase 10 cleanup sequence and definition of done in the
Final Agent Operating System plan (§§47-50, 56-58), at commit `7e0d2df`.
The allowed change surface was `README.md`, dependency files only when an
unused dependency was proven, this audit, and files proven to be orphaned.
Rust behavior, Rust tests, MCP behavior, host adapters, and learning/research
surfaces were outside the change boundary.

## Changes made

### README consolidation

`README.md` was reduced from 779 lines to a concise operator/contributor guide.
It now has one install path, one first-success path, a compact native-surface
table, the host-integration boundary, repository layout, validation commands,
and links to the detailed contracts. Removed material included repeated setup
and proof sections, branch/time-specific operational claims, and the stale
“Universal 5-Role Multi-Agent Architecture” section (which actually numbered
six roles). The README now states the boundary clearly: Keel records and
checks workflow evidence but does not replace an LLM, host, test runner, CI, or
deployment system.

### Orphan fixture removal

Removed:

```text
rust/crates/keel/tests/fixtures/mcp_tools_list_budget_overflow_1200.fixture.json
```

Before removal, exact repository and indexed searches for
`mcp_tools_list_budget_overflow_1200` returned only the fixture itself. The
related regression coverage in `rust/crates/keel/src/mcp/tools.rs:6183-6232`
constructs the catalog parameters inline and does not load a fixture path.
No source, test, manifest, workflow, or documentation consumer was found.

## Audit results

| Area | Result | Evidence and boundary |
| --- | --- | --- |
| Direct dependencies | No unused dependency proven; no manifest change | `cargo check --locked --workspace --all-targets` and `cargo clippy --locked --workspace --all-targets -- -D warnings` passed. A source/test usage scan found each direct dependency in the workspace. `cargo machete` is not installed; a nightly `cargo-udeps` installation was started but not completed, so this is not a cargo-machete/udeps claim. |
| Duplicate dependency versions | No safe direct deduplication | `cargo tree --duplicates` reports the transitive `hashbrown` 0.14.5/0.15.5 split through different owners; changing it would require dependency-graph changes outside this cleanup. |
| Compiler-visible dead/private items | No new warning | Clippy with `-D warnings` passed. The existing `code_graph` compatibility/test-builder `dead_code` allowance remains intentionally owned by that module and was not changed. |
| Orphan files | Closed for the confirmed candidate | The unused MCP fixture was removed after exact `rg` and `keel code-search` evidence. Remaining fixtures and benchmark artifacts have repository consumers or are exercised by their documented suites. |
| Skill inventory and parity | Preserved | The manifest lists all managed skills; `using-keel` is the intentional bootstrap directory outside the manifest. `requesting-code-review` is a real manifest-listed compatibility alias for `reviewer`, so it was not deleted. |
| Duplicate skill owner | Deferred by scope | `docs/skills-audit-p1.md` proposes retiring the alias later, but deletion would require coordinated manifest/routing/test changes. This pass does not make that cross-surface change. |
| Stale mutation paths | Deferred by scope | `.cargo/mutants.toml` and `docs/quality/mutation-baseline.json` still mention historical paths such as `src/plan.rs`; current owners are under `src/utility/`, `src/proxy/`, and `src/review/`. Those files are outside the allowed edit list. |
| Stale traceability prose | Deferred by scope | `docs/plan-traceability.md` contains branch-specific historical wording. It is preserved for its evidence record and requires a dedicated docs update. |

## Verification record

The following checks were run against this worktree after the audit plan was
compiled and before closeout:

```text
keel anvil compile ... --files README.md,Cargo.toml,Cargo.lock          PASS
keel anvil run --dry-run ...                                           PASS (writes=0, executes=0)
keel run -- cargo check --locked --workspace --all-targets              PASS
keel run -- cargo clippy --locked --workspace --all-targets -- -D warnings PASS
```

Post-edit checks completed for this pass are:

```text
keel code-search siblings --workspace-root <worktree> --query "mcp_tools_list_budget_overflow_1200" PASS (siblingCount=0)
cargo fmt --all --check                                                       PASS
keel run -- cargo check --locked --workspace --all-targets                         PASS
keel run -- cargo clippy --locked --workspace --all-targets -- -D warnings         PASS
keel run -- cargo test --locked -p keel --test doc_parity_test                       PASS (27/27)
keel run -- cargo test --locked -p keel --test mcp_protocol                         PASS (11/11)
keel review pre-commit --repo-root <worktree> --format compact                       PASS (blocking=0, warnings=0)
```

The full workspace test was also attempted through `keel run -- cargo test
--locked --workspace`, but the proxy reached its default 300-second timeout
while concurrent worktrees were compiling. It returned a timeout, not a test
failure result; the two focused suites above completed successfully. This audit
does not treat focused checks as hosted release proof.
