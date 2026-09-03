# Contributing

## Purpose

This repository is a managed harness skill pack, not a loose prompt collection. Contributions should preserve the same production-readiness standard across code, docs, tests, generated home wiring, and validation behavior.

## Contribution Workflow

1. Start from a concrete working brief.
2. Preserve one top-level plan item per explicit user task.
3. Keep the first implementation pass anchored to the named scope.
4. Prefer small validated batches over large rewrites.
5. Patch the source doctrine, validator, and contract coverage together when a rule is meant to fail closed.
6. Update README or supporting docs when user-facing behavior changes.
7. Run the proving loop before asking for review.

## Feature Delivery Rules

- Branch model: `main` ← `dev` ← `feat` ← `task/<task>`. Parallel subtask branches use flat sibling names such as `task/<task>-<subtask>`; never use nested `task/<task>/<subtask>` (Git ref collision). Never use `feat/<task>` while bare `feat` exists.
- One task = one `task/<task>` branch (flat sibling subtasks such as `task/<task>-<subtask>`) = merge request into `feat` (or into the parent task branch).
- Fixes for in-flight work stay on the same work branch — never open a new branch for a fix to work already underway.
- Commits: `Add : FEATURE : short info` (capitalized category, spaces around colons).
- Do not mix unrelated features in the same branch.
- **Never delete a branch** after pushing or merging it (`git branch -d/-D`, `git push origin --delete` are not part of the normal flow).
- Use `git add -p` when selective staging is required.
- Review `git diff --cached` before each commit.
- Commit subjects: `Add : FEATURE : short information` (capitalized category; FEATURE uppercase; spaces around colons). Branch names use slashes (`task/rgb-sync`); commit subjects use colons — never conflate them.
- Run `keel git-workflow preflight --repo-root . --base-ref origin/feat` before push or merge-request creation (`origin/dev` when promoting `feat` to `dev`; `origin/main` only when promoting `dev` to `main`).
- When opening a PR or MR from the CLI, use a real multiline body or `--body-file` instead of embedding escaped newline sequences such as `\\n` in the published text.
- Follow [WORKFLOW.md](WORKFLOW.md) when the change touches branching, merge-request shape, or reviewer expectations.

## Required Validation

Run this default native loop from the repository root against a temporary keel home:

```bash
temporary_keel_home="$(mktemp -d)/.keel"
KEEL_HOME="$temporary_keel_home" cargo run --bin keel -- validate --profile smoke
KEEL_HOME="$temporary_keel_home" cargo run --bin keel -- install --repo-root "$PWD"
"$temporary_keel_home/keel" verify --repo-root "$PWD"
"$temporary_keel_home/keel" status --repo-root "$PWD"
```

Windows contributors should run the same Rust CLI shape from PowerShell:

```powershell
$temporaryKeelHome = Join-Path $env:TEMP "keel-test-home\.keel"
New-Item -ItemType Directory -Force -Path $temporaryKeelHome | Out-Null
$env:KEEL_HOME = $temporaryKeelHome
cargo run --bin keel -- validate --profile smoke
cargo run --bin keel -- install --repo-root .
& (Join-Path $temporaryKeelHome "keel.exe") verify --repo-root .
& (Join-Path $temporaryKeelHome "keel.exe") status --repo-root .
```

Use the live `~/.keel` target only as an intentional final check when the change specifically needs that real-home proof. `~/.claude` is the harness engagement home (skills, agents, hooks), not the keel binary home.

PATH wiring is owned by native `keel install`. See the README and [docs/compatibility-matrix.md](docs/compatibility-matrix.md) for the one PATH story; do not re-teach it here.

Full validate now proves the Rust-native CLI foundation. Install the stable Rust toolchain before running the complete repository loop locally; CI enforces the same Rust workspace proof.

When the change touches a narrower surface, also run the smallest direct proof that covers the edited area, such as `cargo test --workspace`, `cargo test -p <crate>`, or `cargo build --release --bin keel`.

## Scope Rules

- Do not add parallel install or update entrypoints when the managed ones can absorb the change.
- Do not add new helper functions when existing code already owns the behavior cleanly.
- Do not present partial implementation as complete.
- Do not weaken runtime-safe clarification, live-research-first behavior, completion discipline, or memory-safety rules.

## Documentation Rules

- Keep committed comments and docs professional, concise, and neutral.
- Use README for end-user setup, architecture, and operational workflow.
- Use AGENTS.md and skill docs for agent doctrine, not marketing copy.
- Keep SECURITY.md current when the reporting path or validated security posture changes.

## Review Expectations

- Findings-first review for bugs, regressions, missing validation, or misleading docs
- Honest status labels for what is verified, inferred, skipped, or blocked
- Root-cause fixes over workaround-only patches

## Cross-Platform Expectations

- macOS, Linux, and Windows behavior should stay aligned
- the Rust-native CLI is the only supported install/update entrypoint
- Shell and PowerShell launcher scripts must not be reintroduced as runtime paths
