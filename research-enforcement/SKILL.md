---
name: research-enforcement
description: Use when implementation depends on unstable or uncertain external-library, API, or framework facts. Verify them against current authoritative sources, reusing a fresh version-matched research-cache record when it fully answers the question and searching only the missing, stale, or time-sensitive delta.
when_to_use: Implementing changes that touch external libraries, APIs, or frameworks; upgrading dependencies; using a library API that the model may not know the current state of; any situation where "I think this API works like..." is the starting point instead of verified documentation.
allowed-tools: Read, Grep, Glob, Bash(keel memory:*), Bash(keel recall:*)
effort: medium
---

# Research Enforcement

## Purpose

The model's training data has a cutoff. Libraries release new versions, APIs
deprecate endpoints, and frameworks change behavior between releases. Implementing
against what you *remember* rather than what *is* produces code that compiles but
fails at runtime, or uses patterns the framework no longer supports.

This skill enforces a mandatory verification step before implementation that
depends on unstable or uncertain external behavior. Verification may reuse a
fresh, version-matched cache record; otherwise it requires current research.

## The Anti-Pattern

> The model's training data is stale. If you haven't searched for the current
> state of [library/framework], you are assuming.

This sentence is the core rule. Every external dependency implementation must
begin with verification, not assumption.

## Mandatory Flow

Before implementing a change that depends on an external library, API, or framework:

### Step 1 — Identify the external dependency

Ask: does this implementation depend on the behavior of something I do not own?

- A library API (e.g., `reqwest::Client::builder()`, `React.useEffect`, `pg.Pool`).
- A framework convention (e.g., Next.js App Router file conventions, Terraform provider syntax).
- A service API (e.g., Stripe checkout flow, GitHub Actions workflow format).
- A language feature that may have changed between versions (e.g., Rust edition differences, Node.js ESM changes).

If yes, proceed to Step 2. If the change is purely internal logic with no external
dependency behavior, skip this skill.

### Step 2 — Reuse or research the current state

First look for a version-matched research-cache result. If it fully answers the
question and its freshness guidance still fits the risk, reuse it. Otherwise run
one or more of:

- `websearch` — search for the current documentation or release notes for the
  specific library/API/framework and version.
- `context7` — query the library's docs for the specific API surface you will use.
- `recall` — retrieve related records and identify only the missing or stale delta.

Record what you found: the version, the API surface, any deprecation notices, and
the source URL.

### Step 3 — Verify against the research

Compare the implementation plan against the researched docs:

- Does the API still exist in the version we are using?
- Has the signature changed?
- Is the pattern we intend to use still the recommended approach?
- Are there known gotchas or breaking changes in recent versions?

If the research contradicts your initial plan, update the plan before implementing.

### Step 4 — Store the research result

Save the research findings to the `research-cache` memory family so future sessions
do not re-research the same dependency at the same version:

```bash
keel memory research-cache record --question "<dependency> <version>" --answer "<findings>" --source "<url>"
# lookup later:
keel memory research-cache lookup --query "<dependency>"
```

`--question` and `--answer` are required for **record**. (`--query` / `--result` are accepted as aliases for record only; **lookup** uses `--query`.)

### Step 5 — Implement with verified knowledge

Proceed with implementation using the researched, verified API surface. Reference
the source URL in code comments when the API behavior is non-obvious or likely
to change.

## Staleness Rules

- Set freshness from volatility and risk: current service APIs, security guidance,
  prices, and fast-moving frameworks need shorter windows; stable protocols and
  version-pinned behavior can use longer windows.
- Any version mismatch, deprecation signal, contradictory runtime evidence, or
  explicitly current/latest request triggers targeted re-research regardless of age.
- Age alone is a nudge, not proof of invalidity. Stale entries remain leads; verify
  only the claims that can affect the implementation.

## Integration with keel Memory

This skill uses the `research-cache` memory family under `keel memory`:

- `keel memory research-cache record --question "..." --answer "..." [--source ...]` — store findings.
- `keel memory research-cache lookup --query "..."` — retrieve prior research.
- `keel memory research-cache stale [--days N]` — list entries beyond a risk-appropriate age.
- `keel memory research-cache reward --id <id>` — mark an entry still valid.

Research-cache entries live under `<claude-home>/<group>/research-cache/` and are
isolated per memory group.

## When To Skip

Skip the research step only when:

- The change is purely internal logic with no external dependency behavior.
- The dependency is a local workspace crate or file whose source is in the repo.
- You just researched the same dependency in this session (within the same conversation).

Everything else requires at least one research action before implementation.

## Examples

### Library API upgrade

Task: "Upgrade reqwest from 0.11 to 0.12."

Mandatory research: search for reqwest 0.12 changelog or migration guide. Do not
assume the builder API is unchanged. Check for breaking changes in TLS, proxy,
or cookie handling before writing code.

### Framework convention change

Task: "Add a new page to the Next.js app."

Mandatory research: query context7 for Next.js App Router file conventions. Do not
assume the `pages/` directory convention still applies if the project uses `app/`.

### Service API integration

Task: "Implement Stripe webhook verification."

Mandatory research: search for the current Stripe webhook verification docs. Do
not assume the signature verification algorithm or header names have not changed.

## Anti-Patterns

- Skipping research because "I'm pretty sure this API works like..."
- Researching once and never checking if the cache is stale.
- Treating training-data recall as equivalent to a web search for current docs.
- Implementing first, researching when tests fail — research is cheaper than
  debugging a stale-API failure.
- Storing research without a timestamp so staleness cannot be checked.

## Validation

Self-check before implementing: did you run at least one research action for every
external dependency this change touches? If you cannot point to a search, a context7
query, or a fresh recall of a non-stale cache entry, you are assuming. Research first.
