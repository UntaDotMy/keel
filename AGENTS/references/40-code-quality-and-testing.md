<!--
Purpose: Capture code quality standards, testing requirements, and feature flags previously inline in AGENTS.md.
Caller: AGENTS.md when implementation, naming, structure, DRY, simplicity, comments, or test discipline is in scope.
Dependencies: keel review, .claude/review.json, native review surfaces.
Main Functions: Define readability rules, scope discipline, structure/modularity, DRY, simplicity, professional comments, and the testing ladder.
Side Effects: None — this file is informational.
-->
# Code Quality Standards and Testing Requirements

## Code Quality Standards

**CRITICAL - Always enforce these rules:**

**Readability (Non-Negotiable):**
- **NO shortform variable names**: Use full, descriptive names
  - BAD: `usr`, `btn`, `tmp`, `data`, `res`, `req`, `arr`, `obj`, `fn`, `cb`, `idx`, `len`, `str`, `num`
  - GOOD: `user`, `button`, `temporaryValue`, `userData`, `response`, `request`, `userArray`, `userObject`, `handleClick`, `callback`, `currentIndex`, `arrayLength`, `userName`, `itemCount`
- **NO single-letter variables** except loop counters in simple loops (i, j, k)
- **NO abbreviations** unless universally known (URL, API, HTTP, ID)
- **Clear function names**: Verb + noun (e.g., `getUserData`, `calculateTotal`, `validateEmail`)

**Scope Discipline & Greenfield vs. Brownfield Rules:**
- **Brownfield (Existing Code)**: Strict compliance. ONLY implement what was requested. NO unrequested features, NO refactoring unrelated code, NO speculative "future-proofing".
- **Named Scope First**: If the user asks to change function A, start with function A and its direct dependencies or callers. Expand only when impact analysis proves a broader change is required.
- **Greenfield (New Projects)**: Architectural Innovation is ALLOWED. If scaffolding a new project, you MUST set up advanced, scalable boilerplate (e.g., proper dependency injection, generic types, robust folder structures) proactively to prevent future technical debt, even if not explicitly detailed by the user.
- **When updating a feature:**
  - Just update it - don't keep old code
  - Delete unused code completely
  - NO backward compatibility unless explicitly requested
  - Prefer small, batch-sized patches that keep review, validation, and rollback simple
  - Re-read the touched code and rerun the lightest proving validation after each batch before expanding scope

**Structure & Modularity (User Preference):**
- Prefer modular structure: keep entrypoints thin and move named logic into focused files or modules.
- Keep route handlers, controllers, pages, CLI entrypoints, and main scripts short; let them orchestrate and delegate instead of owning business logic directly.
- When a project spans backend, API, frontend, workers, or tests, separate those concerns clearly instead of collapsing them into one large file.
- Extend an existing entrypoint, installer, updater, or wrapper before adding a new one; do not create a parallel setup path when the current entry file can absorb the behavior cleanly.
- Keep one obvious install or update path per platform by default; reject extra bootstrap wrappers, duplicate installer scripts, or alternate entry files unless the user explicitly asks for a separate path.
- Prefer surgical patches over full rewrites when only part of a file is affected.
- For code changes, always work in small, reviewable batches rather than one large pass.
- Keep tracing easy: a reviewer should be able to identify where behavior lives without reading one giant file.

**DRY (Don't Repeat Yourself):**
- Reuse existing code, extract shared logic
- No duplicate functions or logic

**Simplicity:**
- Minimal solution that works
- No over-engineering
- No premature optimization
- No fake completion or workaround-only delivery; find the verified root cause and implement the real fix
- **Security**: Validate inputs, no injection risks
- **Testing**: Specific requirements below

**Professional Comments and Documentation:**
- Keep committed comments and documentation professional, concise, and neutral.
- Avoid first-person and second-person pronouns in committed comments or documentation unless quoting user-provided text or an external source.
- Cap each implementation comment (`//`, `#`) at two lines. Longer explanation means the code needs a clearer name or a small refactor, not more prose. Structured file/item doc headers (`///`, `//!`, `<!-- -->`) are exempt from the cap but still follow the wording rules.
- Never use an em dash or en dash in a comment; it reads as machine-generated. Use a period or comma. Avoid the ` - ` aside and chatty filler ("just", "simply", "now we", "hope this helps").
- State what the code does, not a narration of the change. The comment-style gate (`keel review comments`) is fail-closed on added lines: an over-length implementation comment or any em/en dash blocks; first-person, chatty, and hype wording warn.
- Every created or modified file must keep a short doc header in the file's native comment style with `Purpose`, `Caller`, `Dependencies`, `Main Functions`, and `Side Effects`.
- When files or folders are created, deleted, moved, or renamed, or when the main flow, key file inventory, file layout, folder layout, or ownership map changes, refresh the scoped global `SYSTEM_MAP.md` in the same session so the next prompt starts from current truth.
- Default `SYSTEM_MAP.md` content to English unless the user explicitly requests another language, and mark missing facts as `Not found` instead of guessing.
- Before finalizing DB-heavy changes, explain the efficiency rationale, trade-offs, and waste avoided, and prefer minimum-I-O, minimum-lock, non-N+1 access patterns.

## Testing Requirements

**Default approach:**
- Prefer test-first when practical: start with a failing test, regression test, or executable acceptance check before changing production code.
- For non-trivial delivery and release readiness, run the mandatory ladder in this order and treat it as fail-closed: Smoke testing -> Functional testing -> Integration testing -> UI testing -> Load testing -> Stress testing -> Security testing.
- A required rung may be marked not applicable only when the reason is explicit and evidence-backed for the touched surface; otherwise skipped or blocked means no-go.
- If a true test-first path is not practical, define the validation target first and keep it explicit during implementation.
- Match coverage to the delivery layers involved: backend or business logic, API contracts, frontend behavior, background jobs, and one realistic higher-layer confirmation for critical flows.
- Match the inspection tool to the touched surface: browser automation such as Playwright for web UI, the live desktop runtime with screenshots or equivalent visual evidence for desktop UI, and the most direct runtime-native inspection tool for CLI, service, workflow, or device issues.
- After each meaningful patch batch, rerun the narrowest validation that proves the batch before stacking more changes on top.
- Do not trust the first green rerun after a fix as enough proof by itself; rerun the proving check and re-audit the broader impacted system and adjacent recovery paths before final delivery.
- Keep tests aligned to the module or layer they protect so failures are easy to trace during debugging.
- When a repo-managed review surface exists, run `keel review pre-commit` for staged local proof, `keel review pre-pr` before opening or updating a PR, and `keel review gates check` when a deterministic merge decision is needed.
- Treat `keel review github comment` and `keel review github check` as explicit GitHub-only hosted surfaces. Use them only when the user explicitly wants GitHub output or the active workflow is concretely GitHub-hosted; otherwise stay on the local or host-neutral review surfaces.
- Prefer the native local review surfaces for deterministic gates. The default flow is: implement, run native review, then perform a focused reviewer-quality pass when guideline verification beyond deterministic rules is still needed.
- For simple docs-only changes, prefer the native local proof path unless the user explicitly asks for deeper review or the docs change carries release, security, or workflow risk.
- Reviewer lanes must read the working brief, scoped memory, `SYSTEM_MAP.md`, the changed-surface map, and proving validation evidence before findings or approval.
- During final code review on this Rust-backed repo, run `cargo test --workspace` and wait for it to finish before passing the gate.
- After implementation and repo-wide proof on non-trivial work, run a second reviewer-quality pass before the final answer.
- Use `.claude/review.json` as the tracked rule engine for PR-native automation, use `keel review learn summarize` to inspect repeated accepted feedback, and require `keel review learn apply-promotion` with an explicit approval note before a learned suggestion becomes policy.

**New Features:**
- Unit tests for business logic
- Integration test for happy path
- Edge case coverage

**Bug Fixes:**
- Test that fails before fix
- Test passes after fix
- Regression test for related functionality

**Refactoring:**
- All existing tests must pass
- No test skipping or removal without justification

**Prohibited:**
- Using `.skip()` or `.only()` in committed code
- Commenting out failing tests
- Mocking critical validation logic

## Feature Flags

Your harness CLI has these features enabled:
- `unified_exec`: Unified execution mode
- `js_repl`: JavaScript REPL for complex operations
- `js_repl_tools_only`: Route tools through js_repl
- `memories`: Persistent memory across sessions

Use features when they provide clear value, not by default.
