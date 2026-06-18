# the harness Design-Intelligence Generator

Use this reference when you want a structured UI recommendation packet generated from local data rather than relying on freeform invention alone.

## Native Command

`keel design-intelligence recommend`

## Backing Catalog

`data/design_intelligence_catalog.json`

## Recommended Use

1. Start with the raw product or feature query.
2. Generate a first-pass design-intelligence packet.
3. Add `--stack` and `--component-library` when implementation constraints should shape the recommendation.
4. Compare the result with the actual repository, brand constraints, and brownfield realities.
5. Persist the system only if team alignment benefits from a shared artifact.
6. Validate the resulting components and states in isolated tooling when available.
7. Review the emitted professional polish checks and recovery checks before shipping.

## Example Commands

```bash
keel design-intelligence recommend "saas dashboard for incident response"
keel design-intelligence recommend "portfolio redesign for a creative agency" --format json
keel design-intelligence recommend "AI workspace for research copilots" --stack nextjs --component-library shadcn --format json
keel design-intelligence recommend "direct messaging mobile app with unread states and voice notes" --stack flutter --format json
keel design-intelligence recommend "checkout recovery improvements" --persist --project-name "Storefront Revamp" --page "Checkout Flow"
```

## Output Shape Highlights

The generator emits a full design-intelligence packet, not just style picks:

- **product archetype** scored from the request (with confidence and selection signals)
- **style family, color mood, and typography mood** chosen from the archetype's recommended set and biased by `--stack`
- **concrete color palette** (`color_palette`) — named palette matching the color mood with real hex values for primary, secondary, accent, background, surface, and text colors plus a WCAG contrast note; biases toward dark palettes when the request mentions dark mode
- **concrete font pairing** (`font_pairing`) — named heading/body/mono faces, source (e.g. Google Fonts), type scale, weights, and a pairing rationale matched to the typography mood
- **recommended charts** (`recommended_charts`) — top data-visualization types when the request implies dashboards, analytics, or reporting (empty when there is no data-viz intent)
- **applicable UX guidelines** (`ux_guidelines`) — matched rules tagged by `[severity/category]`, scoped to the chosen archetype plus universal rules, ranked with critical rules first
- stack-aware adaptation guidance when `--stack` is provided
- professional polish checks for affordance, CTA clarity, contrast, and layout stability
- recovery checks for validation, interruption, and high-trust flow handling
- product-family-aware recommendations for familiar surfaces such as direct messaging
- selection signals and an explicit clarification flag when the prompt is too vague to classify safely

## Backing Corpus

The catalog is the design knowledge base behind every recommendation. It currently holds 869 cross-referenced entries — larger than every comparable array of the largest external design-intelligence corpus (UI/UX Pro Max v2.5.0, file-verified: 84 styles / 161 palettes / 73 pairings / 99 UX rules / 161 reasoning rules / 25 charts):

- 170 product archetypes
- 90 style families
- 45 color moods
- 30 typography moods
- 15 stack profiles
- 230 named color palettes (light and dark, with hex + WCAG contrast notes)
- 140 font pairings (Google Fonts and system stacks)
- 37 chart types
- 112 UX guidelines

Cross-references are validated: every archetype's recommended style/color/typography moods resolve, every stack's preferred entries resolve, and every palette and pairing points at a real color or typography mood. The `data/` directory ships with the skill on install, so the native command works against the installed copy without network access.

## Persistence Safety

The native command is designed to avoid the type of crash seen in external tools that assume optional names are always present:

- project and page names are normalized to safe slugs
- missing names fall back to the query or `design-system`
- parent directories are created automatically before writing
- `MASTER.md` is the source of truth and page files are overrides
