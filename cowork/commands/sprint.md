# keel sprint

Manage sprint backlog and track story progress.

## Usage

```
/keel:sprint <action> [options]
```

## Actions

| Action | Description |
|---|---|
| `plan` | Add a story to the sprint backlog |
| `status` | Show the current sprint board (daily scrum view) |
| `advance` | Move a story across the board |
| `review` | Check if sprint is complete (fail-closed) |
| `list` | List all sprints |

## Examples

```
/keel:sprint plan --story "As a user, I want to reset my password"
/keel:sprint status
/keel:sprint advance --id 1 --state done
/keel:sprint review
```

## Story States

| State | Description |
|---|---|
| `todo` | Story is planned but not started |
| `in-progress` | Story is being worked on |
| `blocked` | Story is blocked by a dependency |
| `done` | Story is complete and verified |

## Related Commands

- `/keel:recall` — Search memories and working briefs
- `/keel:work` — Work item tracking
