# Security Audit Status

## Purpose

This file is the honest security-status artifact for the current governance pass. It separates what the repo truly proves today from claims that would require a formal audit bundle.

The tracked bundle format now lives in [docs/audit-bundle-format.md](./audit-bundle-format.md). That format is the release gate for future numeric scores or named findings lists.

## Verified On 2026-03-15

The current workspace evidence for this repo includes:

- Native Rust coverage passed with `cargo test --workspace`.
- Native manager validation passed with `keel validate --profile smoke`.
- Windows routing is smoke-verified through the Rust CLI so the ordinary path stays on the native CLI without a Git Bash dependency.
- Static grep found no current uses of `shell=True`, `os.system(...)`, or `Invoke-Expression`. The public installer docs intentionally include optional pipe-to-shell bootstrap one-liners, with manual download alternatives for users who want to inspect scripts first.

## What Is Implemented

- Prompt-injection and external-content-as-data guardrails are documented in `AGENTS.md`, `README.md`, and `docs/runtime-guardrails-and-memory-protocols.md`.
- Cross-platform launcher handling now centers on the Rust CLI.
- Native install/update prefers fresh local Cargo builds from the remembered checkout before reusing an installed binary, which closes one stale-cache drift path during local validation.
- The repo includes Rust validation that enforces the current CLI, flow, platform, review, and release-asset surfaces.

## What Is Not Yet Proven

- The repo does **not** currently ship a source-backed penetration-test artifact that proves a public score such as `94/100` or `97 after fixes`.
- The repo does **not** currently ship an archived machine-readable audit report that names the scanner, version, date, findings, and closure evidence.
- The specific historical findings "Python heredoc variable interpolation", "TOCTOU race condition in backup creation", and "PATH-dependent binary resolution" are not all present today as a preserved audit narrative with linked fixes and regression tests, so they should not be claimed as completed evidence unless that artifact is added.

## Current Bundle Status

- Audit bundle format: published in [docs/audit-bundle-format.md](./audit-bundle-format.md)
- Source-backed audit bundle artifact: published in [docs/audits/2026-04-08-repo-governance-and-static-safety/audit-summary.md](./audits/2026-04-08-repo-governance-and-static-safety/audit-summary.md)
- Numeric security score claim: not allowed yet
- Static-safety and governance bundle status: source-backed and published for the current repo snapshot.

## Release Bar For Future Security Claims

Before publishing a numeric score or a named findings list, add a durable audit bundle that follows [docs/audit-bundle-format.md](./audit-bundle-format.md) and includes:

- tool or agent name and version
- audit date and target revision
- exact finding list with severity
- linked remediation commits or file references
- regression validation that proves each fix stayed closed

The repo now ships a source-backed governance and static-safety bundle for the current snapshot, but broader security posture is **partially implemented and only partially proven** until a deeper audit artifact covers scanner identity, revision-specific findings, and closure evidence across the full security surface.
