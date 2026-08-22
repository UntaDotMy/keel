## Scope

- [ ] This MR contains exactly one feature or one tightly related fix
- [ ] A `task/<task>` work branch (off `feat`) was used for this feature
- [ ] This MR targets `feat` (promotion `feat` → `dev` → `main` happens after verification)
- [ ] Fixes for in-flight work stayed on the same work branch (no new branch for a fix to work already underway)
- [ ] No unrelated changes are mixed into this MR
- [ ] Patch staging (`git add -p`) was used where needed
- [ ] `git diff --cached` and final MR diff were reviewed for scope leaks
- [ ] Commit subjects follow `<Category> : <FEATURE_CATEGORY> : <short info>` (Category capitalized: Add, Config, Refactor, Wip, Fix, Docs; FEATURE_CATEGORY uppercase; spaces around colons)
- [ ] No duplicate implementation or conflicting overlap with another open MR
- [ ] Docs included here only if they belong to this same feature
- [ ] The published MR body uses real multiline text, not escaped sequences such as `\\n`
- [ ] The branch will NOT be deleted after merge (branches are permanent in this repo)

## Feature Summary

Describe the single feature in one sentence.

## Changes

- item
- item

## Testing

- item
- item

## Risks / Merge Order

- [ ] Safe to merge independently
- [ ] Needs another MR merged first
- [ ] Needs rebase onto `feat` if sibling work branch merges first
