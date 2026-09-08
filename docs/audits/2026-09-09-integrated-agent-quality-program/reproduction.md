# Reproduction Commands

Run from the repository root on Windows PowerShell 7:

```powershell
& "docs/audits/2026-09-09-integrated-agent-quality-program/run-phase-0-baseline.ps1"
```

The script executes the required commands serially, keeps stdout and stderr separate, records exact argv and exit codes, and hashes both streams with SHA-256. It writes to:

```text
docs/audits/2026-09-09-integrated-agent-quality-program/raw/phase-0/
```

The script completes every capture even if an earlier command fails, then exits 1 when any required command was nonzero. At this baseline, the expected nonzero command is the prompt's bare completion-gate invocation because the current CLI requires a brief ID and proof.

To check the Phase 0 working brief after all Phase 0 evidence is final:

```powershell
keel memory completion-gate check `
  --brief-id integrated-agent-quality-phase-0-20260909 `
  --proof "Research, raw baseline streams, warning baseline, environment, and fixed-context measurements are linked from phase-0-baseline.md"
```

Do not replace the captured bare command with this scoped command in the baseline manifest. They prove different facts: the first records the requested command's current behavior; the second performs the actual Phase 0 completion check.
