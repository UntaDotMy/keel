# Git Hooks — Language-Agnostic, Agent-Agnostic Enforcement

These hooks enforce code quality and branch discipline at the git level. They work with **any programming language** and **any AI agent tool** because they are native git hooks — they run before `git commit` and `git push`, regardless of whether you use Claude Code, Cursor, OpenCode, Codex, Pi Agent, vim, or VSCode.

## Language Auto-Detection

### pre-commit
Detects the project language from build/config files and runs the appropriate tools:

| Language | Detection | Formatter | Linter |
|----------|-----------|-----------|--------|
| Rust | `Cargo.toml` | `cargo fmt` | `cargo clippy` |
| Go | `go.mod` | `gofmt` | `go vet` |
| Python | `pyproject.toml`, `setup.py`, `setup.cfg` | `ruff` or `black` | `ruff check` |
| JS/TS | `package.json` | `biome`, `prettier`, or `eslint` | same as formatter |
| C/C++ | `CMakeLists.txt` or `*.c`/`*.cpp` source files | `clang-format` | - |

If no recognized language is detected, the commit is allowed through with a note about adding custom checks.

### pre-push
Blocks direct pushes to `main` and `dev` branches. Queue up your changes in a work branch and open a PR instead.
This is **language-agnostic** — it only checks git branch names.

## Installation

### Automatic (Recommended)
```bash
keel hook git-hooks install
```

This sets `core.hooksPath` to `.githooks` and makes hooks executable.

### Manual
```bash
git config core.hooksPath .githooks
chmod +x .githooks/pre-commit .githooks/pre-push
```

## Cross-Agent Compatibility

| Agent | How It Works |
|-------|-------------|
| Claude Code | Native git hooks run before commit/push automatically |
| Cursor IDE | Native git hooks run before commit/push automatically |
| OpenCode | Native git hooks run before commit/push automatically |
| Codex CLI | Native git hooks run before commit/push automatically |
| Pi Agent | Native git hooks run before commit/push automatically |
| VSCode terminal | Native git hooks run before commit/push automatically |
| Any other tool | Any tool that invokes `git commit` or `git push` triggers the hooks |

## Emergency Bypass
```bash
git commit --no-verify   # Skip pre-commit
git push --no-verify     # Skip pre-push
```
Use only for genuine emergencies. CI will catch bypassed checks.

## Adding Your Own Language

Edit `.githooks/pre-commit` and add a detection block. Template:

```bash
# ----- My Language -----
if [ -f "build-file.ext" ]; then
    FOUND_ANY=1
    echo "[mylang] Checking..."
    if ! my-formatter --check . 2>/dev/null; then
        echo "  ✗ formatting failed — run: my-formatter ."
        ERRORS=$((ERRORS + 1))
    else
        echo "  ✓ formatting passed"
    fi
fi
```

## Integration with keel

- **Git hooks**: Enforce quality at the git level (format, lint, branch policy)
- **keel binary**: Enforces quality at the AI-agent level (PreToolUse compaction, review gates, memory, workflow)
- **keel MCP server**: Exposes 31 tools to any MCP-compatible agent (recall, system_map, workflow, review, etc.)
- **keel adapters**: OpenCode, Codex, Cursor, Pi Agent — each injects the iron law into its own lifecycle
