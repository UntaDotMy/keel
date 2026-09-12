# Reliability surface audit

This audit maps the attached operating-system plan's reliability requirements
(sections 53 through 58) to executable repository evidence. It covers the
`task/final-agent-operating-system` delivery point and does not convert local
source-build evidence into packaged-release or hosted-environment proof.

## Evidence matrix

| Requirement | Current evidence | Status and boundary |
| --- | --- | --- |
| Adversarial MCP inputs | `rust/crates/keel/tests/mcp_protocol.rs:466-580` covers null IDs, malformed parameters, missing metadata, and retired initialize requests; `rust/crates/keel/src/mcp/mod.rs:2591` covers interleaved catalog and tool calls. | **Partial**: focused cases exist; the plan's complete malformed-header, cursor-replay, snapshot-mutation, and cache-hint matrix is not one release gate. |
| Adversarial context inputs | `rust/crates/keel/tests/context_gateway_test.rs:6-221` covers empty, duplicate, zero-budget, low-budget failure, and namespace/path cases. | **Partial**: the listed cases are executable; malformed Unicode, reducer failure, RawStore failure, and serialization-failure cases are not consolidated here. |
| Concurrency | `rust/crates/keel/tests/mcp_protocol.rs:542-567` drives 32 parallel HTTP clients; the in-process MCP test also checks pipelined `tools/list` and `tools/call` independence. | **Pass for these deterministic probes; not proof of a full persistence contention or soak profile.** |
| Restart and recovery | `.github/release-smoke.mjs:289-395` writes a brief, retrieves it, closes the MCP process, reconnects, and retrieves the marker again. `release_smoke_test.rs` executes that script against the Cargo-built binary and asserts the restart and bounded-output checks. | **Pass for source-build smoke; packaged archive restart proof remains a release-workflow concern.** |
| Host conformance | `rust/crates/keel/tests/host_conformance.rs:25-93` requires eight passing proxy stages and marks unsupported hosts `not_run`; the release workflow runs the host-conformance gate. | **Partial**: this is the native fixture path, not live third-party-host coverage. |
| Packaged binary smoke | `.github/workflows/release.yml:430-490` extracts each platform archive, installs it, runs status/verify/doctor, and invokes the release smoke script. | **Not verified in this local audit**: a hosted or manually packaged report is required for a release claim. |
| Property/fuzz invariants | No repository property/fuzz target was found for generated cursors, headers, task trees, Unicode, or bounded projections. | **Not verified**: focused examples do not substitute for the plan's generated-input invariants. |
| Bounded storage and lifecycle | Existing memory, recall, and learning tests cover individual bounds and persistence behavior. | **Partial**: a single reliability artifact does not yet prove bounded storage under long-running concurrent/restart load or the complete supersede/expire lifecycle. |

## Local source-build proof

The added integration test requires Node.js because it runs the same
`.github/release-smoke.mjs` used by the release workflow. It stages one real
skill under an isolated `.claude` home, keeps all writes under a unique
temporary `.keel` root, and asserts:

- protocol discovery reports `2026-07-28`;
- every smoke check with token fields stays within its reported budget;
- the 20,000-byte command is marked truncated with more raw than visible
  tokens; and
- the persisted marker is retrieved after the MCP process restarts.

This test proves the script wiring and restart behavior of a built workspace
binary. It does not prove archive extraction, installation, signing, or
platform-specific package behavior.

## Release decision

The reliability surface remains **Partial / NeedsHuman** for the attached
plan. The local source-build smoke, focused concurrency probes, and native host
fixture are useful regression barriers. A release decision still requires the
packaged workflow report, live host evidence where a host is claimed governed,
and an explicit disposition for the unverified property/fuzz and long-running
storage matrix.
