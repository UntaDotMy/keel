# Phase 8 UI/UX Visual Verification and Evidence Storage

## Delivered contract

Keel now supports visual verification and evidence capture via the `keel verify ui` CLI command and RawStore screenshot storage. Automated and human-in-the-loop visual inspection workflows are grounded with immutable screenshots, explicit criteria evaluation, and honest status semantics.

1. **Visual Verification Criteria Schema**:
   - `UiCriterion` captures state name, target route or screen, preconditions, fixture data, auth conditions, user actions, expected visible text, expected interactions, layout responsiveness, and accessibility expectations.
2. **Visual Adapter Resolution Ladder**:
   - Adapter detection automatically resolves in priority order: `computer-use` -> `playwright` -> `needs_human`.
   - When no automated visual tool is available, evaluation safely degrades to `VisualAdapter::NeedsHuman`.
3. **Honest Visual Verdict Semantics**:
   - Four visual verdicts supported: `Pass`, `Fail`, `Unclear`, `NeedsHuman`.
   - `Unclear` visual evaluations map honestly to `needs_human` review status, never yielding a false-green pass.
   - Missing required visual elements trigger deterministic failures with descriptive missing-element lists.
4. **RawStore Visual Evidence Preservation**:
   - `RawStore::save_screenshot` stores screenshots (`screenshot.png`), command invocations (`command.txt`), stdout logs (`stdout.log`), stderr logs (`stderr.log`), and execution metadata (`meta.json`) under immutable UUID-keyed directories.
   - `keel raw <raw_id>` inspects and renders screenshot file locations, output logs, and exit status.
   - `keel raw --path <raw_id>` outputs the exact directory path for programmatic access.
5. **CLI Command Integration**:
   - `keel verify ui` sub-command added to operator help, command parser, and manager dispatch.
   - Supports `--fixture <path>`, `--task <task_id>`, `--plan <plan_id>`, `--adapter <name>`, and `--json` formatting.

## Requirement evidence

| Requirement | Evidence |
| --- | --- |
| Visual criterion schema | `UiCriterion` and `UiEvidenceRecord` serde models in `rust/crates/keel/src/utility/ui_verify.rs`. |
| Adapter resolution | `detect_visual_adapter` priority order verified by `visual_adapter_detection_priority` unit test and `ui_verify_needs_human_when_adapter_missing` integration test. |
| Honest status semantics | `evaluate_visual_criterion` unit tests and `ui_verify_unclear_maps_to_needs_human` integration test verify unclear and missing adapters map to `needs_human` review status. |
| RawStore screenshot storage | `RawStore::save_screenshot` tested in `raw_store_screenshot_storage_and_retrieval` unit test and `ui_verify_pass_and_rawstore_retrieval` integration test. |
| CLI runner integration | `tests/ui_verification_test.rs` integration tests verify end-to-end execution of `keel verify ui` and `keel raw`. |

## Validation Summary

| Test Surface | Result | Detail |
| --- | --- | --- |
| UI verification unit tests | 3 passed, 0 failed | `cargo test -p keel utility::ui_verify::tests` |
| UI verification integration tests | 5 passed, 0 failed | `cargo test -p keel --test ui_verification_test` |
| Workspace test suite | 1,277 passed, 0 failed | `cargo test --workspace` via `review pre-pr` |
| Pre-commit review | pass (0 blocking) | `keel review pre-commit` |
| Pre-pr review | pass (0 blocking) | `keel review pre-pr --base-ref task/integrated-agent-quality-phase-7 --plan plan-1788924151887175800-6292-f12ca3f8` |
| Rust format | pass | `cargo fmt --all -- --check` |
| Rust clippy | pass (0 warnings) | `cargo clippy --all-targets -- -D warnings` |

## Release Ladder

| Rung | Verdict | Evidence |
| --- | --- | --- |
| Smoke | pass | `keel verify ui` help and flag validation run cleanly. |
| Functional | pass | Criterion evaluation handles pass, fail, unclear, and needs_human accurately. |
| Integration | pass | `ui_verification_test.rs` validates end-to-end fixture execution and RawStore retrieval. |
| UI | pass | `verify ui` validates screens, captures PNG artifacts, and binds visual evidence records. |
| Load | not applicable | Synchronous verification over local fixture files and RawStore artifacts. |
| Stress | not applicable | File size limit checks (16 MB bound) prevent resource exhaustion. |
| Security | pass | Sanitized paths, bounded fixture reading, and fail-closed evaluation on missing elements. |
