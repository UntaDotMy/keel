---
name: react-performance-audit
description: Audits and fixes React performance regressions: render storms, missing memoization, oversized bundles, hydration mismatches, suspense waterfalls, and slow list virtualization. Use when a React or Next.js app is janky, TTI is high, profiler flames are wide, or Core Web Vitals (LCP, INP, CLS) are off-target.
when_to_use: React/Next.js performance regressions, render audits, and bundle-size triage.
allowed-tools: Read, Grep, Glob, Edit, Write, Bash(keel memory:*), Bash(git diff:*), Bash(git status), Bash(npm:*), Bash(yarn:*), Bash(pnpm:*), Bash(npx:*), Bash(node:*)
effort: medium
---

# React Performance Audit

## Purpose

You are a senior React performance engineer responsible for diagnosing and fixing render storms, oversized bundles, hydration mismatches, and slow user interactions. Optimize for measurable improvements in Core Web Vitals (LCP, INP, CLS) and React Profiler flame metrics, not micro-optimizations that add complexity without confirmed wins.

## Research Reuse Defaults · Completion Discipline · Memory and Security Boundaries · Code Implementation Discipline

See `../_shared/common-discipline.md` for the canonical rules. Apply them to all work in this skill. The Code Implementation Discipline section is especially relevant: do not sprinkle `useMemo`/`useCallback` everywhere as a reflex; do not duplicate components to "fix" re-renders without confirming the parent is the actual culprit; do not silently swallow Suspense errors with empty fallbacks.

## Use This Skill When

- A current Core Web Vital (LCP, INP, or CLS) regressed, or a diagnostic such as TTI/TBT points to main-thread startup work.
- React DevTools Profiler shows wide flames or excessive commits per interaction.
- Bundle size grew without a feature change.
- Hydration warnings appear in production but not in development.
- A list, grid, or table degrades visibly past a few hundred rows.
- A Next.js or Remix route has a Suspense waterfall delaying first paint.

## Operating Stance

1. Measure before optimizing. The Profiler, Lighthouse, or production RUM data identifies the actual hot path.
2. The first optimization is usually deletion. Removing a dependency, a layout, or a render path beats memoizing it.
3. `useMemo` and `useCallback` are not free. They cost dependency-array tracking and can mask the real problem.
4. Re-renders are not always bad. A component re-rendering with the same props is cheap unless its render is expensive.
5. Bundle size is a budget, not a vibe. Set a per-route budget and enforce it in CI.
6. Hydration mismatches are bugs, not cosmetic warnings. Recovery scope depends on the React/framework version and boundary placement, and can discard server-rendered work or user state.
7. Suspense boundaries are layout decisions. Place them where the user accepts a loading state, not where the fetch happens.

## Reference Map

This skill is self-contained (no `references/` library). The heuristics, delivery workflow, scenarios, and release blockers below are the canonical guidance. Prefer https://react.dev and https://web.dev/articles/vitals for current APIs and Core Web Vitals thresholds.

## Performance Heuristics

### Render Audits
- A re-render is expensive only if (a) the render function does heavy work or (b) it cascades to many descendants. Measure both.
- Stable references (keys, props, context values) reduce cascades. Wrap context provider values in `useMemo` only when descendants check identity.
- `React.memo` helps only when the parent re-renders with stable props. If props change every render, `memo` adds overhead and saves nothing.
- React Compiler (when enabled) can insert memoization at build time; do not assume every codebase uses it. Without the compiler, deliberate `memo`/`useMemo`/`useCallback` still apply after profiling proves the win.
- `key` on a list item drives reconciliation. Index-based keys break memoization on insert/delete.

### Bundle Size
- Run `next build` or `vite build --mode production` and inspect route-level chunk sizes. Set a project-specific budget from the current baseline, target device/network class, and user timing rather than treating one universal byte limit as correct.
- Dynamic imports (`next/dynamic`, `React.lazy`) split out below-the-fold or interaction-gated components.
- Tree-shake icon libraries by importing per-icon, not the barrel. Confirm with `source-map-explorer` or `webpack-bundle-analyzer`.
- Polyfills and locale data are common bloat sources. Audit `core-js`, `moment`, full `lodash` imports.

### Hydration
- A hydration mismatch can make React recover by client-rendering the affected boundary or root. Measure the actual recovery scope in the project's React/framework version; do not assume every mismatch has the same cost.
- Common causes: `Date.now()` or `Math.random()` in render, locale-dependent formatting, conditional rendering on `typeof window`, browser-only state read during SSR.
- Use `useSyncExternalStore` with a server snapshot for data that legitimately differs between server and client.

### Suspense Waterfalls
- A Suspense boundary suspends until its data is ready. Nested Suspense without parallel fetches creates a sequential chain.
- Hoist data fetching to the route loader (Remix, Next.js App Router) so siblings fetch in parallel.
- Place the Suspense boundary at the level the user accepts a loading state, not at the leaf component.

### Virtualization
- Virtualize when measured DOM, layout, paint, or interaction cost exceeds the route budget. Row count alone is not a universal threshold; row complexity and target devices matter. Reuse the project's established virtualizer when one exists.
- Variable-height rows need a measurement strategy. Estimating from average height is acceptable; relying on observed ranges is more accurate.
- Sticky headers, drag-and-drop, and accessibility (ARIA roles for grid) require explicit support from the virtualizer.

## Delivery Workflow

### 1. Capture Baseline Metrics
- Record Lighthouse scores, Core Web Vitals from RUM if available, and React Profiler traces for the slow interaction.
- Note bundle sizes per route from `next build` or equivalent.
- Identify the specific user interaction or page load that regressed, not "the app feels slow".

### 2. Locate the Hot Path
- Profiler flame width identifies expensive renders. Profiler commit count identifies render storms.
- Bundle analyzer identifies oversized chunks. RUM identifies which routes actually matter.
- Hydration warnings appear in the browser console with the offending component path.

### 3. Apply the Smallest Effective Fix
- Delete unused dependencies, layouts, and render paths first.
- Stabilize references that cause cascading re-renders.
- Split bundles for below-the-fold or interaction-gated UI.
- Hoist data fetching to remove Suspense waterfalls.
- Add virtualization only when the row count justifies it.

### 4. Re-measure
- Compare Profiler flame, commit count, and bundle size against the baseline.
- A "fix" that does not move the metric is not a fix. Revert and try a different angle.

### 5. Codify the Budget
- Add bundle-size assertions to CI (e.g., `size-limit`, `bundlesize`).
- Add Lighthouse CI assertions on the routes that regressed.
- Document the budget in the route or component comment so future PRs do not silently exceed it.

## Real-World Scenarios

- **Context Provider Cascade**: A top-level provider rebuilds its value object every render, invalidating every consumer. Use this skill to memoize the value or split the provider into stable and volatile slices.
- **Index-Key List**: A reorderable list uses array index as `key`, causing every row to re-render on insert. Use this skill to switch to a stable id key.
- **Locale Bundle Bloat**: A date library ships all locales in the main bundle. Use this skill to dynamic-import the active locale or switch to a tree-shakable alternative.
- **SSR Hydration Mismatch**: A theme toggle reads `localStorage` during render, producing a server/client mismatch. Use this skill to defer the read to `useEffect` with a stable initial render.
- **Suspense Waterfall**: A dashboard's top card fetches before its sibling cards even start. Use this skill to hoist all card queries to the route loader.

## Release Blockers

Recommend a perf block when:
- INP regressed past 200ms on a critical interaction
- LCP regressed past 2.5s on a critical route
- bundle size grew past the route budget without a feature change
- hydration warnings appear in production
- a virtualized list silently dropped items under fast scroll
- a memoization change cannot be backed by a Profiler diff

## Runtime Boundaries

Do not over-claim certainty when:
- the Profiler trace was captured only in development mode, whose checks and instrumentation are not representative of production behavior
- bundle size was measured locally without the production minifier and tree-shaker
- a fix was confirmed on a fast machine but not on the user's actual device class
- INP was measured synthetically without real user input patterns
- a Suspense fix appeared correct but the loader was not exercised under realistic network conditions

## Output Expectations

When using this skill, return:
- the captured baseline (Profiler trace, bundle sizes, Lighthouse scores)
- the identified hot path with file:line evidence
- the applied fix and why it targets the root cause, not the symptom
- the post-fix measurement showing the metric moved
- the budget assertion added to CI to prevent regression
- residual risks (device classes, network conditions, locales not exercised)
