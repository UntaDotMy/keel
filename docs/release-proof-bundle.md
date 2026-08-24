# Release Proof Bundle

Each notable `keel` release should publish a durable release-proof bundle alongside the native platform archives.

The bundle exists to keep release trust claims source-backed. It gives operators, reviewers, and AI agents one artifact that explains what proof was run, what review posture applied, what hosted release workflow completed, and what benchmark snapshot was current when the release was published.

## Required files

Every release-proof bundle should include these files:

- `validation-summary.md`: the validation command shape and release-preflight outcome used before publication.
- `review-summary.md`: the native review policy or review gate summary that governed the release.
- `hosted-check-status.md`: the hosted release-workflow status, run URL, and the release jobs that completed.
- `benchmark-status-snapshot.md`: the benchmark-suite snapshot that was current when the release was cut.
- `release-proof-manifest.json`: machine-readable metadata for the release tag, build version, workflow run, and included proof files.

## Validation precondition

Release preflight requires a successful `Validate` workflow run for the exact
release `GITHUB_SHA`. Main pushes trigger validation automatically. Tag and
manual releases require a manual `Validate` dispatch against the same ref
before release preflight can continue.

## Published asset shape

The release workflow should upload one proof archive named `keel_release-proof_<build_version>.tar.gz`.

That proof archive is intentionally plain markdown plus JSON so a human operator or an AI agent can inspect it directly after download without needing the source checkout.

## What the bundle proves

A release-proof bundle is meant to answer four concrete questions:

- What validation ran before the release was published?
- What review policy and gate posture governed that release?
- Which hosted release workflow completed successfully?
- What benchmark snapshot was current at release time?

## What the bundle does not replace

The release-proof bundle does not replace the platform archives, the generated release notes, or the benchmark suite itself.

It is the durable trust artifact that ties those surfaces together for one release point.
