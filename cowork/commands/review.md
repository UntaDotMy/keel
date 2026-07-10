# keel review

Run pre-commit or pre-PR code review.

## Usage

```
/keel:review <target> [options]
```

## Targets

| Target | Description |
|---|---|
| `pre-commit` | Review before committing changes |
| `pre-pr` | Review before opening a pull request |

## Examples

```
/keel:review pre-commit
/keel:review pre-pr
```

## What the Review Checks

### Stage 1: Diff Reconciliation
- Verify changes against working brief requirements
- Check for scope creep or missed acceptance criteria
- Validate test coverage

### Stage 2: Feedback Synthesis
- Code quality and style
- Security considerations
- Performance implications
- Documentation completeness

## Related Commands

- `/keel:sprint` — Sprint management
- `/keel:workflow` — Workflow routing
