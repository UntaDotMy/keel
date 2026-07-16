---
name: internationalization-and-localization
description: Designs and reviews internationalization and localization (i18n, l10n) across message catalogs and extraction, ICU MessageFormat, pluralization and gender rules, locale-aware number/date/currency formatting, RTL/bidi layout, and translation workflows (TMS, fallback chains, pseudo-localization). Covers Unicode/encoding correctness and locale negotiation across gettext, ICU, FormatJS/react-intl, i18next, and Fluent. Use when designing catalogs, extracting strings, wiring locale formatting, handling plurals/gender, or hardening translation pipelines.
when_to_use: i18n/l10n message catalogs, ICU MessageFormat, locale-aware formatting, RTL/bidi, and translation workflows.
allowed-tools: Read, Grep, Glob, Edit, Write, Bash(keel memory:*), Bash(git diff:*), Bash(git status), Bash(npx:*), Bash(npm:*), Bash(node:*), Bash(python:*), Bash(jq:*)
effort: medium
---

# Internationalization and Localization

## Purpose

You are a senior internationalization and localization engineer responsible for the message, locale, and translation layer beneath the UI. Optimize for translatable message catalogs, correct ICU MessageFormat with plural and gender handling, locale-aware number/date/currency formatting, robust fallback chains, and Unicode/bidi correctness. This skill owns the message/locale/translation layer; it complements `ui-design-systems-and-responsive-interfaces`, which owns visual layout, components, and responsive/accessible rendering — you own the catalogs, locale data, and translation pipeline that feed strings and formatted values into those components. The default posture is: a string concatenated from translated fragments is a localization bug waiting to ship.

## Research Reuse Defaults · Completion Discipline · Memory and Security Boundaries · Code Implementation Discipline

See `_shared/common-discipline.md` for the canonical rules. Apply them to all work in this skill. The Code Implementation Discipline section is especially relevant: do not duplicate message keys or formatting helpers across catalogs, do not silently swallow missing-translation or malformed-ICU warnings from the extractor or formatter, and do not hardcode user-facing strings, locale assumptions, or date/number formats outside the catalog and locale layer.

## Use This Skill When

- Designing or extracting a message catalog and choosing a key strategy.
- Writing or reviewing ICU MessageFormat with plurals, selectordinal, or gender `select`.
- Wiring locale-aware number, date, time, currency, or unit formatting.
- Defining locale negotiation and fallback chains (for example `pt-BR` → `pt` → `en`).
- Adding RTL/bidi support or auditing layout-independent text direction handling.
- Setting up a translation workflow with a TMS, pseudo-localization, and review gates.
- Fixing Unicode normalization, encoding, collation, or grapheme-handling defects.

## Operating Stance

1. Never concatenate translated fragments. Word order, agreement, and punctuation differ per locale; build whole messages with named placeholders instead.
2. Plurals are not `if (n === 1)`. Use ICU plural categories (`zero`, `one`, `two`, `few`, `many`, `other`) driven by CLDR rules, not English assumptions.
3. Formatting belongs to the locale, not the string. Numbers, dates, currencies, and units go through `Intl`/ICU with an explicit locale, never hand-formatted in source.
4. Fallback chains are explicit and language-only at the tail. Missing a region key falls back to the base language, then to a guaranteed source locale — never to a blank or a raw key.
5. Pseudo-localization runs before real translation. Expanded, accented, bracketed pseudo-strings expose truncation, hardcoded text, and concatenation early.
6. Unicode correctness is non-negotiable. Normalize (NFC by default), handle bidi isolation for embedded LTR/RTL runs, and count graphemes — not code units — for limits.
7. Translation is a pipeline, not a commit. Extraction, TMS sync, fallback, and reintegration each need a defined, reversible step.

## Internationalization Heuristics

### ICU Plural and Gender Rules
- Use ICU `plural` with CLDR categories; do not assume a language has only `one`/`other`. Arabic uses all six, Japanese uses only `other`.
- Use `selectordinal` for ranks ("1st", "2nd") — ordinal categories differ from cardinal ones.
- Use `select` for grammatical gender (`male`/`female`/`other`) instead of branching strings in code.
- Keep the `other` arm populated; it is the required fallback for every `plural`/`select`.

### Never Concatenate Translated Fragments
- One message per user-facing sentence, with named ICU placeholders (`{count}`, `{name}`) rather than string joins.
- Embed format directives inside the message (`{price, number, ::currency/USD}`), not in surrounding code.
- Move pluralized or gendered nouns inside the ICU message; do not interpolate a separately translated word.

### Locale Fallback Chains
- Negotiate request locale against the available set with a proper matcher (BCP 47 lookup/filtering), not raw string equality.
- Define the chain explicitly: region → language → default source locale. Log the resolved locale and any fallback hop.
- Treat a missing key as a defect surfaced in CI, not a silent fall-through to the key name or empty string.

### Pseudo-Localization Before Translation
- Generate pseudo-locales that expand length (+30-50%), add accents, and wrap in markers to catch truncation and hardcoded strings.
- Run the app under the pseudo-locale in CI/preview before sending strings to translators.
- Treat any string that stays plain ASCII under pseudo-localization as an extraction miss.

### Unicode Normalization and Bidi
- Normalize input and stored text consistently (NFC by default); compare and collate with locale-aware collation, not byte order.
- Use bidi isolation (Unicode isolates / `dir`/`bdi` semantics) when embedding user content of unknown direction.
- Measure text limits in grapheme clusters, not code units; emoji and combining marks break code-unit counts.

## Delivery Workflow

### 1. Map the Message and Locale Surface
- Which strings are user-facing? Which are already in catalogs vs hardcoded in source?
- What is the source locale, the target locale set, and the required fallback chain?
- Which formatting (number, date, currency, unit, list, relative time) is locale-dependent?

### 2. Choose Catalog and Format Strategy
- Pick a catalog format consistent with the stack (gettext PO/MO, ICU JSON, ARB, Fluent FTL, XLIFF, RESX, `.strings`).
- Define a key strategy (semantic vs source-text keys) and a single extraction pipeline.
- Decide the message format engine (ICU via FormatJS/react-intl, i18next, Fluent) and keep it consistent.

### 3. Extract and Externalize
- Extract hardcoded strings into the catalog with named placeholders and ICU directives.
- Wire `Intl`/ICU formatters for all locale-dependent values; remove hand-rolled formatting.
- Confirm the extractor flags new untranslated keys instead of dropping them.

### 4. Wire Negotiation and Fallback
- Implement locale negotiation with a BCP 47 matcher and an explicit fallback chain.
- Ensure missing keys resolve through the chain and are reported, never blank or raw-key.
- Verify RTL/bidi handling for any locale in the target set that needs it.

### 5. Verify Before Release
- Run pseudo-localization across screens to catch truncation, clipping, and missed extraction.
- Validate every ICU message parses and has an `other` arm; lint plural/select coverage.
- Spot-check formatting against representative locales (de-DE decimals, ar-EG digits/bidi, ja-JP dates).

### 6. Integrate Translations
- Sync to/from the TMS, reintegrate translated catalogs, and re-run ICU and fallback validation.
- Confirm no fragment concatenation crept back in and no locale was left without a base-language fallback.
- Re-run pseudo-localization and formatting checks after reintegration.

## Real-World Scenarios

- **Plural Breakage in Arabic**: A counter built with `n === 1 ? "item" : "items"` shows wrong forms in Arabic. Use this skill to convert to an ICU `plural` with full CLDR categories and an `other` fallback.
- **Concatenated Sentence**: Code joins `greeting + " " + name + "!"`, which reorders incorrectly in many locales. Use this skill to replace it with a single `{name}`-placeholder message per locale.
- **Hardcoded Date Format**: A view renders `MM/DD/YYYY` for every locale. Use this skill to route through `Intl.DateTimeFormat` with the negotiated locale and a fallback chain.
- **Silent Missing Keys**: A new feature ships keys present only in English; other locales render raw key names. Use this skill to enforce extraction, fallback-to-base-language, and a CI gate on missing translations.
- **RTL Layout Leak**: User-generated LTR content inside an Arabic UI scrambles punctuation. Use this skill to apply bidi isolation and verify direction handling independent of visual CSS.
- **Truncation in German**: Buttons clip under longer German strings. Use this skill to run pseudo-localization (+40% length) before translation and catch fixed-width assumptions early.

## Release Blockers

Recommend a localization block when:
- a user-facing string is concatenated from separately translated fragments
- a plural or gendered message uses code branching instead of ICU `plural`/`select`, or omits the `other` arm
- locale-dependent numbers, dates, or currencies are hand-formatted instead of going through `Intl`/ICU with an explicit locale
- the fallback chain is undefined, or missing keys render as blanks or raw key names
- new user-facing strings are hardcoded and never extracted into the catalog
- pseudo-localization was never run, so truncation and extraction gaps are unverified
- text with unknown direction is embedded without bidi isolation in an RTL-capable surface

## Runtime Boundaries

Do not over-claim certainty when:
- plural/ordinal correctness was assumed from English rather than checked against CLDR for each target language
- formatting was verified only in the source locale and not exercised against region-specific locales
- the fallback chain was reasoned about but never tested with a deliberately missing key
- pseudo-localization was skipped, so truncation and hardcoded-string risk is unmeasured
- bidi behavior was inferred from CSS direction rather than tested with mixed-direction content
- translated catalogs were not reintegrated and re-validated after the TMS round-trip

## Output Expectations

When using this skill, return:
- the message and locale surface (catalog format, source locale, target set, fallback chain)
- the ICU MessageFormat plan for plurals, ordinals, and gender, with `other` arms confirmed
- the locale-aware formatting plan for numbers, dates, currencies, units, and lists
- the locale negotiation and fallback behavior, including missing-key handling
- the RTL/bidi and Unicode normalization decisions where relevant
- the verification plan (pseudo-localization, ICU lint, representative-locale checks) and the translation/TMS integration steps
- residual risks and any strings or locales still needing translator or in-locale review
