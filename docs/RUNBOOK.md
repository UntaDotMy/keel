# Runbook: install and uninstall PATH

Operator steps for the PATH behavior shipped in native `keel install` / `keel uninstall`. Not a product spec. Downloaders never edit PATH. There is no hosted fish/zsh/CMD install proof in this cycle.

## Install (default home)

1. Run one downloader, or run the binary from an extracted release:

   - macOS / Linux / WSL (bash): `curl -fsSL https://raw.githubusercontent.com/UntaDotMy/keel/main/install.sh | bash`
   - Windows PowerShell: `irm https://raw.githubusercontent.com/UntaDotMy/keel/main/install.ps1 | iex`
   - Windows CMD: `curl -fsSL https://raw.githubusercontent.com/UntaDotMy/keel/main/install.cmd -o install.cmd && install.cmd && del install.cmd`
   - Extracted release: `./keel install` or `.\keel.exe install`

   Optional pin: `CLAUDE_SKILLS_VERSION=latest` (default) or `CLAUDE_SKILLS_VERSION=bootstrap-<sha>`. Do not use a `vX.Y.Z` pin.

2. The downloader (if used) only downloads, extracts, and invokes native `keel install`. PATH is written by `rust/crates/keel/src/manager/install/path.rs` when the published binary lands in the **default** keel home (`~/.keel` / `%USERPROFILE%\.keel`). A custom `KEEL_HOME` skips PATH.

3. Unix writes:

   - `$KEEL_HOME/env` — rustup-shaped POSIX guard + `export PATH`
   - `$KEEL_HOME/env.fish` — `if not contains` + `set -x PATH` (not `export`, not `fish_add_path`)
   - always: `~/.profile`, `~/.zshenv`, `~/.config/fish/conf.d/keel.fish`
   - if they already exist: `~/.bashrc`, `~/.bash_profile`, `~/.zshrc`

4. Windows writes the User Path (`HKCU\Environment\Path`) and broadcasts `WM_SETTINGCHANGE` / `Environment`. No `setx`. No pwsh profile edits.

5. Verify with the **explicit** binary (works in this session):

   ```bash
   ~/.keel/keel status
   ```

   ```powershell
   & "$env:USERPROFILE\.keel\keel.exe" status
   ```

6. For `keel` on PATH, open a **new** bash, zsh, sh/dash, or fish shell, or a **new** Windows console / Windows Terminal window. Do not expect the current Windows console or Git Bash inherit to pick it up.

## Uninstall (default home)

1. Run `keel uninstall` or `~/.keel/keel uninstall` / `%USERPROFILE%\.keel\keel.exe uninstall`.

2. Native uninstall reverses PATH **silently** when the target is the default keel home:

   - deletes `$KEEL_HOME/env` and `$KEEL_HOME/env.fish`
   - strips the `# keel PATH (managed by the keel installer)` marker and the following source line from the rc files listed above
   - sweeps old triplicate `export PATH="…:$PATH"` pairs that used that marker
   - Windows: removes the keel home from User Path and broadcasts again

3. Stdout is `Uninstall complete` plus a removed-files count. It does not say PATH was restored.

4. Open a new session so the reversed PATH is what you see. The current Windows console still has the old process PATH.

5. Custom `KEEL_HOME` installs never wrote user PATH, so uninstall does not edit rc files or HKCU Path for that home. Keep using the explicit binary path until you delete that home yourself.

## What CI actually proves

- `bash -n install.sh`
- PowerShell parser on `install.ps1`
- `install.cmd` readable by cmd, no NUL, has `@echo off`

That is syntax only. It is not proof that a hosted fish, zsh, or CMD install wired PATH.

PATH writer tests run against a temp `HOME` (Unix) and a PathPersist double (Windows). They do not touch the live HKCU hive.
