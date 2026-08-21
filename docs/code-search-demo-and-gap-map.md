<!--
Purpose: Document the persistent deterministic code-index and retrieval surface.
Caller: Operators and agents locating source files, symbols, and relationships.
Dependencies: keel code-index and code-search commands.
Main Functions: Explain refresh, status, map, ranked search, and sibling scans.
Side Effects: Documentation only.
-->
# Code Search And Workspace Index

## Live surface

```bash
keel code-index refresh
keel code-index status --json
keel code-index map
keel code-search search --query "run_recall_search" --json
keel code-search search --query "FlagSet" --path "rust/crates/keel"
keel code-search siblings --query "the bug shape"
```

The workspace index is stored in the global per-workspace memory lane. It never
writes generated artifacts into the repository unless an explicit output path is
requested by another command.

## Index contents

The persistent SQLite index records:

- eligible source and documentation files;
- stable content hashes, size, modification time, and indexed Git commit;
- symbols with kind, signature, documentation, and exact line ranges;
- file and symbol chunks for context packing;
- resolved import relationships;
- deterministic FTS5 search entries;
- generation and stale-state metadata.

Refresh is incremental by file hash and commits changes atomically. A deleted
file removes its symbols, chunks, search entries, and relationships. Retrieval
never falls back to a live filesystem scan.

## Search behavior

`code-search search` fuses four ranked channels:

1. exact symbol and qualified-name matches;
2. FTS5 Porter/Unicode full-text matches;
3. path and filename matches;
4. verified graph relationships from exact symbols.

The channels use weighted Reciprocal Rank Fusion, then return bounded evidence:

```text
path:start-end [symbol] (reason) score=... snippet
```

`--json` returns the same path, symbol, line, score, reason, and snippet fields
for MCP and automation callers.

## Map behavior

`code-index map` and the MCP `system_map` surface are generated from the same
index. The map includes the indexed commit, generation, file inventory, symbol
locations, and relationship evidence. Scope refresh persists the same map under
the workspace reference lane.

## Completeness behavior

`code-search siblings` queries the persistent index using explicit query text or
distinctive tokens from the current Git diff. It excludes changed files, reports
other indexed matches, and clears the existing completeness gate only after the
indexed scan succeeds.

## Failure policy

- Missing or corrupt index: return an error and require an explicit refresh.
- Stale indexed commit: report stale status; do not silently claim freshness.
- Deleted source: remove it from the index during refresh.
- Unresolved external import: retain the raw import only; do not invent an edge.
- No indexed match: return an honest empty result.

The surface is local-first, deterministic, and model-free. It is designed to
answer exact code-location and dependency questions without making the agent
walk or guess through the repository.
