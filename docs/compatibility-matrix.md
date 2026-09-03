# Compatibility Matrix

This matrix documents the supported environments and key CLI surfaces for `keel`.

The goal is to keep the supported entry points explicit for both human operators and AI agents. The managed install publishes the native CLI into the harness home root, so an agent does not need to stay inside this repository checkout to call the tool.

## Preferred entry points

- Source checkout with Rust/Cargo available: `cargo run --bin keel -- ...`
- Installed global CLI on macOS or Linux: `~/.keel/keel ...`
- Installed global CLI on Windows: `~/.keel/keel.exe ...`
- First-time extracted release bundle, no Rust needed: `./keel install` or `.\keel.exe install`

## Environment and execution-context support

| Context | Windows | macOS | Linux | Agent-friendly | Primary command shape | Notes |
| --- | --- | --- | --- | --- | --- | --- |
| Source checkout with Rust toolchain | Supported | Supported | Supported | Supported | `cargo run --bin keel -- ...` | Best fit when iterating inside the repository itself. |
| Installed global CLI from the keel home | Supported | Supported | Supported | Supported | `~/.keel/keel(.exe) ...` | Preferred agent entry point when PATH is not guaranteed and the managed install already exists. Manager commands reuse the recorded owning checkout from install metadata when launched from another project. |
| Extracted release bundle | Supported | Supported | Supported | Supported | `./keel install` or `.\keel.exe install` | Useful on fresh machines before the managed install is present. The install command infers the release/source root from the current directory or executable location; `--repo-root` is only an advanced override. |
| Hosted GitHub Actions runners | Supported | Supported | Supported | Supported | native Rust CLI or `cargo run --bin keel -- ...` | Current hosted proof already runs repo-wide and cross-OS manager loops on all three platforms. |

## Key surface support

| Surface | In repo root | Outside repo root with `--repo-root` | Installed global CLI | Hosted automation | Notes |
| --- | --- | --- | --- | --- | --- |
| `help`, `help advanced`, `version`, `platform` | Supported | Supported | Supported | Supported | Safe discovery surfaces for both operators and agents. |
| `install`, `update`, `status`, `doctor`, `repair`, `verify`, `uninstall` | Supported | Supported | Supported | Supported | Checkout and packaged-release installs retain their owning source so lifecycle commands work outside the original extraction directory. |
| Host adapter wiring | Supported | Supported | Supported | Supported | Claude, OpenCode, Codex, Cursor, Pi, Cowork, Command Code, Grok, Oh My Pi, ZCode, and Antigravity are detected or selected with `--with`; `keel doctor` verifies installed runtime dependencies instead of file presence alone. |
| `review pre-commit`, `review pre-pr`, `review gates check` | Supported | Supported | Supported | Supported | Native review surfaces are the default deterministic proof path. |
| `git-workflow preflight` | Supported | Supported | Supported | Supported | Main branch and PR hygiene gate before publish or merge. |
| `memory scope`, `memory system-map`, `memory working-brief`, `memory completion-gate`, `memory recall` | Supported | Supported | Supported | Supported | Core surfaces of the **unified** `keel memory` group. |
| `memory research-cache`, `maintenance`, `agent-registry`, `agent-packets`, `loop-guard`, `entity`, `graph`, `retrieve`, `status`, `instincts`, `consolidate` | Supported | Supported | Supported | Supported | Family commands under the single memory group. `memory report` (alias for `status`) and `memory index` (rebuilds the recall index) are also supported; `memory hook` points to `keel hook ...`; `memory consolidate` scans family directories and reports counts/previews (status summary, not a merge/promote). |
| `code-index refresh|status|map` | Supported | Supported | Supported | Supported | Persistent deterministic workspace index for files, symbols, chunks, paths, relationships, commit generation, and stale-state reporting. |
| `code-search search` | Supported | Supported | Supported | Supported | Indexed ranked retrieval. Path filters accept `/` and `\` on every platform; results include path, symbol, line range, score, reason, and snippet. |
| `code-search siblings` | Supported | Supported | Supported | Supported | Indexed completeness scan: searches explicit query text or tokens from the current git diff and lists every other in-repo copy. Writes the completeness-gate marker. Required after a fix or implement. |

## Host integration coverage

| Host | Rules / discovery | Lifecycle enforcement | MCP | Important limitation |
| --- | --- | --- | --- | --- |
| Claude Code | Managed global contract and full skill catalog | Native Keel hooks | Native config | Primary, deepest integration. |
| Codex CLI | `~/.codex/AGENTS.md` plus `~/.agents/skills/using-keel` | Codex plugin hooks | Native `config.toml` entry | Codex may require the user to trust newly installed hooks after restart. |
| OpenCode | Shared gateway skill plus TypeScript plugin | Plugin lifecycle events | `opencode.json` | The installed plugin requires the bundled `_shared/ts/bridge-core.ts`; doctor verifies it. |
| Pi Agent / Oh My Pi | Host `AGENTS.md` plus gateway skill | TypeScript extension | Native `mcp.json` | OMP is wired at `~/.omp/agent`, distinct from Pi's home. |
| ZCode | Global `AGENTS.md` plus gateway skill | Native event hooks in `config.json` | Native `mcp.servers` entry | Keel preserves an explicit user `hooks.enabled = false`; doctor reports hooks disabled. |
| Google Antigravity | Global plugin rule plus gateway skill | CamelCase JSON hook adapter | Plugin `mcp_config.json` | IDE and `agy` CLI use different global plugin directories; Keel detects each. The hook command requires `node` on PATH and doctor verifies it. |
| Grok CLI | Native host configuration; Grok also discovers shared Agent Skills | Claude-compatible hooks by default; native fallback when compatibility is disabled | Native TOML entry | Existing sessions must be restarted to load new configuration. Keel keeps exactly one effective hook source. |
| Cursor | `.cursorrules` | Native hooks | Native JSON entry | Use `--with cursor` when its config directory is absent during installation. |
| Command Code | Mod-provided instructions | Mod lifecycle events | Native JSON entry | The installed mod requires the bundled `_shared/ts/bridge-core.ts`; doctor verifies it. |
| Claude Desktop / Cowork | MCP tool descriptions only | Not available | Native Desktop config | Cowork exposes no lifecycle hook surface, so it cannot enforce the pre-edit gate. |

## Agent execution guidance

When an AI agent is operating from an arbitrary workspace or a harness home installation:

- Prefer the explicit installed path `~/.keel/keel` or `~/.keel/keel.exe` when PATH resolution is uncertain.
- The installed global CLI records the owning checkout in install metadata, so manager commands can recover that source automatically when they are launched from another project.
- Use `--repo-root <path>` only when the command needs a different owning repository than the current directory, extracted bundle, executable location, or recorded install source.
- Keep the native CLI as the only install/update surface; shell and PowerShell wrapper launchers are not supported runtime entrypoints.
- Treat bare `keel ...` as a convenience command shape, not a guarantee that the executable is on PATH in every runtime.

## Shell PATH

Native `keel install` is the only PATH writer. Downloaders (`install.sh`, `install.ps1`, `install.cmd`) never write PATH. Proof is the PATH tests under temp `HOME` / the PathPersist double, not a hosted fish/zsh/CMD installer job. PATH write and reverse run only for the default keel home.

| Shell / host | After native `keel install` | This cycle | Honest note |
| --- | --- | --- | --- |
| bash (interactive) | Shared POSIX `$KEEL_HOME/env` sourced from `.bashrc` | Supported when `.bashrc` already exists | `.bashrc` is not created. Login bash still gets always-created `.profile` |
| zsh (interactive + `zsh -c`) | Shared env sourced from `.zshenv` | Supported | Do not rely on `.zshrc` alone. Existing `.zshrc` is updated if present |
| sh/dash login | Always `.profile` | Supported | Login `sh` does not read `.bashrc` |
| fish | `conf.d/keel.fish` sources `env.fish` (`set -x PATH`) | Supported | Not `export`. Not `fish_add_path` |
| Windows User PATH, **new** console / **new** WT window | HKCU + `WM_SETTINGCHANGE` | Supported | No `setx`. No pwsh 5/7 profile edits. Entry is appended |
| Current Windows console / current WT tab | Not updated | **Not this cycle** | Open a new console or a new Windows Terminal window |
| Git Bash inherit | Untouched | **Not this cycle** | Named hole |
| Hosted fish / zsh / CMD installer | Downloaders only | **Not proven** | `validate.yml` cheap-parse ≠ hosted install |
| Custom `KEEL_HOME` | PATH not written | Skipped | Use the explicit binary |

Uninstall at the default home silently reverses those PATH files and the User Path entry, and sweeps old triplicate `export PATH="…:$PATH"` marker pairs. Stdout does not claim PATH was restored. Open a new session afterward.

## Minimum proof expectations

Compatibility claims in this repository should stay tied to real proof:

- repo-wide Rust validation
- native review gates
- cross-OS manager-loop checks
- explicit docs and contract coverage when the supported surface changes
