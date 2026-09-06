# Model tiers (provider-aware guidance)

**Date-stamped:** 2026-09-06  
**Honesty bar:** Keel does **not** route models at runtime. Anvil lock placeholders stay `frontier` / `cheap` / `mid`. Host CLIs and agent profiles choose concrete IDs. This doc is guidance for humans and skills — not a live router.

## Anvil lock placeholders (unchanged)

| Placeholder | Role |
| --- | --- |
| `frontier` | Strongest critic / planner class for the host |
| `mid` | Default implementer / stamp class |
| `cheap` | Light implementer / explorer / loop class |

Lock `models.compile|cast|stamp|loop` may hold these placeholders or host-cli defaults. Env overrides (`ANVIL_COMPILE_MODEL`, etc.) remain host configuration.

## Current provider pins (docs + existing host config)

Replace **Claude 3.x** recommendations. Prefer IDs that already appear in host config when present; otherwise use the research-current IDs below. Do not invent new pins in host TOML/JSON that the host does not already ship.

| Tier | Anthropic (Claude Code) | OpenAI (Codex) | Google (Antigravity / Gemini) | Z.ai |
| --- | --- | --- | --- | --- |
| **frontier** (critics / planners / architecture) | `claude-fable-5-1`, `claude-opus-5` | `gpt-6-Astra` (host pin) | Gemini Pro / Thinking (deep reasoning) | `glm-5.3` |
| **mid** (default implementers / reviewers) | `claude-sonnet-5` | `gpt-5.6-*` family (host uses `gpt-5.6-luna`) | `gemini-3.7-flash` / documented peers | `glm-5.3` |
| **cheap** (light tasks / explorers / loops) | `claude-haiku-4-5` / `claude-haiku-4-5-20251001` | `gpt-5.6-luna` (max reasoning in Codex profiles) | `gemini-3.7-flash` | `glm-5.3-flash` |

### Notes

- **Anthropic:** Do not document `claude-3-5-haiku`, `claude-3-7-sonnet`, or `claude-3-opus` as current. Haiku 4.5 retirement is not sooner than 2026-10-15 — plan a cheap-tier successor when that lands.
- **OpenAI:** Existing Codex agent profiles pin `gpt-6-Astra` (planner/reviewer) and `gpt-5.6-luna` (implementer/explorer). Keep those pins; docs say Astra / 5.6.
- **Google:** Flash class for light work; deep reasoning / Pro Thinking for critics. No eternal #1 benchmark claims.
- **Z.ai:** `glm-5.3` / `glm-5.3-flash` for mid/cheap guidance when that host is in use.
- **Secrets:** Never put API keys in this doc, README, CI, or skill bodies.

## Skill / AGENTS pointers

Skill model-policy tables and `AGENTS/references/20-skill-routing.md` must match this file. Anvil does not select providers; state the intended tier in the working brief when useful.

## Out of scope

Runtime model routing inside Keel; crates.io publish; inventing host pins that are not already configured.
