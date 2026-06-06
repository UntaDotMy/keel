# Git Expert Source Anchors

Use these sources for current, evidence-backed Git workflow guidance.

## Core Git Commands (Official)

- Git reference docs home: https://git-scm.com/docs
- `git add`: https://git-scm.com/docs/git-add
- `git commit`: https://git-scm.com/docs/git-commit
- `git branch`: https://git-scm.com/docs/git-branch
- `git switch`: https://git-scm.com/docs/git-switch
- `git push`: https://git-scm.com/docs/git-push
- `git fetch`: https://git-scm.com/docs/git-fetch
- `git merge`: https://git-scm.com/docs/git-merge
- `git rebase`: https://git-scm.com/docs/git-rebase
- `git revert`: https://git-scm.com/docs/git-revert
- `git reset`: https://git-scm.com/docs/git-reset
- `git reflog`: https://git-scm.com/docs/git-reflog
- `git cherry-pick`: https://git-scm.com/docs/git-cherry-pick
- `git clean`: https://git-scm.com/docs/git-clean

## Claude Code Agent Orchestration References

- Claude Code slash commands: https://docs.anthropic.com/en/docs/agents-and-tools/claude-code/cli-usage
- Claude Code changelog: https://docs.anthropic.com/en/docs/agents-and-tools/claude-code/changelog

- (removed: js_repl is not part of Claude Code)
- (removed: not part of Claude Code)
- (removed: js_repl is not part of Claude Code)

## Collaboration and Pull Request Workflow

- GitHub Issues docs: https://docs.github.com/en/issues
- Creating a pull request: https://docs.github.com/en/pull-requests/collaborating-with-pull-requests/proposing-changes-to-your-work-with-pull-requests/creating-a-pull-request
- Requesting a PR review: https://docs.github.com/en/pull-requests/collaborating-with-pull-requests/getting-started/helping-others-review-your-changes
- Linking PRs to issues: https://docs.github.com/en/issues/tracking-your-work-with-issues/linking-a-pull-request-to-an-issue
- About protected branches: https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/managing-protected-branches/about-protected-branches
- GitHub CLI manual: https://cli.github.com/manual/
- GitHub secret scanning and push protection: https://docs.github.com/en/code-security/secret-scanning/working-with-secret-scanning-and-push-protection

## Commit and History Hygiene

- This project's commit convention: `<category>: <FEATURE>: <short information>` — categories (lowercase) `add`, `config`, `refactor`, `wip`, `fix`, `docs`; `<FEATURE>` uppercase (e.g. RGB, LED, ARGB, SENSOR). Example: `wip: RGB: Build light effect mode (multi color)`. This is the enforced format and supersedes the generic Conventional Commits style for this repository.
- Conventional Commits specification (background reference only): https://www.conventionalcommits.org/en/v1.0.0/
- Git ignore pattern format: https://git-scm.com/docs/gitignore
- Force with lease guidance (git push): https://git-scm.com/docs/git-push#Documentation/git-push.txt---force-with-lease
