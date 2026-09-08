# Compiled Planner

The planner turns a request into a versioned, checked delivery contract before
implementation.

## Workflow

```bash
keel plan specify --request "Add an observable behavior with a named verification command."
keel plan research --plan <plan-id>
keel plan tasks --plan <plan-id>
keel plan check --plan <plan-id>
keel plan check --rtm --plan <plan-id>
```

Use `--workspace-root <path>` to target a workspace other than the current
directory. Use `--claude-home <path>` only to select a non-default Keel data
root. `--json` returns a machine-readable result.

The state sequence is:

```text
specified -> researched -> tasked -> valid
```

`plan check` is the implementation gate. A non-zero exit means the delivery
contract is incomplete or internally inconsistent.

## Storage

Each plan is stored at:

```text
<keel-home>/memories/workspaces/<workspace-key>/plans/<plan-id>/
```

The directory contains:

| Artifact | Contract |
| --- | --- |
| `spec.md` | The verbatim request, 22 required specification sections, REQ records, and structured AC records. |
| `research.json` | Classified factual claims and source links used by requirements and criteria. |
| `architecture.md` | The grounded design note and requirement/claim references. |
| `tasks.json` | Implementation tasks linked to requirements, criteria, verification methods, and evidence types. |
| `rtm.json` | The requirement traceability matrix from each REQ to AC, task, verification, and evidence. |
| `status.json` | Current stage, research/task/check status, clarification state, and validation errors. |

Every JSON artifact has `schemaVersion: 1`. Markdown artifacts start with a
strict `schema_version: 1` header. Unsupported or malformed versions fail
validation rather than being rewritten.

`plan specify` publishes a new six-file bundle by directory rename. Later stage
updates use Keel's atomic text writer and publish `status.json` last.

## Specification and ambiguity rules

Functional requirements use `REQ-001`-style IDs. Acceptance criteria use
`AC-001`-style IDs and require:

- requirement references;
- precondition and action;
- expected observable outcome;
- negative or failure outcome;
- verification method and expected evidence type;
- one owner role: `implementer`, `verifier`, `reviewer`, or `human`.

Criteria containing `works`, `correct`, `nice`, `good`, `proper`, `fast`, or
`secure` must also define an observable threshold or result. Material vague
terms cause `specify` to record interpretations and a clarification question.
The resulting `unresolved_material_ambiguity` decision blocks `tasks` and
`check` until the specification records a grounded decision.

## Claim and traceability checks

Every structured factual claim must be classified as `verified`, `assumption`,
or `derived`. Verified claims require source IDs. Claim `usedBy` references must
resolve to an existing REQ or AC.

`plan check` reports all detected defects in one run. It checks:

- every required section and schema header;
- unique REQ and AC IDs;
- every REQ has an AC;
- every AC maps to an existing REQ and has verification/evidence fields;
- research is complete and every claim is classified;
- every REQ and AC maps to an implementation task;
- every RTM row maps REQ to AC, task, verification, and evidence;
- status and artifact identities agree with the requested plan ID.

`plan check --rtm` returns the validated RTM alongside the check result. Plan
IDs are restricted to one safe path segment and cannot escape the workspace
plan lane.
