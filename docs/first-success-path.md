# First Success Path

This guide is the fastest honest path through the current native CLI surface.

It is for a new operator who already has `keel` installed and wants one satisfying end-to-end run without memorizing every command first.

## Goal

Install and verify the binary, capture one real request as a tracked working brief, carry the change through the Anvil delivery loop, prove the branch through the review gates, and close out with the proof recorded against the brief.

## Five-Minute Path

### 1. Refresh the native shell and verify readiness

```bash
keel install
keel status
keel doctor
```

What to look for:
- managed install and binary health (`keel status`)
- doctor follow-up guidance if the environment is not ready yet

### 2. Capture the job as a tracked working brief

```bash
keel memory working-brief write --request "Compare the current repo, fix the biggest gap, and carry the branch to closure" --constraints "<hard limits>" --acceptance-criteria "<what proves it>"
keel memory working-brief list
```

Hold onto the brief id; the completion gate at closeout checks against it.

### 3. Carry the change through the Anvil delivery loop

```bash
keel anvil compile
keel anvil run
```

Anvil is the only core delivery-loop surface: frozen prompt prefix, PPT+EV tracking, bounded loop. `keel anvil cast`, `anvil sieve`, `anvil stamp`, and `anvil prefix-check` cover the supporting steps.

### 4. Turn local work into proof before you call it done

```bash
keel review pre-pr --base-ref origin/main
keel git-workflow preflight --repo-root . --base-ref origin/main
```

### 5. If the branch is on GitHub, wait for the real hosted result

```bash
gh pr checks --watch
```

If a hosted lane fails, fix the root cause on the same branch, push again, and rerun `gh pr checks --watch`.

### 6. Close the brief only after the proof is real

```bash
keel memory completion-gate check --brief-id <brief-id> --proof "review pre-pr passed; hosted checks green"
```

The `--proof` text lands on the completion-gate record so future audits can see what was claimed when the work was closed.

## Why this is the first success path

- It starts from one broad request captured in a tracked brief instead of forcing vocabulary first.
- It uses only commands the runtime actually implements today.
- It keeps one visible route from intake to proof to closeout against the working brief.
- It ends with real proof recorded on the gate, not a confident-looking summary.
