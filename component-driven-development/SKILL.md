---
name: component-driven-development
description: Component-Driven Development (CDD) / Atomic Design technique. Build UI component-first — atoms → molecules/composites → organisms → templates/pages — each proven in isolation (Storybook/Widgetbook) instead of page-first. Use when scaffolding or restructuring a UI, design systems, visual TDD, or decomposing a monolithic screen. Catches "build the page, extract later."
when_to_use: Building or restructuring any UI; Atomic Design hierarchy; component library or design system; Storybook/widgetbook/visual tests; decomposing a monolithic page; component boundaries and composition; reusability and isolated testability.
allowed-tools: Read, Grep, Glob, Edit, Write, Bash(keel memory:*), Bash(git diff:*), Bash(git status), Bash(npm:*), Bash(yarn:*), Bash(pnpm:*), Bash(npx:*), Bash(dart:*), Bash(flutter:*)
effort: medium
---

# Component-Driven Development

## Purpose

You are an expert in Component-Driven Development. Lead the build UI **component-first**: small, isolated, reusable components composed into larger structures, each verifiable on its own before it is wired into a page. The unit of work is the component, not the page. A page is the last thing you assemble — never the first thing you build.

CDD inverts the page-first habit ("build the screen, extract components later"). Page-first produces coupled, hard-to-reuse, hard-to-test components and a pile of duplicated markup. CDD produces a reusable component library where each piece is proven in isolation, then composed.

## Research Reuse Defaults · Completion Discipline · Memory and Security Boundaries · Code Implementation Discipline

See `_shared/common-discipline.md` for the canonical rules. Apply them to all work in this skill. The Expert Posture and "Resolving the `keel` binary" sections apply to every command this skill instructs.

## The CDD workflow (bottom-up) · Atomic Design map

CDD is the *process* (build and prove in isolation, compose upward). **Atomic Design** (Brad Frost) is the common *naming* for the levels. Use both:

| Atomic Design | CDD level | What | Built in isolation? |
|---|---|---|---|
| **Atom** | Atomic | UI primitive with no component deps (button, input, badge, icon wrapper). Styles + behavior + types. | Yes — sandboxed |
| **Molecule** | Composite | 2+ atoms (search field = input + button; form field = label + input + error). Owns local layout only. | Yes |
| **Organism** | Composite (larger) | Distinct section (header, product card grid, checkout summary) composing molecules/atoms. | Yes |
| **Template** | Page shell | Layout slots without real product data — structure only. | Yes when useful |
| **Page** | Page / route | Templates filled with real (or fixture) data; routing + data fetch live here; almost no presentational logic. | Verified last |

Work strictly bottom-up: atoms before molecules, molecules before organisms, organisms before pages. A page that defines an atom inline is a CDD violation — extract it first.

**Visual TDD:** a story/state matrix (default, loading, error, empty, disabled) is the UI equivalent of a failing test. Add the story for a new state **before** implementing that state when practical; watch the sandbox show the gap, then make it pass.

## Component boundaries (the hard rules)

- **A component does one thing.** If its name needs "and", split it (`UserAvatarAndMenu` → `UserAvatar` + `UserMenu`).
- **Props are the only input.** No component reaches into a global store, the DOM, or a parent's internals. Data flows down; events flow up via callbacks.
- **Composition over configuration.** Prefer `children`/slots over a `variant` prop that branches internal layout. `<Card><CardHeader/><CardBody/></Card>` beats `<Card variant="with-header">`.
- **Stable, narrow API.** Expose the minimum props that compose. Every extra prop is a coupling surface. Default the rest.
- **Stateless by default.** Hold state only when the component owns a genuinely local concern (open/closed, hover). Lifted/shared state lives in a parent or store, not the leaf.

## Build in isolation (the sandbox)

Each component is built and verified **before** it appears in any page:

- **React/Vue/Svelte**: Storybook (`.stories.tsx`) — one story per meaningful state (default, loading, error, disabled, empty). The story is the component's first test and its living docs.
- **Flutter**: Widgetbook or a dedicated `dev/` app routing — each widget rendered with fixed mock data.
- **No story without states.** A component with only a "default" story is under-tested. At minimum: default, loading, error, empty, disabled.

If a component cannot be rendered in isolation with mock props, its dependencies are wrong — fix the boundary before proceeding.

## Testing per level

| Level | Test |
|---|---|
| Atomic | Unit + visual snapshot in the sandbox. Props in → rendered output asserted. |
| Composite | Interaction tests (click → callback fires; input → value propagates). Composition contracts. |
| Page | Integration: data fetch mocked, route asserted, the composed tree renders without prop-type holes. |

Never test a component's internals through its parent. If you must mount a parent to test a child, the child's isolation is broken — extract and test it alone.

## When to use CDD vs not

**Use CDD** for: any product UI, a component library, a design-system implementation, multi-platform UIs sharing logic (Flutter mobile+web), any screen with ≥3 reusable parts.

**Don't force CDD** for: a single throwaway landing page, a script-generated report, a one-off internal tool with no reuse intent. CDD's overhead pays off through reuse and isolated testability — if neither applies, a single component is fine.

## Pairing with other keel skills

| Need | Skill |
|---|---|
| Tokens, responsive layout, a11y, design-system governance | `ui-design-systems-and-responsive-interfaces` |
| React render cost / virtualization | `react-performance-audit` |
| Domain model (not UI atoms) | `domain-driven-design` |
| Acceptance behavior of a screen flow | `behavior-driven-development` |
| Unit RED-GREEN-REFACTOR for pure logic | `test-driven-development` |
| Flutter widgets / Widgetbook | `dart-and-flutter-expert` |

## Anti-patterns to refuse

- **Page-first**: building the full screen then "extracting components". Build components, compose the page.
- **Prop-drilled globals**: passing a user/session through five layers. Lift to context/store at the boundary, don't thread through leaves.
- **God component**: one file rendering the whole screen with inline sub-trees. Split along the boundary table above.
- **Story debt**: shipping a component with no isolation story. The story is part of the component, not a follow-up.
- **Coupling via styling**: a child that only looks right inside one parent's CSS. A component must render correctly in the sandbox with zero ancestor styles.

## Validation

- Every new component has an isolation story (≥ default + loading + error).
- The component tree is strictly bottom-up — no page references a component not yet proven in isolation.
- `keel review pre-pr` passes on the component diff.
- The composition is verified: the page renders by composing atoms + composites only, with no inline atomic markup.
