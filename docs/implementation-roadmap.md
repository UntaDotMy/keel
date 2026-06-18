<!--
Purpose: Expand the active todo roadmap into a concise implementation direction for contributors.
Caller: Contributors and AI agents translating todo.md into concrete delivery lanes.
Dependencies: todo.md, native memory scope, managed prompt surfaces, and Rust tests.
Main Functions: Summarize the objective, baseline, delivery lanes, and validation targets for the current initiative.
Side Effects: Guides current implementation sequencing for the repository.
-->
# Implementation Roadmap From `todo.md`

## Objective

Turn the active roadmap in [todo.md](../todo.md) into a delivery plan that makes global startup context deterministic: scoped memory first, scoped `SYSTEM_MAP.md` second, then targeted repository analysis without dirtying the user workspace.

## Current baseline

The repo already has the foundations that the next roadmap should extend, not replace:

- native memory scope resolution already exposes project-scoped global reference lanes under Claude Code home
- deterministic review and branch-closeout surfaces already exist through `review pre-commit`, `review pre-pr`, `review gates check`, and `git-workflow preflight`
- managed root guidance, skill routing docs, and README surfaces already exist and can be strengthened instead of recreated
- Rust tests and docs validation guard the prompt surface, help surface, and roadmap consistency

## Delivery principles

- Keep the work agent-first: startup context should survive across sessions without polluting user repositories.
- Prefer native project-scoped global artifacts over ad-hoc workspace files whenever the native memory surface can own the job.
- Keep the navigation rules generic across stacks and repository layouts instead of hardcoding this repo's topology into the prompt surface.
- Pair prompt-surface changes with the narrowest Rust or docs validation that proves the new startup behavior really shipped.

## Lane 1: Native Scope Contract

Goal:

- Expose a canonical project-scoped global `SYSTEM_MAP.md` target through the native memory scope.

Primary targets:

- `rust/crates/keel/src/utility.rs`
- `rust/crates/keel/src/runtime.rs`
- `rust/crates/keel/src/help_operator.txt`
- Rust command tests under `rust/crates/keel/src/commands.rs`

Acceptance criteria:

- `memory scope resolve` reports the global `SYSTEM_MAP.md` path under the workspace reference lane
- the path stays outside the user workspace
- help and contract tests make that behavior visible

## Lane 2: Managed Prompt Wiring

Goal:

- Make the installed guidance read scoped memory and `SYSTEM_MAP.md` before broad repository exploration.

Primary targets:

- `AGENTS.md`
- `00-skill-routing-and-escalation.md`
- `rust/crates/keel/src/help_operator.txt`
- `docs/runtime-guardrails-and-memory-protocols.md`

Acceptance criteria:

- startup guidance says to resolve scope first, then read or refresh the global `SYSTEM_MAP.md`
- the map rules stay general across repository shapes and stacks
- documentation rules require synchronized doc headers and map refreshes

## Lane 3: Roadmap And Docs Reset

Goal:

- Replace the previous benchmark-centered roadmap with the new startup-context initiative without breaking published benchmark docs.

Primary targets:

- `todo.md`
- `README.md`
- `docs/implementation-roadmap.md`
- focused Rust tests under `rust/crates/`

Acceptance criteria:

- `todo.md` reflects the new initiative only
- README explains the global project system map clearly
- benchmark and release docs no longer depend on the old roadmap text to stay valid

## Validation plan

- focused Rust tests for memory scope, CLI help, manager behavior, and doc-facing command contracts
- repo-wide Rust test pass after the focused batch is green
- completion-gate recheck before handoff so the new initiative is evidenced rather than implied
