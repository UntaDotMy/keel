<!--
Purpose: Capture feature delivery rules, best practices, and prohibited shortcuts previously inline in AGENTS.md.
Caller: AGENTS.md when shaping commits, PRs, or scope discipline.
Dependencies: keel git-workflow, keel review, request_user_input.
Main Functions: Define one-feature-per-branch discipline, do/don't rules, and the prohibited-shortcut taxonomy.
Side Effects: None — this file is informational.
-->
# Feature Delivery Rules, Best Practices, and Prohibited Shortcuts

## Data and Scope Preservation (read first — overrides the action/autonomy bias)

Never remove or replace existing data, fields, columns, outputs, or records to fit a new format. ADD alongside, and ASK before dropping anything the user did not explicitly name. When a wrong guess would destroy data or waste work, asking is correct, not a failure of decisiveness.

- **Destructive means data loss too, not only dangerous shell.** Removing or replacing a field, column, output, or record in a code or doc edit deserves the same caution as `DROP TABLE`, `rm -rf`, or `git push --force`. The destructive-action radar is not only for shell and infra.
- **Autonomy is for reversible, low-stakes choices only** — naming, formatting, equivalent implementations. It never covers deleting or replacing data, changing a data contract, schema, or report shape, or any choice where two readings differ in what is kept versus discarded. For those, ASK.
- **Ambiguity with a destructive branch = ASK.** If a request could mean "add" or "replace", do not pick the destructive reading to keep moving. Ask "add alongside, or replace?" and wait.
- **Flag-After tripwire.** If you are about to write "note: I removed/changed X" after acting, that disclosure proves you should have asked before acting. Ask first.
- **Scope-diff before finishing.** Before declaring done, state what was asked and what you changed, and confirm the change is a strict superset of existing data unless removal was requested. Surface anything extra.

## Feature Delivery Rules

### Branch model

Hierarchy, promoted one direction only. Integration tiers are permanent; task branches carry hands-on commits:
- **`main`** — final stable, verified. Only receives merges from `dev`. Never commit directly.
- **`dev`** — staging integration. Receives merges from `feat`; features are verified here before promotion to `main`. Never commit directly.
- **`feat`** — feature integration. Receives merges from `task/<task>` once verified. Never commit directly. Bare `feat` only — not `feat/<task>` (Git ref collision).
- **`task/<task>`** — one complete task; branches off `feat` (or a parent task when stacked).
- **`task/<task>/<subtask>`** — one subtask; branches off `task/<task>`.

Promotion flow: `task/<task>/<subtask>` → `task/<task>` → `feat` → `dev` → `main`.

- One task = one `task/<task>` branch (optionally with subtask branches) = merge request into `feat` (or into the parent task for a subtask).
- **Fixes for in-flight work stay on the same work branch** — never open a new branch for review or test fixes.
- Never mix multiple unrelated tasks in the same branch or merge request.
- **Never delete a branch after pushing or merging it** — no `git branch -d/-D`, no `git push origin --delete`. Branches are permanent in this model.
- **Legacy prefixes** (`add/`, `fix/`, `feature/`, …) may finish in flight; preflight warns. New work uses `task/`.
- Use `git add -p` when selective staging is required.
- Review `git diff --cached` before each commit.
- Commit subjects: `Add : FEATURE : short information` — Category capitalized (`Add`, `Config`, `Refactor`, `Wip`, `Fix`, `Docs`); `FEATURE` uppercase; spaces around colons. Example: `Wip : RGB : build light effect mode (multi color)`. Branch names use slashes (`task/rgb-sync`); commit subjects use colons — never conflate them.
- When a commit body is needed, keep it professional and non-chatty, make the title and body match the committed diff exactly, and include only the sections the change genuinely needs. Use this order when present: `Problem`, `Solution`, `Summary`, `Notes`, `What Changed`, `Test Result`. Omit `Problem` and `Solution` when the commit is additive, preventive, or housekeeping rather than fixing a concrete issue, keep `Test Result` limited to validation that directly proves the committed change, and do not mention the harness, keel, or tool-brand validation in commit or PR text unless the change itself is about those surfaces.
- Run `keel git-workflow preflight --repo-root . --base-ref origin/feat` before push or merge-request creation (`origin/dev` when promoting `feat` to `dev`; `origin/main` only when promoting `dev` to `main`).
- When opening a PR or MR from the CLI, never publish bodies with escaped newline sequences such as `\\n`; use a real multiline body or a `--body-file` flow instead.
- Reject or request a split when the diff cannot be described as one cohesive feature.

## Best Practices

### Do:
- Read files before modifying
- Understand existing patterns
- Write minimal, focused code
- Test critical functionality
- **Perform Deep Research** when encountering technical blockers, bug fixes, or how-to implementations. Rely on the 3-round research loop and internal analysis rather than interrupting the user for technical help.
- When the user asks to compare against a repo, product, system, or familiar example, compare apples to apples: match the same surface, same feature class, same scope, and same evaluation criteria instead of blending unrelated strengths. For example, compare workflow versus workflow, memory versus memory, indexing versus indexing, proof surface versus proof surface, or homescreen versus homescreen.
- **Clarify with runtime-safe controls**: If the business requirements, user stories, or product logic are ambiguous, ask the user directly in the normal turn, or use `request_user_input` when that control exists in the active runtime. For non-trivial implementation work, do this before coding whenever acceptance criteria, priorities, or tradeoffs are still unclear after repo inspection. It is critical that the agent and the user stay aligned to prevent "drifting" and building the wrong product. Do not guess the user's intent, and do not start implementation while the core product direction is still unclear.
- Use appropriate skill profiles for task type

### Don't:
- Over-route simple tasks
- Over-engineer solutions
- Add unnecessary features
- Skip security considerations
- Ignore existing code patterns
- Create duplicate functionality

## Prohibited Shortcuts

**Never take these shortcuts** - they create technical debt and maintenance problems:

### Code Quality Shortcuts (CRITICAL)
- **Shortform Variable Names**: Using `usr`, `btn`, `tmp`, `data`, `res`, `req`, `arr`, `obj`, `fn`, `cb` instead of full descriptive names
- **Single-Letter Variables**: Using `x`, `y`, `z`, `a`, `b`, `c` (except i, j, k in simple loops)
- **Cryptic Abbreviations**: Using unclear abbreviations that require mental translation
- **Disabling Linting**: Using `// eslint-disable` or `// @ts-ignore` without clear justification
- **Any Type Abuse**: Using `any` type in TypeScript instead of proper typing
- **Copy-Paste**: Duplicating code instead of extracting shared logic
- **Hardcoding**: Hardcoding values instead of using configuration

### Scope Creep Shortcuts (CRITICAL)
- **Adding Unrequested Features**: Implementing features that weren't asked for
- **Unnecessary Refactoring**: Refactoring code not related to the task
- **Over-Engineering**: Adding abstraction, configuration, or flexibility that wasn't requested
- **Parallel Entry Paths**: Adding extra wrappers, duplicate bootstrap files, alternate installer scripts, or second entrypoints when the existing file can be extended safely
- **Backward Compatibility**: Adding compatibility layers when just updating the feature
- **Keeping Dead Code**: Keeping old code "just in case" instead of deleting it
- **Defensive Programming**: Adding error handling for scenarios that can't happen
- **Speculative Features**: Adding features "for future use"

### Data Loss Shortcuts (CRITICAL)
- **Silent Field Removal**: Deleting a field, column, output, or record the user did not name, to fit a new format
- **Derived Displacing Source**: Replacing a measured or source value with a value computed from it instead of adding alongside
- **Template Omission Copying**: Dropping fields a reference format happens to omit when told to "match" it
- **Destructive Reading of Ambiguity**: Picking "replace" over "add" when the request supports both, to keep moving
- **Flag-After Instead of Ask-Before**: Disclosing a removal after acting instead of asking before

### Testing Shortcuts
- **Test Skipping**: Using `.skip()`, `.only()`, or commenting out failing tests
- **Incomplete Coverage**: Skipping tests for "simple" code or edge cases
- **Mock Abuse**: Mocking critical validation or business logic

### Security Shortcuts
- **Validation Skipping**: Removing validation "temporarily" or only validating client-side
- **Force Flags**: Using `--force`, `--no-verify`, or similar without understanding why
- **Secret Exposure**: Committing secrets, API keys, or credentials

### Performance Shortcuts
- **Premature Optimization Removal**: Removing optimization because "it's too complex"
- **Ignoring Metrics**: Not measuring performance impact of changes

**If you're tempted to take a shortcut, stop and ask:**
1. Why is the proper solution difficult?
2. What's the root cause of the problem?
3. How can I solve it properly?
4. What help do I need?
