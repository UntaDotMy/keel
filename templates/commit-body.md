<!--
Purpose: Provide a professional commit-body template for scoped repository changes.
Caller: keel git-workflow commit-message and contributors preparing commits.
Dependencies: Professional text linting rules and changed-file/test evidence.
Main Functions: Offers optional sections for diff-scoped commit bodies.
Side Effects: None.
-->
# Commit Body Template

Subject line (strictly enforced): `<category>: <FEATURE>: <short information>`
- `<category>` (lowercase): add | config | refactor | wip | fix | docs
- `<FEATURE>` (uppercase component): e.g. RGB, LED, ARGB, SENSOR
- Example: `wip: RGB: Build light effect mode (multi color)`

Problem
<Only include when the diff fixes a concrete problem.>

Solution
<Only include when the implementation choice needs explanation.>

What Changed
- <Changed file or behavior tied to the actual diff.>

Test Result
- <Validation command and outcome that directly proves the commit.>
