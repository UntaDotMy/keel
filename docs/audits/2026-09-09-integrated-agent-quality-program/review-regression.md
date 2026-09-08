# Phase 0 Review Regression

## Failing run

- Command: `cargo test --workspace --locked --no-fail-fast`
- Exit: 101
- RawStore ID: `20260909-011602-c0bf1b51`
- Standard output SHA-256: `ece9cbe7e619a97f943f4c26ecf65688fcbf8c4805d6e37a09f3b22c5b07e410`
- Standard error SHA-256: `281a658090f892e26e14107114cf2b2181a4db8fac9304c7622a048b4b798468`
- Result: 1,261 library tests passed and one failed; all remaining workspace targets passed
- Failure: `utility::code_graph::tests::from_json_file_round_trips_and_impact_works` returned `None` at the `round-trip` expectation

## Unmodified control run

- Command: `cargo test --workspace --locked --no-fail-fast -q`
- Exit: 0
- RawStore ID: `20260909-011759-2994383c`
- Standard output SHA-256: `24f85cba2f1f8ec4c54c16dc01a27330ad559a9d4787f30898c51a026db8a44d`
- Standard error SHA-256: `72310365dc4868c3839482a0f776831826426a11237c7c60bd2247a61db34460`
- Result: the unmodified control completed 1,407 tests with 0 failed and 0 ignored across 17 test and doc-test result rows

## Post-fix full run

- Command: `cargo test --workspace --locked --no-fail-fast -q`
- Exit: 0
- RawStore ID: `20260909-013126-475700cc`
- Standard output SHA-256: `b7cf4ca5b3b76e9556ce68471ffa5cc64e4d3a6cb2bc56bd9e00c2891a8dede4`
- Standard error SHA-256: `72310365dc4868c3839482a0f776831826426a11237c7c60bd2247a61db34460`
- Result: the post-fix run completed 1,407 tests with 0 failed and 0 ignored across 17 test and doc-test result rows

## Final staged-content full run

- Command: `keel run -- cargo test --workspace --locked --no-fail-fast`
- Exit: 0
- RawStore ID: `20260909-022104-c2a04cae`
- Standard output SHA-256: `a45c91fb0e77482727260550dd45304c302c256572b4082d71ae2d2de7410ac4`
- Standard error SHA-256: `01253a1180d5948557d681e94e7e07ecec13bedd709016786999e5bd69f067bf`
- Result: the final staged-content run completed 1,407 tests with 0 failed and 0 ignored across 17 test and doc-test result rows

## Targeted checks

| RawStore ID | Scope | Result | stdout SHA-256 | stderr SHA-256 |
|---|---|---|---|---|
| `20260909-012647-76d8ce40` | Exact round-trip test after the first isolation edit | 1 passed | `ca8ffb403a2ddd9f3bd0356865a12137f77661f0dfc1ed84e0bcbe0e34aa9c79` | `29c4913e6289d55aa33cd9fdc6097d5a944f08b68dd5aba3aea790f073261a67` |
| `20260909-012858-cbbe871d` | All four `from_json_file_` tests after sibling reconciliation | 4 passed | `6aafdd6a91fd08904488bb5aa308011ffd37c163667c1d9a8298c2e005cd6c27` | `e7507d708b0a83a96010387405e8269cde98d6f27f0852152ef493d0e6ac4ae9` |

## Causal path

1. **Verified:** the test wrote `code-graph.json` directly under its temporary workspace.
2. **Verified:** `artifact_claude_home` could not recover a Keel home from that non-canonical path and returned the default selector.
3. **Verified:** `build_graph_from_workspace_index` resolves that selector through process-global home variables, and the Rust suite runs environment-mutating tests in parallel.
4. **Derived:** a home change between index refresh and read explains the observed `None`; the timing-dependent failure and passing isolated control are consistent with that path.
5. **Verified by the post-fix checks:** explicit unique homes and `cached_artifact_path` keep the three graph-validation tests on isolated index lanes.

The timing dependency is the supported root-cause interpretation, not a claim that the failed interleaving was directly instrumented. The exact test passing alone and the unmodified full-suite control do not negate the failed run.

## Raw transcript packaging

- Command streams use `.stdout.log` and `.stderr.log` so authored-prose gates do not interpret immutable tool output as repository prose.
- `cargo-test.stdout.log` retains Cargo's final blank output line byte-for-byte. A staged `git diff --check` therefore reports that one generated-evidence line; it is a documented raw-transcript exception, not an authored-source whitespace defect.
- The pre-commit slop gate reports six non-blocking copy-duplication candidates. Four come from repeated Cargo output or fixed JSON schema fields; two come from the research record's repeated publication and retrieval metadata labels. Removing any of them would corrupt raw evidence or make the source records structurally inconsistent, so all six are reviewed false positives rather than ignored warnings.
