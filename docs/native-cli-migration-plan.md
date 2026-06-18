<!--
Purpose: Document the Rust-native keel CLI migration state and remaining hardening work.
Caller: Contributors, release maintainers, and agents checking whether the runtime may fall back to Go.
Dependencies: Rust workspace, shell and PowerShell wrappers, release workflow, validate workflow, and managed install surfaces.
Main Functions: State the canonical implementation language, supported entrypoints, validation gates, and no-Go fallback rule.
Side Effects: Sets contributor expectations for future runtime work and release proof.
-->
# Native CLI Migration Plan

## Status

The canonical public runtime is now Rust.

- `rust/crates/keel` owns top-level CLI dispatch.
- The Rust CLI no longer imports or invokes any legacy fallback binary.
- Unknown commands fail closed with an explicit Rust-runtime error instead of spawning Go.
- Source-checkout wrappers build with `cargo build --release --bin keel`.
- Validate and release workflows build and test the Rust workspace rather than running Go commands.

## Decision Record

### Selected Native Language

Rust is the selected implementation language for the native CLI.

### No-Go Fallback Rule

The runtime must not fall back to Go.

- Do not add Go source-checkout invocations to wrappers, hooks, workflows, docs, or Rust command dispatch.
- Do not rebuild a sibling legacy fallback binary.
- Do not shell out to Go to satisfy an unported command.
- If a command is incomplete, keep it Rust-native and fail honestly or implement the missing owner in Rust.

## Current Rust-Owned Surfaces

- `help`, `version`, `platform`, `bootstrap-info`
- `install`, `update`, `status`, `doctor`, `verify`, `uninstall`, `validate`, `all`, `menu`
- `flow start`, `flow check`, `flow finish`
- `review` local gates and hosted artifact export
- `git-workflow` text generation and lint helpers
- `run`, `rewrite`, and `hook`
- lightweight Rust-native `code-search`, `design-intelligence`, `memory`, `memoriesv2`, `orchestration`, `workflow`, `gain`, and `bench` surfaces

## Validation Contract

Use Rust proof for this repository:

```bash
cargo fmt --all --check
cargo test --workspace
cargo build --release --bin keel
./target/release/keel validate --repo-root . --profile smoke
./target/release/keel flow finish --repo-root . --json
```

## Remaining Hardening Work

The first Rust no-fallback cut preserves the public command names and core lifecycle behavior. Follow-up work should deepen command parity in Rust without restoring Go:

- Replace lightweight memory/workflow/orchestration placeholders with full Rust state machines.
- Expand Rust command-output compaction policies beyond raw-output recovery.
- Restore release asset breadth for every target once each runner or cross-linker is proven.
- Keep deleted Go-era behavior from reappearing by requiring new runtime work to land in Rust crates with Rust validation.
- Keep docs and examples centered on `cargo run --bin keel -- ...`, wrappers, or installed Rust binaries.
