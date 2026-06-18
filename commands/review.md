---
description: Run the keel native review gates (pre-commit, pre-pr, gate check) on the current diff before closing work. Use to get a deterministic local quality gate with fail-closed verdicts.
argument-hint: "[pre-commit|pre-pr|gates] [base-ref]"
allowed-tools: Read, Bash(keel review:*), Bash(git diff:*), Bash(git status)
---

# /keel:review

Run a keel native review surface. Arguments: **$ARGUMENTS**

Use the installed binary path (bare `keel` is not guaranteed on PATH):
`~/.claude/keel` (macOS/Linux), `%USERPROFILE%\.claude\keel.exe`
(Windows), or `cargo run --bin keel --` from a source checkout.

Map the surface in `$0` to the matching native subcommand:

- `pre-commit` → `review pre-commit --format compact` — local pre-commit gate.
- `pre-pr` → `review pre-pr --base-ref <ref> --format compact` — pre-PR gate (defaults base-ref to origin/feat when none given, since work branches branch off `feat`; use origin/dev when promoting `feat` to `dev`, and origin/main only when promoting `dev` to `main`).
- `gates` → `review gates check --surface pre-pr --base-ref <ref> --format compact` — explicit gate verdict.
- `diff` → `review diff` — review the working diff.

If no surface is given, default to `review pre-pr --base-ref origin/feat`.

This is the deterministic CLI gate. For a deeper, evidence-backed code review of
non-trivial changes, also invoke the `reviewer` skill — the CLI gate and the
reviewer skill are complementary, not substitutes. Report the verdict honestly:
if a gate fails or is blocked, say so and name the blocker instead of soft-passing.
