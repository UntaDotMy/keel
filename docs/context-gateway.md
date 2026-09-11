<!--
Purpose: Describe the native context, broker, execution, raw-evidence, and
verification boundaries implemented by Keel.
Caller: Operators and reviewers validating the production gateway contract.
Dependencies: proxy/context.rs, proxy/execution.rs, proxy/raw_store.rs,
mcp/tools.rs, utility/ui_verify.rs, and the benchmark baseline artifact.
-->
# Production Context Gateway

Keel keeps complete command/tool/evidence data in its owned stores and exposes a
bounded projection to the model on governed paths.

## Boundary and failure behavior

`proxy::run` executes through the existing command boundary, stores the raw
capture, reduces it with the existing adapter, and then passes the reduced
result through `proxy/context.rs`. A projection carries a stable id, exact
`o200k_base` token count, provenance, cache class, truncation state, omitted
items, and an optional RawStore recovery id. A failed reduction or budget check
returns an explicit blocked state; it never falls back to raw output.

MCP `tools/call` results use the same firewall. `tools/list` supports `core` and
`full` profiles, deterministic ordering, opaque pagination cursors, and
progressive levels (capability index, metadata, full schema). The no-cursor MCP
request (`params` omitted or `{}`) always returns a bounded first page; when more
tools remain it includes `nextCursor`, so an oversized catalog is never returned
as one unbounded response. `keel/discover` returns ranked compact metadata and
`keel/activate` writes a session/workspace activation receipt. The default profile
is `core`; `full` remains an explicit compatibility/debug choice.

The current ratified `core` profile advertises 17 stable tools and defers 20
specialized tools. The live catalog count and exact `o200k_base` footprint are
reported by `keel stats tools --json`; the archived baseline records the same
measurement and the host capability matrix.

## Execution and evidence

Every governed command receives an immutable execution identity and one of
`intercepted`, `executed`, `reduced`, `bypassed`, `not_intercepted`, `blocked`,
`failed`, or `unknown`. Only `executed` and `reduced` are successful states.
Host capability declarations are attached to JSON results; an unregistered host
cannot claim interception.

RawStore entries are atomic, capped, namespace-capable, and contain a versioned
integrity manifest. Reads reject traversal and symlink artifacts and verify the
manifest before returning bytes. New manifests use SHA-256 for persistent
integrity; legacy FNV-1a manifests remain readable for recovery but are not
treated as authenticity proofs. Retention and stale staging cleanup remain
bounded and recoverable.

Memory recall is pull-based: results contain a bounded excerpt, stable memory
and provenance ids, a dedupe key, and a retrieval reference. Full durable memory
stays in the recall index and memory lane.

UI verification writes screenshots to RawStore plus compact
`ui-verification/<task>/manifest.json` and `verdicts.json` artifacts. An unclear
visual verdict is preserved for diagnosis but promoted to `needs_human` for
review/completion decisions.

## Operator measurements

```text
keel stats context --json
keel stats tools --json
keel stats gain --json
```

The reproducible baseline and required quality/latency/cache gates are recorded
in [`benchmarks/context-gateway-baseline.json`](benchmarks/context-gateway-baseline.json).
The catalog footprint is not, by itself, permission to change the default
profile.

## Scope honesty

The gateway is native for the Rust command proxy, MCP server, recall projection,
RawStore, planner classification, and UI verification surfaces above. Host
adapters still expose only the capabilities they can prove; unsupported host
boundaries remain explicitly unprotected rather than being represented as a
successful governed path.

Protocol era is stated the same way. The MCP revision in force (`2026-07-28`)
distinguishes *modern* servers (per-request `_meta` version, `server/discover`,
`resultType`, `ttlMs`/`cacheScope` on list results), *legacy* servers
(`initialize` handshake), and *dual-era* servers that serve both. Keel is a
legacy-era server speaking `2025-11-25`, and it rejects the older `2024-11-05`
initialize on Streamable HTTP with an explicit migration message instead of
mis-serving it. A legacy client therefore interoperates and a modern one does
not; no request silently mixes the two eras. Cleanup status and the full
limitation list are in [`gateway-cleanup-report.md`](gateway-cleanup-report.md).
