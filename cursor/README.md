# keel Cursor Rules

Injects keel's iron law, skill catalog, and operating instructions into Cursor IDE via the `.cursorrules` file.

## What This Does

Cursor reads `.cursorrules` from the project root (or `~/.cursorrules` globally) and injects the contents as system instructions. This adapter puts keel's discipline — the four iron law rules, the full 44-skill catalog, workflow commands, and branch/commit rules — into that injection surface so Cursor follows keel operating contract automatically.

Unlike the OpenCode and Codex adapters, this is a **static rules file**, not a hook-based bridge. Cursor does not have a plugin lifecycle or bridge subcommands. The `.cursorrules` file ensures the model operating in Cursor has the full keel discipline available from the first prompt.

## Prerequisites

1. Cursor IDE installed.
2. The `keel` binary installed at `~/.claude/keel` (unix) or `~/.claude/keel.exe` (win32), or on `PATH`.

## Install

### Option A: Project-scoped (recommended)

Copy `.cursorrules` into your project root:

```bash
cp cursor/.cursorrules /path/to/your/project/
```

On Windows:

```powershell
Copy-Item cursor\.cursorrules "C:\path\to\your\project\"
```

Cursor loads it automatically when you open the project.

### Option B: Global

Copy to your home directory:

```bash
cp cursor/.cursorrules ~/.cursorrules
```

On Windows:

```powershell
Copy-Item cursor\.cursorrules "$env:USERPROFILE\.cursorrules"
```

This applies keel discipline to every project in Cursor.

## What the Rules Include

### Iron Law (4 rules)

1. **Understand before building.** Restate the request, confirm the user story, research the owning module and framework. No guessing.
2. **Skills first.** Invoke the matching skill before writing code. The cost of skipping is shipping a regression.
3. **Native commands before raw shell.** Use `keel run -- <command>` for noisy commands. Never run raw and compact after.
4. **Find the root cause.** Trace symptoms end-to-end with file:line evidence before changing anything.

### Workflow Commands

| Command | Use |
|---|---|
| `keel workflow route --request "..."` | Route a broad request to a preset |
| `keel workflow start --preset <preset> --request "..."` | Start work |
| `keel workflow cockpit` | View live state |
| `keel workflow finish --id <id> --proof "..."` | Finish a workstream |
| `keel review pre-pr --base-ref origin/feat` | Review before PR |
| `keel memory scope resolve --create-missing --refresh-system-map` | Refresh memory |
| `keel code-search search --workspace-root "$PWD" --query "..."` | Search code |

### Branch and Commit Rules

- Branch model: `main` ← `dev` ← `feat` ← `<category>/<FEATURE>`
- Commit format: `<category>: <FEATURE>: <short info>`
- Never delete a branch after push or merge

### Skill Catalog (44 skills)

Full catalog organized by domain: Security & Review, API & Backend, Infrastructure & DevOps, Data & ML, Frontend & Mobile, Quality & Testing, Architecture & Planning, Delivery & Git, Code Quality & Dependencies. Each skill includes its `whenToUse` guidance.

## Differences from Other Adapters

| Aspect | OpenCode Adapter | Codex Adapter | Cursor Adapter |
|---|---|---|---|
| Mechanism | TypeScript plugin with lifecycle hooks | Codex plugin with hooks.json + script | Static .cursorrules file |
| Runtime bridge | Yes — `bridge` subcommands per event | Yes — `bridge` subcommands per event | No — rules only, manual keel CLI |
| Context injection | Automatic per session/prompt | Automatic per session/prompt | Via Cursor's rules injection |
| Observation recording | Automatic on tool events | Automatic on tool events | Manual via keel CLI |
| Learning checkpoints | Automatic on compaction | Automatic on compaction | Manual via keel CLI |

The Cursor adapter is simpler because Cursor does not expose a hook lifecycle. It ensures the model always has the iron law and skill catalog available, and the model can call keel CLI commands directly for workflow, review, and memory operations.
