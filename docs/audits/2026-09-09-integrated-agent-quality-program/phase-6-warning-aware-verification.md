# Phase 6 Warning-Aware Verification and Resolution Gate

## Delivered contract

`keel run` now parses structured diagnostic warnings from compiler, linter, analyzer, and package manager streams independently of output compaction. An exit code of 0 is no longer treated as proof of zero defects when warnings are emitted.

1. **Strict Project Verification Policy**: `keel.filters.toml` supports a declared `[verification].commands` list. `keel doctor` and `keel verify config` inspect the workspace and provide actionable diagnostics if Flutter or Dart projects lack strict analyzer verification, without silently rewriting commands.
2. **Multi-Language Diagnostic Parser**: `rust/crates/keel/src/proxy/warnings.rs` extracts diagnostics across:
   - Flutter/Dart: bullet format (`info • message • file:line:col • rule`) and machine format (`severity|type|code|path|line|col|length|message`)
   - Rust: rustc stderr warning spans and compiler JSON messages
   - TypeScript: tsc diagnostic format (`file(line,col): warning/error TS...`)
   - C/C++: gcc and clang warning format (`file:line:col: warning:`)
   - Python: pytest warning summary lines
   - Package managers: npm, pnpm, and yarn deprecation and security notices
3. **Persistent Warning Lifecycle Ledger**: Located at `~/.keel/memories/workspaces/<workspace-key>/warnings/ledger.jsonl`:
   - `baseline`: Established on first family run; visible in `keel warn list` but non-blocking for unrelated work.
   - `open`: Newly introduced diagnostics; blocking for task completion and review.
   - `resolved`: Automatically marked when a relevant clean re-run runs without the warning fingerprint.
   - `waived`: Created via `keel warn waive <fingerprint> --reason "<>=10 chars>" --expires <duration>`; reopens when expired.
4. **Ticket & Traceability Integration**: Open warnings automatically enrich active task tickets in the `lint_warnings` layer and update RTM traces so warning resolution is a tracked subtask.
5. **Enforcement Gates**: `warnings_gate` is integrated into `keel review pre-pr` and `keel memory completion-gate check`.
6. **Token-Bounded Next-Turn Pointer**: Injected context is strictly limited to `warnings: <N> open (<M> new) - run keel warn list`, proven by tests to remain well under the 30-token cap.
7. **Offline Flutter E2E Verification**: `tests/warning_workflow_test.rs` validates the complete open -> ticket enrichment -> review gate block -> fix -> resolved -> pass cycle without network access.

## Requirement evidence

| Requirement | Evidence |
| --- | --- |
| Strict verification config | `keel.filters.toml` declared `[verification].commands`; tested via `keel verify config` and unit test `verify_config_reports_actionable_flutter_policy_without_failing_or_writing`. |
| Diagnostic parser coverage | Real fixtures for Flutter/Dart, Rust, TypeScript, gcc/clang, Python/pytest, and npm/pnpm in `proxy::warnings::tests`. |
| Fingerprint deduplication | Hash of file, line, rule, and normalized message tested in `fingerprints_deduplicate_normalized_messages`. |
| Baseline non-blocking | `first_scan_is_baseline_then_new_warning_opens_and_clean_rerun_resolves` and `gate_lists_baseline_without_blocking_and_blocks_only_open`. |
| New warning blocks review | `warnings_gate_blocks_open_diagnostics_but_not_baseline` in `review::tests`. |
| New warning blocks completion | `completion_gate_blocks_when_workspace_has_open_warning` in `utility::memory::tests`. |
| Ticket subtask generation | `open_warning_adds_lint_warnings_subtask_without_replacing_existing` in `warnings::tests`. |
| Waiver lifecycle | `waiver_requires_reason_and_reopens_after_expiry` and `completion_gate_and_waiver_lifecycle_offline`. |
| Token pointer budget | `pointer_stays_below_thirty_tokens` validates that the pointer costs 16 tokens (well below the 30 token cap). |
| Offline Flutter E2E | `flutter_offline_e2e_open_fix_resolved_lifecycle` in `tests/warning_workflow_test.rs` validates the full lifecycle with an offline mock flutter runner. |

## Validation Summary

| Test Surface | Result | Detail |
| --- | --- | --- |
| Warning unit tests | 14 passed, 0 failed | `cargo test -p keel proxy::warnings` |
| Warning integration tests | 2 passed, 0 failed | `cargo test --test warning_workflow_test` |
| Review gate regressions | 87 passed, 0 failed | Includes `warnings_gate` unit tests |
| Memory completion regressions | 49 passed, 0 failed | Includes warning-aware completion gate tests |
| Rust format | pass | `cargo fmt --all -- --check` |
| Rust clippy | pass (0 warnings) | `cargo clippy --all-targets -- -D warnings` |

## Release Ladder

| Rung | Verdict | Evidence |
| --- | --- | --- |
| Smoke | pass | `keel warn list` and `keel verify config` execute cleanly. |
| Functional | pass | 16 dedicated unit and integration tests prove parser, ledger, waivers, and gates. |
| Integration | pass | Warning lifecycle binds seamlessly to task tickets, review gates, and completion gates. |
| UI | not applicable | CLI and proxy runtime surfaces only. |
| Load | not applicable | Ledger file bounded to 16 MB; pubspec traversal capped to 1 MB and 128 entries. |
| Stress | not applicable | Local file-based JSONL ledger and synchronous proxy reconcile. |
| Security | pass | Rejects path traversal; enforces mandatory non-empty waiver rationale and expiration. |
