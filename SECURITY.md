# Security Policy

## Supported Surface

This repository manages local harness skill-pack install and validation flows, generated harness-home wiring, and memory-maintenance helpers. Security-sensitive issues include:

- unintended mutation of `~/.claude`
- path handling and temp-directory cleanup
- command execution boundaries
- secret leakage through docs, tests, or generated files
- prompt-injection or external-content handling guidance

## Reporting a Vulnerability

- Do not open a public issue for a suspected security vulnerability until the impact is understood.
- Prefer GitHub private vulnerability reporting when it is available for this repository.
- If private reporting is not available, contact the repository owner through GitHub first and share only the minimum reproduction needed.

Include:

- affected file or command path
- reproduction steps
- expected behavior
- actual behavior
- impact assessment

## Handling Expectations

- validate and reproduce the report on the narrowest affected surface first
- fix the root cause, not only the visible symptom
- rerun `validate`, `install`, `verify`, and `status` before closing the report
- update README, docs, validator checks, and Rust tests together when the fix changes user-facing security behavior

## Current Validation Posture

The current repository security posture is summarized in [docs/security-audit-status.md](docs/security-audit-status.md). That document is the honest source of what is validated today versus what is still partial or environment-dependent.
