<!--
Purpose: Provide a professional commit-body template for scoped repository changes.
Caller: keel git-workflow commit-message and contributors preparing commits.
Dependencies: Professional text linting rules and changed-file/test evidence.
Main Functions: Offers optional sections for diff-scoped commit bodies.
Side Effects: None.
-->
# Commit Body Template

Subject line (strictly enforced): `<Category> : <FEATURE> : <short information>`
- `<Category>` (Title Case): Add | Config | Refactor | Wip | Fix | Docs
- `<FEATURE>` (uppercase feature): use the concise feature name from the actual diff
- Example: `Wip : FEATURE : short information`

Note: the commit subject uses colons with spaces (`Add : HOOK : sync command routing`); the branch name uses a slash (`task/hook-routing`). Never mix the two.

Problem
<Only include when the diff fixes a concrete problem.>

Solution
<Only include when the implementation choice needs explanation.>

What Changed
- <Changed file or behavior tied to the actual diff.>

Test Result
- <Validation command and outcome that directly proves the commit.>
