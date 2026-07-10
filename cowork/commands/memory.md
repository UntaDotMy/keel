# keel memory

Memory scope and system-map management.

## Usage

```
/keel memory <action> [options]
```

## Actions

| Action | Description |
|---|---|
| `scope` | Manage workspace memory scope |
| `system-map` | System map commands (refresh, show) |
| `working-brief` | Working brief management |
| `completion-gate` | Completion gate verification |
| `recall` | Full-text search over memories |
| `status` | Show memory health overview |

## Examples

```
/keel memory scope resolve
/keel memory scope resolve --create-missing --refresh-system-map
/keel memory system-map refresh
/keel memory working-brief write --request "..." --acceptance-criteria "..."
/keel memory status
```

## System Map

The SYSTEM_MAP.md file tracks the project structure and is refreshed
automatically every 10 edit-class tool calls. Use `system-map refresh`
to force a manual refresh.

## Related Commands

- `/keel:recall` — Full-text search
- `/keel:sprint` — Sprint management
