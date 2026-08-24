<!--
Purpose: Capture Windows environment guidance and cross-platform script portability rules previously inline in AGENTS.md.
Caller: AGENTS.md when running commands on Windows or shaping repo-managed runtime helpers.
Dependencies: native keel run_command, PowerShell, cmd.exe.
Main Functions: Define when to use which shell, when PowerShell is appropriate, and the portability requirements for runtime helpers.
Side Effects: None — this file is informational.
-->
# Windows Environment and Cross-Platform Script Portability

## Windows Environment

When running commands on Windows:
- Let the harness choose the most appropriate supported tool surface for the active runtime.
- Use the most direct supported tool surface for the task. Reach for the persistent `eval` tool only when a persistent Node/Python context helps, JavaScript-side orchestration is clearly better, or the runtime explicitly requires that path.
- Use the native `run_command` surface with direct argv or script execution; prefer direct command strings and avoid wrapping ordinary commands in `powershell.exe -NoProfile -Command "..."`.
- Use PowerShell only for PowerShell cmdlets/scripts or when shell-specific quoting, pipelines, or object semantics are required.
- Use `cmd.exe /c` for `.cmd`/batch-specific commands or `%VAR%` syntax.
- Git Bash available but not assumed

## Cross-Platform Script Portability

Repo-managed runtime helpers must stay portable across Windows, Linux, and macOS:

- Prefer Rust for repo-managed runtime-critical manager logic, and do not add new shell, PowerShell, or Python-managed manager surfaces unless the user explicitly asks for them.
- Use `pathlib`, UTF-8, launcher detection, and separator-agnostic paths.
- Keep install, update, verify, status, hook, flow, and review behavior aligned in the single Rust-native CLI across Linux, macOS, and Windows.
- Do not rely on one shell, one path separator, or one platform-specific binary layout when portable alternatives exist.
