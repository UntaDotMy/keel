# keel recall

Search memory and working briefs using full-text search.

## Usage

```
/keel:recall <query> [--limit N] [--json]
```

## Description

Searches Markdown and JSON files under `<claude-home>/memories` and
`<claude-home>/working-briefs` via SQLite FTS5. The index is refreshed
automatically on every call.

## Examples

```
/keel:recall design decisions
/keel:recall authentication pattern --limit 10
/keel:recall "database schema" --json
```

## Arguments

| Argument | Description |
|---|---|
| `query` | Search query text |
| `--limit N` | Maximum number of results (default: 10) |
| `--json` | Output results as JSON |

## Related Commands

- `/keel:memory` — Memory scope and system-map management
- `/keel:anvil` — Delivery loop
