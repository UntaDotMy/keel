[![Validate](https://github.com/UntaDotMy/keel/actions/workflows/validate.yml/badge.svg)](https://github.com/UntaDotMy/keel/actions/workflows/validate.yml)

# keel

**Discipline as code for the harness.**

Keel is a single Rust binary that adds discipline-as-code to AI coding harnesses — iron-law hooks, skills, MCP, multi-host install, and fail-closed anvil/review gates — so agents research first and cannot merge on vibes alone. It does not route models.

No Node. No Python. No daemon. One binary.

## Why

Coding agents are great at producing patches and bad at proving they understood the repo. Keel wires the harness so the boring discipline is mechanical:

- Restate the iron law on every prompt (read first, understand before building, invoke the right skills, find the root cause).
- Refresh a structural map across compactions.
- Write a working brief before non-trivial work.
- Run fail-closed anvil + review gates before closeout.

You keep your host (Claude Code, Cursor, Codex, and friends). Keel adds the discipline layer.

## Install (one paste)

Works on macOS, Linux (incl. WSL), and Windows — x86_64 and arm64. No Rust toolchain required.

```bash
# macOS / Linux / WSL
curl -fsSL https://raw.githubusercontent.com/UntaDotMy/keel/main/install.sh | bash
```

```powershell
# Windows PowerShell
irm https://raw.githubusercontent.com/UntaDotMy/keel/main/install.ps1 | iex
```

```bat
:: Windows CMD
curl -fsSL https://raw.githubusercontent.com/UntaDotMy/keel/main/install.cmd -o install.cmd && install.cmd && del install.cmd
```

If raw.githubusercontent.com returns 403, grab install.sh (or the platform archive) from https://github.com/UntaDotMy/keel/releases/latest instead.

Then open a **new** terminal and check: `keel status` (or `~/.keel/keel status` / Windows explicit path).

PATH quirks, shared MCP daemon, and host adapters → docs/ (start with docs/compatibility-matrix.md).

## 60-second path

```bash
keel anvil compile --goal "…" --bar "…" --files "…"
keel anvil run
keel review pre-pr
```

Anvil never commits or pushes.

## What you get

| Surface | What it does |
| --- | --- |
| Iron-law hooks | Session start, per-prompt restatement, reviewer nudge, map refresh across compaction. |
| Skills + install | Multi-host install into Claude/Cursor/Codex and related harnesses; managed skills and slash wrappers. |
| MCP | `keel mcp serve` (stdio) and `keel mcp serve-http` (loopback). |
| Anvil + review | Fail-closed delivery loop and local review gates so non-trivial work does not self-merge on vibes. |
| Brownfield flow check | For edits to established source, review can require owner-path evidence (`keel flow …`) before gates pass. Soft claim only — not a bakeoff-proven uniqueness line. |

## What keel is not

| Not this | Why |
| --- | --- |
| A model router | Host/workspace pick models. Keel does not route models at runtime. |
| A coding agent / IDE | Disciplines the harness you already use. |
| A fleet / ops plane | Single-machine discipline layer, not multi-tenant agent ops. |
| AGENTS.md alone | Markdown helps; keel adds hooks, gates, install, MCP. |
| A crates.io crate named `keel` | Not this cycle — distribute via GitHub Releases. |

No fake testimonials. Homepage blank until a real URL exists.

## Demo

No polished public demo clip in this draft. Real walkthrough docs: docs/demo-pr-fix-flow.md, docs/demo-branch-closeout-flow.md, docs/first-success-path.md.

## Docs and license

docs/why-keel.md · docs/compatibility-matrix.md · docs/model-tiers.md · MIT LICENSE
