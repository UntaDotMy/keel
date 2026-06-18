<!--
Purpose: Keep the native implementation gap comparison useful without hardcoding third-party project names.
Caller: Contributors and agents comparing native keel surfaces against external output reducers, runtime shells, and memory systems.
Dependencies: None; documentation only.
Main Functions: Document output compaction, runtime-shell, and memory/retrieval gap tables plus current native priorities.
Side Effects: None.
-->
# Native Gap Map

This page keeps the comparison useful without hardcoding third-party project names in the repository. Comparator labels describe capability classes only.

## Output Compaction Comparison

| Capability | External output reducers | Native implementation | Gap |
| --- | --- | --- | --- |
| Transparent command interception | Usually automatic once installed | Hook transparently rewrites commands via toolInputOverride before noisy output exists | Covered (no agent rerun needed) |
| Raw-output recovery | Expected for trustworthy compaction | Full stdout and stderr are saved to a recovery log for wrapped commands | Covered |
| Semantic reducers | Often include command-specific parsers | Current reducers select high-signal lines by command family and then preserve head/tail context | Add structured parsers for common machine-readable streams |
| Streaming | Some tools cap live output while keeping a full capture | `--stream` emits bounded live output and keeps the full capture for final compaction | Covered, but no adaptive progress UI yet |
| Savings analytics | Often reports token or byte savings | `gain` reads persisted compaction events with reducer and family dimensions | Covered for wrapped commands only |
| Shell-aware rewrite | Expected for common shell wrappers and pipelines | `rewrite` handles environment prefixes, shell separators, and shell wrapping | Covered for supported command families |
| Command coverage | Broad test, build, search, package, platform, and CI commands | Native coverage includes Rust, JavaScript, Python, JVM, platform, VCS, search, container (docker/kubectl/helm), and cloud (aws/az/gcloud) families | Add more CI, database, and mobile command families |

## Runtime-Shell Comparison

| Capability | Runtime-shell peers | Native implementation | Gap |
| --- | --- | --- | --- |
| First-run setup | Friendlier guided setup and repair | Native install, status, verify, update, doctor, and menu paths exist | Make the first-run path more conversational without weakening proof gates |
| Live operator HUD | Richer live shell state and team visibility | Cockpit, dashboard, watch, and branch finish expose proof state | Improve live display polish and team-lane visibility |
| Team coordination | More visible team runtime helpers | Native workflow supports team mode and proof ledgers | Add clearer task board, lane handoff, and inbox-style coordination |
| Completion discipline | Often lighter or more conversational | Review gates, hosted-check proof, and completion ledgers are stricter | Native is stronger here; keep it strict |

## Memory And Retrieval Comparison

| Capability | Memory/retrieval peers | Native implementation | Gap |
| --- | --- | --- | --- |
| Layered durable memory | Rich typed memory and graph recall | Scoped workspace, workstream, role, agent, entity, and relation memory exist | Improve query ergonomics and recall scoring |
| Semantic retrieval | Vector or hybrid retrieval is common in deeper systems | Local code search and memory indexing are lexical and lineage-aware | Add optional semantic ranking without requiring hosted services |
| Transformation graph | Dedicated indexing tools expose transformation and export graphs | Native search has incremental lineage and demo evidence | Add explicit transformation DAG and multi-target export if needed |
| Reproducible benchmarks | Mature peers publish narrower benchmark runners | This repo has source-backed scenarios and shared harness docs | Add recurring benchmark runs for memory and retrieval quality |

## Current Native Priorities

- Replace named benchmark references with capability-class labels in docs and prompt surfaces.
- Add structured reducers for machine-readable test/build output before broadening more command families.
- Improve runtime-shell polish while keeping closeout proof stricter than peer workflows.
- Expand memory/retrieval benchmarks before making stronger comparison claims.
