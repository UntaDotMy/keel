# Changelog

Operator-facing notes. No release-tag pin until a tag exists.

## Unreleased

### PATH honesty

Native `keel install` is the only PATH writer. `install.sh`, `install.ps1`, and `install.cmd` download a release and invoke that native step; they do not edit PATH. Installers no longer coerce a numeric pin into `v*`.

**Unix.** rustup-shaped `$KEEL_HOME/env` plus `$KEEL_HOME/env.fish`. Fish PATH is `set -x PATH` (not `export`, not `fish_add_path`). Thin sources: always `.profile`, `.zshenv`, and fish `conf.d/keel.fish`; existing `.bashrc` / `.bash_profile` / `.zshrc` only.

**Windows.** User PATH (`HKCU\Environment\Path`) plus `WM_SETTINGCHANGE`. No `setx`. No PowerShell profile edits. Open a new console or a new Windows Terminal window; the current window is not updated.

**Uninstall.** At the default keel home, PATH files and the User Path entry are reversed silently. Old triplicate `export PATH="…:$PATH"` marker pairs are swept. Stdout does not claim PATH was restored.

**Not this cycle.** Git Bash inherit; the already-open Windows console. A custom `KEEL_HOME` skips PATH write and PATH reverse.

**Proof.** PATH unit tests under a temp `HOME` / PathPersist double. CI cheap-parse of the three downloaders is syntax only — not hosted fish, zsh, or CMD install proof.

**License.** Root `LICENSE` is MIT.
