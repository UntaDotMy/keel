# Workflow

## Native Command Routing — Must Follow First

Before running raw shell, broad search, or patching existing source, route through the native `keel` surface:

**Token-saving rule:** the goal is to prevent noisy raw command output from entering the harness context. Do not run a raw noisy command first and compact afterward; route through `keel run -- <command>` or the hook-provided `Rerun that as:` wrapper before noisy output is produced.

- **Noisy shell commands:** prefer `keel run -- <command>` for test, build, lint, log, status, search, Docker, Kubernetes, Terraform, package-manager, and CI-style commands. Use `keel rewrite "<command>"` when unsure whether a command has native compaction.
- **Hook block-and-rerun:** if the managed `PreToolUse` hook returns `Rerun that as: <command>`, immediately run that exact command. Do not ask the user, do not treat the hook block as a task failure, and do not repeat the raw command first.
- **Repository search:** prefer `keel code-search search --workspace-root "$PWD" --query "<query>"` before raw `rg`/`grep`/`find`/`git grep`.
- **Existing-source edits:** run or validate Preserve Existing Flow evidence with `keel flow start`, `keel flow check`, and `keel flow finish`, and record the owner path in the global per-workspace flow-check artifact before patching.
- **Commit/PR/final-response text:** use `keel git-workflow commit-message --from-diff`, `keel git-workflow pr-body --from-diff`, and `keel git-workflow lint-message <file>` before submitting, then `keel git-workflow preflight` and `keel review pre-pr` before merge.

## Hook Retry Handling

The managed `PreToolUse` hook may return a harness denial whose reason begins with `Rerun that as:`. This is expected behavior, not a failure. Copy the suggested command, run it exactly once, preserve the exit code and output, and continue from the compacted output. Only ask the user when the suggested command itself is destructive or outside the requested task.

## Branch Model

Four tiers, promoted one direction only. Three tiers are permanent; work branches carry day-to-day commits:

```
main  (final stable — verified after dev passes staging)
dev  (active development — verified when testing new features, staging)
feat  (new features, fixes, subtasks — development branch)
<category>/<FEATURE>  (work branch — all hands-on commits, branch off feat)
```

**Promotion flow:** `work branch` → `feat` → `dev` → `main`

**Fixes stay on the same work branch.** If you commit `Add: RGB: synchronize all` and later find a problem during verification, commit the `Fix: RGB: ...` on that **same** work branch. Do **not** open a new branch for the fix. A work branch accumulates every commit for its feature until verified, then merges up to `feat`. A new branch is only for a genuinely new, separate feature.

**Never delete a branch.** After pushing or merging — at any tier — leave the branch in place. No `git branch -d/-D` or `git push origin --delete`. Branches are permanent.

**Commit locally first.** Always commit to local branch before pushing to the server. Avoid direct commits to the server.

## Feature Branch and Merge Request Rules

- One feature = one `<category>/<FEATURE>` work branch = one merge request into `feat`.
- Do not mix multiple features in the same branch or merge request.
- Create a new work branch off `feat` only for a genuinely new, separate feature — never for a fix to in-flight work.
- Fixes, retries, and subtasks for an in-flight feature commit to that feature's existing work branch.
- If unrelated work is already in the working tree, split it before committing.
- Use patch staging (`git add -p`) to stage only the required feature.
- Review `git diff --cached` before every commit.
- If a change belongs to another feature, move it to that feature's work branch.
- Do not open a merge request with mixed feature scopes.
- Rebase remaining open work branches onto `feat` after another work branch merges.
- Never delete a branch after pushing or merging it.

## Required Naming

- Permanent tiers: `main` (stable), `dev` (staging), `feat` (integration). Hands-on work uses `<category>/<FEATURE>` off `feat` (e.g. `add/RGB`, `Fix/SENSOR`, `Wip/ARGB`).
- **Commit subjects strictly follow `[category]: [feature_category]: short information`**:
  - `[category]` — one of: `Add`, `Config`, `Refactor`, `Wip`, `Fix`, `Docs`
  - `[feature_category]` — what you are working on, uppercase: `RGB`, `LED`, `ARGB`, `SENSOR`
  - `short information` — concise description
  - Example: `Wip: RGB: Build light effect mode (multi color)`
- **Colon vs slash:** commit subject uses colons (`Add: RGB: sync all`); branch name uses a slash (`add/RGB`). Never write a commit with a slash or a branch with a colon.
- When a commit body is needed: `Problem`, `Solution`, `Summary`, `Notes`, `What Changed`, `Test Result` — in that order when present. Omit `Problem` and `Solution` for additive/preventive/housekeeping commits.

## Required Preflight

```bash
keel git-workflow preflight --repo-root . --base-ref origin/feat
```

Use `origin/feat` for work branches; `origin/dev` when promoting `feat` to `dev`; `origin/main` only when promoting `dev` to `main`.

Reject or request a split when:
- the merge request contains more than one feature
- the branch includes unrelated changes
- docs belong to a different feature
- the diff cannot be described as one cohesive feature

## Practical Branch Flow

1. Start from `feat`. Pull it current.
2. If the request is still broad, run `keel workflow route --request "..."` first so the lane choice is explicit. See [docs/first-success-path.md](docs/first-success-path.md) when an operator wants the named end-to-end path before widening into custom flows.
3. Create one new work branch off `feat` (e.g. `git switch -c add/RGB`). Use a `<category>/<FEATURE>` name.
4. Implement only that feature. Fixes and retries for it stay on this same branch.
5. Keep `keel workflow cockpit`, `keel workflow status`, or `keel workflow watch` visible.
6. Use `git add -p` when selective staging is required.
7. Review `git diff --cached`.
8. Commit using the `[category]: [feature_category]: short information` format (categories: `Add`, `Config`, `Refactor`, `Wip`, `Fix`, `Docs`; feature_category uppercase, e.g. `Wip: RGB: Build light effect mode`).
   Commit body when needed: `Problem`, `Solution`, `Summary`, `Notes`, `What Changed`, `Test Result` — in that order when present. Omit `Problem` and `Solution` for additive/preventive/housekeeping commits. Do not mention the harness or keel in commit text unless the change is about those surfaces.
9. Run `keel workflow status` or `keel workflow cockpit`.
10. Run `keel git-workflow preflight --base-ref origin/feat`.
11. Commit locally, then push the work branch. Open one merge request into `feat`. Never delete the branch after pushing or merging.
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

Automation cannot prove semantic singlefeature scope perfectly. Human review and the merge-request checklist remain required for that judgment.

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
