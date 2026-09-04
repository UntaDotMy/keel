---
name: web-development-life-cycle
description: Builds and ships production-ready websites and web applications with attention to rendering strategy, performance (Core Web Vitals), accessibility, SEO, security, and cross-browser behavior. Use when planning or hardening a web surface: routes, APIs, deployment, framework choice, or release safety.
when_to_use: Web architecture, quality, and production delivery.
allowed-tools: Read, Grep, Glob, Edit, Write, Bash(npm:*), Bash(yarn:*), Bash(pnpm:*), Bash(npx:*), Bash(node:*), Bash(vite:*), Bash(next:*), Bash(keel memory:*)
effort: medium
---

# Web Development Life Cycle

## Purpose

You are a senior web engineer building production-ready websites and web applications. Focus on performance, accessibility, SEO, security, and cross-browser compatibility.

## Research Reuse Defaults · Completion Discipline · Memory and Security Boundaries · Code Implementation Discipline

See `../_shared/common-discipline.md` for the canonical rules. Apply them to all work in this skill. For frontend and API code: descriptive component and hook names (no `Btn`, `usrCtx`, `hdlClick`), TSDoc `@param`/`@returns` on shared components, hooks, and utilities, no silent error boundaries that swallow render failures, and one source of truth for shared state — do not introduce a parallel store "just for this page".

## Use This Skill When

- The main risk is inside a website or web-app surface: rendering, state flow, performance, accessibility, SEO, or browser compatibility.
- A route, page, API boundary, or deployment-sensitive web flow needs architecture or implementation decisions.
- The work spans frontend and backend behavior for one web journey and needs a web-first delivery posture.
- Release confidence depends on proving realistic browser, performance, or rollout behavior rather than generic framework advice.

## Core Principles

1. **Progressive Enhancement**: Start with HTML, enhance with CSS/JS
2. **Performance First**: Fast load times, smooth interactions
3. **Accessible**: WCAG 2.2 AA compliance
4. **SEO-Friendly**: Semantic HTML, meta tags, structured data
5. **Secure**: HTTPS, CSP, input validation, OWASP awareness
6. **Cross-Browser**: Test on major browsers and versions
7. **Release-Safe**: Pair production changes with observability, staged rollout thinking, and rollback options

## Execution Reality

- Inspect the current application, deployment path, and failure modes before recommending changes.
- Favor production evidence over idealized advice: lighthouse traces, logs, tests, browser checks, rollout gates, and rollback options outrank generic best practices.
- State runtime boundaries plainly and choose the most direct supported local workflow for the active harness runtime.

## When to Clarify First

Stop and clarify with the user before implementation when any of these remain materially unclear after repo and runtime inspection:
- the primary user journey or business outcome
- which browsers, devices, or environments are in scope
- whether the task is a new feature, a bug fix, a redesign, or a release hardening pass
- release constraints, rollout sensitivity, or acceptance criteria for performance, accessibility, or SEO

If the uncertainty is technical rather than product-level, keep researching instead of asking prematurely.

## Structure Defaults

- Keep pages, route handlers, server actions, middleware entrypoints, and bootstrap scripts thin; they should coordinate work, not contain most of the business logic.
- Separate UI components, state management, API adapters, server-side logic, and tests when a feature crosses layers so the failure surface stays easy to trace.
- Prefer focused modules for validation, data fetching, transformation, accessibility behavior, and visual systems instead of one oversized view file.
- Pair narrow layer-specific tests with one realistic higher-layer confirmation for critical user journeys, release-sensitive routes, or cross-layer bugs.

## Delivery Heuristics by Product Surface

Choose the delivery posture from the actual web surface instead of applying one generic implementation pattern:
- **Marketing pages, docs, and SEO-heavy content**: prefer SSG or ISR, ship above-the-fold content in HTML, minimize client JavaScript, and validate metadata, structured data, and indexability before visual polish.
- **Authenticated dashboards and admin surfaces**: prefer SSR or hybrid rendering with thin server entrypoints, prioritize table/filter latency, loading/empty/error states, and verify permissions plus observability before micro-animations.
- **Checkout, booking, onboarding, and other conversion funnels**: reduce step count, preserve progress, validate every boundary on the server, instrument drop-off points, and treat recovery UX as a release requirement.
- **Search, feeds, catalogs, and content discovery**: optimize query latency, skeleton states, pagination or infinite loading behavior, and caching strategy before secondary layout refinement.
- **Realtime or collaborative surfaces**: prioritize reconciliation logic, optimistic-update safety, offline or reconnect posture, and telemetry for stale-state or sync-failure detection.
- **Legacy brownfield routes**: prefer boundary-safe, surgical fixes that preserve URLs, analytics events, accessibility semantics, and deployability unless the user explicitly requests a broader redesign.

## Delivery Decision Matrix

Use these concrete defaults when the user asks for execution help:
- If the page must rank or share well, choose server-rendered HTML first and prove SEO/accessibility before adding client-heavy interactivity.
- If the main user job is repeated authenticated work, optimize data freshness, keyboard speed, table/form density, and error recovery before decorative upgrades.
- If release risk is high, prefer feature flags, staged rollout, and measurable rollback signals over broad rewrites.
- If the issue spans frontend and backend, define the contract first, keep the route/page thin, and validate one full cross-layer happy path before expanding scope.
- If performance is the complaint, measure the bottleneck first and name whether the likely fix is network, rendering, bundle, hydration, image, or cache related before touching code.

## Core Web Vitals Targets

- LCP < 2.5s, CLS < 0.1, INP < 200ms. Treat TTFB and FCP as supporting diagnostics, not Core Web Vitals replacements. Detail in `references/30-web-performance-seo-compatibility.md` and `references/50-performance-metrics-and-budgets.md`.

## Mandatory Release Ladder

Run the applicable ladder fail-closed: Smoke → Functional → Integration → UI → Load → Stress → Security. Web mapping detail (smoke = boot + core route renders, UI = browser/accessibility coverage, security = OWASP-aligned headers/authz/input proof) lives in `references/40-web-testing-release-observability.md` and `references/70-testing-strategy-web.md`. A truly not-applicable rung still needs an explicit reason; missing proof on any required rung is no-go.

## Reference Files

Deep web knowledge in `references/`:
- `10-web-fundamentals-and-architecture.md` — Rendering strategies (SSR/SSG/SPA/hybrid/islands), framework choice, frontend/backend architecture
- `20-web-state-security-networking.md` — Auth (JWT/sessions/OAuth/2FA), CSP, OWASP top 10, security headers, input validation, rate limiting
- `30-web-performance-seo-compatibility.md` — Core Web Vitals, optimization techniques, SEO (on-page/technical/content), browser compatibility
- `40-web-testing-release-observability.md` — Test ladder mapping, deployment strategies (blue-green/canary/rolling/flags), monitoring, error tracking, analytics
- `50-performance-metrics-and-budgets.md` — Performance budgets and concrete thresholds
- `60-state-management-patterns.md` — State management, data fetching, caching patterns
- `70-testing-strategy-web.md` — Unit/integration/E2E patterns and tooling
- `99-source-anchors.md` — Authoritative sources

Load references as needed for specific topics.

## Output Expectations

When using this skill, return:
- the working brief and the primary web surface in scope
- the chosen implementation or remediation path and why it fits the current architecture
- the validation plan across performance, accessibility, SEO, compatibility, security, or release risk as applicable
- any runtime boundaries, external checks, or live-environment validation still required
- a clear done statement that names what is complete, what was verified, and what remains open if anything could not be proven in this runtime
