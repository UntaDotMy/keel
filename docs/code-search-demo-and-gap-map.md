<!--
Purpose: Honest code-search surface and remaining gaps versus fuller indexing tools.
Caller: Operators and agents choosing how to search the workspace.
Dependencies: keel code-search search|siblings (Rust utility/code_search.rs).
Main Functions: Document the live command, path filters, skip list, and explicit non-goals.
Side Effects: None. Documentation only.
-->
# Code Search Demo And Gap Map

## Live surface (implemented)

```bash
keel code-search search --workspace-root "$PWD" --query "incremental lineage proof"
keel code-search search --workspace-root "$PWD" --query "FlagSet" --path "rust/crates/keel"
keel code-search siblings --query "the bug shape"
```

Flags:

| Flag | Purpose |
| --- | --- |
| `--query` | Required substring to match (lexical, case-sensitive `contains`) |
| `--workspace-root` | Root directory to walk (defaults to resolved repo root) |
| `--path` | Optional path filter; `/` and `\` are treated equivalently on Windows/macOS/Linux |

What it does today:

- Walks the tree with a fixed skip list (`target`, `node_modules`, `.git`, research clones like `hermes-agent`, …)
- Matches **literal substrings** in text files (not embeddings / semantic ranking)
- Prints `relative/path:line:snippet` rows (capped)
- Does **not** build or query a persisted index

There is **no** `code-search index`, `demo`, `status`, or `reset` subcommand. Older docs that named those verbs are obsolete; use `search` only.

## Related surfaces

| Need | Command |
| --- | --- |
| Structural import graph / reverse impact | `keel code-graph build`, `keel code-graph impact --changed a,b` |
| Durable memory search | `keel memory recall <query>` |
| Compaction of noisy `rg`/`grep` | `keel run -- rg …` |

## Current gap notes

- Search is lexical, not embedding-backed semantic retrieval
- No persisted index, lineage DAG, or multi-target export
- Skip list is name-based (not a full `.gitignore` parser); large unlisted trees can still be walked
- Relevance scores are not produced; MCP `code_search` is the same lexical path

## Why the local-first posture stays

- No hosted dependency to search the workspace
- Agents can inspect results directly in the CLI transcript
- Review and closeout proof stay inside the same native workflow
