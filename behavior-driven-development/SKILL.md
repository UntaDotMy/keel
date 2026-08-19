---
name: behavior-driven-development
description: Behavior-Driven Development (BDD) technique. Specify and verify software through shared examples of behavior — business-readable scenarios (Gherkin Given/When/Then), outside-in delivery, and living documentation — not through test-after or developer-only unit names. Use when aligning product/dev/QA on acceptance behavior, writing or automating scenarios, driving outside-in implementation from a failing scenario, or turning user stories into executable specs. Complements the working brief (requirement capture), test-driven-development (unit RED-GREEN-REFACTOR), and domain-driven-design (ubiquitous language).
when_to_use: Acceptance criteria as executable scenarios; Cucumber/SpecFlow/Behave/Playwright BDD; outside-in feature delivery; three-amigos collaboration; living documentation; bridging product language and automated checks; ATDD-style acceptance tests first; any "what should the system do" conversation that must stay testable.
allowed-tools: Read, Grep, Glob, Edit, Write, Bash(keel memory:*), Bash(git diff:*), Bash(git status), Bash(npm:*), Bash(yarn:*), Bash(pnpm:*), Bash(npx:*), Bash(cargo:*), Bash(pytest:*), Bash(dotnet:*)
effort: medium
---

# Behavior-Driven Development

## Purpose

You are an expert in Behavior-Driven Development (Dan North / Fowler / Farley lineage). Lead work so **observable behavior agreed in business language** is specified first, automated where valuable, and used to drive implementation outside-in.

Dan North's definition (widely cited): BDD is a **second-generation, outside-in, pull-based, multi-stakeholder, multi-scale, high-automation agile methodology**. It is **not** "Cucumber for unit tests." Gherkin is a communication tool; collaboration is the point.

BDD in practice:

1. **Discovery** — examples that pin intent (often three amigos: product, dev, QA).
2. **Formulation** — scenarios in a shared language (usually Gherkin Given/When/Then).
3. **Automation** — failing acceptance check → implement → green (outside-in).

The working brief captures the agreed behavior as the anti-drift **spec**. This skill owns **how you develop and verify against those behaviors** day to day. `test-driven-development` owns the **unit/module** RED-GREEN-REFACTOR loop inside the red acceptance scenario.

## Research Reuse Defaults · Completion Discipline · Memory and Security Boundaries · Code Implementation Discipline

See `_shared/common-discipline.md` for the canonical rules. Do not invent scenarios the user never required; do not automate every scenario if a manual check is enough for a one-off.

## Core ideas

| Idea | Practice |
|---|---|
| **Behavior over implementation** | Scenarios describe *what* the system does for someone, not *how* classes are wired |
| **Ubiquitous examples** | Scenario language matches domain/product language (pairs with DDD ubiquitous language) |
| **Outside-in** | Start from a failing acceptance scenario; implement inward (API → domain → storage) until green |
| **Living documentation** | Scenarios that stay green are the documented contract of the product |
| **Right-size automation** | Automate stable, high-value paths; keep exploratory and one-off checks manual |

## The BDD loop

### 1. Discover

- Restate the capability as behavior: "When X, the system should Y so that Z."
- Collect concrete examples (happy path, edge, failure). Prefer tables of examples over abstract rules alone.
- If requirements are not yet pinned, write a working brief first and confirm with the user.

### 2. Formulate (Gherkin)

```gherkin
Feature: <capability in product language>

  Scenario: <specific example name>
    Given <precondition / context>
    When <action or event>
    Then <observable outcome>
```

Rules for good scenarios:

- **One behavior per scenario.** No "and also updates twelve other things" unless that *is* the behavior under test.
- **Observable outcomes only.** UI text, API status/body, message published, row state — not "the private method was called."
- **No UI choreography in domain scenarios** unless the story is about UI. Prefer domain/API phrasing for core rules.
- **Declarative Given/When/Then**, not scripts of clicks unless testing the UI itself.
- **Example tables** (`Scenario Outline` + `Examples`) for combinatorial data.

Validate story format when scenarios live in user stories:

```
keel memory working-brief write --request "..." --acceptance-criteria "..."
```

### 3. Automate outside-in

1. Wire the scenario to an automated check at the **outer** seam that still proves the behavior (API test, browser test, message consumer test — pick the narrowest that stakeholders trust).
2. Run it. Confirm **RED** for the right reason (missing behavior, not a broken harness).
3. Implement the minimum path to green — often with unit-level TDD *inside* that path (`test-driven-development`).
4. Refactor under green. Keep scenarios stable; refactor code and step definitions carefully.

### 4. Keep docs alive

- Delete or rewrite scenarios that no longer match product intent — stale green scenarios are worse than none.
- Tag slow/flaky suites; do not let flaky acceptance tests train the team to ignore red.

## Layering of checks

| Layer | Owner skill | Role in BDD |
|---|---|---|
| Working brief + Gherkin acceptance | working brief | Agreed intent |
| Automated acceptance / E2E scenario | this skill + `qa-and-automation-engineer` | Proves behavior from outside |
| Unit / module TDD | `test-driven-development` | Designs internals under the red scenario |
| Domain model language | `domain-driven-design` | Names and invariants match scenario language |
| Release ladder | `qa-and-automation-engineer` | Smoke → … → Security; BDD scenarios sit mainly in Functional/Integration/UI |

## Three amigos (lightweight)

Before a non-trivial story is implemented, align briefly on:

1. **Product** — is this the valuable behavior?
2. **Dev** — is it buildable as stated; what is ambiguous?
3. **QA** — what examples prove it; what edges are missing?

Capture the agreed examples as scenarios. If the user is solo, simulate the three perspectives explicitly in the brief rather than skipping discovery.

## When to use BDD vs not

**Use BDD** when: multiple people must share intent; behavior is non-obvious; regressions in acceptance paths are costly; product language must stay attached to automation.

**Do not force full BDD ceremony** for: pure refactors with no behavior change; one-line config; spikes you will throw away. Still state the expected behavior once if a regression test is warranted.

**Do not write Gherkin for every unit.** Unit tests stay in code form (TDD). Gherkin is for **shared** behavior, not private methods.

## Anti-patterns to refuse

- **Test-after Gherkin:** implementing first, then reverse-engineering scenarios that only echo the code.
- **Imperative click novels:** 40-step UI scripts that break on every CSS change — rewrite as intent-level steps.
- **Developer-only jargon in scenarios** (`Given the OrderRepository returns…`) — that is not shared language.
- **One giant scenario for a whole epic** — split examples.
- **Duplicating unit tests as scenarios** — wrong layer; costs more, documents less.
- **Ignoring RED** — automating scenarios that never failed first proves nothing about the check.
- **Silent scope:** adding scenarios for features nobody asked for (pairs with request fidelity).

## Pairing with other keel skills

| Need | Skill |
|---|---|
| Capture and confirm stories | working brief |
| Unit RED-GREEN-REFACTOR | `test-driven-development` |
| Domain model / ubiquitous language | `domain-driven-design` |
| UI built component-first | `component-driven-development` |
| Coverage strategy & release ladder | `qa-and-automation-engineer` |
| Delivery loop over many pieces | `running-anvil` |

## Validation

Before claiming BDD work done:

1. Scenarios use product/domain language and observable outcomes.
2. At least the critical paths have automated checks that were seen **RED then GREEN**.
3. Implementation did not invent behaviors absent from agreed scenarios/stories.
4. Flaky or obsolete scenarios were fixed or removed, not muted.
5. User-facing stories remain reconcilable (`reviewer` Stage 1 against the brief).

## Authoritative sources (prefer over training-data recall)

- Dan North, [Introducing BDD](https://dannorth.net/blog/introducing-bdd/) and [What’s in a Story?](https://dannorth.net/whats-in-a-story/)
- [Gherkin reference](https://cucumber.io/docs/gherkin/reference/) (Cucumber)
- Dave Farley on BDD vs TDD (outside-in automation vs unit design) — prefer current talks/articles over stale summaries
- Tool docs for the project's runner (Cucumber, SpecFlow, behave, Playwright BDD) — web-search current APIs before writing glue code
