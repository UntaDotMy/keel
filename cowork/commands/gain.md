# keel gain

Token-saving measurement and compaction analysis.

## Usage

```
/keel gain [options]
```

## Description

Reports the measured token savings from command compaction. Shows:
- Total tokens saved across all commands
- Per-command breakdown
- Compaction ratio
- Efficiency metrics

## Examples

```
/keel gain
/keel gain --since 7d
/keel gain --format json
```

## Options

| Option | Description |
|---|---|
| `--since <duration>` | Report since duration (e.g., 7d, 24h) |
| `--format <format>` | Output format (text, json) |
| `--limit <n>` | Limit to top N commands |

## Related Commands

- `keel run -- <command>` — Run command with compaction
