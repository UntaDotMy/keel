# Compatibility Matrix

This matrix documents the supported environments and key workflow surfaces for `claude_skills`.

The goal is to keep the supported entry points explicit for both human operators and AI agents. The managed install publishes the native CLI into the Claude Code home root, so an agent does not need to stay inside this repository checkout to call the tool.

## Preferred entry points

- Source checkout with Rust/Cargo available: `cargo run --bin claude-skills -- ...`
- Installed global CLI on macOS or Linux: `~/.claude/claude-skills ...`
- Installed global CLI on Windows: `~/.claude/claude-skills.exe ...`
- First-time extracted release bundle, no Rust needed: `./claude-skills install` or `.\claude-skills.exe install`

## Environment and execution-context support

| Context | Windows | macOS | Linux | Agent-friendly | Primary command shape | Notes |
| --- | --- | --- | --- | --- | --- | --- |
| Source checkout with Rust toolchain | Supported | Supported | Supported | Supported | `cargo run --bin claude-skills -- ...` | Best fit when iterating inside the repository itself. Workflow shell suggestions keep the source-checkout launcher instead of falling back to bare `claude-skills`. |
| Installed global CLI from Claude Code home | Supported | Supported | Supported | Supported | `~/.claude/claude-skills(.exe) ...` | Preferred agent entry point when PATH is not guaranteed and the managed install already exists. Manager commands reuse the recorded owning checkout from install metadata when launched from another project, and workflow shell suggestions keep the installed executable surface. |
| Extracted release bundle | Supported | Supported | Supported | Supported | `./claude-skills install` or `.\claude-skills.exe install` | Useful on fresh machines before the managed install is present. The install command infers the release/source root from the current directory or executable location; `--repo-root` is only an advanced override. |
| Hosted GitHub Actions runners | Supported | Supported | Supported | Supported | native Rust CLI or `cargo run --bin claude-skills -- ...` | Current hosted proof already runs repo-wide and cross-OS manager loops on all three platforms. |

## Key workflow-surface support

| Surface | In repo root | Outside repo root with `--repo-root` | Installed global CLI | Hosted automation | Notes |
| --- | --- | --- | --- | --- | --- |
| `help`, `help advanced`, `version`, `platform` | Supported | Supported | Supported | Supported | Safe discovery surfaces for both operators and agents. |
| `install`, `update`, `status`, `doctor`, `verify`, `uninstall` | Supported | Supported | Supported | Supported | Managed-pack lifecycle works from the native CLI. |
| `workflow route`, `workflow start`, `workflow cockpit`, `workflow watch`, `workflow finish` | Supported | Supported | Supported | Supported but mainly interactive | Primary operator and agent workflow surface once the repo context is known. These surfaces now share the same compact shell summary fields: `stage`, `active_lane`, `proof_state`, `blocker`, `next_command`, and `recovery_path`. |
| `review pre-commit`, `review pre-pr`, `review gates check` | Supported | Supported | Supported | Supported | Native review surfaces are the default deterministic proof path. |
| `git-workflow preflight` | Supported | Supported | Supported | Supported | Main branch and PR hygiene gate before publish or merge. |
| `memory scope`, `memory system-map`, `memory working-brief`, `memory completion-gate`, `memory recall` (and the same set under `memoriesv2`) | Supported | Supported | Supported | Supported | Core memory surfaces. |
| `memory research-cache`, `maintenance`, `agent-registry`, `agent-packets`, `loop-guard`, `entity`, `graph`, `retrieve`, `status`, `instincts` (and the same set under `memoriesv2`) | Supported | Supported | Supported | Supported | Family commands persisting flat-JSON records under `<claude-home>/<group>/<family>/`, isolated per command group. `memory report` (alias for `status`) and `memory index` (rebuilds the recall index) are also supported; `memory hook` points to `claude-skills hook ...`; only `consolidate` is unimplemented. |
| `orchestration runtime-preflight`, `resume-status`, `task begin|progress|complete|list`, `checkpoint` | Supported | Supported | Supported | Supported | Orchestration ledger and snapshot surfaces. |
| `code-search search` | Supported | Supported | Supported | Supported | The single implemented code-search subcommand. `code-search index|demo|status|reset` are referenced in older docs but are not implemented today. |

## Agent execution guidance

When an AI agent is operating from an arbitrary workspace or a Claude Code home installation:

- Prefer the explicit installed path `~/.claude/claude-skills` or `~/.claude/claude-skills.exe` when PATH resolution is uncertain.
- The installed global CLI records the owning checkout in install metadata, so manager commands can recover that source automatically when they are launched from another project.
- Use `--repo-root <path>` only when the command needs a different owning repository than the current directory, extracted bundle, executable location, or recorded install source.
- The workflow shell preserves the active launch surface in its suggested commands, so source-checkout runs keep `cargo run --bin claude-skills -- ...` and installed-home runs keep the installed executable path.
- Keep the native CLI as the only install/update workflow surface; shell and PowerShell wrapper launchers are not supported runtime entrypoints.
- Treat bare `claude-skills ...` as a convenience command shape, not a guarantee that the executable is on PATH in every runtime.

## Minimum proof expectations

Compatibility claims in this repository should stay tied to real proof:

- repo-wide Rust validation
- native review gates
- cross-OS manager-loop checks
- explicit docs and contract coverage when the supported surface changes
