# Source Anchors

Authoritative sources for the rules in this skill. Use these when a reviewer or contributor needs to verify a rule against its origin instead of recalling it from memory.

## Skill Source

- `preserve-existing-flow/SKILL.md` — the canonical rules. Every reference file in this directory restates a section of SKILL.md in the depth a reader needs to apply it.

When SKILL.md and a reference disagree, SKILL.md wins. Open a follow-up to reconcile the reference.

## Section-to-Reference Map

| SKILL.md section | Reference file |
|---|---|
| Required First Step | 10-brownfield-investigation-checklist.md |
| Brownfield Investigation Checklist | 10-brownfield-investigation-checklist.md |
| Ownership Rules | 20-ownership-rules-and-no-overwrite.md |
| No-Overwrite Rules | 20-ownership-rules-and-no-overwrite.md |
| Reference Comparison Rule | 20-ownership-rules-and-no-overwrite.md |
| Safe Extension Pattern | 30-safe-extension-and-implementation-gate.md |
| Implementation Gate | 30-safe-extension-and-implementation-gate.md |
| Reporting Format Before Code | 30-safe-extension-and-implementation-gate.md |
| Code Change Rules | 30-safe-extension-and-implementation-gate.md |
| Review Fail Conditions | 40-review-fail-conditions.md |
| Final Answer Requirements | 40-review-fail-conditions.md |

## Related Skills

These skills hand off to or from preserve-existing-flow. Load them when the work crosses their domain.

- `reviewer/SKILL.md` — the production-readiness verdict that consumes the working brief, changed-surface map, and validation evidence produced here.
- `software-development-life-cycle/SKILL.md` — cross-domain planning when the brownfield change spans multiple specialist surfaces.
- `qa-and-automation-engineer/SKILL.md` — test design and the release ladder when validation needs to be deepened beyond the narrow batch validation in this skill.
- `git-expert/SKILL.md` — branching, history, and PR workflow for changes that need conflict resolution or staged rollout.
- `security-and-compliance-auditor/SKILL.md` — when ownership migration touches auth, secrets, or compliance-sensitive surfaces.

## Tooling Anchors

Commands referenced by this skill, with the command surface that owns them:

- `keel flow start` — open the per-workspace flow-check artifact for a brownfield change.
- `keel flow check` — gate before edits; refuses when owner-path or validation evidence is thin.
- `keel flow finish` — gate before final review; ensures the artifact is closed against actual evidence.
- `keel memory scope resolve --create-missing --refresh-system-map` — refresh the workspace memory before reading existing flows.

These commands are owned by the `keel` Rust CLI in `rust/crates/keel/`. If the command surface changes, this anchor needs to update with it.

## External References

The skill borrows vocabulary from these prior-art sources. They are pointers, not requirements — useful when explaining a rule to a contributor unfamiliar with the brownfield discipline.

- Michael Feathers, "Working Effectively with Legacy Code" — the seam concept and characterization tests inform the Safe Extension Pattern.
- Hyrum's Law — every observable behavior of a system becomes someone's contract; the No-Overwrite Rules are written assuming this.
- The "boring technology" rule — prefer the existing owner over a new one until the existing owner is proven wrong; this shapes the Implementation Gate questions.

These references are not load-bearing for the rules; they are background reading for a contributor who wants to understand the reasoning behind the discipline.
