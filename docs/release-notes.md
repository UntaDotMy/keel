# Release Notes

`keel` release notes are meant to explain three things clearly:

- what changed in the release
- why the release matters to operators using the harness day to day
- what proof bar the release passed before it was published

## What a release note should contain

Every published GitHub release should include:

- the release tag and build version
- the operator-facing summary for that release
- links to the comparison surface and operator docs
- the matching release-proof bundle for that release
- the validation baseline for the shipped asset
- GitHub-generated pull request and commit notes for the exact release range

## What this repository avoids in release notes

- broad unsupported market claims
- benchmark claims before the benchmark suite exists
- calling work complete without matching review and validation proof

## Current release-note posture

The release workflow currently asks GitHub to generate the detailed PR and commit notes for the actual release range. It does not prepend a tracked `keel` release-note preamble.

The generated notes are expected to provide:

- the release's GitHub-generated diff summary

The tracked documents linked above provide the stable operator context and
validation evidence separately.

## Related docs

- [Why `keel`](./why-keel.md)
- [Release proof bundle](./release-proof-bundle.md)
- [Compatibility matrix](./compatibility-matrix.md)
- [Benchmark and demo suite](./benchmark-suite.md)
- [README](../README.md)
- Implementation direction: see `docs/competitive-gap-closure.md` and working briefs
