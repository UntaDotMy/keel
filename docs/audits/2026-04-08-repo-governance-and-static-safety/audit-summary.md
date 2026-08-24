# Repo Governance And Static Safety Audit Summary

- Audit date: 2026-04-08
- Repository: `UntaDotMy/keel`
- Audited revision: `5e0bcb33f20ba626ca3e32657ae6f1c8350b9b2c`
- Bundle status: historical; not current proof for later revisions.
- Scope: governance posture, trust-artifact posture, native CLI compatibility, and static-safety evidence
- Evidence sources: this bundle's manifest, findings, remediation map, regression proof, and the referenced repository paths present at the audited revision.

## Conclusion

The current repo snapshot closes the earlier trust-artifact and CLI-compatibility gaps that blocked a stronger evidence-backed comparison against `runtime-shell comparator` and `workflow-teaching comparator`.

This bundle proves a narrow claim:

- the repo now publishes a source-backed audit artifact for governance and static-safety posture
- the native CLI now resolves memory and orchestration command groups through the Rust runtime instead of a legacy fallback

This bundle does not claim a full penetration test, a full hosted-runtime audit, or a numeric security score.

## Closed Findings

1. `audit-bundle-missing`
   The repo had an audit-bundle format but no published current bundle artifact.
   Status: closed by this bundle publication and the linked status-page update.
2. `legacy-memory-command-friction`
   Memory command groups previously depended on legacy routing during the audit walkthrough.
   Status: closed by Rust-native memory command-group resolution.
3. `legacy-task-lifecycle-flag-friction`
   Task lifecycle command groups previously depended on legacy routing during the audit walkthrough.
   Status: closed by Rust-native orchestration and workflow command-group resolution.

## Remaining Boundary

This bundle is enough to support source-backed governance and static-safety claims for the current repo snapshot. It is not enough to claim overall leadership versus both comparison repos on every dimension. The next proof gap is a shared benchmark harness plus a smoother first-run operator path.
