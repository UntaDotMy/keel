# Workflow

## Native Command Routing — Must Follow First

Before running raw shell, broad search, or patching existing source, route through the native `keel` surface:

**Token-saving rule:** the goal is to prevent noisy raw command output from entering Claude Code context. Do not run a raw noisy command first and compact afterward; route through `keel run -- <command>` or the hook-provided `Rerun that as:` wrapper before noisy output is produced.

- **Noisy shell commands:** prefer `keel run -- <command>` for test, build, lint, log, status, search, Docker, Kubernetes, Terraform, package-manager, and CI-style commands. Use `keel rewrite "<command>"` when unsure whether a command has native compaction.
- **Hook block-and-rerun:** if the managed `PreToolUse` hook returns `Rerun that as: <command>`, immediately run that exact command. Do not ask the user, do not treat the hook block as a task failure, and do not repeat the raw command first.
- **Repository search:** prefer `keel code-search search --workspace-root "$PWD" --query "<query>"` before raw `rg`/`grep`/`find`/`git grep`.
- **Existing-source edits:** run or validate Preserve Existing Flow evidence with `keel flow start`, `keel flow check`, and `keel flow finish`, and record the owner path in the global per-workspace flow-check artifact before patching.
- **Commit/PR/final-response text:** use `keel git-workflow commit-message --from-diff`, `keel git-workflow pr-body --from-diff`, and `keel git-workflow lint-message <file>` before submitting, then `keel git-workflow preflight` and `keel review pre-pr` before merge.

## Hook Retry Handling

The managed `PreToolUse` hook may return a Claude Code denial whose reason begins with `Rerun that as:`. This is expected behavior, not a failure. Copy the suggested command, run it exactly once, preserve the exit code and output, and continue from the compacted output. Only ask the user when the suggested command itself is destructive or outside the requested task.

## Branch Model

Four tiers, promoted in one direction only. The three upper tiers are permanent; work branches are where day-to-day commits land.

- **`main`** — final stable, verified. Only receives merges from `dev`. Never commit directly to `main`.
- **`dev`** — staging integration. Receives merges from `feat` and is where a feature is verified on staging before promotion to `main`. Never commit directly to `dev`.
- **`feat`** — feature integration. Receives merges from work branches once each piece of work is verified. Never commit directly to `feat`.
- **work branch** `<category>/<FEATURE>` (e.g. `add/RGB`, `fix/SENSOR`) — all hands-on commits live here. Branch off `feat`, keep one coherent feature per branch.

Promotion flow: `work branch` → `feat` (feature integration) → `dev` (staging verify) → `main` (stable).

**Fixes for in-flight work stay on the same work branch.** If you commit `add: RGB: synchronize all` and later find a problem during verification, commit the `fix: RGB: ...` on that **same** work branch — do **not** open a new branch for the fix. A work branch accumulates every commit for its feature (any category: `add`, `fix`, `wip`, `refactor`, ...) until the whole feature is verified, then merges up to `feat`. A new branch is only for a genuinely new, separate feature.

**Never delete a branch.** After pushing new work or merging it — at any tier — leave the branch in place. Do not run `git branch -d/-D` or `git push origin --delete`. Branches are permanent history in this model; merge does not imply cleanup.

## Feature Branch and Merge Request Rules

- One feature = one `<category>/<FEATURE>` work branch = one merge request into `feat`.
- Do not mix multiple features in the same branch or merge request.
- Create a new work branch off `feat` only for a genuinely new, separate feature — never to hold a fix for work already in flight on another branch.
- Fixes, retries, and subtasks for an in-flight feature commit to that feature's existing work branch, regardless of their commit category.
- If unrelated work is already in the working tree, split it before committing.
- Use patch staging (`git add -p`) to stage only the required feature.
- Review `git diff --cached` before every commit.
- If a change belongs to another feature, move it to that feature's work branch.
- Do not open a merge request with mixed feature scopes.
- Avoid duplicate behavior or overlapping implementation across work branches.
- Rebase remaining open work branches onto `feat` after another work branch merges.
- Never delete a branch after pushing or merging it.

## Scope Definition

A feature branch may contain:
- the code for one user-visible feature or one tightly related fix
- tests for that same feature
- docs only for that same feature

A feature branch must not contain:
- unrelated refactors
- another feature
- another bug fix outside the same feature scope
- opportunistic cleanup unless explicitly requested

## Required Naming

- The permanent tiers are `main`, `dev`, and `feat`. Hands-on work uses a `<category>/<FEATURE>` work branch off `feat` (e.g. `add/RGB`, `fix/SENSOR`, `wip/ARGB`); fixes and subtasks for that feature stay on the same work branch.
- **Commit subjects must follow `<category>: <FEATURE>: <short information>`** and this format is strictly enforced.
  - `<category>` is one of (lowercase): `add`, `config`, `refactor`, `wip`, `fix`, `docs`.
  - `<FEATURE>` is the component or area being worked on, written in uppercase, e.g. `RGB`, `LED`, `ARGB`, `SENSOR`.
  - `<short information>` is a concise description of the change.
  - Example: `wip: RGB: Build light effect mode (multi color)`.
  - **Colon vs slash:** the commit subject uses colons (`add: RGB: sync all`); the branch name uses a slash (`add/RGB`). Same category vocabulary, different separator — never write a commit with a slash or a branch with a colon.
- When a commit body is needed, keep it professional, non-chatty, and matched to the committed diff. Use a precise title, include only the sections the change genuinely needs, and keep this order when a section is present: `Problem`, `Solution`, `Summary`, `Notes`, `What Changed`, `Test Result`. Omit `Problem` and `Solution` when the commit is additive, preventive, or housekeeping rather than fixing a concrete issue, and keep `Test Result` limited to validation that directly proves the committed change.
- do not mention Claude Code, keel, or tool-brand validation in commit or PR text unless the change itself is about those surfaces.

## Required Preflight

Run the native Git workflow preflight before push or merge-request creation. Use the integration target as the base ref — `origin/feat` for work branches, `origin/dev` when promoting `feat` to `dev`, and `origin/main` only when promoting `dev` to `main`:

```bash
keel git-workflow preflight --repo-root . --base-ref origin/feat
```

The preflight blocks on branch naming, dirty worktrees, empty diffs, and missing committed history against the target base ref. It warns when commit subjects drift from the `<category>: <FEATURE>: <short information>` format or suggest mixed scope.

When opening a GitHub pull request or GitLab merge request from the CLI:
- do not pass literal escaped newline sequences such as \`\\n\`, \`\\r\`, or \`\\t\` inside the rendered title or body text
- use a real multiline body, an editor flow, or a body file such as \`gh pr create --body-file <path>\`
- preview the rendered body before submission when the command path performs shell quoting or variable interpolation

## Merge Request Template

- GitLab contributors should use [.gitlab/merge_request_templates/Feature.md](.gitlab/merge_request_templates/Feature.md).

## Reviewer Reject Rules

Reject or request a split when:
- the merge request contains more than one feature
- the branch includes unrelated changes
- docs belong to a different feature
- the diff cannot be described as one cohesive feature

## Practical Branch Flow

1. Start from `feat` (the feature-integration branch). Pull it current.
2. If the request is still broad, run `keel workflow route --request "..."` first so the lane choice is explicit. See [docs/first-success-path.md](docs/first-success-path.md) when an operator wants the named end-to-end path before widening into custom flows.
3. Create one new work branch off `feat` with normal Git tooling (e.g. `git switch -c add/RGB`). Use a `<category>/<FEATURE>` name.
4. Implement only that feature. Fixes and retries for it stay on this same branch — do not branch again for a fix to in-flight work.
5. Keep `keel workflow cockpit`, `keel workflow status`, or `keel workflow watch` visible while the branch is active so stage, active lane, proof state, blockers, and the next command stay easy to scan.
6. Use `git add -p` when selective staging is required.
7. Review `git diff --cached`.
8. Commit using the `<category>: <FEATURE>: <short information>` format (categories: `add`, `config`, `refactor`, `wip`, `fix`, `docs`; FEATURE uppercase, e.g. `wip: RGB: Build light effect mode (multi color)`).
   If a commit body is included, keep it professional, make the title and body match the committed diff exactly, include only the sections the change genuinely needs, and keep this order when a section is present: `Problem`, `Solution`, `Summary`, `Notes`, `What Changed`, `Test Result`. Omit `Problem` and `Solution` when the commit is additive, preventive, or housekeeping rather than fixing a concrete issue, and keep `Test Result` limited to validation that directly proves the committed change.
   do not mention Claude Code, keel, or tool-brand validation in commit or PR text unless the change itself is about those surfaces.
9. Run `keel workflow status` or `keel workflow cockpit` when the team needs the current ledger state in one place.
10. Run `keel git-workflow preflight --base-ref origin/feat`.
11. Push the work branch and open one merge request into `feat`. Never delete the branch after pushing or merging.
12. Once the feature is verified, promote `feat` into `dev` and verify on staging; after staging passes, promote `dev` into `main`. Repeat on a new work branch for the next feature.

If another, separate feature appears during implementation:
- do not keep it in the same branch
- stash it or leave it unstaged
- create another `<category>/<FEATURE>` work branch for it later

## Automation Boundaries

Automation can enforce:
- branch naming
- clean working tree state before push
- base-ref visibility
- changed-file and commit-subject reporting
- merge-request checklist presence

Automation cannot prove semantic single-feature scope perfectly. Human review and the merge-request checklist remain required for that judgment.

## Completion and Re-Audit Rules

- **Honest-closeout gate** (`CLAUDE_SKILLS_STORY_CLOSEOUT_GATE`): if the workspace has an active sprint with open or blocked stories, the PostToolBatch gate injects a gap report naming each unfinished story. Do not present work as done while stories remain incomplete — loop back, finish them, or document the blocker honestly. Clear the gate by advancing stories (`keel sprint advance --id <id> --state done`) until `keel sprint review` reports COMPLETE.
- Do not call a task done when the implementation is only partially complete.
- For brownfield work, identify the preserved flow before implementation: target file or function, current behavior to preserve, entry point, producer, source of truth, storage or queue, side-effect owner, consumer, cleanup or recovery, edit boundary, and validation needed. If that ownership path is still unknown, keep reading or report the blocker instead of patching the first suspicious branch.
- Existing source-file edits need preserve-existing-flow evidence in the global per-workspace flow-check artifact unless the task is docs-only, formatting-only, generated-only, or explicitly greenfield. Use `keel flow start`, `keel flow check`, and `keel flow finish` to create and validate that artifact.
- Before closing any task, re-audit the finished change against the user story, PRD or spec when one exists, explicit task list, active plan items, tracked requirements, required lanes, and closure-ready proof.
- Do not close the current job scope until it is 100% complete for that scope, not just partially green.
- If the task is tracked in phases or priorities such as P0, P1, and P2, do not advance to the next layer until the current layer is fully complete and re-audited.
- If the audit still shows an open task, active plan item, unresolved requirement, non-terminal required lane, or missing proof, the work is not finished.
- Do not trust the first green rerun after a fix as closure by itself; rerun the narrow proving checks and re-audit the broader impacted system before handoff.
- Use \`keel workflow route\` when the request is broad and the right lane is not obvious yet; the route surface should explain why the recommended path fits the job before any stateful work begins.
- Use \`keel workflow cockpit\` for the live operator console with stage, active entries, blockers, and the next command, \`keel workflow status\` and \`keel workflow dashboard\` for the broader ledger state, and \`keel workflow watch\` for ongoing lane health.
- Use \`keel workflow finish --id <entry-id> --proof "..."\` when the workstream is ready to close so the closure proof lands on the workflow ledger entry. Use \`keel workflow resume --id <entry-id>\` to reopen a tracked workstream.

## Spawned Agent Discipline

- Before spawning another same-role lane, inspect the current registry or list view and reuse the existing agent for that workstream when it is still the right owner.
- Do not spam new spawned agents for the same role and workstream when a reusable lane already exists.
- If a spawned agent is materially required for the task, wait for its terminal state before calling the job done.
- Never rush a required spawned agent or other required dependent lane. Careful review, debugging, and specialist work are slower by design, and waiting is better than self-certifying early.
- Do not interrupt a required spawned agent just to hurry a result unless the user explicitly cancels or redirects that lane.

## Research-First Implementation Rules

- Understand the request before building. Restate what the user actually asked, confirm the user story, and identify what is genuinely needed before writing code. Do not guess, do not assume, do not build against an imagined spec. The fact-research bullets below refresh *how* to build (syntax, releases, conventions); this bullet establishes *what* to build. Correct code that solved the wrong problem still gets thrown away, so confirm the target first. If the request is ambiguous in a way that changes what you build, ask before building.
- Use `preserve-existing-flow` before changing any existing source file, established function, loop, handler, queue, state machine, transport path, firmware path, protocol flow, or source-of-truth ownership. New behavior should layer through the existing owner unless the user explicitly approves replacing that owner.
- When the job is covered by a native `keel` command, prefer the native executable or source-checkout command path instead of recreating the behavior through ad hoc generic tool calls.
- Before writing non-trivial code, run a targeted research pass for the active language, framework, runtime, and harness so syntax, release changes, tooling behavior, and repository conventions are current instead of assumed from memory.
- Verify the relevant language, framework, runtime, and tooling release notes, syntax changes, validation behavior, and repository harness conventions before coding.
- Treat model memory as a starting point, not proof. Refresh the exact parts that affect the code being written.
- For benchmark claims, competitive audits, product comparisons, or example-following requests, compare feature by feature and apples to apples: workflow versus workflow, memory versus memory, indexing versus indexing, proof surface versus proof surface, or homescreen versus homescreen.
- Match the inspection tool to the surface being validated: use browser automation such as Playwright for web flows, use the live desktop runtime with screenshots or equivalent visual evidence for desktop flows, and use the most direct runtime-native inspection tool for CLI, service, workflow, or device issues.
- Re-audit the finished result against the user story, PRD or spec when one exists, explicit tasks, active plan items, and validation proof before calling the scoped job complete.

## Hosted PR Check Discipline

- When a repository has CI or CD, do not treat local green as final proof by itself.
- After opening or updating the PR, wait at least 20 seconds so the hosted lanes have time to appear, then inspect the real hosted checks with \`gh pr checks --watch\` or the equivalent hosted watcher.
- If a hosted lane fails, inspect the failing logs, identify the root cause, add or tighten the regression guard, push the fix to the same branch, and wait again.
- Never rush a required spawned agent, required validation lane, or other required dependent lane. Careful review, debugging, and specialist work are slower by design, and waiting is better than self-certifying early.
- When hosted lanes fail, capture the failing lane name, root cause, regression requirement, and rerun proof commands together in the working brief or PR notes so the repair path is explicit and reusable.
- Do not open a second PR for the same feature just to recover from a failing check; keep fixing the same PR until the hosted lanes are green or a real blocker is documented.
- Treat repeated hosted failures as reusable knowledge. The goal is to understand the failure class so the same mistake does not need to be rediscovered on the next feature.
- Do not end the task or the turn while a required validation command, hosted check, or other dependent process is still running, failing, or unresolved when the issue is fixable in scope.
- If validation, review, or hosted checks fail, keep iterating in the same turn until the failure is fixed or a real blocker is documented honestly.
