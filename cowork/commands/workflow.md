# keel workflow

Route requests and drive the development workflow.

## Usage

```
/keel workflow <action> [options]
```

## Actions

| Action | Description |
|---|---|
| `route` | Route a request to the appropriate specialist |
| `start` | Start work with a preset workflow |
| `cockpit` | Watch workflow state |
| `finish` | Finish branch with proof |

## Examples

```
/keel workflow route --request "Add user authentication"
/keel workflow start --preset feature --request "Add OAuth login"
/keel workflow cockpit
/keel workflow finish
```

## Branch Model

```
main  (final stable)
dev   (active development)
feat  (feature branch)
<category>/<FEATURE>  (work branch)
```

## Related Commands

- `/keel:review` — Code review
- `/keel:sprint` — Sprint management
