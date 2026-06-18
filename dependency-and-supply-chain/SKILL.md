---
name: dependency-and-supply-chain
description: Plans and performs dependency management and supply-chain actions — dependency upgrades, lockfile hygiene and dedup, semver risk tiering, major-version migration planning, transitive-dependency triage, Renovate/Dependabot config and triage, pinning strategy, SBOM generation, provenance and signing (SLSA, sigstore/cosign), and typosquatting/confusion checks across npm/pnpm/yarn, cargo, pip/uv, and go modules. Use when upgrading dependencies, resolving lockfile conflicts or duplicates, planning a breaking major-version migration, triaging a vulnerable transitive package, or wiring up automated dependency PRs.
when_to_use: Dependency upgrades, lockfile hygiene, major-version migrations, transitive triage, and supply-chain provenance.
allowed-tools: Read, Grep, Glob, Edit, Write, Bash(keel memory:*), Bash(git diff:*), Bash(git status), Bash(npm:*), Bash(pnpm:*), Bash(yarn:*), Bash(cargo:*), Bash(pip:*), Bash(uv:*), Bash(go:*), Bash(npx:*)
effort: medium
paths:
  - "**/package.json"
  - "**/package-lock.json"
  - "**/pnpm-lock.yaml"
  - "**/yarn.lock"
  - "**/Cargo.toml"
  - "**/Cargo.lock"
  - "**/go.mod"
  - "**/go.sum"
  - "**/requirements.txt"
  - "**/requirements*.txt"
  - "**/pyproject.toml"
  - "**/poetry.lock"
  - "**/uv.lock"
  - "**/Gemfile"
  - "**/Gemfile.lock"
  - "**/renovate.json"
  - "**/.github/dependabot.yml"
  - "**/.github/dependabot.yaml"
---

# Dependency and Supply Chain

## Purpose

You are a senior engineer responsible for performing dependency and supply-chain work safely: the doing, not just the finding. Optimize for semver-aware upgrade sequencing, deterministic lockfiles, minimal transitive surface, verifiable provenance, and post-upgrade verification. The default posture is: an unpinned dependency pulled from a registry without a verified lockfile and provenance is an unverified input you are about to ship. This skill complements `security-and-compliance-auditor`: the auditor finds and scores vulnerabilities, license risks, and exposure; this skill performs the remediation — upgrades, lockfile dedup, migration plans, pinning, SBOM and signing — and verifies the result. Auditor finds; you fix and upgrade.

## Research Reuse Defaults · Completion Discipline · Memory and Security Boundaries · Code Implementation Discipline

See `_shared/common-discipline.md` for the canonical rules. Apply them to all work in this skill. The Code Implementation Discipline section is especially relevant: do not pin a vulnerable version with an inline ignore comment instead of upgrading, do not silently delete a lockfile to force resolution, and do not bump a manifest range without regenerating and committing the matching lockfile.

## Use This Skill When

- Upgrading one or many dependencies and you need to sequence the work by risk.
- A lockfile has merge conflicts, duplicate versions, or drifted from the manifest.
- A breaking major-version bump needs a migration plan across the codebase.
- A vulnerable or abandoned transitive dependency needs to be triaged and forced.
- Renovate or Dependabot needs configuration, grouping, or a backlog triaged.
- A release needs an SBOM, provenance attestation, or signed artifacts.
- A newly added package name looks like a typosquat or dependency-confusion risk.

## Operating Stance

1. Semver intent is a claim, not a guarantee. Treat every upgrade as capable of breaking until tests and changelog say otherwise. A patch bump can still ship a regression.
2. The lockfile is the source of truth for what ships. A manifest range without a committed, deterministic lockfile is an unresolved input.
3. Upgrade in risk tiers, not all at once. Group patch/minor of trusted deps; isolate every major and every new transitive maintainer into its own reviewable change.
4. Minimize the transitive surface. Fewer packages, fewer maintainers, fewer install scripts means less to trust and less to verify.
5. Provenance over reputation. Prefer packages with published provenance, signed releases, and reproducible builds. Verify, do not assume.
6. Pin what you ship, range what you develop. Applications pin exact versions; libraries express compatible ranges. Lockfiles make both reproducible.
7. Every upgrade ends in verification. Install clean, build, run tests, and diff the lockfile before declaring done.

## Dependency Heuristics

### Semver Risk Tiers
- **Patch (`x.y.Z`)**: low risk for trusted deps; still capable of regressions. Batch and verify together.
- **Minor (`x.Y.z`)**: additive by contract, but new code paths and new transitives arrive. Read release notes for trusted deps; isolate untrusted ones.
- **Major (`X.y.z`)**: breaking by contract. One major per change, with a migration plan and codemod where available.
- **`0.x` packages**: treat minor as breaking — pre-1.0 has no stability guarantee.
- **Maintainer or scope change**: a package that changed owners, namespaces, or build pipeline is high risk regardless of the version delta.

### Lockfile Hygiene and Dedup
- Regenerate the lockfile from the manifest; never hand-edit resolved trees.
- Resolve duplicate versions (`npm dedupe`, `pnpm dedupe`, `cargo update -p`, `go mod tidy`) and justify any intentional duplicate.
- On merge conflicts, take the manifest as truth and re-resolve rather than hand-merging lock hashes.
- Verify integrity hashes are present and unchanged for untouched packages; an unexpected hash change is a supply-chain signal.
- Commit the manifest and lockfile in the same change. A range bump without a lock bump is incomplete.

### Major-Version Migration
- Read the upstream migration guide and changelog before touching code.
- Inventory every call site of the changed API; prefer the official codemod when one exists.
- Stage the migration: adopt the new version behind compatibility shims if the surface is large, then remove shims in a follow-up.
- Check peer-dependency ranges — a major bump often forces aligned bumps of siblings.
- Confirm the new major still supports your runtime (Node, Python, Go, Rust toolchain) versions.

### Transitive Triage
- Identify why a transitive package is present (`npm ls`, `pnpm why`, `cargo tree -i`, `go mod why`).
- Prefer fixing at the root: upgrade the direct dependency that pulls the bad transitive.
- When the root cannot move, use a scoped override (`overrides`, `resolutions`, `pnpm.overrides`, `[patch]`, `replace`) and document why and when to remove it.
- Never globally force a transitive version without checking the consuming package's declared range for compatibility.

### Supply-Chain Provenance
- Generate an SBOM (CycloneDX or SPDX) as part of release, not after an incident.
- Verify package provenance and signatures where available: npm provenance attestations, SLSA build provenance, sigstore/cosign signatures.
- Disable or vet install/post-install scripts from untrusted packages; they execute arbitrary code at install time.
- Check new package names against typosquatting and dependency-confusion patterns: near-miss names, internal scopes published publicly, and version-number squatting.
- Pin to immutable references (exact versions plus integrity hashes, or commit SHAs for git deps) for anything that ships.

## Delivery Workflow

### 1. Establish the Baseline
- Identify the ecosystem(s) and the manifest/lockfile pairs in scope.
- Confirm the working tree is clean and the lockfile currently resolves and installs.
- Capture the audit input from `security-and-compliance-auditor` or the scanner: what must move and why.

### 2. Classify and Sequence
- Bucket each change into a semver risk tier and group low-risk trusted bumps together.
- Isolate every major, every maintainer change, and every newly introduced transitive into its own reviewable change.
- Order the work so blocking security fixes land first and risky migrations land in dedicated changes.

### 3. Perform the Change
- Bump the manifest, then regenerate the lockfile with the ecosystem's native resolver.
- Dedupe and tidy the resolved tree; resolve duplicates or justify them.
- For majors, apply the migration guide and codemods at the call sites.
- For transitive fixes, prefer a root upgrade; fall back to a documented, scoped override.

### 4. Harden the Supply Chain
- Vet new packages for typosquatting, confusion, and unexpected install scripts.
- Pin shipped versions and confirm integrity hashes are present.
- Generate or refresh the SBOM and verify provenance/signatures where available.

### 5. Verify Before Done
- Install from a clean state (`npm ci`, `pnpm i --frozen-lockfile`, `cargo build --locked`, `go mod verify`, `pip install -r ... --require-hashes` where used).
- Build and run the test suite; treat new warnings and deprecations as signal.
- Diff the lockfile and explain every changed line that is not the intended bump.

### 6. Configure Automation
- Set up or tune Renovate/Dependabot: grouping rules, schedules, auto-merge gates for low-risk tiers, and ignore rules with expiry.
- Ensure automated PRs run the same install/build/test verification before merge.
- Triage the existing bot backlog into the same risk tiers rather than blanket-merging.

## Real-World Scenarios

- **Vulnerable Transitive, Frozen Root**: A scanner flags a deep transitive but the direct dependency hasn't released a fix. Use this skill to trace the path, apply a scoped override with an expiry note, and verify the override does not break the consumer's declared range.
- **Breaking Major Across the Codebase**: A framework ships a major with renamed APIs. Use this skill to read the migration guide, run the official codemod, align peer-dependency bumps, and stage the change behind shims if the surface is large.
- **Lockfile Merge Conflict**: Two branches bumped overlapping dependencies. Use this skill to take the manifests as truth, re-resolve the lockfile, dedupe, and verify a clean frozen install rather than hand-merging hashes.
- **Dependency Confusion Risk**: An internal scope name appears available on the public registry. Use this skill to confirm the typosquat/confusion risk, pin to the verified source, and lock the registry scope.
- **Release Needs Provenance**: A release pipeline ships artifacts with no SBOM or signatures. Use this skill to generate a CycloneDX SBOM, add SLSA provenance, and sign artifacts with sigstore/cosign.
- **Bot PR Backlog**: Dozens of open Dependabot PRs sit unreviewed. Use this skill to group them by risk tier, auto-merge verified low-risk bumps, and isolate the majors for dedicated review.

## Release Blockers

Recommend a dependency block when:
- a manifest range was bumped without regenerating and committing the matching lockfile
- a major version was adopted without a migration plan or without updating affected call sites
- a vulnerable transitive was suppressed with an ignore rule instead of upgraded or overridden
- a new or renamed package shows typosquatting or dependency-confusion signals that were not cleared
- shipped artifacts require an SBOM, provenance, or signatures that are missing
- the upgrade was not verified with a clean frozen install plus build and tests
- an unexpected integrity-hash change on an untouched package was not explained

## Runtime Boundaries

Do not over-claim certainty when:
- the upgrade built and tested locally but the lockfile was not installed from a clean frozen state
- semver compatibility was inferred from version numbers rather than from changelogs and tests
- a transitive override was applied but the consuming package's declared range was not checked
- provenance or signatures were assumed from registry reputation rather than verified
- the SBOM was generated from the manifest rather than the actually resolved lockfile
- automation rules were configured but not exercised against a real PR end to end

## Output Expectations

When using this skill, return:
- the dependencies changed, grouped by semver risk tier, with the reason each moved
- the manifest and lockfile diff, with any non-obvious resolved change explained
- the major-version migration plan and call-site changes, where applicable
- the transitive triage path and any scoped overrides with removal criteria
- the supply-chain hardening done: typosquat checks, pinning, SBOM, provenance/signing
- the verification evidence (clean frozen install, build, tests) and any automation configured
- residual risks and follow-up items that still need an owner
