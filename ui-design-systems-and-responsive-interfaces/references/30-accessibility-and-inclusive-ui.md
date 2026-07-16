# Accessibility and Inclusive UI

## Accessibility Baseline

Target **WCAG 2.2 Level AA** (W3C Recommendation, 12 December 2024: https://www.w3.org/TR/WCAG22/). Content that conforms to 2.2 also conforms to 2.0 and 2.1. Prefer 2.2 as the product baseline so 2.2-only criteria are not left untested.

- Define target conformance level and audit criteria early.
- Ensure full keyboard support for all interactive paths.
- Ensure semantic structure and labeling for assistive technologies.
- Ensure focus order is logical and visible.
- Ensure focus is not fully obscured by sticky headers/overlays (2.4.11 Focus Not Obscured (Minimum), AA).
- Ensure minimum target size for pointer targets (2.5.8 Target Size (Minimum), AA: 24x24 CSS px with exceptions).
- Support accessible authentication without cognitive function tests where possible (3.3.8 Accessible Authentication (Minimum), AA).
- Prefer non-drag alternatives for drag-only operations (2.5.7 Dragging Movements, AA).
- Keep help mechanisms in a consistent relative location when provided (3.2.6 Consistent Help, A).
- Avoid re-asking information the user already provided in the same process when possible (3.3.7 Redundant Entry, A).

## Inclusive Interaction

- Support multiple input modes (touch, keyboard, pointer, voice where relevant).
- Keep target sizes large enough for reliable interaction.
- Avoid time-sensitive interactions without user control.
- Provide clear error prevention, detection, and recovery cues.
- Respect user preferences such as reduced motion and contrast-related needs.

## Content and Language

- Use plain language and predictable terminology.
- Support localization, text expansion, and right-to-left rendering when required.
- Ensure icon-only controls include clear accessible names.
- Keep form guidance actionable with specific error messages and recovery paths.

## Accessibility QA

- Combine automated checks with manual audits.
- Include screen reader checks for critical flows.
- Include zoom/reflow checks and color-contrast validation.
- Track accessibility issues in backlog with severity and user impact.
- Validate keyboard interaction patterns against common WAI-ARIA practices where relevant.
- Include dark/light mode checks for text legibility and control visibility.
- Include button/CTA state checks (focus/disabled/loading/error) in both modes.

## Semantic HTML

```html
<header>, <nav>, <main>, <article>, <section>, <aside>, <footer>
<button> for actions, <a> for navigation
<h1>-<h6> for headings (logical hierarchy)
<label> for form inputs
```

## ARIA (Use Sparingly)

- Use semantic HTML first
- Add ARIA when HTML semantics insufficient
- Common: `aria-label`, `aria-describedby`, `aria-live`, `role`

## Keyboard Navigation

- Tab order follows visual order
- All interactive elements keyboard accessible
- Escape closes modals/dropdowns
- Enter/Space activates buttons
- Arrow keys for custom controls

## Screen Reader Testing

- Test with actual screen readers (NVDA, JAWS, VoiceOver)
- Ensure logical reading order
- Verify all content accessible
- Check form labels and error messages
