<!--
Purpose: Provide a professional commit-body template for scoped repository changes.
Caller: keel git-workflow commit-message and contributors preparing commits.
Dependencies: Professional text linting rules and changed-file/test evidence.
Main Functions: Offers optional sections for diff-scoped commit bodies.
Side Effects: None.
-->
# Commit Body Template

Subject line (strictly enforced): `<Category>: <FEATURE>: <short information>`
- `<Category>` (Title Case): Add | Config | Refactor | Wip | Fix | Docs
- `<FEATURE>` (uppercase component): e.g. RGB, LED, ARGB, SENSOR
- Example: `Wip: RGB: Build light effect mode (multi color)`

Note: the commit subject uses colons (`Add: RGB: sync all`); the branch name uses a slash (`add/RGB`). Never mix the two.

Problem
<Only include when the diff fixes a concrete problem.>

Solution
<Only include when the implementation choice needs explanation.>

What Changed
- <Changed file or behavior tied to the actual diff.>

Test Result
- <Validation command and outcome that directly proves the commit.>
