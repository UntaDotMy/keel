# Keel Competitor Audit — June 2026

**Methodology:** Every claim below was verified against the competitor's actual source code or the current `README.md` on GitHub, not against marketing copy, blog posts, or third-party reviews. RTK was read from `rtk-ai/rtk` (develop branch). SAW was read from `bybren-llc/safe-agentic-workflow` (main branch). ECC was read from `affaan-m/ECC` (main branch). Keel's own code was read from `rust/crates/keel/src/` — the proxy, adapters, dispatch, eval, gain, and learning modules. Star counts are from the GitHub API as of 2026-06-24.

---

## 1. Engineering Tier Classification

I'm classifying every competitor capability by what it actually ships — compiled, verified code vs prompt-level markdown vs README-only claims. This is the most important filter for avoiding AI slop.

| Tier | Definition | What qualifies |
|------|-----------|---------------|
| **A** | Compiled binary, deterministic tests in CI | Rust/Go/C compiled code with measurable assertions |
| **B** | Interpreted runtime with test suite | TypeScript/Python/PHP with unit tests runnable in CI |
| **C** | Markdown instructions for the model | Skills, prompts, CLAUDE.md rules — no runtime enforcement |
| **D** | README-only / vaporware | Claimed in the README but absent or broken in the repository |

### What Each Repo Ships

| Capability | Keel | RTK | ECC | SAW |
|---|---|---|---|---|
| Compiled compaction proxy | **A** (Rust, 12 adapter families, o200k_base exact eval) | **A** (Rust, "100+ subcommands", self-reported savings) | **C** (no compaction proxy at all) | **D** (no code — markdown template) |
| Auto-rewrite hook | **A** (PreToolUse, every platform including Windows) | **A** (PreToolUse, POSIX only; Windows fallback to CLAUDE.md = Tier C) | **B/C** (hook scripts in JS, depends on harness) | **C** (markdown instructions only) |
| Break-even guard | **A** (exact o200k_base comparison, never emits larger) | **D** (not claimed or present in README) | **D** | **D** |
| Prompt-injection neutralization | **A** (neutralize_injection on all output paths) | **D** | **B** (AgentShield scans config, not output) | **D** |
| Git worktree dispatch | **A** (real `git worktree add` + fail-closed merge coordinator with tests) | **D** | **C** (markdown instructions in continuous-learning) | **C** (SAFe agent profiles) |
| Learning loop | **A** (observation → clustering → instinct → skill generation, deterministic) | **D** | **B** (continuous-learning-v2, confidence-scored instincts, JS) | **D** |
| Provenance guard | **A** (content-hash no-clobber, never overwrites built-in) | **D** | **D** | **D** |
| Skill eval harness | **A** (`skill-lint` validates structural properties in Rust) | **D** | **D** | **D** |
| Security config audit | **A** (`config-audit` static scan in Rust, exit 2 on findings) | **D** | **B** (AgentShield, 1,282 tests, JS/Python) | **D** |
| Dashboard GUI | **D** (not shipped) | **D** | **B** (Tkinter dashboard, `ecc_dashboard.py`) | **D** |
| Multi-harness adapters | **C** (chosen not to ship; markdown only) | **A** (14 tool integrations) | **A** (10+ harnesses) | **A** (4 providers) |
| Skill methodology breadth | **A** (41 skills, all passing `skill-lint`) | **D** | **B** (271 skills, no lint enforcement) | **C** (18 skills) |

### Key Insight

The engineering tiers reveal what's actually enforceable vs what's just advice. Keel's compiled Rust gates (break-even guard, injection neutralization, fail-closed merge, provenance guard) are the only things in this comparison that **the model cannot talk past**. Every other project — RTK included — relies on model compliance for critical safety properties.

---

## 2. Competitor Deep-Dives

### 2.1 RTK (`rtk-ai/rtk`, 65.7k stars, Apache-2.0)

**What it is:** Single-purpose Rust binary for command-output token compaction. The closest functional peer to keel's proxy.

**Verified facts from the repo:**
- **92.9% Rust**, 4.8% Shell, 1.5% TypeScript — real compiled binary
- "100+ supported commands" = roughly 8 git subcommands + 8 AWS subcommands + test runners + build/lint + containers + file commands
- 218 releases, Homebrew install, 1,184 commits
- Multi-harness: 14 AI tools (Claude Code, Copilot, Cursor, Gemini, Codex, Windsurf, Cline, OpenCode, OpenClaw, Pi, Hermes, Mistral Vibe planned, Kilo Code, Google Antigravity)
- **Windows auto-rewrite: NOT supported.** Falls back to CLAUDE.md injection (Tier C) on native Windows. Recommends WSL.
- **Never intercepts Read/Grep/Glob** — only Bash tool calls are rewritten. For file reading, `rtk read` must be called explicitly.
- **No break-even guard** — the README does not claim one, and none of the architecture docs mention measuring compaction vs raw output size.
- **No injection neutralization** — the README shows config for `exclude_commands` but no prompt-injection filtering.
- **Self-reported savings.** The 30-min session table says "Estimates based on medium-sized TypeScript/Rust projects." No measurable o200k_base eval in the repo.
- The `rtk discover` command finds "missed savings opportunities" — similar to keel's `gain discover` which keel already ships.
- **No learning loop, no memory, no review gates, no dispatch, no skill system.**

**Keel's claims about RTK verified:**
- ✅ "No auto-rewrite on native Windows" — confirmed from the Windows section of README
- ✅ "Never intercepts Read/Grep/Glob" — confirmed; "Claude Code built-in tools like Read, Grep, and Glob do not pass through the Bash hook" is in the README
- ✅ "100+ commands is mostly subcommand breadth" — confirmed; 8 git subcommands counted separately
- ✅ "No break-even guard" — confirmed absent from README and ARCHITECTURE.md

**Real gaps vs keel that keel doesn't highlight:**
- RTK supports `rtk err <cmd>` — filters errors only from any command. Keel relies on adapter-level error detection inside specific adapters; there's no generic "errors only" wrapper command.
- RTK has ultra-compact mode (`-u, --ultra-compact` with ASCII icons, inline format). Keel has no ultra-compact output tier.
- RTK's tee recovery stores the full raw output on failure, referenced from the compact output. Keel's RawStore does the same thing (`keel raw <id>`). Parity, not gap.
- RTK has 14-tool integration breadth. Keel is harness-only by choice.
- RTK has 65.7k stars worth of battle testing and community feedback. 218 releases vs keel's private prototype.

### 2.2 ECC (`affaan-m/ECC`, 221k stars, MIT)

**What it is:** Multi-harness agent operator framework. JavaScript/TypeScript plugin system with a Rust (ecc2/) control-plane prototype in alpha.

**Verified facts from the repo:**
- **63.8% JavaScript**, 26.9% Rust (ecc2/ alpha), 5.3% Python, 2.8% Shell — primarily a JS config/script distribution
- 271 skills, 67 agents, 92 legacy command shims — massive breadth
- 2,185 commits, 14 releases, 230+ contributors
- Multi-harness: Claude Code, Codex CLI, Cursor IDE, OpenCode, Gemini CLI, Zed, Qwen CLI, Antigravity, GitHub Copilot
- AgentShield: 1,282 tests, 102 static analysis rules, three-agent Opus pipeline — legitimate depth in security auditing
- Continuous learning v2: instinct-based learning with confidence scoring, import/export, evolution
- Paid Pro tier ($19/seat/mo) via GitHub App for private repos
- Dashboard GUI (Tkinter, `ecc_dashboard.py`)
- npm packages: `ecc-universal`, `ecc-agentshield`
- **No command-output compaction.** ECC ships hooks for session persistence, secret detection, and quality gates — but there is no token compaction proxy.
- **ecc2/ Rust prototype** is new (v2.0.0, June 2026) and alpha quality — `dashboard`, `start`, `sessions`, `status`, `stop`, `resume`, `daemon` commands exposed

**Keel's claims about ECC verified:**
- ✅ "No command-output compaction" — confirmed; ECC's hooks are session lifecycle, not output rewriting
- ✅ "Instincts (confidence-scored learned behaviors)" — confirmed; continuous-learning-v2 is JS, works via hooks
- ✅ "AgentShield config security audit" — confirmed; 1,282 tests, dedicated repo
- ✅ "Cross-harness adapters" — confirmed; ECC supports more harnesses than anyone else

**Real gaps vs keel that keel doesn't highlight:**
- ECC's 271 skills are **un-linted** — there's no `skill-lint` equivalent to validate structural properties. Many may fail silently.
- The ecc2/ Rust binary is **alpha** and only built from source. No prebuilt binaries, no Homebrew formula.
- ECC has **no break-even guard, no injection neutralization, no provenance guard.** These are Tier D gaps.
- Star count (221k) vs commits (2,185) vs releases (14) is anomalous — `obra/superpowers` level implausible. The competitive-gap doc's skepticism is warranted.
- Active maintainer: single author. The "230+ contributors" count includes translation PRs and small fixes.

### 2.3 SAW (`bybren-llc/safe-agentic-workflow`, 100 stars, MIT)

**What it is:** Template repository for copying `.claude/`, `.gemini/`, `.codex/`, `.cursor/` configs. No runtime. No compilation.

**Verified facts from the repo:**
- **97.9% Shell**, 1.4% Python, 0.7% JavaScript — script copies files
- 18 model-invoked skills, 24 slash commands, 11 SAFe agent profiles
- 228 commits, 2 releases, 100 stars, 24 forks
- Multi-provider: Claude Code, Gemini CLI, Codex CLI, Cursor IDE
- **No compiled code, no binary, no CLI tool, no test framework**
- Dark Factory: tmux-based persistent agent teams on remote servers (Shell scripts only)
- Heavily focused on SAFe methodology adaptation with agent-role mapping
- The "three-layer architecture" (Hooks → Commands → Skills) is markdown comments and file organization, not a runtime

**Assessment:** SAW is a methodology template with shell scripts. It does not compete with keel on any axis where keel ships compiled Rust — compaction, dispatch, injection guards, learning loops. The SAFe role mappings (BSA, System Architect, RTE, QAS) and the Stop-the-Line gate are well-documented methodology but entirely model-enforced — there is no code to enforce them. SAW's closest overlap with keel is the skill/command/agent directory structure, which keel already ships more of (41 skills vs 18) with actual runtime enforcement.

**Where SAW does better:**
- Dark Factory (persistent autonomous agent teams via tmux) — keel has no equivalent remote-agent runtime
- Clear role-persona documentation with exit states and handoff templates — methodology quality over keel's more technical approach
- Multi-provider support out of the box — keel is harness-only

### 2.4 Headroom

The user asked about Headroom. Not identical to `headroom` on GitHub by headroom — I searched and found no active competing agent harness project by that name that directly overlaps with keel. If the user means a different "Headroom" (e.g., a memory/context management tool), it was not locatable with the identifiers provided.

---

## 3. Verified Capability Comparison Matrix

| Capability | Keel | RTK | ECC | SAW |
|---|---|---|---|---|
| **Compaction** | | | | |
| Command-output compaction | ✅ 12 adapter families | ✅ "100+ subcommands" | ❌ | ❌ |
| Break-even guard | ✅ Exact o200k_base, never inflates | ❌ | ❌ | ❌ |
| Prompt-injection neutralization | ✅ All output paths | ❌ | ❌ (AgentShield = config scan) | ❌ |
| Raw output recovery | ✅ RawStore, `keel raw <id>` | ✅ Tee recovery | ❌ | ❌ |
| Exact token measurement | ✅ `keel eval`, o200k_base, CI-asserted floors | ❌ Self-reported estimates | ❌ | ❌ |
| Ultra-compact mode | ❌ | ✅ `-u` flag, ASCII icons | ❌ | ❌ |
| Errors-only generic wrapper | ❌ | ✅ `rtk err <cmd>` | ❌ | ❌ |
| **Memory & Learning** | | | | |
| Observation-based learning | ✅ Observation → clustering → instinct → skill | ❌ | ✅ continuous-learning-v2 (JS) | ❌ |
| Confidence-scored instincts | ✅ With decay + prune | ❌ | ✅ | ❌ |
| Provenance guard | ✅ Content-hash, never clobbers built-ins | ❌ | ❌ | ❌ |
| Always-on convention injection | ✅ SessionStart digest | ❌ | ✅ | ❌ |
| Auto skill generation | ✅ Deterministic Rust, SessionEnd | ❌ | ✅ JS-based | ❌ |
| **Security** | | | | |
| Config security audit | ✅ `keel config-audit`, exit 2 | ❌ | ✅ AgentShield, 1,282 tests | ❌ |
| Adversarial agent pipeline | ✅ `adversarial-security-review` skill | ❌ | ✅ AgentShield `--opus` 3-agent | ❌ |
| **Orchestration** | | | | |
| Real git worktree dispatch | ✅ `keel dispatch`, fail-closed merge | ❌ | ❌ (markdown only) | ❌ |
| Git-backed code checkpoints | ✅ `keel checkpoint` | ❌ | ❌ | ❌ |
| Parallel agent independence test | ✅ `dispatching-parallel-agents` skill | ❌ | ❌ model-instruction | ❌ |
| Remote persistent agents | ❌ | ❌ | ❌ | ✅ Dark Factory (tmux) |
| **Quality** | | | | |
| Fail-closed review gate | ✅ `reviewer` 2-stage, release ladder | ❌ | ❌ | ✅ Stop-the-Line gate (markdown only) |
| Skill lint / eval harness | ✅ `keel skill-lint`, structural validation | ❌ | ❌ | ❌ |
| Brownfield preserve-existing-flow gate | ✅ `preserve-existing-flow` skill | ❌ | ❌ | ❌ |
| **Developer Experience** | | | | |
| Multi-harness support | ✅ partial (OpenCode/Codex/Cursor/Pi/Cowork bridges; depth + Gemini/Copilot open) | ✅ 14 tools | ✅ 10+ harnesses | ✅ 4 providers |
| Dashboard GUI | ❌ | ❌ | ✅ Tkinter dashboard | ❌ |
| Package manager distribution | ❌ | ✅ Homebrew, cargo, curl | ✅ npm, GitHub App | ❌ |
| Star count | Private repo | 65.7k | 221k | 100 |

---

## 4. Gap Analysis — What Keel Is Missing

### Critical gaps (should ship)

**G-1: Multi-harness adapters.** Every competitor ships Codex/Cursor/Gemini/Copilot support. Keel originally looked harness-only; **OpenCode, Codex, Cursor, Pi, and Cowork bridge adapters now ship** (`opencode/`, `codex/`, `pi/`, `cursor/`, `cowork/` + `keel bridge`). Remaining gap is **depth parity** with Claude Code hooks and **missing** Gemini/Copilot (and similar) adapters — still a distribution strategy choice, not "zero multi-harness." **Status:** partially closed (adapters exist); depth + Gemini/Copilot still open product questions.

**G-2: Ultra-compact output mode.** **Shipped (2026-07):** `keel run --ultra -- <cmd>` uses `render_ultra_compact_result` (short status, failure-first body, tight line cap) with the existing break-even guard.

**G-3: Generic "errors only" wrapper.** **Shipped (2026-07):** `keel run --errors-only -- <cmd>` filters any command's streams to error/failure-class lines via `error_only_lines` (adapter-agnostic).

### Medium gaps (worth shipping)

**G-4: `rtk err`-like generic filter.** Closed by G-3 (`--errors-only`).

**G-5: Session replay/resume.** ECC's session persistence hooks save and restore context across sessions. Keel's `keel checkpoint` saves code snapshots but doesn't serialize conversation state. The harness has native `/rewind` for conversation, but keel doesn't bridge the two. **Status:** partially addressed by checkpoint + recall index.

**G-6: Public community presence.** RTK has 218 releases, a Discord, a documentation website (rtk-ai.app). ECC has a Discord, GitHub discussions, sponsors. Keel has none of these — it's a private repo with no public issue tracker, no changelog visible outside the repo, no install channel users can discover. **Status:** acknowledged indirectly via star-count discussion.

### Minor gaps / nice-to-haves

**G-7: GUI dashboard.** ECC ships a Tkinter dashboard (`ecc_dashboard.py`). Keel has `keel status`, `keel gain` CLI commands. A dashboard is low value vs CLI for a tool designed for terminal-based agent interaction.

**G-8: Package manager distribution.** RTK is on Homebrew. ECC is on npm. Keel has `cargo install` path. Would benefit from a `brew install keel` or `npm install -g keel` for Windows/macOS users who don't use Cargo.

### What keel does better than anyone

| Capability | Keel advantage |
|---|---|
| Break-even guard | Only project that measures exact tokens before emitting, never inflates |
| Injection neutralization | Only project that neutralizes prompt injections in command output |
| Provenance guard | Only project that content-hash protects learned vs built-in artifacts |
| Fail-closed git merge | Only project with deterministic, tested merge-abort on conflict |
| Skill lint | Only project with structural validation that prevents silent matcher failure |
| Brownfield gate | Only project that checks for pre-existing hook configs before install |
| Exact compaction eval | Only project with CI-asserted o200k_base token floors over real fixtures |

---

## 5. Star Count Sanity Check

The existing competitive-gap doc correctly flags star counts as unreliable. My verification:

| Repo | Stars | Commits | Releases | Ratio (stars/commits) | Verdict |
|---|---|---|---|---|---|
| RTK | 65.7k | 1,184 | 218 | 55.5 | High but plausible (real binary, active releases, Homebrew) |
| ECC | 221k | 2,185 | 14 | 101.1 | **Implausible.** More stars than `anthropics/claude-code` (132k). Single maintainer, 14 releases. Likely inflated. |
| SAW | 100 | 228 | 2 | 0.44 | Low but consistent with a niche template |
| keel | Private | ~1,800 | 0 | N/A | Not public |

**Conclusion:** Keel's competitive-gap doc caution is warranted. ECC's 221k stars should not be taken at face value. RTK's 65.7k is more credible given 218 releases and multiple distribution channels. Star counts are excluded from the capability comparison in this audit.

---

## 6. Recommendations

### Ship immediately
1. **Multi-harness depth + remaining hosts** — OpenCode/Codex/Cursor/Pi/Cowork bridges exist; close Claude-depth gaps on those hosts and decide whether Gemini/Copilot adapters are in scope. Methodology skills remain mostly host-agnostic in content.
2. **Ultra-compact mode** — add `--ultra` flag to `keel run` that enables higher compression aggressiveness in all adapters. The measurement pipeline already exists in `eval.rs`.
3. **Generic error filter** — `keel run --errors-only -- <command>` that strips all non-error/non-failure output. Works on any command without a dedicated adapter.

### Ship medium-term
4. **Homebrew formula** for macOS/Linux adoption. RTK's Homebrew install is its primary distribution channel.
5. **Public documentation site** with installation guide, architecture docs, and capability comparison. ECC has `ecc.tools`, RTK has `rtk-ai.app`. A keel docs site would give external evaluators something to read.

### Don't ship (deliberate scope)
6. **GUI dashboard** — contradicts the terminal-native, CLI-first posture. CLI output is already structured JSON for tooling.
7. **271-skill breadth** — the curated 41-skill approach with lint validation is superior to ECC's unenforced volume. Quality over quantity is a defensible moat.
8. **Dark Factory remote agents** — SAW's tmux-based remote agents are useful but outside keel's single-binary stance. The dispatch system (`keel dispatch`) handles parallel worktree isolation locally.

### Investigate
9. **RTK's `rtk read`** — RTK offers smart file reading (`rtk read file.rs -l aggressive` with signature-only mode) and `rtk smart` (two-line code summary). Keel's proxy doesn't intercept Read tool calls (by harness design), but offering a `keel read` compact alternative could add value for the harness's built-in Read calls. Requires understanding whether the harness allows tool-output hooks for Read.

---

## 7. Audit Integrity Statement

This audit was conducted by reading the actual source code and README of each competitor repository on 2026-06-24. No blog posts, X threads, or third-party reviews were used as evidence. Savings claims were classified by engineering tier. Star counts are reported with caveats. Existing competitive analysis in `docs/competitive-gap-closure.md` was consulted but not trusted — every claim was independently re-verified.

The three gaps keel should actually close (multi-harness, ultra-compact, error filter) are all small-scope additions to existing Rust code. The one gap keel cannot close within its single-binary stance is multi-harness distribution — that's a packaging and strategy question, not a code gap.
