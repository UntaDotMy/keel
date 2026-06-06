## Scope

- [ ] This MR contains exactly one feature or one tightly related fix
- [ ] A new `feat/<topic>` branch (off `dev`) was used for this feature
- [ ] This MR targets `dev` (promotion to `main` happens after staging verification)
- [ ] No unrelated changes are mixed into this MR
- [ ] Patch staging (`git add -p`) was used where needed
- [ ] `git diff --cached` and final MR diff were reviewed for scope leaks
- [ ] Commit subjects follow `<category>: <FEATURE>: <short information>` (categories: add, config, refactor, wip, fix, docs; FEATURE uppercase)
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
- [ ] Needs rebase onto `dev` if sibling feature branch merges first
