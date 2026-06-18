<!--
Purpose: Document the native preserve-existing-flow evidence artifact schema.
Caller: Agents, reviewers, and native review gates that need machine-checkable brownfield evidence.
Dependencies: keel-flow schema validation and keel flow commands.
Main Functions: Defines ~/.claude/memories/workspaces/<workspace-slug>/flow/flow-check.json fields, exemptions, and validation rules.
Side Effects: None.
-->
# Preserve Existing Flow Check Schema

Existing source-file edits need a flow-check artifact before review gates pass. The default path is `~/.claude/memories/workspaces/<workspace-slug>/flow/flow-check.json`, created with:

```bash
keel flow start --target-file rust/crates/keel/src/commands.rs --target-function Application::run
```

The default artifact lives in Claude Code-global per-workspace storage, not in the repository checkout. CI review gates should read that same global path or an explicit `--artifact <path>` override; do not commit the default runtime artifact into the user workspace. It records the evidence that must exist before editing established code.

## Schema

```json
{
  "version": 1,
  "task": "short task label",
  "target_file": "rust/crates/keel/src/commands.rs",
  "target_function": "runExample",
  "current_behavior_to_preserve": "What currently works and must stay working.",
  "entry_point": "Where the input, command, event, request, or timer enters.",
  "producer": "Where intent or data is created.",
  "source_of_truth": "Where the final behavior decision is owned.",
  "storage_state_queue_owner": "Where state, queue, cache, or persistence is owned; use Not found if none exists.",
  "side_effect_owner": "Where the system writes, sends, persists, renders, or mutates outside state.",
  "consumers": ["Who reads or acts on the value; use Not found if none exists."],
  "cleanup_recovery_path": "Where success, failure, retry, rollback, or cleanup is handled.",
  "edit_boundary": "The files or functions allowed to change and what must not move.",
  "validation_needed": ["The checks that prove preserved behavior."],
  "validation_evidence": ["The checks actually run and their outcome."],
  "duplicate_owner_logic": false,
  "migration_approved": false,
  "docs_only": false,
  "formatting_only": false,
  "generated_only": false,
  "greenfield": false
}
```

## Validation Rules

`keel flow check` and native review gates require `version`, `target_file`, the owner path fields, at least one consumer, at least one validation target, and at least one validation evidence item for existing-source edits.

Docs-only, formatting-only, generated-only, and greenfield changes are explicit exemptions. Review gates do not require a flow check when all changed source files are newly added, and they do not require it for docs-only changes.

If `duplicate_owner_logic` is true, `migration_approved` must also be true. This keeps duplicated owner paths blocked unless the artifact records an explicit ownership migration approval.
