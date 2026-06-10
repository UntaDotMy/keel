<!--
Purpose: Capture feature delivery rules, best practices, and prohibited shortcuts previously inline in AGENTS.md.
Caller: AGENTS.md when shaping commits, PRs, or scope discipline.
Dependencies: claude-skills git-workflow, claude-skills review, request_user_input.
Main Functions: Define one-feature-per-branch discipline, do/don't rules, and the prohibited-shortcut taxonomy.
Side Effects: None — this file is informational.
-->
# Feature Delivery Rules, Best Practices, and Prohibited Shortcuts

## Feature Delivery Rules

### Branch model

Four tiers, promoted one direction only. The three upper tiers are permanent; work branches carry hands-on commits:
- **`main`** — final stable, verified. Only receives merges from `dev`. Never commit directly.
- **`dev`** — staging integration. Receives merges from `feat`; features are verified here before promotion to `main`. Never commit directly.
- **`feat`** — feature integration. Receives merges from work branches once verified. Never commit directly.
- **work branch** `<category>/<FEATURE>` (e.g. `add/RGB`, `fix/SENSOR`) — all hands-on commits. Branch off `feat`, one coherent feature per branch.

Promotion flow: `work branch` → `feat` → `dev` → `main`.

- One feature = one `<category>/<FEATURE>` work branch = one merge request into `feat`.
- **Fixes for in-flight work stay on the same work branch** — if `add: RGB: synchronize all` is committed and testing surfaces a problem, commit the `fix: RGB: ...` on that same branch, never a new one. A work branch accumulates every commit for its feature until verified, then merges up to `feat`. A new branch is only for a genuinely new, separate feature.
- Never mix multiple features in the same branch or merge request.
- **Never delete a branch after pushing or merging it** — no `git branch -d/-D`, no `git push origin --delete`. Branches are permanent in this model.
- Use `git add -p` when selective staging is required.
- Review `git diff --cached` before each commit.
- Commit subjects strictly follow `<category>: <FEATURE>: <short information>` (colon-separated). Categories (lowercase): `add`, `config`, `refactor`, `wip`, `fix`, `docs`. `<FEATURE>` is the component/area in uppercase (e.g. RGB, LED, ARGB, SENSOR). Example: `wip: RGB: Build light effect mode (multi color)`. The commit uses colons; the branch name uses a slash (`add/RGB`) — never conflate them.
- When a commit body is needed, keep it professional and non-chatty, make the title and body match the committed diff exactly, and include only the sections the change genuinely needs. Use this order when present: `Problem`, `Solution`, `Summary`, `Notes`, `What Changed`, `Test Result`. Omit `Problem` and `Solution` when the commit is additive, preventive, or housekeeping rather than fixing a concrete issue, keep `Test Result` limited to validation that directly proves the committed change, and do not mention Claude Code, claude-skills, or tool-brand validation in commit or PR text unless the change itself is about those surfaces.
- Run `claude-skills git-workflow preflight --repo-root . --base-ref origin/feat` before push or merge-request creation (`origin/dev` when promoting `feat` to `dev`; `origin/main` only when promoting `dev` to `main`).
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
