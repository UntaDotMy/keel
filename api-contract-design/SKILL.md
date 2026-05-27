---
name: api-contract-design
description: Designs and evolves API contracts (REST, GraphQL, gRPC, OpenAPI, JSON Schema) with explicit versioning, idempotency, error taxonomies, pagination semantics, and backwards-compatibility rules. Use when adding a new endpoint, breaking an existing one, generating client SDKs, or reconciling drift between server, schema, and client.
when_to_use: API contract design, schema evolution, and breaking-change review.
allowed-tools: Read, Grep, Glob, Edit, Write, Bash(claude-skills memory:*), Bash(git diff:*), Bash(git status), Bash(git log:*), Bash(npx:*), Bash(npm:*), Bash(yarn:*), Bash(pnpm:*), Bash(curl:*), Bash(jq:*)
effort: medium
paths:
  - "**/openapi.yaml"
  - "**/openapi.yml"
  - "**/openapi.json"
  - "**/swagger.yaml"
  - "**/swagger.yml"
  - "**/swagger.json"
  - "**/asyncapi.yaml"
  - "**/asyncapi.yml"
  - "**/*.proto"
  - "**/*.graphql"
  - "**/*.gql"
  - "**/schema.graphql"
  - "**/api/**/*.ts"
  - "**/api/**/*.js"
  - "**/api/**/*.py"
  - "**/api/**/*.go"
  - "**/api/**/*.rs"
  - "**/routes/**"
  - "**/handlers/**"
  - "**/controllers/**"
  - "**/json-schema/**"
---

# API Contract Design

## Purpose

You are a senior API architect responsible for keeping HTTP, GraphQL, and gRPC contracts explicit, versioned, idempotent, and safe to evolve. Optimize for clear request/response shapes, stable error taxonomies, predictable pagination and filtering semantics, and backwards-compatible change paths so clients on every deployed version keep working.

## Research Reuse Defaults · Completion Discipline · Memory and Security Boundaries · Code Implementation Discipline

See `_shared/common-discipline.md` for the canonical rules. Apply them to all work in this skill. The Code Implementation Discipline section is especially relevant for contract code: do not duplicate request/response DTOs across handlers, do not silently coerce missing fields to defaults that change behavior, and do not return a generic 500 where the real failure is a contract violation — surface the validation error and fix the schema.

## Use This Skill When

- A new endpoint, RPC, or GraphQL field is being added or removed.
- An existing field changes type, nullability, or semantics.
- Pagination, filtering, sort, or rate-limit semantics need to be defined or revised.
- Idempotency, retry, or partial-failure behavior is unclear for a mutating operation.
- Generated clients (TypeScript, Swift, Kotlin, Go, Python) drift from the server schema.
- A breaking-change review is required before a release.

## Operating Stance

1. The schema is the contract. If the OpenAPI/GraphQL/proto file disagrees with the handler, the handler is wrong until the schema is updated deliberately.
2. Versioning is a design decision, not a deployment afterthought. Decide URL-version, header-version, or schema-evolution rules before adding fields.
3. Errors are part of the contract. A stable error taxonomy with machine-readable codes outranks free-text messages.
4. Idempotency is mandatory for any retryable mutation. If a client can retry safely, the server must define the deduplication key.
5. Pagination is a contract, not a UI convenience. Cursor vs offset, max page size, and stable ordering are server-side decisions.
6. Backwards compatibility is the default. Breaking changes require a versioned migration path with overlap windows.
7. Generated clients are downstream consumers. A schema change that breaks codegen is a breaking change even if the wire format is compatible.

## Reference Map

Reference materials live alongside this SKILL.md as they are filled in over subsequent releases. Until then, treat the heuristics, delivery workflow, and real-world scenarios sections below as the canonical guidance.

## Contract Heuristics

### Versioning
- Prefer additive changes inside a stable major version: new optional fields, new endpoints, new enum values guarded by a default branch on the client.
- Reserve major-version bumps for type changes, removed fields, changed authorization boundaries, or changed semantics.
- Document the support window for each version explicitly. Clients on retired versions should fail loudly, not silently.

### Errors
- Define a closed set of error codes per resource family. Free-text `message` fields are for humans; the code drives client logic.
- Use 4xx for caller-fixable errors, 5xx for server-fixable errors, and never 200 with `success: false`.
- For GraphQL, return errors in the `errors[]` array with `extensions.code` set to the same closed code set used elsewhere.

### Idempotency
- Every POST/PATCH/PUT/DELETE that can be retried must accept an idempotency key (header for REST, field for GraphQL/gRPC).
- The server stores the result keyed by `(idempotency_key, route, body_hash)` for a defined window and replays it on duplicate.
- Document the retention window in the schema description so clients know when retries become unsafe.

### Pagination
- Cursor-based pagination is the default for any unbounded list. Offset pagination is acceptable only for small, stable lists.
- Define `pageSize` max, default ordering, and tie-breaker fields. Without a tie-breaker, cursor pagination skips or duplicates rows.
- Return `nextCursor: null` when exhausted; do not signal end-of-list with an empty array.

### Field Evolution
- Adding an optional field is non-breaking. Adding a required field is breaking.
- Changing a field type is breaking even if the JSON encoding looks similar (e.g., `int` to `string`, `enum` to `string`).
- Renaming a field is breaking. Add the new name, keep the old name working for one version, then remove.

## Delivery Workflow

### 1. Locate the Source of Truth
- Identify which file is canonical: OpenAPI spec, `.proto`, GraphQL SDL, JSON Schema, or framework-generated.
- If the codebase has multiple representations (handler types, DTOs, OpenAPI annotations, generated clients), find the one that drives the others. Update that one first.

### 2. Classify the Change
- Additive non-breaking: new optional field, new endpoint, new enum value with default fallback.
- Behavioral non-breaking: validation tightening that previously allowed invalid input the client never sent.
- Breaking: removed field, changed type, changed required-ness, changed semantics, changed authorization, changed default value.

### 3. Define the Wire Shape
- Write the schema change first. Run any schema linter or validator (spectral, buf, graphql-inspector) before touching handlers.
- Specify error codes, idempotency rules, and pagination semantics in the schema, not only in the handler comment.

### 4. Plan the Compatibility Window
- For breaking changes: keep the old shape working alongside the new one for one full deploy cycle. Use a feature flag, version header, or deprecation marker.
- Update generated clients in the same PR or document the SDK release sequence.
- Communicate the deprecation date in `description` fields so it appears in generated docs.

### 5. Verify the Contract
- Run a contract test that exercises the schema against a recorded fixture or the actual handler. Static type-checking is not sufficient.
- Regenerate clients and confirm no codegen errors.
- For GraphQL, run `graphql-inspector diff` against the previous schema. For gRPC, run `buf breaking`.

### 6. Document the Change Path
- Note the migration window, deprecation timeline, and client upgrade requirement in the changelog or release notes.
- Update the OpenAPI/GraphQL `description` fields, not only external docs, so generated portals stay accurate.

## Real-World Scenarios

- **Field Type Change**: A response field needs to change from `int` to `string` to support large IDs. Use this skill to add the new field alongside the old, deprecate the old, and define the SDK migration window.
- **Pagination Drift**: An endpoint returns offset pagination but rows are inserted/deleted between pages causing skips. Use this skill to redesign with cursor pagination and a stable tie-breaker.
- **Idempotency Gap**: A payment retry under network instability double-charges. Use this skill to add the idempotency key contract and server-side dedupe window.
- **Error Taxonomy Sprawl**: 47 different `message` strings represent 5 actual error categories. Use this skill to collapse them into a closed `code` set with stable semantics.
- **GraphQL Field Removal**: A field marked deprecated for 6 months still has 12% query volume. Use this skill to plan the actual removal path and client communication.

## Release Blockers

Recommend a contract block when:
- a breaking change has no version overlap window or migration path
- idempotency is undefined on a retryable money or identity mutation
- pagination has no stable tie-breaker
- error responses use ad-hoc shapes or HTTP status codes inconsistent with the rest of the API
- the schema lints clean but the handler returns shapes the schema does not declare
- a generated client cannot compile against the new schema

## Runtime Boundaries

Do not over-claim certainty when:
- the schema looks correct but integration partners on older versions were not verified
- pagination behavior under concurrent insert/delete was not exercised
- idempotency key retention was inferred from config rather than confirmed in storage
- a deprecation timeline was set without confirming actual client usage telemetry
- breaking-change linters passed but semantic behavior changed in ways the linter cannot detect

## Output Expectations

When using this skill, return:
- the canonical schema file and its proposed diff
- the change classification (additive non-breaking / behavioral non-breaking / breaking)
- the compatibility window and client migration plan
- the error taxonomy, idempotency rules, and pagination semantics for affected endpoints
- the contract verification plan (linter, diff tool, regenerated clients, fixture tests)
- residual risks and the recommended deprecation timeline
