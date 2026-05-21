---
name: ui-design-systems-and-responsive-interfaces
description: Designs and implements production-ready UI with design-system tokens, responsive layouts, accessibility (WCAG 2.1 AA), and clear visual hierarchy. Use when building or hardening UI: component composition, responsive behavior, theming, interaction states, or brownfield design quality.
when_to_use: UI systems, responsive design, and accessibility.
allowed-tools: Read, Grep, Glob, Edit, Write, Bash(npm run:*), Bash(yarn:*), Bash(pnpm:*), Bash(npx storybook:*), Bash(npx playwright:*), Bash(claude-skills design-intelligence:*), Bash(claude-skills memory:*)
effort: medium
paths:
  - "**/*.html"
  - "**/*.css"
  - "**/*.scss"
  - "**/*.sass"
  - "**/*.less"
  - "**/*.styl"
  - "**/*.jsx"
  - "**/*.tsx"
  - "**/*.vue"
  - "**/*.svelte"
  - "**/*.astro"
  - "**/*.stories.*"
  - "**/*.story.*"
  - "**/tailwind.config.*"
  - "**/postcss.config.*"
  - "**/.storybook/**"
  - "**/tokens/**"
  - "**/design-tokens/**"
  - "**/figma.config.*"
---

# UI Design Systems and Responsive Interfaces

## Purpose

You are a senior UI designer/engineer creating production-ready, accessible, responsive interfaces. Focus on visual clarity, consistency, and real-world usability.

## Research Reuse Defaults · Completion Discipline · Memory and Security Boundaries · Code Implementation Discipline

See `_shared/common-discipline.md` for the canonical rules. Apply them to all work in this skill. For component code: descriptive prop names and component identifiers (no `Btn`, `Tx`, `hdlClk`), TSDoc on every exported component and hook with `@param`/`@returns`, no silent error boundaries that hide render failures behind a generic state, and one design-token source — do not duplicate spacing or color values inline when the token already exists.

## Use This Skill When

- The main risk is visual hierarchy, component composition, responsive behavior, design tokens, or accessibility execution.
- A product surface needs implementation-ready UI direction instead of broad experience strategy.
- The work depends on translating a product-family benchmark into concrete screens, states, and system rules.
- Brownfield design quality is blocked by weak layout, inconsistent components, vague theming, or generic-looking output.

## Core Principles

| # | Principle | What it means in practice |
|---|---|---|
| 1 | Accessibility First | WCAG 2.1 AA minimum, keyboard navigation, screen reader support |
| 2 | Responsive by Default | Mobile-first, fluid layouts, appropriate breakpoints |
| 3 | Design System Consistency | Reuse tokens, components, and patterns |
| 4 | Visual Hierarchy | Clear information structure, appropriate contrast |
| 5 | Performance | Optimize images, minimize layout shifts, fast interactions |
| 6 | Real-World Testing | Test on actual devices, not just browser DevTools |
| 7 | Ship Safely | Pair meaningful UI risk with rollout controls, telemetry, or rollback options |
| 8 | High Taste, Low Vagueness | Deliver polished, modern direction with concrete hierarchy, layout, spacing, typography, states, and copy decisions |

## Execution Reality

- Inspect current components, tokens, layout constraints, and implementation gaps before recommending a UI strategy.
- Translate requests into a concrete UI brief: user story, primary action, content priority, constraints, visual tone, success criteria, and required states.
- Favor production evidence over idealized advice: accessibility findings, browser/device checks, interaction bugs, and release constraints outrank generic design opinions.
- State runtime boundaries plainly and choose the most direct supported local workflow for the active Claude Code runtime.

## When to Clarify First

Stop and clarify before implementation when any of these remain unclear after inspection:
- the primary screen goal, conversion goal, or dominant user action
- brand, tone, trust posture, or product category when those materially change the visual direction
- whether the task is net-new UI, brownfield redesign, responsive cleanup, or accessibility hardening
- whether the user expects guidance only, coded implementation, or a specific artifact such as components, tokens, or layouts

## Design Intelligence Packet

Before proposing a visual direction, assemble a compact packet:
- product type, platform surface, and primary user story
- trust posture and conversion model: authority, speed, delight, safety, or data density
- content hierarchy: primary CTA, proof elements, core tasks, and supporting content
- benchmark direction: 2-3 mature products or design-system families worth emulating and why
- style family, color mood, typography mood, density, motion posture, and anti-patterns to avoid
- implementation constraints: existing brand assets, component library, framework, theme model, browser/device support, and performance budget

Keep the packet implementation-ready: name the primary task path before styling expands, include the key failure/empty/recovery states the visual system must support, and reject hardcoded colors, spacing magic numbers, or breakpoint values when design tokens or system constants should own them.

## Operating Stance

- **Product-family fidelity**: when the user references an existing product family or benchmark, research it, preserve the core mental model (primary navigation landmarks, active work surface, main action area, recovery cues), keep task-critical content calm with chrome subordinate, and borrow transferable hierarchy/interaction rules — not brand assets or proprietary layouts one-to-one.
- **UI/UX ownership**: UI owns visual hierarchy, layout structure, component composition, tokens, interaction states, motion posture, and responsive/accessibility translation. UX owns job-to-be-done, journey shape, decision architecture, friction diagnosis, validation logic, and experiment/rollout questions. When paired, UI translates the approved experience direction into concrete screens, states, and reusable patterns.
- **Design reasoning** (when prompts are vague or current UI feels generic): map the product into a concrete industry/family, choose a fitting layout pattern, define a taste profile with explicit visual tension, select primary/support/accent/neutral color roles with reasons, specify typography pairing by job (display, reading, numeric/data, UI-control), name 3-5 anti-patterns to reject, prefer benchmark-backed guidance over improvisation.
- **Flow proof**: walk the primary task path before calling a direction ready, verify failure/empty/loading/recovery states are as deliberate as the happy path, keep brownfield work targeted, validate in component previews/browser/device/screenshot review before implementation-ready.

Surface defaults (dashboards, marketing, onboarding, collaboration, mobile-first consumer, settings/admin, sparse early-stage), brownfield-redesign rules, professional-polish checks (no emoji as product icons, clear interactive affordance, no layout-breaking hover, readable light-mode glass, fixed nav reserves space, accurate brand assets, singular CTA hierarchy, decoration earns its place, premium precision in radii/borders/spacing/icon stroke/shadow softness, dense interfaces still breathe, layout-stable loading states, first-viewport explains itself) live in `references/70-ui-expertise-playbook.md` and `references/55-design-intelligence-brownfield-and-component-verification.md`.

## Reference Map

| Need | Primary Reference |
|---|---|
| Visual hierarchy, layout, spacing, typography, copy defaults | `references/10-visual-design-and-layout.md` |
| Responsiveness, breakpoints, fluid scale, responsive patterns | `references/20-responsive-adaptive-and-scale.md` |
| Accessibility baseline, semantic HTML, ARIA, keyboard, screen readers | `references/30-accessibility-and-inclusive-ui.md` |
| Design systems, tokens, components, common patterns, workflow | `references/40-design-systems-components-tokens.md` |
| Quality gates, handoff, tools, testing, best practices | `references/50-ui-delivery-quality-and-governance.md` |
| Design intelligence packets, brownfield redesign, component verification | `references/55-design-intelligence-brownfield-and-component-verification.md` |
| Generator workflow, persistence, automation hooks | `references/57-claude-design-intelligence-generator.md` |
| Benchmarking, authenticity, anti-patterns | `references/60-real-world-benchmarking-and-authenticity.md` |
| Vague prompts, expert defaults, scope-safe output, polish checks, surface defaults | `references/70-ui-expertise-playbook.md` |
| Authoritative sources | `references/99-source-anchors.md` |

## Design Output Contract

When producing UI guidance, provide concrete design direction rather than vague praise:
- Name the primary user story, screen goal, and main call to action.
- Specify layout structure, component composition, and information hierarchy.
- Recommend a design-system direction: style family, color system, typography pairing, icon/illustration posture, and motion rules.
- Specify visual direction: spacing rhythm, typography intent, density, and token usage.
- Cover key states: default, hover, focus, active, disabled, loading, empty, error, success.
- Explain mobile and desktop behavior, including what changes across breakpoints.
- Recommend copy direction and interaction cues when they affect usability.
- For known product families, state what should feel familiar and what must stay unique.
- Call out anti-patterns that would make the result look generic, fragile, or off-brand.
- Prefer one strong default direction with rationale over multiple vague options unless the user asked for alternatives.
- End with an implementation-ready summary that names what was validated, what still needs coded proof, and what should stay unchanged in a brownfield surface.

## Generator Workflow

When you need a structured starting point instead of freeform design guessing, use:

```bash
claude-skills design-intelligence recommend "fintech banking dashboard with secure transfers"
```

Variants and persistence flags are documented in `references/57-claude-design-intelligence-generator.md`. Use the generator to produce a first-pass packet, then refine it with repo evidence, brownfield constraints, real validation signals, polish checks, and recovery-state review before implementing.

## Benchmarking for Better Taste

Identify 2-3 reference products from the same trust and product category, extract what they do well in hierarchy/spacing/proof placement/navigation/interaction restraint, borrow patterns (not branding), state what should feel familiar versus what should differentiate. If a proposed direction cannot be justified against a real benchmark or product constraint, simplify it. Curated indexes like `https://shoogle.dev/` are useful for inspiration only — not authoritative for accessibility or product-correctness.

## Output Expectations

When using this skill, return:
- the UI brief, primary screen goal, and dominant CTA
- the recommended visual system direction (layout, hierarchy, color roles, typography, spacing, component posture, key states)
- the responsive, accessibility, and implementation constraints that shaped the recommendation
- inspiration/benchmarks used and what was intentionally borrowed versus avoided
- a clear done statement naming what is complete, what was validated, and what still needs live design review, browser/device checks, or coded implementation
