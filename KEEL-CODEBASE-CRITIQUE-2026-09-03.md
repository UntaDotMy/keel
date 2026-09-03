<!--
Purpose: Independent deep critique of the keel codebase for maintainer review.
Caller: Repository maintainers prioritizing remediation work.
Dependencies: Source tree at commit 3043023 (main), five subsystem audit passes plus spot verification.
Main Functions: Record verified blockers, security findings, scalability limits, drift, and strengths with file:line evidence.
Side Effects: None - informational document, not wired into gates.
-->

# Keel Codebase Critique (2026-09-03)

- Reviewed commit: `3043023` (main)
- Scope: full workspace, 5 crates, ~87K LOC Rust, 1,283 test functions, 490 commits, plus the 271-file markdown corpus
- Method: five parallel subsystem audits (proxy/adapters, hooks/runner, memory/index, manager/review/MCP, docs/corpus), followed by direct spot verification of every blocker claim in source
- Compile check: `cargo check --workspace --all-targets` clean, zero warnings, 7.4s

## Executive summary

Keel is an unusually disciplined, well-engineered codebase whose enforcement machinery has holes exactly where an adversarial agent would probe, and whose cross-host TypeScript adapters contain drift and dead paths that the recent "harden hooks" commit (53f5837) did not close, including two verified correctness blockers. The discipline layer (review gates, closeout) is partially self-attested rather than mechanically enforced, and three workspace-slug schemes plus four near-copies of doctrine prose have already drifted apart.

Three themes run through every finding:

1. **Enforcement honesty.** Several gates are satisfiable by the policed agent itself (self-attested proofs, marker mtimes, exit-0 closeout). The system enforces by convention in places where it claims enforcement by mechanism.
2. **Divergent copies.** Four TS bridge adapters, three slug schemes, and 9+ restatements of the same doctrine rule have drifted. No cross-implementation contract test pins the bridge protocol against its consumers.
3. **Scalability claims not yet earned.** `code-search` re-reads the whole workspace per query; memory writes re-read the whole corpus. Fine today, wrong at the documented 20K-file target.

## Verified blockers

Each of these was confirmed by reading the source directly, not just reported by audit.

### B1. Pi adapter mints a new session identity per event

- `pi/keel-pi.ts:164-174`
- `resolveSessionId()` returns a fresh `crypto.randomUUID()` whenever the host omits a session id, and it is called independently inside `handleSessionStart`, `handleUserPrompt`, `handleToolCall`, and `handleToolExecutionEnd`.
- The inline comment says the old PID fallback "causes session collisions"; the replacement is stable-per-nothing. Every event becomes a phantom session: the edit gate's evidence can never be recorded under the same key, so the gate denies forever, session-start runs repeatedly, and observations scatter.
- Fix: cache the resolved id in module state on first resolution, or derive it once from the Pi session object.

### B2. Codex stdin reassembly corrupts payloads over 64 KiB (fail-open)

- `codex/keel-codex.ts:350-358`
- `chunks.push(buf.subarray(0, n))` pushes views into the single reused 64 KiB buffer, not copies. Once the payload exceeds one read iteration, every earlier chunk aliases the final buffer contents, `JSON.parse` fails, and `main()` exits silently.
- Consequence: any PreToolUse payload with a large `tool_input` skips the gate entirely. That is a trivial bypass, not just a bug.
- Secondary issue on the same lines: on some Node versions on Windows, `fs.readSync` at EOF on pipes throws rather than returning 0; the catch discards all previously read bytes, silently no-oping the whole adapter.
- Fix: copy each chunk (`Buffer.from(buf.subarray(0, n))`) and handle EOF per-read, or switch to async stdin streams.

### B3. Pi rewrite call is dead on arrival

- `pi/keel-pi.ts:305-310` and dead helper at `pi/keel-pi.ts:152-157`
- Calls `bridge rewrite` with `--command`, which is not a registered flag (`runner/bridge.rs:425-430`); `FlagSet::parse` errors on unknown flags, so the call exits 2 with empty output and no rewrite happens. Separately, `500` (intended as a timeout) is passed into the `stdinInput` parameter.
- Root cause across B1-B3: no test pins the bridge protocol (flags, JSON shapes, key sanitization) against the TS consumers. Add a cross-implementation contract test.
- Fix: pass the command on stdin the way codex/opencode do, delete the dead helper, add the contract test.

## Security findings

### MCP HTTP transport

- **`Origin: null` accepted** (`mcp/http.rs:534`). Combined with no `Content-Type` validation in `handle_post` (`mcp/http.rs:626-698`) and `MCP-Session-Id` validated only when present (`http.rs:652-667`), a drive-by page can POST `tools/call` against the default loopback bind and execute side effects even though the response is unreadable cross-origin.
  - Fix: reject `Origin: null`, require `application/json`, require a valid session id or a per-boot token echoed by clients for `tools/call`.
- **Run-command confirm gate bypassable by argv spelling** (`mcp/tools.rs:1115-1184`). `command_base_name` exact-matches a fixed basename list; `argv:["env","bash","-c",...]`, `python3.11`, `nodejs` all skip `command_requires_confirmation`. The gate's adversary is the agent that crafts the argv, so exact-match is weak.
  - Fix: resolve argv[0] through PATH, classify interpreter + `-c`/script pairs as shell-form, and fail closed (require confirm) on unrecognized programs.
- **Unbounded sessions set** (`mcp/http.rs:711-718`): grows on every `initialize`, shrinks only on explicit DELETE. Bound or expire sessions.

### Proxy / output compaction

- **Injection guard is a five-pattern, prefix-anchored blocklist** (`proxy/injection_guard.rs:97-107, 151-162`). One leading non-whitespace character defeats it (its own test proves quoting defeats it, lines 274-287). Missing pattern classes: "ignore previous instructions", fake tool-result blocks, `<system>`/`<goal>` XML, `Assistant:`/`Human:` turns.
- **The neutralization marker hands over the recovery key** (`proxy/run.rs:50-52`). The agent-visible marker says raw content "is available via `keel raw <id>`", and `keel raw` prints raw stdout/stderr verbatim, unredacted. Injected content needs only to say "run keel raw <id>" to complete the round trip into agent context.
  - Fix: put the recovery pointer on human-facing stderr only, or neutralize/redact `keel raw` output.
- **Secret redaction skipped on short passthroughs** (`adapters/common.rs:51-64`, `adapters/generic.rs:109-111`). `compact_edges` returns output unchanged when under the line cap, so `cat .env` with fewer than 80 lines reaches the agent unredacted. The cloud and database adapters redact the small path; the other seven do not.
- **Test-failure adapter redacts nothing** (`adapters/tests.rs:96-118`). Test failures (assert payloads, env dumps, stack traces) are the most common real-world leak path.
- **Shared redactor pattern gaps** (`adapters/common.rs:134-179`): misses `Authorization: Bearer` headers, JSON-style `"api_key": "..."` (needles require `KEY=`), bare AWS access-key IDs (AKIA is 20 chars; the long-token rule needs 32+), and unprefixed `postgres://user:pass@host` connection strings outside the database adapter.
- **Event log persists unredacted command lines** (`proxy/event_log.rs:81-121`): `meta.command` includes env-prefixed secrets (`GITHUB_TOKEN=ghp_... cargo ...`) and is written to the claude home with default permissions (raw store files get 0600; the event log does not).
- **Windows cmd.exe fallback breaks quoting** (`proxy/run.rs:525-527` vs `runtime.rs:1298-1305`). `shell_join` always emits PowerShell single-quote semantics on Windows, but the platform layer falls back to `cmd /C` when PowerShell is absent. In cmd, `'` is literal, so an arg containing `& del /q *` escapes and executes.
  - Fix: detect the resolved shell and quote for it, or refuse the cmd fallback for metacharacter-bearing args.
- **Streaming path bypasses the timeout contract** (`proxy/run.rs:570-687`): `run_command_streaming_proxy` spawns with no deadline and no process-tree ownership, unlike the capture path (`runtime.rs:694-771`). A hung child or grandchild blocks the proxy forever. Also leaks multi-line injection blocks when neutralizing per-chunk (`run.rs:654-659`).
- **Registry ordering makes project filters dead** (`proxy/adapters.rs:20-37`, `proxy/registry.rs:19-26`): builtins are matched before loaded project filter adapters, so a project filter for `cargo test` can never fire. Register project filters first (generic last).

## Enforcement honesty (review gates and metrics)

- **Closeout exits 0 on "blocked"** (`review/closeout.rs:1571-1575`): the nonzero exit requires `--strict`. Any agent or wrapper keying on exit code gets a green closeout with unresolved findings.
  - Fix: exit 1 on "blocked" by default; let `--strict` add requirements rather than gate the exit code.
- **Acceptance proofs are self-attested** (`closeout.rs:1246-1282, 1517-1529`): `--proof "req=cargo test passed"` closes a requirement; nothing re-runs the command or checks output. Re-run single-command proofs or label the field as attestation.
- **Completeness gate is marker-mtime theater** (`review/diff_gates.rs:397-409`, `runner/hook_lifecycle/post_batch.rs:214-232`): writing `<home>/state/completeness-gate/<workspace>.scanned` (agent-writable) satisfies the blocking gate regardless of scan results. The HEAD + diff fingerprint binding in closeout (`closeout.rs:310-333`), by contrast, is real enforcement.
- **Stop hook emits uncapped `decision: "block"`** (`post_batch.rs:1161-1232`): three gates default to enforcing, and `run_hook_stop` never reads or increments the gate counters. If any clearing action is impossible (unwritable state dir, broken binary mid-session), the agent loops stop-continue forever. This contradicts the termination guarantees documented and proven for the PostToolBatch path (`post_batch.rs:636-718`).
- **Gate counters are non-atomic read-modify-write** (`state.rs:155-170`): concurrent hooks lose increments; a Windows sharing violation silently resets the count to 0 via `unwrap_or(0)`.
- **`gain` counts passthrough as savings** (`utility/gain.rs:457-467`): `after` defaults to 0 when omitted, so uncompacted events contribute their full size to `tokens_saved`. This contradicts `gain discover` (lines 319-395), which counts the same events as missed savings. A tool whose brand is honesty should not inflate its own metric.
- **Gate fail-closed wedges the agent** (codex/keel-codex.ts:247-276 and siblings): when gate status is `unknown` (binary missing, timeout), every edit and shell call is denied, including the suggested remedy `keel doctor`, which is itself a shell call. Allow allowlisted keel commands through the unknown path.

## Scalability and concurrency (memory / index)

- **Every `code-search` re-reads the whole workspace** (`utility/workspace_index.rs:403, 966-1028`): `search_filtered` unconditionally calls `refresh()`; change detection requires the content hash, so every file is fully read even when unchanged, then `DELETE FROM edges` + full rebuild runs inside one transaction even with no changes (lines 268-347). At the documented 20,000-file target this is hundreds of MB of IO per query while holding the WAL write lock.
  - Fix: store mtime+size, stat first, read only on metadata change; rebuild edges only when symbols changed.
- **One unreadable file aborts the entire refresh** (`workspace_index.rs:1002-1003`): a UTF-16 file, an editor-locked file, or a long path kills every subsequent search. `recall.rs:1049-1059` already implements the right skip-and-count behavior; unify.
- **Concurrent searches hard-fail on lock contention** (`workspace_index.rs:654-664`): busy_timeout 5s then `database is locked` propagates as exit 1. Recall has lock-contention degradation (`recall.rs:202-217, 763-766`); port it.
- **Every memory write re-reads the whole memory corpus** (`utility/recall.rs:1027-1045` via `reindex_after_write`): freshness checks re-read full file bytes even when mtime+size match. O(writes x corpus) in an agent loop that writes many records.
- **Three workspace-slug schemes diverge**: `system_map.rs:26-42` (bounded slug + hash suffix for long paths) vs `system_map_cmd.rs:61` (unbounded sanitize) vs `code_graph.rs:84` (another variant). For long Windows paths the SYSTEM_MAP lane and the code-index lane silently split. Short/unicode slugs can collide (`system_map.rs:6-20`), merging distinct workspaces' memory with no error.
  - Fix: one canonical `workspace_key()` used by all three lanes; always append the hash suffix.
- **Hardcoded personal skip lists** (`workspace_index.rs:1330-1334`, `code_graph.rs:940-945`): `"hermes-agent" | "karpathy-skills-cmp" | "agent-tools" | "terminals" | "mcps"` are excluded from indexing for every user, silently. Move to config (`.keelignore` / filters).
- **Windows long paths handled for the DB but not the walk** (`sqlite.rs:44-61` vs `workspace_index.rs:966-993`): the win32-longpath VFS is applied to sqlite, but `fs::read_to_string` uses plain paths, so deep trees fail (triggering the abort above).
- **Nondeterministic MAX_FILES cap** (`workspace_index.rs:966-993`): the cap breaks the inner loop but the walk continues; the sort runs after the cap, so which 20K files get indexed depends on readdir order, and the report has no "truncated" field.
- **Loop guards fail open** (`memory_families.rs:93, 114-121, 870-876`): a locked/corrupt record resets the anti-loop counter to 0 exactly under the contention multi-agent runs create. Distinguish NotFound from real errors; fail closed.
- **Unbounded growth**: record stores (research cache, instincts, graph edges, agent packets) accumulate forever; `maintenance trim` covers only working-buffer.md. Add a retention sweep keyed on recordedAt.
- **Code graph materializes the whole workspace in RAM** (`code_graph.rs:376`, `workspace_index.rs:96-74`): stream-hash instead of concatenating every file's text for the fingerprint.

## Cross-host adapter drift

- **Sanitizer drift hides Rust-written markers from TS readers**: TS `sanitizeSessionKey` (`_shared/ts/bridge-core.ts:63-66`) replaces all non-ASCII with `-`; Rust's sanitizer keeps it. A session id or workspace path with non-ASCII characters produces different keys, so `mark_iron_law_satisfied` (Rust, `pre_tool.rs:108-117`) writes a file the TS-side double-check never sees. No golden-vector test pins TS/Rust key parity.
- **State-root resolution drift** (`_shared/ts/bridge-core.ts:37-48` vs `runtime.rs:270-288`): TS falls back to `~/.claude/state` and honors only `KEEL_HOME`; Rust always resolves `~/.keel` plus `CLAUDE_TARGET_OVERRIDE`. Gates flap across roots before first write creates `~/.keel`.
- **OpenCode rewrite lost its timeout in the hardening commit** (`opencode/keel.ts:88-109`): the old `.timeout(2000)` was dropped when moving to `Bun.spawn`; a hung keel.exe now blocks the whole agent indefinitely. `runBridge` (line 78) kept its timeout; `runBridgeWithStdin` did not.
- **Shared fallback session keys leak gate state** (`codex/keel-codex.ts:151-159`, `opencode/keel.ts:164-165` + `bridge.rs:404-408`): id-less sessions share one marker key (`"unknown"` / `"default"`), so session A's research unlocks session B's edit gate. Refuse to evaluate the gate, or use a per-process random key, when the host supplies no id.
- **Observe payloads omit the shell command on opencode and pi** (`opencode/keel.ts:242-255`, `pi/keel-pi.ts:337-349`): shell `keel` research never clears the iron law on those hosts; codex and native Claude do.
- **Four hand-forked copies of the same bridge logic** (`_shared/ts/bridge-core.ts`, `codex/keel-codex.ts`, `opencode/keel.ts`, `pi/keel-pi.ts`): the TS tokenizer diverges from the Rust one (no backslash escapes), the state root differs, timeouts differ, and there is no shared contract test. Generate the adapters or at minimum pin the protocol (flags, JSON shapes, key sanitization) in a test both sides must pass.

## Other correctness findings worth noting

- `pre_tool.rs:127-149`: `is_keel_research_tool_name` accepts any tool name containing `keel__`, so a foreign MCP server's tool (e.g. `mcp__vendor__keel__anything`) clears the Strict edit gate. Anchor the match to `mcp__keel__`.
- `proxy/classify.rs:100-107`: bash wrapper detection matches any flag containing `c` (`--norc` qualifies), misclassifying the command position.
- `proxy/command_ast.rs:74-87, 210-227`: classification matches args anywhere rather than positionally (`cargo build --test foo` routes as Test; `--test-timestamps` flips routes). Match the first non-flag argument.
- `adapters/git.rs:64-69`: `git -C path status` picks `path` as the subcommand and skips the status reducer. Skip flag+value pairs.
- `adapters/common.rs:56` (duplicated at `generic.rs:115`): `compact_edges` ignores caps below 10 (`max(5)` floor), violating the caller's `max_lines`.
- `manager/agent_config.rs:73-79`: classic unescape-ordering bug (`\n` decoded before `\\`), silently corrupting rendered agent-profile TOML for literal backslash-n sequences. Single-pass unescape.
- `install/codex.rs:161-178` (also 423-429, 469-475): line-based TOML surgery treats any `[...]` line inside multi-line strings as a table header, potentially truncating values; also drops user keys from keel's own section. Use `toml_edit`.
- `manager/hosts.rs:310`: Grok hook file written with non-atomic `std::fs::write` instead of the atomic `write_text` used everywhere else; a torn file breaks Grok's hook parsing.
- `manager/hosts.rs:76-80` and `mcp_register.rs:246`: `~/.claude/settings.json` and `~/.claude.json` (auth + project history) are rewritten without the pre-write backup every other managed write takes. Route both through `backup_target_before_overwrite`.
- `runner/hook_lifecycle/git_hooks.rs:40-44`: config rewrite normalizes CRLF to LF, gratuitously rewriting user `.git/config` on every install.
- `post_tool.rs:151, 207`: comment-lint and graph-context match `"Edit" | "Write" | "MultiEdit"` case-sensitively while the counter logic (`state.rs:91-107`) is broader and case-insensitive, so nudges are dead code on Cursor/Grok-style payloads.
- `hooks/claude.rs:113`: `HOOK_EVENTS` ships `matcher: ""` for PermissionRequest while the repo plugin narrowed it to `"Bash"`, so installed settings spawn the hook process on every tool call just to filter in-process.
- `utility/memory/working_brief_cmd.rs:367-368`: summary ids are millisecond timestamps without the nonce `unique_timestamped_id` exists to provide; two summaries in the same millisecond overwrite.
- `review/closeout.rs` and `diff_gates.rs` gates are regex/string matching with blocking on em/en-dash and comment length; trivially gamed and false-positive prone. The language gates (cargo/clippy/black/ruff/mypy) are real; keep those front and center.

## Documentation and corpus drift

Corpus measured: 271 markdown files, ~1.46 MB repo-wide, ~1.62 MB (~400K tokens) in the synced-to-host set. Per-session injection is genuinely disciplined (8.7 KB bootstrap plus ~80 tokens per prompt). The problems are divergent copies, not volume:

- **Branch model contradiction** (`00-skill-routing-and-escalation.md:29`, `CONTRIBUTING.md:19-20`): both teach the nested `task/<task>/<subtask>` model that `WORKFLOW.md:33`, `AGENTS.md:45`, and `AGENTS/references/50-delivery-and-prohibited-shortcuts.md:29-33` explicitly forbid as a Git ref collision.
- **Hook mechanism contradiction**: `AGENTS/references/10-native-command-routing.md:19, 67-84` says the hook transparently rewrites ("do not re-run the original raw command"); `WORKFLOW.md:10-17` and `README.md:110-131` say the hook blocks and the agent must copy-and-run the rerun line. An agent following the synced reference does the opposite of the top-level doc.
- **Phantom CLI flags**: `AGENTS/references/10-native-command-routing.md:139-145` advertises `keel gain --daily|--weekly|--monthly|--chart`; `utility/gain.rs:29-33` registers none of them (only json/since/adapter/top). `doc_parity_test.rs:497` guards this class but only for `help_operator.txt`, not markdown.
- **Dead commands in authoritative docs**: `AGENTS/references/99-source-anchors.md:97` and `30-execution-strategy.md:4` reference `keel orchestration` / `keel checkpoint`, surfaces the parity test itself treats as removed (lines 834-851) but only scans 5 named files, not `references/`.
- **MCP tool list wrong in CLAUDE.md** (`CLAUDE.md:113`): names nonexistent tool `checkpoint`; omits 5 real ones. Only README's list is tested (`doc_parity_test.rs:244`).
- **Hook event counts disagree three ways**: CLAUDE.md "all 31 events", README "18 of the 30" (verified correct: 30 rows, 18 `installs_in_settings` in `hooks/claude.rs`), and the reference lists events (`MessageSend`, `MessageReceive`, `Resume`) absent from `HOOK_EVENTS`.
- **Roster count stale** (`00-skill-routing-and-escalation.md:58`): "Specialist Roster (24)", actual 26. The parity guard scans this file but only for 6 hardcoded substrings, so the heading sails through.
- **Mojibake**: 60+ occurrences of ` ,  ` (a broken em-dash scrub) across README and CLAUDE.md, the two most-read files. Add a mojibake check to CI.
- **Two-home story contradicts artifact paths** (`README.md:44` vs `README.md:558, 572`, `_shared/common-discipline.md:58`): the migration story says data moved to `~/.keel`, while paths still documented under `~/.claude/memories/` and `~/.claude/raw-output/`.
- **doc_parity_test.rs precision gap** (1,131 lines): header claims counts "can no longer rot silently", but enforcement is 6 exact substrings over 5 named files plus structural manifest checks (those are real). Nothing scans `references/`, WORKFLOW.md, CONTRIBUTING.md, or link targets. Every drift finding above lives in the unscanned area. Extending the substring approach to the branch-model sentence and a small link-checker would have caught most of it.
- **Orphan and forks**: root `anvil.md` (19 KB) is referenced by nothing and duplicates CLAUDE.md's Anvil section; `cowork/skills/using-keel/SKILL.md` is a hand-forked copy of the 8.7 KB bootstrap already diverging in wording; `KEEL-AUDIT-2026-06.md` sits at repo root as if current but contains stale counts and first-person voice the repo's own prose gate bans.

## Test coverage gaps (highest value first)

1. No cross-implementation test pins the bridge protocol (flags, KEEL_REWRITE/KEEL_GATE JSON shapes, session-key sanitization) that the three TS adapters consume. This is how B1-B3 shipped.
2. No test renders the actual PreToolUse output JSON end to end (the `updatedInput`/`allowRules`/deny payload shapes are untested; only decision functions are).
3. No test that `run_hook_stop` stops blocking once a marker is written, nor that repeated unsatisfied stops keep blocking (loop semantics).
4. `run_command_streaming_proxy` has no tests at all: live caps, cross-chunk injection leak, hang-on-grandchild behavior.
5. Redaction boundaries: no test that short passthrough output is unredacted (it is), none for the Bearer/JSON-key/AKIA misses.
6. No concurrency tests anywhere in the memory/index layer (lock contention, WAL recovery, non-UTF8 abort, MAX_FILES truncation).
7. No test asserts the SYSTEM_MAP lane and code-index lane resolve to the same workspace directory (would have caught the slug split).
8. Installers are only syntax-checked in CI (bash -n, PowerShell parser), never executed; install.sh/ps1/cmd have no runtime proof (the labels admit this).
9. `repair.rs` has 4 tests for a command that unconditionally rewrites `settings.json` and `~/.claude.json`.
10. Registry ordering: no test that a project filter for a classifiable command is (or is not) selected.

## Strengths (keep and protect)

1. **Config mutation discipline** (manager/): parse-fail-abort instead of clobber, atomic temp+fsync+rename everywhere, backup trees with abort-on-backup-failure, allowlisted orphan deletion with traversal refusal, copy-only migration with destination-wins semantics, Windows running-image park-and-restore for self-update.
2. **Release engineering** (.github/workflows/release.yml): exact-HEAD CI evidence gates publish, 6-platform smoke installs of real artifacts, SBOM + attestation + checksums, SHA-pinned actions, least-privilege tokens.
3. **The break-even guard** (`proxy/run.rs:296-313`): compaction is discarded when the wrapper would tokenize larger than the raw output. Honest, measured, documented.
4. **The PostToolBatch gate state machine** (`post_batch.rs:636-718`): pure `decide_gate`, monotonic counter, documented termination proof, dedicated loop-termination tests.
5. **Depth-capped destructive scanner** (`shell_rewrite.rs:1219-1310`) that recursively unwraps `keel run --` and `bash -lc` payloads, with a regression test for `keel run -- rm -rf /`. Idempotent rewriter refuses double-wrapping.
6. **Raw store hygiene** (`proxy/raw_store.rs`): staging dir + atomic rename publish, 0700/0600 modes with a dedicated Unix test, strict raw-id traversal validation, dead-PID-gated cleanup.
7. **Windows long-path sqlite handling** (`utility/sqlite.rs:15-83`): win32-longpath VFS plus a regression test building a 300+ character path.
8. **Zero production unwraps in the entire memory/utility layer**, centralized path-traversal validation in `record_store.rs`, skip-with-warning per-file error handling in recall.
9. **The recall ranker**: exact-to-relaxed-to-fuzzy cascade with coverage/proximity re-rank, few hardcoded constants each with a rationale, and a unicode-panic regression test.
10. **Link hygiene at scale**: 2 broken relative links across 271 markdown files, plus a real (if under-scoped) parity test harness.

## Prioritized remediation

### Blockers (fix before next release)

1. B2 codex stdin view-corruption (`Buffer.from` per chunk) + Windows EOF handling.
2. B1 pi phantom sessions (cache resolved id in module state).
3. B3 pi dead rewrite path (stdin payload, delete `bridgeRewrite`).
4. MCP HTTP: reject `Origin: null`, require JSON content type and valid session id for `tools/call`; fail closed on unrecognized argv[0] in the run-command policy.
5. Closeout exit 1 on "blocked" by default.
6. Allow allowlisted keel commands through the gate `unknown` path so "run keel doctor" is executable.

### High-value structural fixes

7. Add the bridge protocol contract test (flags + JSON shapes + key sanitization) run against all TS adapters.
8. Metadata-first change detection in `workspace_index` (stat before read; rebuild edges only on change); port recall's lock-contention degradation; skip unreadable files instead of aborting.
9. One canonical `workspace_key()` for the three slug lanes; always append the hash suffix.
10. Redact the short-passthrough path and the test adapter; extend the shared redactor (Bearer, JSON keys, AKIA, connection URIs); redact the event log's persisted command.
11. Cap the Stop-hook `decision: "block"` with the same per-session counters as PostToolBatch.
12. Detect the resolved shell before quoting on Windows; refuse cmd for metacharacter args.

### Nudges

13. Unify `signal_lines`/`compact_edges`/`program_base` duplicates into `adapters/common.rs`; register project filters before builtins.
14. Fix the closingout proof story (re-run single-command proofs or label as attestation); make the completeness gate verify scan content, not mtime.
15. Correct `gain` passthrough accounting to match `gain discover`.
16. Move hardcoded skip-list entries to config.
17. Docs: fix branch model in the two offending files, pick one hook-mechanism story, add the parity guard over `references/`, fix `gain` flags in docs, fix the roster/event counts, scrub mojibake, delete or link `anvil.md`, generate the cowork bootstrap from the canonical one.
18. Atomic write for the Grok hook file; backup before rewriting `~/.claude.json` and `~/.claude/settings.json`; single-pass unescape in `agent_config.rs`; `toml_edit` for codex config surgery.

## Method note

Findings were produced by five parallel read-only audit passes over the named subsystems, each required to cite file:line evidence and to avoid vague hunches. Every item in the "Verified blockers" section was then re-confirmed by direct source reads. Severity labels (blocker / major / minor / nit) reflect user impact: a blocker breaks the gate contract (fail-open or permanent wedge), a major is a real defect an adversary or heavy user will hit, minor/nit are correctness, hygiene, or drift items.

Per the critic skill discipline, this document is findings only; remediation routes through `receiving-code-review` and the normal task-branch workflow.
