# Execution Environment: Windows Shells

## Purpose

Use this guide when command behavior depends on the Windows shell environment:
- PowerShell (`powershell.exe`)
- Command Prompt (`cmd.exe`)
- Git Bash (`bash`) when installed

This repo is Windows-first. Do not assume any Linux subsystem is installed.

## Detection Sequence

1. Start with the host's native command runner (for this repo, the Rust-native `keel run`/`keel` command owner); choose an explicit shell only when command behavior depends on it.
2. Avoid PowerShell wrappers for ordinary commands; prefer direct command strings or explicit executable calls inside `exec_command`.
3. Check whether PowerShell is required (common signals):
   - `.ps1` scripts, PowerShell cmdlets, `$env:` syntax, or pipeline/object semantics
4. Check whether `cmd.exe` is required (common signals):
   - `.cmd` scripts, docs referencing `%VAR%`, or tooling that depends on `where.exe`
5. Check whether Git Bash exists only when needed:
   - `Get-Command bash -ErrorAction SilentlyContinue`

## Command Selection Rules

1. Default to the host's native command runner with direct executable arguments or command strings for ordinary commands.
2. If a command is PowerShell-specific, run it explicitly:
   - Script file: `powershell.exe -NoProfile -ExecutionPolicy Bypass -File <script.ps1>`
   - One-liner: `powershell.exe -NoProfile -Command "<command>"`
3. If a command is `cmd.exe`-only, run it explicitly:
   - `cmd.exe /c "<command>"`
4. If Bash is required, ask for confirmation that Git Bash is installed (or provide a non-Bash alternative).
5. Prefer Windows-native paths in guidance:
   - `D:\repo\project` not `/repo/project`

## Testing and Validation Rules

1. Record whether a validation step stayed shell-neutral or required PowerShell, `cmd.exe`, or Git Bash.
2. If shell choice is unclear or mixed, ask concise clarification before finalizing instructions.

## Common Failure Modes

- An ordinary command is unnecessarily wrapped in PowerShell and breaks because of quoting or escaping.
- A command exists in PowerShell PATH but not `cmd.exe` PATH (or vice-versa).
- Path separators/drive notation incompatible across shells.
- Scripts assume Bash while environment expects PowerShell.
- Toolchain installed only for one shell environment.

Mitigate by detecting the shell early and selecting command forms intentionally.
