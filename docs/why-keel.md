# Why keel

## Purpose

This page provides an evidence-based comparison surface for this repository. It shows relative strengths, areas where it's catching up, and the operator problems it solves.

Comparisons reference native harness primitives, a runtime-shell comparator, and a workflow-teaching comparator. Last re-checked on 2026-05-12.

---

## When to Choose keel

Choose keel when you need:
- A specific structured path: open a working brief with `keel memory working-brief write`, compile an explicit candidate set with `keel anvil compile --files <csv>`, carry it through `keel anvil run`, prove it with `keel review pre-pr`, and close it with `keel memory completion-gate check`
- Deterministic local review gates before treating a PR as real
- Explicit hosted-check discipline after the PR opens
- Durable artifacts for briefs, requirements, lanes, and closure
- Native install/update/status/verify/uninstall flows
- A branch-closeout path that refuses to call work done while proof is still incomplete

## Versus Raw the harness

Raw the harness provides powerful primitives. keel adds a stricter operator layer on top.

**keel strengths:**
- Operator-first delivery loop through Anvil plus tracked working briefs and completion gates
- Tracked completion-gate artifacts instead of chat-only closure claims
- Native pre-commit and pre-PR review surfaces
- Hosted-check fix-loop guidance with branch-closeout discipline
- Repo-managed install/status surfaces instead of ad hoc setup

**Choose raw the harness when:** you want the thinnest possible layer without extra workflow or proof posture.

## Versus Runtime-Shell Comparator

The comparator emphasizes a richer runtime experience around the harness including setup, role keywords, and team runtime helpers.

**keel advantages:** Stronger deterministic closeout posture around review, verification, and hosted green checks; more explicit branch-to-proof path under one workflow surface; native repo-managed commands; clearer distinction between local proof, hosted proof, and final closure.

**Comparator advantages:** More polished runtime-first onboarding, stronger keyword-driven daily interaction, more visible team runtime presentation.

## Versus Workflow-Teaching Comparator

The comparator emphasizes skill-driven workflow phases, explicit design gates, TDD discipline, and composable process skills.

**keel advantages:** Native manager packaging with a release-managed install path, native review gates and host-neutral review artifact generation, integrated hosted-check watching and PR-fix workflow, workflow worktree/branch-finish paths productized under one CLI.

**Comparator advantages:** Very clear phase-based software workflow education, stronger skill-first planning and implementation framing, simpler install story.

## Where It's Still Catching Up

- Benchmark evidence is shipped across tracked scenarios but peer repos still need more entries before universal claims are justified
- Demo flows cover greenfield delivery, stateful fixes, hosted rescue, branch closeout, and more — but only for tracked scenarios published so far
- The operator surface is still more rigorous than friendly

## Summary

Use keel when you want a harness-native workflow layer that is harder to fake as finished. Use another layer when you want a friendlier runtime shell, a more guided skill-first teaching surface, or a lighter install footprint.
