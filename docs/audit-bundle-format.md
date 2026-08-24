# Audit Bundle Format

This document defines the durable audit bundle format for future security and governance claims.

The same format also applies when benchmark posture claims need a durable, source-backed artifact instead of prose-only scorecard language.

The goal is to make any numeric trust claim or named findings list source-backed, reviewable, and consumable by both humans and AI agents without requiring hidden context from a prior chat or an untracked scanner run.

## Schema version 2

Published bundles use `schema_version: 2`. Historical bundles retain their
original audited revision but must set `bundle_status: "historical"` and
`current_snapshot_claim: false`; they are not evidence for later repository
revisions. When the audit tool version was not recorded, use the explicit value
`"not-recorded"` rather than implying a current tool version.

## Required files

Every published audit bundle should include these files:

- `audit-summary.md`: target scope, audit date, operator or tool identity, and the high-level conclusion.
- `findings.json`: machine-readable finding list with severity, status, evidence path, and remediation state.
- `remediation-map.md`: link each finding to the fixing commit, PR, or exact file reference.
- `regression-proof.md`: list the tests, validations, or hosted checks that prove the finding stayed closed.
- `audit-manifest.json`: machine-readable bundle metadata including repository slug, audited revision, generated date, and file inventory.

## Minimum metadata

Every audit bundle should record:

- `schema_version`
- `bundle_status` (`current` or `historical`)
- repository slug
- audited revision or release tag
- audit date
- auditor identity and tool/agent version
- finding severity scale
- closure status for each finding
- exact evidence paths or URLs
- remediation state for each finding
## Publication rules

- Do not publish a numeric score unless the matching audit bundle exists.
- Do not claim a historical finding is closed unless the remediation and regression proof are included in the bundle.
- If the claim is about benchmark posture, keep the bundle explicit about which repository runs are source-backed and which peer comparisons remain public-surface-only.
- Keep the files plain markdown and JSON so operators and AI agents can inspect them directly after download.
- Prefer one bundle per notable audit event or release rather than one constantly mutated file.

## Relationship to other trust artifacts

The audit bundle is separate from the release-proof bundle.

- The release-proof bundle explains release validation, review posture, hosted workflow completion, and benchmark snapshot state.
- The audit bundle explains security or governance findings, remediation evidence, and regression closure proof.
