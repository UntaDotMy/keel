---
description: Lint user stories against strict Agile/Jira format (Connextra "As a/I want/so that" + Gherkin Given/When/Then, validated against INVEST). Use before building to confirm the requirement spec is well-formed.
argument-hint: "[lint] [file-path]"
allowed-tools: Read, Bash(keel user-story:*)
---

# /keel:user-story

Validate user story format. Arguments: **$ARGUMENTS**

Use the installed binary path (bare `keel` is not guaranteed on PATH):
`~/.claude/keel` (macOS/Linux), `%USERPROFILE%\.claude\keel.exe`
(Windows), or `cargo run --bin keel --` from a source checkout.

Map the action in `$0` to the matching native subcommand:

- `lint` (default) → `user-story lint --file <path>` — validate that user stories follow:
  - **Connextra format:** "As a `<role>`, I want `<goal>`, so that `<benefit>`"
  - **Gherkin acceptance criteria:** Given/When/Then scenarios
  - **INVEST validation:** Independent, Negotiable, Valuable, Estimable, Small, Testable

If no file path is given, lint the stories in the current working brief.

**This is the anti-drift gate.** Stories that fail INVEST or lack Gherkin criteria are not ready for implementation. Fix format issues before handing to `writing-plans` or `test-driven-development`. The confirmed stories become the spec that `reviewer` Stage 1 reconciles the diff against — every requirement maps to a story, and nothing is built that no story asked for.
