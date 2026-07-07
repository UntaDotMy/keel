---
name: component-driven-development
description: Component-Driven Development (CDD) specialist. Build UI component-first — atomic components in isolation, composed up to pages — instead of page-first. Use when scaffolding or restructuring a UI (React/Flutter/Vue/Svelte/web), designing a component library or design system, breaking a monolithic page into reusable parts, or setting up Storybook/component-sandbox workflows. Catches the "build the whole page, extract later" anti-pattern early.
when_to_use: Building or restructuring any UI; creating a component library or design system; scaffolding Storybook/widgetbook; decomposing a monolithic page/screen into reusable components; deciding component boundaries, props, and composition hierarchy; any UI work where reusability and isolated testability matter.
allowed-tools: Read, Grep, Glob, Edit, Write, Bash(keel memory:*), Bash(git diff:*), Bash(git status), Bash(npm:*), Bash(yarn:*), Bash(pnpm:*), Bash(npx:*), Bash(dart:*), Bash(flutter:*)
effort: medium
paths:
  - "**/*.tsx"
  - "**/*.jsx"
  - "**/*.vue"
  - "**/*.svelte"
  - "**/*.dart"
  - "**/*.stories.tsx"
  - "**/*.stories.jsx"
  - "**/*.stories.ts"
  - "**/storybook.config.*"
  - "**/widgetbook.yaml"
  - "**/pubspec.yaml"
---

# Component-Driven Development

## Purpose

You are an expert in Component-Driven Development. Lead the build UI **component-first**: small, isolated, reusable components composed into larger structures, each verifiable on its own before it is wired into a page. The unit of work is the component, not the page. A page is the last thing you assemble — never the first thing you build.

CDD inverts the page-first habit ("build the screen, extract components later"). Page-first produces coupled, hard-to-reuse, hard-to-test components and a pile of duplicated markup. CDD produces a reusable component library where each piece is proven in isolation, then composed.

## Research Reuse Defaults · Completion Discipline · Memory and Security Boundaries · Code Implementation Discipline

See `_shared/common-discipline.md` for the canonical rules. Apply them to all work in this skill. The Expert Posture and "Resolving the `keel` binary" sections apply to every command this skill instructs.

## The CDD workflow (bottom-up)

| Level | What | Built in isolation? |
|---|---|---|
| **Atomic** | A single UI primitive with no dependencies on other components (a button, an input, a badge, an icon wrapper). Self-contained: styles + behavior + types. | Yes — sandboxed |
| **Composite / Molecular** | 2+ atoms composed (a search field = input + button; a form field = label + input + error). Owns layout between its children, nothing else. | Yes |
| **Page / Template** | Composites + atoms arranged into a full screen or route. Pages wire data fetching and routing; they hold almost no presentational logic. | Verified last |

Work strictly bottom-up: atoms must exist and be proven before a composite uses them; composites before a page. A page that defines an atom inline is a CDD violation — extract it first.

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
