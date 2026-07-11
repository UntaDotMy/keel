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

## Git Workflow

▎ Durable reference. Consulted before any branch/commit decision. Applies to all features going forward.

### Branch structure

```
main           Final stable, verified. Only after dev passes staging. Nothing committed directly — receives merges only.
dev            Active development / staging. Daily commits land here via merge from feat. A feature set is tested here before promotion to main.
feat           Feature development base. New features and fixes branch off here. Merges from feature/<name> sub-branches.
feature/<name> One feature or subtask. Short-lived. One MR each. The actual work commits.
```

▎ **Namespace note:** feature branches use `feature/<name>`, NOT `feat/<name>`. Git stores branches as files under `refs/heads/`, so `feat` (a file) and `feat/x` (needs `feat` to be a directory) collide. `feature/` avoids this. The integration branch stays `feat`.

**Flow direction:** `feature/<name>` → `feat` → `dev` → `main`. Work moves only upward, via merge. Never commit directly to `main`, `dev`, or `feat`.

**Verification gates:**
- A feature set ready for staging test → `feat` merges into `dev`.
- Staging passes → `dev` merges into `main`.
- `dev` is not a daily commit target — it's the staging layer features merge into for testing.

**Hard rule:** When pushing and merging, NEVER DELETE ANY BRANCH. Merged branches stay as permanent references.

### Commit convention

```
<Category>: <FEATURE_CATEGORY> : <short info>
```

- **Category** (capitalized first letter): `Add`, `Config`, `Refactor`, `Wip`, `Fix`, `Docs`
- **FEATURE_CATEGORY** (uppercase): `RGB`, `LED`, `ARGB`, `SENSOR`, `PROTOCOL`, `UI`, `HID`, `WATCHER`, `CATALOG`, `DEVICE`
- Spaces around all colons.
- Examples:
  - `Add : PROTOCOL : rgb sync ask and ack parse`
  - `Wip : RGB : Build light effect mode (multi color)`
  - `Fix : UI : show rgb sync state on device card`

▎ The existing repo history mixes casing for older areas (`protocol`, `docs`, `watcher`, `catalog` lowercase; `UI`/`HID` uppercase). Going forward use UPPERCASE for FEATURE_CATEGORY. Legacy commits are not rewritten.

Keep commits small — one layer/concern per commit.

### Core rules

1. Never delete branches — local or remote. Ever.
2. Commit locally first. Never push or open an MR until the work is correct and verified, and wait for explicit user request before pushing or opening a merge request. Do not push or open an MR on your own initiative — "do the work" is not "publish the work." Local first, always.
3. Fixes stay on the same branch. If `Add : RGB : synchronize all` has a bug found in testing, the fix commits to the same `feature/RGB` branch — not a new branch. The branch is the unit of the feature; fixes extend it.
4. No self-merge without review. Self-merging was an old habit — it's done. Features get an MR.
5. Work moves only upward: `feature/<name>` → `feat` → `dev` → `main`.

### Starting a new feature

Every new feature starts from one of two points, decided by a single question: does this feature need code from another feature that is not yet merged?

**Case 1 — Independent feature (the default).** The feature does not depend on any unmerged work. Branch off `feat`.

```bash
git checkout feat && git pull && git checkout -b feature/<name>
```

**Case 2 — Dependent feature (stacked).** The feature needs code from a `feature/<parent>` branch that is finished but not yet merged (sitting in review/testing). You don't want to wait for the merge. Branch off the parent's tip so the new branch inherits the parent's code.

```bash
git checkout feature/<parent>          # the finished, unmerged feature
git checkout -b feature/<name>         # new branch has the parent's code
```

Do not branch off `feat` here — it wouldn't have the parent's code and won't compile.

**When NOT to stack:** if the parent is unstable and likely to change a lot during review, don't stack on it yet. Wait until it stabilizes, or merge it first. Stacking on heavily-churning work causes repeated rebase conflicts.

### Branch naming

`feature/<short-kebab-name>` — describes the feature, not the dependency. Example: `feature/rgb-sync`.

### Working on a feature (both cases)

```bash
# commit per layer, e.g.:
#   Add : PROTOCOL : rgb sync ask and ack parse
#   Add : UI : show rgb sync state on device card
git push -u origin feature/<name>
# open MR: feature/<name> → feat
```

### Keeping a stacked branch in sync

A dependent (stacked) branch must track its parent. The parent can change in two ways — handle each:

**When the parent gets fixes (still unmerged).** Rule 3 guarantees this happens: bugs found in testing get fixed on the parent's own branch. After the parent gets new commits, absorb them into the child:

```bash
git checkout feature/<child>
git fetch origin
git rebase feature/<parent>            # absorb the parent's new commits
git push --force-with-lease            # update the child MR
```

Rebase onto the parent's current tip (`feature/<parent>`), not onto `feat`.

**When the parent merges into feat.** Once the parent is merged, `feat` contains its commits. Rebase the child onto `feat` so the parent's commits drop out and the child keeps only its own:

```bash
git checkout feature/<child>
git fetch origin
git rebase origin/feat                 # drops parent's now-merged commits
git push --force-with-lease            # updates the child MR
```

Git detects the parent's commits are already in `feat` and drops them automatically.

**The two rebase triggers (reference):**

| Trigger | Rebase target | Effect |
|---|---|---|
| Parent gets fixes, still unmerged | `feature/<parent>` | Child absorbs the parent's new commits |
| Parent merges into `feat` | `origin/feat` | Parent's commits drop out; child keeps only its own |

### Deep stacks — rebase bottom-up

When multiple branches are stacked and a lower one changes (fixes or merge):

```
feat ─── feature/a (got fixes, or merged)
          └── feature/b (has a+b)
               └── feature/c (has a+b+c)
```

Rebase lowest first, each onto the one below it:

```bash
# if feature/a got fixes (unmerged):
git checkout feature/b && git rebase feature/a && git push --force-with-lease
git checkout feature/c && git rebase feature/b && git push --force-with-lease

# if feature/a merged into feat:
git checkout feature/b && git fetch origin && git rebase origin/feat && git push --force-with-lease
git checkout feature/c && git rebase feature/b && git push --force-with-lease
```

Never rebase the top before the middle — the middle goes stale and must be rebased again.

### When a feature merges

Leave the branch. Never delete. It stays as a permanent reference. If the branch had stacked children, rebase them (per the triggers above) before or after the merge.

### Rollback reference

| State | Command | Scope |
|---|---|---|
| Before push | `git reset --hard <commit>` | current branch only |
| After push (solo branch) | `git reset --hard <commit> && git push --force-with-lease` | current branch only |
| Before any rebase | `git tag pre-rebase <branch>`; undo with `git reset --hard pre-rebase` | that branch |
| Parent merged, found bad | `git revert -m 1 <merge-sha>` on the integration branch | reverts parent; rebase children afterward |

### Invariants

1. A dependent feature branches off its parent's tip, never off `feat`.
2. Fixes to the parent are expected. After fixing the parent, rebase the child onto the parent's new tip.
3. Rebase a stacked child whenever its parent changes (fixes or merge) — don't let it drift.
4. Rebase bottom-up in deep stacks.
5. Tag known-good points before rebases.
6. Never delete branches.
7. One MR per branch.

### Anti-patterns

- ❌ Branching a dependent feature off `feat` instead of the parent → won't compile.
- ❌ Letting a stacked child drift after its parent changes → stale child, bigger conflicts later.
- ❌ Merging the parent into the child instead of rebasing the child → merge bubbles, noisy history.
- ❌ Rebasing the top of a stack before the middle → stale middle.
- ❌ Self-merging without review.
- ❌ Committing directly to `main`, `dev`, or `feat`.
- ❌ Pushing before committing locally.
- ❌ Pushing or opening an MR before the work is verified correct, or without an explicit user request to do so. Local first — "do the work" ≠ "publish the work."

### Agent execution checklist

Before starting a feature:
1. Determine dependency: does this feature need code from an unmerged `feature/<name>`?
   - NO → `git checkout feat && git pull && git checkout -b feature/<name>`.
   - YES (stable parent) → `git checkout feature/<parent> && git checkout -b feature/<name>`.
   - YES (unstable parent) → wait; don't stack on churning work.
2. Work + commit per layer: `<Category>: <FEATURE_CATEGORY> : <info>`. Commit locally first.
3. Verify locally: run the app, test the behavior, confirm the work is correct. Fix on the same branch if not.
4. Push + open MR — only on explicit user request, and only after verified. Do NOT push or open an MR automatically. Target `feat`. Do not delete on merge.
5. If stacked — on parent fixes (still unmerged):
   - `git checkout feature/<child>`
   - `git fetch origin`
   - `git rebase feature/<parent>` ← absorb fixes
   - Resolve conflicts if any → `git rebase --continue`.
   - `git push --force-with-lease`
   - For deeper stacks, repeat bottom-up.
6. If stacked — on parent merge into `feat`:
   - `git checkout feature/<child>`
   - `git fetch origin`
   - `git rebase origin/feat` ← drop merged parent commits
   - `git push --force-with-lease`
   - For deeper stacks, repeat bottom-up.
7. On own merge: leave the branch. Never delete.

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
