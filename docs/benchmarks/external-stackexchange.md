# External benchmark: Stack Overflow tags

Purpose: score keel's routing against data keel did not author. The fixture
benchmark (`keel decision benchmark`) uses keel's own curated trigger phrases, so
it measures vocabulary agreement, not real-world routing. This one takes its
prompts and its labels from outside.

Command:

    keel decision benchmark --external --remote --per-tag 25

## Method

- Prompts: recent question titles from the public Stack Exchange API
  (`/2.3/questions?site=stackoverflow&tagged=<tag>`), 25 per tag, 8 tags.
- Labels: the community tag, mapped mechanically to the keel skill that owns that
  domain. `rust` to `rust`, `unit-testing` to `test-driven-development`,
  `debugging` to `systematic-debugging`, `security` to
  `adversarial-security-review`, `postgresql` to `postgres-migration-safety`,
  `websocket` to `websocket-realtime-design`, `internationalization` to
  `internationalization-and-localization`, `kubernetes` to
  `cloud-and-devops-expert`. The mapping is the only human step and it is
  one tag per skill.
- Baseline: classifier.dev (`jev-1.13.0`) receives the same 200 titles with the
  same eight skill labels plus its documented none-of-the-above option.
- The corpus is cached under the keel state directory, so a rerun scores the same
  rows offline instead of spending the keyless quota again.

## Result (2026-09-21, 200 titles)

    keel local       correct 12/200   decided 12   silent 188
    classifier.dev   correct 156/200   p50 261 ms   model jev-1.13.0
    classifier.dev   confidence vs correctness: brier 0.1374   ece 0.0753

- keel was silent on 188 of 200 titles; every one of those counted as a miss in
  the correct column. Where it did speak, all 12 were right.
- classifier.dev answered every row and was right on 156. Three runs scored 154,
  155, and 156 with brier between 0.1328 and 0.1374, so treat a few rows of
  difference as service variance, not signal.
- keel's own brier and ece are absent here on purpose: only 12 rows carried a
  decision, so there is no calibration to score. That absence is the finding.

## What this says

keel's curated phrase tier is high precision and very low recall on real
question phrasing: a title about E0502 borrow-checker failure is not the phrase
`fix the borrow checker error`. On keel's own fixture set the same router
scores 260/260; on external rows it decides 6 percent of the time. That gap is
the benchmaxxing the fixture set was hiding.

Next work this result sets, in order: widen the router's trigger vocabulary
against real phrasing rather than adding more fixture rows, then recalibrate the
confidence that currently reads brier 0.2920 against 260/260 accuracy on the
fixture set, and only then let a trained model weight drive a decision.

## Reproduction

    keel decision benchmark --external --refresh --per-tag 25            # refetch
    keel decision benchmark --external --json > external.json            # raw rows

The Stack Exchange API needs no key: 300 requests a day, 100 rows a page, and a
`backoff` field the fetcher honors before touching the same method again.
