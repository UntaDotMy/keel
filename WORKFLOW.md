# Workflow

## Native Command Routing — Must Follow First

Before running raw shell, broad search, or patching existing source, route through the native `keel` surface:

**Token-saving rule:** the goal is to prevent noisy raw command output from entering the harness context. Do not run a raw noisy command first and compact afterward; route through `keel run -- <command>` or the hook-provided `Rerun that as:` wrapper before noisy output is produced.

- **Noisy shell commands:** prefer `keel run -- <command>` for test, build, lint, log, status, search, Docker, Kubernetes, Terraform, package-manager, and CI-style commands. Use `keel rewrite "<command>"` when unsure whether a command has native compaction.
- **Hook block-and-rerun:** if the managed `PreToolUse` hook returns `Rerun that as: <command>`, immediately run that exact command. Do not ask the user, do not treat the hook block as a task failure, and do not repeat the raw command first.
- **Repository search:** prefer `keel code-search search --workspace-root "$PWD" --query "<query>"` before raw `rg`/`grep`/`find`/`git grep`. After a fix or implement, run `keel code-search siblings` and handle every hit.
- **Existing-source edits:** run or validate Preserve Existing Flow evidence with `keel flow start`, `keel flow check`, and `keel flow finish`, and record the owner path in the global per-workspace flow-check artifact before patching.
- **Commit/PR/final-response text:** use `keel git-workflow commit-message --from-diff`, `keel git-workflow pr-body --from-diff`, and `keel git-workflow lint-message <file>` before submitting, then `keel git-workflow preflight` and `keel review pre-pr` before merge.

## Hook Retry Handling

The managed `PreToolUse` hook may return a harness denial whose reason begins with `Rerun that as:`. This is expected behavior, not a failure. Copy the suggested command, run it exactly once, preserve the exit code and output, and continue from the compacted output. Only ask the user when the suggested command itself is destructive or outside the requested task.

## Git Workflow

▎ Durable reference. Consulted before any branch/commit decision. Applies to all features going forward.

### Branch structure

| Branch | Purpose | What lands here |
|---|---|---|
| `main` | Final stable, verified. Only after `dev` passes staging. | Receives merges only — never direct commits. |
| `dev` | Staging layer. Feature sets are tested here before promotion to `main`. | Verified features merged from `feat`. |
| `feat` | Integration base. All task branches and their subtasks merge here eventually. | Merges from `task/<task>` branches. |
| `task/<task>` | One complete task. Parent branch. Sub-branches stack off here. | Merges from `task/<task>/<subtask>` sub-branches. |
| `task/<task>/<subtask>` | One subtask or concern within a task. Short-lived. One MR each. | The actual work commits. |

▎ **Namespace note (Git hard rule):** The integration branch is bare `feat` (`refs/heads/feat`). Work branches **must not** use `feat/<task>` — Git cannot store both `refs/heads/feat` and `refs/heads/feat/...` at the same time (ref lock collision). That is why work lives under `task/<task>` and `task/<task>/<subtask>`.

**Flow direction:** `task/<task>/<subtask>` → `task/<task>` → `feat` → `dev` → `main`. Work moves only upward, via merge. Never commit directly to `main`, `dev`, or `feat`.

**Verification gates:**
- A subtask done → `task/<task>/<subtask>` merges into `task/<task>`.
- All subtasks for a task done → `task/<task>` merges into `feat`.
- A feature set ready for staging → `feat` merges into `dev`.
- Staging passes → `dev` merges into `main`.

**Hard rule:** Never delete any branch — local or remote. Merged branches stay as permanent references.

**Legacy branches:** In-flight work on older prefixes (`add/`, `fix/`, `feature/`, etc.) may continue until merge. Preflight warns but does not block. **New** branches use `task/<task>` (or `task/<task>/<subtask>`).

### CI / Action workflow gate

This gate applies only when explicitly requested to push or merge. It never triggers automatically.

**Before opening an MR or merging**
1. Check if the repo has Action workflows: `ls .github/workflows/` (or GitHub UI → Actions).
2. If workflows exist — read each file. Understand triggers, checks, and what must pass.
3. Before committing, ensure the work satisfies every check the workflow will run.
4. After pushing — wait for green before merging: push → CI runs → all checks green → then merge (if requested).
5. If CI fails: fix on the **same** branch, push again, wait for green again. Do not merge a red branch.
6. If CI is skipped or the repo has no workflows: proceed directly to merge (if requested).
7. If workflows exist on `feat`, `dev`, or `main`: the same gate applies when merging upward.

Use `keel git-workflow await-ci --watch` (auto-detects `glab` then `gh`). It polls the head commit and exits non-zero while pending, red, or timed out.

▎ Never merge while CI is running. Never merge on a red status. If not requested to merge — stop after push and report the CI status.

### Commit convention

```
<Category> : <FEATURE_CATEGORY> : <short info>
```

- **Category** (capitalized first letter): `Add`, `Config`, `Refactor`, `Wip`, `Fix`, `Docs`
- **FEATURE_CATEGORY** (uppercase): e.g. `RGB`, `LED`, `ARGB`, `SENSOR`, `PROTOCOL`, `UI`, `HID`, `WATCHER`, `CATALOG`, `DEVICE`
- Spaces around all colons.
- Examples:
  - `Add : PROTOCOL : rgb sync ask and ack parse`
  - `Wip : RGB : build light effect mode (multi color)`
  - `Fix : UI : show rgb sync state on device card`

Keep commits small — one layer or concern per commit.

▎ Legacy history may use lowercase categories or no spaces around colons. Going forward, use the capitalized + spaced form. Do not rewrite legacy commits.

### Worktree discipline (parallel work)

When running many tasks simultaneously, each task lives in its own worktree: **one worktree = one branch = one task**. Never mix branch work across worktrees.

```bash
# Task branch
git worktree add ../project-<task> task/<task>

# Subtask branch
git worktree add ../project-<task>-<subtask> task/<task>/<subtask>
```

Before any work, confirm identity:

```bash
git worktree list
git branch --show-current   # must match the task
pwd                         # must be the correct worktree directory
```

Never `git checkout` another task's branch inside a worktree — use that task's worktree. A branch can only be checked out in one worktree at a time. Commits, rebases, and pushes happen inside the worktree that owns the branch.

### Starting a new task

Single question: does this task depend on code from an unmerged `task/<other>` branch?

**Case 1 — Independent task (default).** Branch off `feat`:

```bash
git checkout feat && git pull
git checkout -b task/<task>
# optional: git worktree add ../project-<task> task/<task>
```

**Case 2 — Dependent task (stacked).** Branch off the parent task's tip:

```bash
git checkout task/<parent-task>
git checkout -b task/<task>
```

Do not branch off `feat` when the parent is unmerged — it will not have the parent's code.

**When NOT to stack:** if the parent is unstable and churning, wait.

### Starting a subtask

A subtask always branches off its parent task:

```bash
git checkout task/<task>
git checkout -b task/<task>/<subtask>
# optional: git worktree add ../project-<task>-<subtask> task/<task>/<subtask>
```

Never branch a subtask off `feat` directly.

### Working on a branch

```bash
# Commit per layer, e.g.:
#   Add : PROTOCOL : rgb sync ask and ack parse
#   Add : UI : show rgb sync state on device card
git add <files>
git commit -m "Add : PROTOCOL : rgb sync ask and ack parse"
```

Verify locally first. Fix on the same branch. Only push and open an MR when explicitly requested.

### Pushing and opening an MR

Only on explicit user request, after local verification:

```bash
# Subtask MR: task/<task>/<subtask> → task/<task>
git push -u origin task/<task>/<subtask>

# Task MR: task/<task> → feat
git push -u origin task/<task>
```

Then follow the CI gate. Wait for green before merging (if requested).

### Keeping stacked branches in sync

**Parent gets fixes (still unmerged):**

```bash
git checkout task/<task>/<subtask>
git fetch origin
git rebase task/<task>
git push --force-with-lease
```

**Parent merges into its integration branch:**

```bash
git checkout task/<task>/<subtask>
git fetch origin
git rebase origin/task/<task>   # or origin/feat if the task merged into feat
git push --force-with-lease
```

Deep stacks: rebase bottom-up. Never rebase the top before the middle.

### Review process

- One MR per branch. Subtask MR targets parent task; task MR targets `feat`.
- Keep MRs small. One layer or concern per commit.
- MR description: what the branch does, parent target, stack context, expected CI checks.
- Do not open a draft/WIP MR unless asked. Open when verified and ready.
- CI green before review is requested (if workflows exist).
- Resolve comments on the same branch — never a new branch for review fixes.

### When a branch merges

Leave the branch. Never delete. Rebase stacked children if any.

### Rollback reference

| State | Command | Scope |
|---|---|---|
| Before push | `git reset --hard <commit>` | Current branch only |
| After push (solo branch) | `git reset --hard <commit>` && `git push --force-with-lease` | Current branch only |
| Before any rebase | `git tag pre-rebase-<branch>`; undo with `git reset --hard pre-rebase-<branch>` | That branch |
| Parent merged, found bad | `git revert -m 1 <merge-sha>` on the integration branch | Reverts parent; rebase children afterward |

### Invariants

1. A subtask branches off its parent task's tip — never off `feat` directly.
2. A dependent task branches off the parent task's tip — never off `feat` if the parent is unmerged.
3. Fixes to a parent are expected; rebase children onto the new tip.
4. Rebase a stacked child whenever its parent changes (fixes or merge).
5. Rebase bottom-up in deep stacks.
6. Tag known-good points before rebases.
7. Never delete branches.
8. One MR per branch.
9. Confirm branch and worktree identity before every operation.
10. If workflows exist, read them before pushing; wait for green before merging.
11. Never use `feat/<task>` as a work branch while bare `feat` is the integration tier (Git ref collision).

### Anti-patterns

- Branching a subtask off `feat` instead of `task/<task>`.
- Branching a dependent task off `feat` instead of the parent task.
- Using `feat/<task>` while bare `feat` exists (Git cannot store both).
- Running work for task A inside task B's worktree.
- Checking out a different branch inside a worktree instead of using the correct worktree.
- Letting a stacked child drift after its parent changes.
- Merging the parent into the child instead of rebasing the child.
- Rebasing the top of a stack before the middle.
- Self-merging without review.
- Committing directly to `main`, `dev`, or `feat`.
- Pushing before committing locally.
- Merging while CI is red or still running.
- Pushing or opening an MR before the work is verified, or without an explicit user request.

### Workflow preference memory

`keel git-workflow configure --model four-tier [--note "..."]` saves the chosen branch+commit workflow to the global per-workspace memory lane (never the repo). Recall with `keel git-workflow show`.

### Agent execution checklist

**Before starting**
1. Confirm identity: `git worktree list` + `git branch --show-current` + `pwd`.
2. Determine dependency:
   - Independent → `git checkout feat && git pull && git checkout -b task/<task>`.
   - Dependent (stable parent) → `git checkout task/<parent> && git checkout -b task/<task>`.
   - Subtask → `git checkout task/<task> && git checkout -b task/<task>/<subtask>`.
   - Dependent but parent unstable → wait.
3. Optional worktree: `git worktree add ../<name> <branch>`.

**During work**
- Commit per layer: `Add : FEATURE : info`. Commit locally first.
- Verify locally. Fix on the same branch.

**Before pushing (only on explicit request)**
- Check workflows; satisfy them; then `git push -u origin <branch>`.
- Open MR targeting the correct parent. Include purpose, target, stack context, expected CI.

**After pushing**
- Wait for green if workflows exist. Fix on same branch if red. Merge only if requested and green.

**If stacked — parent gets fixes (unmerged)**
- Rebase child onto parent; deep stacks bottom-up; `--force-with-lease`.

**If stacked — parent merges**
- Rebase child onto `origin/task/<task>` or `origin/feat` as appropriate.

**On merge**
- Leave the branch. Never delete. Rebase stacked children if any.

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

## Automation Boundaries

Automation can enforce:
- branch naming
- clean working tree state before push
- base-ref visibility
- changed-file and commit-subject reporting
- merge-request checklist presence

Automation cannot prove semantic singlefeature scope perfectly. Human review and the merge-request checklist remain required for that judgment.

## Completion and Re-Audit Rules

- **Honest-closeout gate** (`CLAUDE_SKILLS_BRIEF_GATE` / completion-gate): do not present work as done while the working brief's acceptance criteria are unmet. Clear it with `keel memory completion-gate check` after the criteria have evidence, and use `keel anvil sieve` / `keel anvil stamp` when the delivery ran through Anvil.
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
