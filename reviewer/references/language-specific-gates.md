# Reviewer Reference ,  Language-Specific Gates

Deterministic closeout: `keel review pre-commit` (fmt + lint) and `keel review pre-pr` (fmt + lint + typecheck + unit tests) auto-detect **root** project markers ,  same set as `.githooks/pre-commit`. Tool not on PATH → gate status `Blocked` (non-blocking). Tool present and failing → blocking `Fail`.

| Language | Root marker | pre-commit | pre-pr adds |
|---|---|---|---|
| Rust | `Cargo.toml` | `cargo fmt --check`, `cargo clippy -D warnings` | `cargo test --workspace` |
| Python | `pyproject.toml` / `setup.py` / `setup.cfg` | `black --check`, `ruff check` | `mypy`, `pytest` (or `unittest discover`) |
| JS/TS | `package.json` | `prettier --check`, `eslint` | `tsc --noEmit` (if `tsconfig.json`), `npm test --if-present` |
| Go | `go.mod` | `gofmt -l .`, `go vet ./...` | `go test ./...` |
| C/C++ | `CMakeLists.txt` or root `*.c`/`*.cpp`/`*.h` | `clang-format --dry-run --Werror` | format only (no portable auto unit-test runner) |

Exit code 5 from pytest (no tests collected) **or** `python -m unittest discover` (NO TESTS RAN) is non-blocking (`Blocked` / not applicable), so empty pyproject-only trees do not fail pre-pr.

Git-level enforcement (any agent that runs `git commit`): `.githooks/pre-commit` covers the same language set. Install with `keel hook git-hooks install`.

## Rust (manual review)
- `unsafe` blocks must be justified with a safety comment
- `unwrap()` / `expect()` usage should be minimized or justified
- Prefer `match` or `if let` over `unwrap()`
- Use `thiserror` or `anyhow` for error types

## TypeScript / JavaScript (manual review)
- Strict mode (`strict: true`) required in `tsconfig.json`
- Minimize `any` usage ,  prefer `unknown` when type is uncertain
- Use `const` over `let` where possible
- Prefer `async`/`await` over raw promises

## Python (manual review)
- Type hints required for function signatures
- Use `pathlib` over `os.path`
- Prefer `Exception` subclasses over string-based errors

## Go (manual review)
- Check error returns ,  never ignore them with `_`
- Use `context.Context` for cancellation and timeouts
- Prefer `go test -race` for concurrent code
