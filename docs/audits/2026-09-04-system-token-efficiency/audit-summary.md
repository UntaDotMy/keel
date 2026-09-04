# Keel system wiring and token-efficiency audit

Date: 2026-09-04

Scope: current source, installed Windows runtime, all host adapters, the 37-tool MCP catalog, lifecycle hooks, Anvil, memory, learning, skills, code intelligence, review/delivery, and token accounting.

## Outcome

The reported Antigravity failure was not an MCP registration failure. Antigravity had discovered all 37 tools and the installed server completed `initialize` and `tools/list`. Its installed hook adapter was an older Keel-generated file that did not translate Antigravity's generic `call_mcp_tool` payload to `mcp__keel__<ToolName>`. The installer classified every differing adapter as user-customized, so reinstalling the latest release preserved the broken generated file.

The adapter, installer upgrade path and executable priority, host skill resources, Pi/OMP lifecycle seam, prompt overhead, eval isolation, token accounting, workspace identity, learning signal quality, review marker, and strict closeout Flow refresh were repaired. Requested live hosts now report wired: OpenCode, Pi, Codex, Cursor, Grok, OMP, ZCode, and Antigravity.

Antigravity 2.12.0 and an older 2.5.5 "Antigravity IDE" are both installed on this machine. The current 2.12.0 configuration launches the repaired `C:\Users\Administrator\.keel\keel.exe`; an already-open process must be restarted to reload the updated plugin and MCP server. Neither application was removed.

## Confirmed findings and remediation

| ID | Severity | Surface | Root cause | Remediation | Evidence |
|---|---:|---|---|---|---|
| F1 | High | Antigravity MCP gate | Generic `call_mcp_tool` remained generic, so successful Keel research was not recognized by the research gate. | Translate `ServerName=keel` and `ToolName=X` to `mcp__keel__X`; added real pre/post/edit lifecycle regression. | Live adapter sequence allows MCP pre-call, records post-call, then allows edit; adapter tests 9/9. |
| F2 | High | Cross-host updates | `copy_managed_file` preserved every changed file, including old Keel-generated adapters. Fixes could not reach installed hosts. | Added explicit `keel:managed-host-file` ownership, legacy Keel-adapter migration, and opt-out by removing the marker. Unmarked custom files remain preserved. | Installer and platform tests prove legacy upgrade, custom preservation, and uninstall ownership. |
| F3 | High | Pi and OMP lifecycle | The extension used stale `input`/`message_start` events, discarded lifecycle context, combined pre/post compaction after compaction, ignored `isError`, and passed `5000` as stdin instead of timeout. | Use `before_agent_start` system-prompt injection, split compact events, cache post-compact context for the next turn, handle `isError`, and pass timeout in the correct argument slot. | Pi/OMP host contract passes and bundled extension loads under Node. |
| F4 | High | Context cost | UserPromptSubmit injected the full operating contract, workspace digest, and full matched skill body on every prompt. | Emit compact contract pointers; keep the workspace digest at SessionStart; route skills by name without inlining their bodies. | Exact `o200k_base`: base prompt 1,754 -> 237 tokens; code-change prompt now 288. |
| F5 | Medium | Reducer evaluation | `keel eval` reused the project adapter registry, so a repository `cargo test` project filter replaced embedded fixtures and caused a panic. | Evaluate fixtures with a hermetic builtin registry. | 7 fixtures complete: 2,218 raw, 1,613 compact, 605 saved, 27.28%. |
| F6 | Medium | Savings analytics | `gain`, `stats`, and `session` called gross output reduction “tokens saved” while omitting wrapper overhead. | Preserve legacy gross fields, add gross/overhead/net fields, label text honestly, and disclose excluded hook/MCP context. | 30-day totals: 729,482 gross, 1,953 overhead, 727,529 net, 90.77% net. |
| F7 | Low | Code graph impact | The current graph returned zero impacted nodes for cross-host adapter/installer changes. | Treat graph impact as advisory; retain explicit owner tracing, sibling scans, installer tests, and host E2E tests as gates. | `code-graph impact` returned `impactedCount: 0` while direct tests proved cross-host consumers. |
| F8 | High | Parallel code intelligence | Concurrent search calls both refreshed the shared index with deferred transactions; initialization and later writes could each fail `database is locked`. | Retry WAL initialization under a short bounded timeout, then apply a 10-second transaction timeout and acquire refresh writes with an immediate transaction. | Eight-writer regression passes in five consecutive runs; parallel live sibling scans also pass. |
| F9 | High | Skills on non-Claude hosts | The gateway copied only `using-keel/SKILL.md`; its referenced files were absent, and first-party links used `_shared/...` even though Agent Skills resolves links relative to the skill directory. | Recursively install gateway references and shared resources, mark owned files for safe upgrades, and change first-party links to `../_shared/...`. | Source parity rejects broken relative links; live Agent, Pi, OMP, ZCode, and Antigravity skill roots contain the skill, both references, and shared resources. |
| F10 | High | Workspace memory and Flow | Path hashing varied by caller and Windows 8.3 versus long paths, creating separate Flow, index, Anvil, and SYSTEM_MAP lanes for one checkout. | Canonicalize once, expand Windows long paths, use one bounded hashed key, and copy legacy state into the canonical lane without deleting the source. | Scope, index, and Flow resolve to `c-users-administrator-desktop-dev-studytime-keel-keel-811612c5`; short/long-path and migration regressions pass. |
| F11 | High | Learning quality | Shell scaffolding, assignments, URLs, and routine Git/status commands could become instincts and template skills. | Extract real package-runner targets, reject control/noise signatures before clustering and retention, and roll back untouched generated templates whose evidence becomes invalid. | Noise and rollback regressions pass; live learning reports 250 observations, no qualifying noise signals, and zero installed learned skills. |
| F12 | High | Review and stop gates | Passing formatting-oriented `pre-commit` or arbitrary gates cleared the required reviewer marker; repeated Antigravity Stop calls could re-block. | Only successful `pre-pr` or strict `closeout` clears review state; honor Antigravity execution count and Claude-compatible `stop_hook_active`. | Review-marker and active-stop regressions pass; an installed second Stop invocation returns `allow`, preventing a loop. |
| F13 | Medium | Flow status | `flow check` hard-coded `finalized` to whether the current command was `finish`, so an immediate check falsely reported a finalized artifact as unfinished. | Derive `finalized` from persisted evidence and add `current` from the live HEAD/diff fingerprint. | Regression covers current and stale artifacts; the live finish followed by check reports both fields true. |
| F14 | High | Installed executable | When the installed binary already matched the highest-priority release artifact, publication skipped it and copied the first lower-priority differing debug artifact. | Select the first existing artifact by strict priority, then decide whether publication is needed; never fall through because the preferred artifact is already installed. | Eight focused publication tests pass, including identical-release retention and true debug fallback; repeated live install preserves release identity. |
| F15 | High | Strict review closeout | Automatic closeout evidence ran `flow start` for one changed source and evaluated the Flow gate before `flow finish`, so strict closeout always produced incomplete evidence. | Build the complete changed-source target set and finish the generated artifact before collecting review gates. | Regression protects multi-file targets and the finish command; strict closeout passes with zero unresolved findings or requirements. |
| F16 | High | Codex plugin installation | Keel copied a source bundle and enabled its marketplace key but used an invalid `~` source path, never ran Codex's install step, and doctor treated source presence as runtime wiring. Hooks therefore remained absent from ChatGPT Desktop and Codex while doctor reported success. | Use Codex's required `./` local path, run the idempotent host-native plugin install, verify the versioned cache byte-for-byte, and remove that cache on uninstall. | Source-only regression warns; 25 focused Codex tests pass; live `codex plugin list` reports `keel@personal-keel` installed and enabled; doctor reports the plugin installed and current. Hook trust remains an explicit user decision. |

## Token ledger

All counts use the exact `o200k_base` tokenizer.

| Cost or saving | Before | Current | Interpretation |
|---|---:|---:|---|
| UserPromptSubmit, simple prompt | 1,754 | 237 | 86.5% lower recurring prompt injection. |
| UserPromptSubmit, code-change prompt | 1,754-class full injection | 288 | Skill pointer and work reminder without skill-body duplication. |
| SessionStart context | 1,534 | 996 | 35.1% lower one-time lifecycle context, including workspace digest. |
| MCP catalog | 2,885 | 2,885 | Fixed catalog cost for 37 tools; not counted as command savings. |
| 30-day command output | 801,531 raw | 74,002 delivered | 729,482 gross reduction; 1,953 overhead; 727,529 net reduction (90.77%). |
| Deterministic reducer fixtures | 2,218 raw | 1,613 delivered | 605 saved (27.28%); failure output remains intentionally less aggressive than passing output. |

The MCP catalog remains the largest fixed context cost. Hiding tools would break direct feature discovery, so this audit does not silently remove tools. A future opt-in gateway/catalog profile should be benchmarked against task success before changing the default.

## SDLC feature verification matrix

| SDLC phase | Feature families | Runtime evidence | Status / critique |
|---|---|---|---|
| Align | `system_map`, `context_brief`, working briefs, scope resolution | Current index: 449 files, 3,560 symbols, 4,009 chunks, 12,758 edges; active brief retained. Two unchanged refreshes preserve the SYSTEM_MAP hash and modification time. | Verified after F10. Prompt pointers avoid repeated full-map injection. |
| Research | `recall`, `recall_status`, code search/index/graph, research cache | Memory family status is readable; concurrent and final sibling scans pass. | Verified after F8. Graph impact remains advisory because F7 returned a false-empty result. |
| Plan / impact | Anvil compile/cast, flow ownership trace, code graph | Anvil compiled and dry-ran before edits; the finalized flow artifact covers every branch-level established-source owner. | Verified for this change. Strict stamping is not applicable because the required dry-run produced no survivors; it failed closed rather than inventing a score. |
| Implement | Host-neutral bridge, PreToolUse gate, rewrite, host adapters | Adapter E2E and host installer tests pass; live Antigravity gate path passes. | Verified after F1-F3. |
| Test | `run`, reducers, raw/replay, eval, skill eval | Hermetic eval 7/7 completes; skill routing 17/17. | Verified after F5. |
| Verify | doctor, validate, config audit, platform tests | Doctor launches MCP; config audit: 0 high/medium/low findings. | Verified for requested live hosts; final full suite recorded below. |
| Review | review diff/pre-commit/pre-pr/closeout and gates | `review diff` and the branch-wide gates both pass with no findings, warnings, or blocking failures. | Verified; the pre-PR gate passes cleanly. |
| Deliver | git preflight, commit/PR generation, CI wait | Delivery surfaces are implemented and locally gated. | Hosted results are reported from GitHub after publication rather than inferred locally. |
| Learn | observe, learn status/dry-run/run, instincts, generated skills | 250 observations, no qualifying signals, 14 retained historical instincts, zero installed learned skills; continuous PostToolUse + SessionEnd enabled. | Operational after F11. Generated artifacts remain provenance-marked and user edits are protected. |
| Measure | gain, telemetry, stats, session | Gross, overhead, net, and fixed context costs are separate. | Verified after F6; session-level negative net is visible for tiny non-compacted sessions. |

## Host matrix

| Host | Rules/skills | Lifecycle bridge | MCP | Live status | Evidence / limitation |
|---|---|---|---|---|---|
| Claude Code | Yes | Native hooks | Yes | Wired | Primary lifecycle owner; doctor launches MCP. |
| OpenCode | Yes | Plugin | Yes | Wired | Explicitly installed; plugin and bridge contract tested. |
| Codex | AGENTS + plugin | Plugin hooks | Native MCP | Wired | Marketplace source, installed runtime cache, enablement, and native MCP are current. New or changed hooks remain inactive until the user reviews and trusts them. |
| Cursor | Rules | Native hook adapter | Yes | Wired | Installer/platform tests; managed script migrated. |
| Pi | AGENTS + skill | Extension | Yes | Wired | Current `before_agent_start` seam and exact source hash installed. |
| Oh My Pi | AGENTS + skill | Pi-compatible extension at OMP root | Yes | Wired | Stale extension upgraded after F2/F3. |
| ZCode | AGENTS + skill | Native configured hooks | Yes | Wired | Config merge retains unrelated keys and installs lifecycle events. |
| Grok | Shared gateway | Claude-compatible hooks or native fallback | Native MCP | Wired | Exactly one hook source; Windows-compatible invocation covered. |
| Antigravity IDE | GEMINI + plugin skill/rule | CamelCase hook adapter | Global + plugin declaration | Wired | Installed release hash matches source; direct initialize, 37-tool list, SYSTEM_MAP call, skill routing, context injection, and stop-loop proof pass. Restart/reload is needed for an already-open IDE process. |
| Command Code | Source support | Mod | Host configuration | Not live-installed | Contract and installer tests cover it; binary/config not detected on this machine. |
| Cowork | Guidance | No public hook API | MCP only | Not live-installed | Deliberately MCP-only; no lifecycle-hook claim. |

## MCP catalog (37 tools)

- Orientation and memory: `context_brief`, `system_map`, `system_map_refresh`, `recall`, `recall_status`, `memory_status`, `memory`, `brief_list`, `brief_get`, `brief_create`.
- Execution and recovery: `run_command`, `command_output`, `command_kill`, `rewrite`, `raw`, `cli`.
- Skills and learning: `skill_route`, `skill_get`, `skill_list`, `skill_lint`, `skill_eval`, `learn`, `observe`.
- Code intelligence: `code_search`, `code_index`, `code_graph`, `flow`, `design_intelligence`.
- Delivery and health: `anvil`, `review`, `git_workflow`, `doctor`, `config_audit`.
- Measurement: `gain`, `telemetry`, `session`, `stats`.

The installed server negotiated MCP protocol `2025-11-25`, identified itself as `keel` version `0.1.0`, returned all 37 tools, and included `system_map`, `recall`, and `run_command`.

## Validation ledger

| Check | Result |
|---|---|
| Baseline workspace suite before edits | 1,202 tests passed. |
| Hook lifecycle focused suite | 111 passed. |
| Installer unit suite | 115 passed before final OpenCode regression; focused OpenCode migration test passed afterward. |
| Antigravity platform tests | 3 passed. |
| Host adapter contracts | 9 passed, 148 assertions. |
| Gain / stats / session focused tests | 9 / 6 / 17 passed. |
| Eval focused tests and live CLI | 6 passed; CLI completed all 7 fixtures. |
| Skill routing eval | 17 passed. |
| Skill lint | 51 passed, average score 100, no warnings. |
| Config audit | 0 findings. |
| Live installed MCP | initialize + tools/list passed, 37 tools. |
| Live Antigravity adapter | research pre-call allow -> successful post observation -> edit pre-call allow. |
| Full workspace suite before the final concurrency fix | 1,301 tests passed. |
| Concurrent workspace-index regression | Passed; three parallel live sibling scans also passed. |
| Final post-fix workspace suite | 1,320 tests passed. |
| Format, lint, whitespace | `cargo fmt --all -- --check`, workspace Clippy with warnings denied, and `git diff --check` passed. |
| Reviewer gates | `review diff` and branch-wide gates pass; strict closeout reports zero unresolved findings and requirements. The reviewed exact-ID static baseline expires 2026-12-04 and cannot suppress dynamic or changed-line findings. |
| Installed release identity | Release and installed executable SHA-256 hashes match exactly. |
| Executable publication priority | Eight focused tests pass; an identical preferred release is retained instead of being replaced by a debug build. |
| Installed host assets | Antigravity source and installed adapter hashes match; every requested host reports wired. |
| SYSTEM_MAP idempotence | Two unchanged refreshes returned `systemMapChanged: false`, with identical content hash and modification time. |
| Flow status accuracy | Live `finish` and immediate `check` both report `finalized: true` and `current: true`; a tracked edit regression reports `current: false`. |
| Live installed Antigravity MCP | Protocol `2025-11-25`, 37 tools, all eight critical tools present, and `system_map` plus `skill_route` calls succeeded. |
| Anvil | Compile and required dry-run passed (`pieces=1`, `casts=3`, `gates=1`, no writes or executes); strict stamp correctly rejected the empty survivor set. |

## Current-source documentation used

- Antigravity MCP configuration: <https://codelabs.developers.google.com/gemini-mcp-agy>
- Antigravity plugins: <https://antigravity.google/docs/cli/plugins/>
- Antigravity hooks: <https://antigravity.google/docs/hooks>
- Pi extensions: <https://pi.dev/docs/latest/extensions>
- ZCode hooks: <https://zcode.z.ai/en/docs/hooks>
- OpenCode skills: <https://opencode.ai/docs/skills>
- OMP extension authoring: <https://github.com/can1357/oh-my-pi/blob/main/docs/skills/authoring-extensions.md>
- Cursor hooks: <https://prod.cursor.com/docs/agent/hooks>
- Grok skills/plugins: <https://docs.x.ai/build/features/skills-plugins-marketplaces>
- Codex hooks: <https://learn.chatgpt.com/docs/hooks.md>
- Agent Skills specification: <https://github.com/agentskills/agentskills/blob/main/docs/specification.mdx>
- RTK source and current behavior: <https://github.com/rtk-ai/rtk>
- Reflexion: <https://arxiv.org/abs/2303.11366>
- MemGPT: <https://arxiv.org/abs/2310.08560>
- Generative Agents: <https://arxiv.org/abs/2304.03442>
- WCAG 2.2: <https://www.w3.org/TR/WCAG22/>
