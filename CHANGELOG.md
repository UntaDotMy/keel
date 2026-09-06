# Changelog

Operator-facing notes. No release-tag pin until a tag exists.

## Unreleased

### SUPERHARNESS P1 — ClarifyPacket gate, model tiers, skills audit

- **ClarifyPacket gate:** when armed (`--clarify-required`, `clarify.required` sentinel, or existing `clarify.packet.json`), refuse anvil **lock write** on missing/malformed packet, `hard_block` (unanswered required questions), `drift_check` failure, or immutable `locked_brief.goal` mismatch vs compile/run `--goal`. Status: `CLARIFY_BLOCKED`. Enforced in `compile::write_lock` for both `anvil compile` and `anvil run` auto-compile. Packet/sentinel preserved across compile generation swap.
- **AppSec:** clarify artifacts must be non-symlink regular files inside the anvil bank (path jail). Answer text is untrusted (sanitize/size-bound); secret-shaped values redacted on refuse Display paths — prefer env names in `locked_brief`.
- **docs/model-tiers.md:** provider-aware frontier/cheap/mid guidance; Keel does **not** route models at runtime; Claude 3.x removed as current from README/AGENTS/skills.
- **docs/skills-audit-p1.md:** keep|merge-into|retire for installed `SKILL.md` packs; no new megaskill; remote skill load N/A (pin+hash if ever added).

Merged: PR #255 (`cde6192c45f740cacff907b8dbf4c371d62d3572`).

### PATH honesty

Native `keel install` is the only PATH writer. `install.sh`, `install.ps1`, and `install.cmd` download a release and invoke that native step; they do not edit PATH. Installers no longer coerce a numeric pin into `v*`.

**Unix.** rustup-shaped `$KEEL_HOME/env` plus `$KEEL_HOME/env.fish`. Fish PATH is `set -x PATH` (not `export`, not `fish_add_path`). Thin sources: always `.profile`, `.zshenv`, and fish `conf.d/keel.fish`; existing `.bashrc` / `.bash_profile` / `.zshrc` only.

**Windows.** User PATH (`HKCU\Environment\Path`) plus `WM_SETTINGCHANGE`. No `setx`. No PowerShell profile edits. Open a new console or a new Windows Terminal window; the current window is not updated.

**Uninstall.** At the default keel home, PATH files and the User Path entry are reversed silently. Old triplicate `export PATH="…:$PATH"` marker pairs are swept. Stdout does not claim PATH was restored.

**Not this cycle.** Git Bash inherit; the already-open Windows console. A custom `KEEL_HOME` skips PATH write and PATH reverse.

**Proof.** PATH unit tests under a temp `HOME` / PathPersist double. CI cheap-parse of the three downloaders is syntax only — not hosted fish, zsh, or CMD install proof.

**License.** Root `LICENSE` is MIT.
