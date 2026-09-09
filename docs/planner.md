# Compiled Planner

The planner turns a request into a versioned, checked delivery contract before
implementation.

## Workflow

```bash
keel plan specify --request "Add an observable behavior with a named verification command."
keel plan research --plan <plan-id> \
  --claim "Exact external fact used by the plan" \
  --source-url "https://vendor.example/current-doc" \
  --source-type official-doc \
  --retrieved-at "2026-09-09T00:00:00Z" \
  --support "Brief original paraphrase of the supporting passage" \
  --freshness fresh \
  --used-by REQ-001,AC-001
# Complete the generated architecture.md design note.
keel plan design --plan <plan-id>
keel plan tasks --plan <plan-id>
keel plan check --plan <plan-id>
keel plan check --rtm --plan <plan-id>
```

Use `--workspace-root <path>` to target a workspace other than the current
directory. Use `--claude-home <path>` only to select a non-default Keel data
root. `--json` returns a machine-readable result.

The state sequence is:

```text
specified -> researched -> designed -> tasked -> valid
```

`plan design` validates the architecture before task compilation. `plan check`
is the implementation gate. A non-zero exit from either command means the
delivery contract is incomplete or internally inconsistent.

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
| `architecture.md` | The grounded, bounded design note with component-to-REQ/AC mappings, alternatives, failure and fallback semantics, measurement, rollback, and research references. |
| `tasks.json` | Backward-compatible task summary plus the indexed `ticketFiles` list and each task's `ticketFile` link. |
| `task-<n>.json` | Evidence-bound task ticket with REQ/AC references and scope-derived checklist layers. Written by `plan tasks`. |
| `rtm.json` | The requirement traceability matrix from the submitted request through REQ, AC, task, checklist subtask, and evidence reference. |
| `status.json` | Current stage, research/architecture/task/check status, clarification state, and validation errors. |

Every JSON artifact has `schemaVersion: 1`. Markdown artifacts start with a
strict `schema_version: 1` header. Unsupported or malformed versions fail
validation rather than being rewritten.

`plan specify` publishes the initial six-file bundle by directory rename.
`plan tasks` adds one task ticket per requirement. Later stage updates use
Keel's atomic text writer and publish `status.json` last.

## Task tickets and evidence

Each `task-<n>.json` retains the aggregate task's `id`, title, requirement and
acceptance references, then organizes subtasks under these base layers:
`design`, `implementation`, `build`, `tests`, `lint_warnings`, `security`,
`performance_tokens`, `docs`, `ui`, `ux_accessibility`, and
`release_rollback`. Keel derives mandatory, non-empty layers from the request
and the architecture component list:

| Detected scope | Required layers |
| --- | --- |
| Source code | `implementation`, `build`, `tests`, `lint_warnings`, `docs` |
| Public behavior or API | `compatibility`, `docs`, `security`, `release_rollback` |
| UI behavior | `ui`, `ux_accessibility`, `screenshots`, `test_fixtures` |
| Dependency | `security`, `license_audit`, `reproducibility`, `bloat_analysis` |
| Token or performance | `benchmark`, `performance_tokens`, `fixed_context_runtime` |
| Data or memory | `migration`, `privacy`, `integrity`, `rollback` |
| Host integration | `host_adapter_contracts`, `install_provision_parity` |

Every generated subtask carries its own ID, description, REQ/AC links, status,
expected evidence type, evidence reference, reason, owner role, and verification
timestamp. Derived layers and generated subtasks cannot be removed. A subtask
may be `skipped`, `not_applicable`, or `needs_human` only with a non-empty
reason visible to the reviewer.

A `done` subtask requires an RFC3339 verification timestamp and an
`evidence_ref` to a bounded regular JSON file inside the plan directory. The
reference records the file's content fingerprint. Evidence supports command,
named-test, lint diagnostic, source-hash, RawStore, screenshot, and benchmark
records. Keel resolves the referenced source or RawStore record where that
evidence type requires it and rejects traversal, symlinks, failed results,
malformed payloads, changed content, or a ticket/evidence identity mismatch.

Running `plan tasks` again preserves existing tickets, validates them, and
regenerates only the aggregate summary and RTM. `plan check --rtm` validates the
complete chain:

```text
User request -> Requirement -> Acceptance criterion -> Task -> Checklist subtask -> Evidence
```

It rejects deleted derived layers, missing or duplicate ticket links, dangling
RTM links, criteria without evidence-producing subtasks, unjustified statuses,
and `done` subtasks without resolvable evidence.

## Architecture gate

`plan research` writes a pending `architecture.md` scaffold. Complete that
canonical file, set `Status: complete`, then run `plan design`. The command
reads at most 65,536 bytes and requires all 14 numbered sections. The design
must map every component to existing REQ and AC IDs and record:

- a bounded input and one policy owner;
- alternatives, tradeoffs, the chosen option, infrastructure reuse, and fit;
- risks, compatibility, and host impact;
- explicit failure status, fallback, and operator/user/reviewer/status visibility;
- security/privacy and token impact with a measurement plan;
- verification, rollback, and complete REQ, AC, and classified claim references.

Host-impacting designs must state that all 11 adapter contracts remain
preserved. Running `plan research` again rewrites the design scaffold and resets
architecture and task readiness to `pending`; `plan design` and `plan tasks`
then fail closed until the note is completed again.

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
- architecture design is complete, bounded, fully mapped, and internally consistent;
- every REQ and AC maps to an implementation task;
- every task ticket retains its required layers, subtask fields, and REQ/AC links;
- every RTM row and trace maps request to REQ, AC, task, checklist subtask, and evidence;
- every completed subtask has valid, unchanged, machine-resolvable evidence;
- status and artifact identities agree with the requested plan ID.

`plan check --rtm` returns the validated RTM alongside the check result. Plan
IDs are restricted to one safe path segment and cannot escape the workspace
plan lane.

## Research provenance and freshness

`plan research` accepts one host-supplied external source record or, when no
source flags are supplied, checks the research cache before using the local
workspace index. Supplying any source flag makes `--claim`, `--source-url`,
`--source-type`, `--retrieved-at`, `--support`, `--freshness`, and `--used-by`
mandatory. `--publication-date` is also mandatory in effect for historical
papers and standards because validation rejects either without it.

Each source in `research.json` records `sourceId`, `sourceUrl`, `sourceType`,
`publicationDate`, `retrievedAt`, `support`, `freshness`, and `usedBy`. Each
claim records its classification, source IDs, and REQ/AC consumers. Supported
source types are `official-doc`, `repository`, `issue`, `paper`, `standard`,
`local-code`, and the planner-owned `user-request` record.

The freshness classes have distinct meanings:

- `fresh`: current external product, API, tool, or version evidence. Retrieval
  is limited to 90 days by default.
- `historical`: a paper or standard with a publication date. It can be older
  than the product-evidence window.
- `local-only`: a local-code or submitted-request source. This is the visible
  fallback for a host without web research.

Project configuration can narrow or widen the product-evidence window in the
root `keel.filters.toml` file:

```toml
[research]
max_age_days = 30
```

The value must be greater than zero; a missing section uses 90 days and zero or
negative values fail closed. When several complete cache records match, current
official documentation has priority, followed by repository and issue evidence;
papers and standards remain background evidence. The newest retrieval wins
within the same source type. Unsupported secondary-source types do not pass the
research validator.

A matching cache record past its TTL returns `re-search required` rather than
falling back silently to local code. Even a cache record whose TTL is still
active must pass the project retrieval-age policy before tasks can be compiled.

For a branch that modifies established source, pre-PR and closeout review also
require the researched and designed plan:

```bash
keel review pre-pr --base-ref origin/main --plan <plan-id>
keel review closeout --base-ref origin/main --plan <plan-id> --brief-id <brief-id>
```

The blocking `research_traceability` gate reports missing, stale, malformed, or
unlinked source and claim records. The separate `architecture_design` gate
reports incomplete mappings, decisions, fallbacks, measurement, or rollback.
A separate `task_evidence` gate validates ticket schemas, required layers, RTM
links, completion evidence, and evidence content fingerprints.
A branch that adds only new source or changes non-source files remains
greenfield for all three gates.
