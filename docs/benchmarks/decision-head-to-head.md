# Decision head-to-head: keel routing vs Jev-backed classifier.dev

Measured 2026-09-21 on branch `task/keel-classifier`, run by
`keel decision benchmark --remote`. The remote column is opt-in because it needs
the network; everything else in the run is local and offline.

## What was measured

- **Cases:** 260. 255 curated trigger phrases from keel's own routing table
  (`CURATED_SKILL_TRIGGERS`), each with the skill it must route to, plus 5
  control prompts that must stay unrouted.
- **keel local:** the curated-tier lookup, in process, no network.
- **Remote:** classifier.dev, one free POST carrying all 260 inputs and 22
  labels (21 skills plus `none of these`). The service answered with
  `"model": "jev-1.13.0"`.

| System | Correct | Accuracy | p50 | Cost |
|---|---|---|---|---|
| keel (curated tier, local) | 260/260 | 100% | under 1 ms | $0 |
| classifier.dev (jev-1.13.0, remote) | 237/260 | 91.2% | 253 ms | $0 free tier |

All five controls were handled by both: keel stayed silent on every one, the
remote model returned the none-of-the-above label on every one (confidence 0.54
to 0.99).

The 23 remote misses sit where keel's vocabulary is idiomatic and short:
`critic` 11 of 13, `systematic-debugging` 7 of 10, `running-anvil` 3 of 6,
`ui-design-systems-and-responsive-interfaces` and `qa-and-automation-engineer`
1 each. The remote model read `ready to merge` as `git-expert`, `stress-test
this approach` as `qa-and-automation-engineer`, and `implement this change` as
none-of-the-above.

**Read the keel column with its caveat.** The cases are keel's own vocabulary,
so 260/260 is a round-trip, not held-out accuracy. The informative number is the
remote column: a general zero-shot service, never trained on this vocabulary,
lands at 91.2% on 260 short prompts with 22 candidate labels.

## What the vendors and independents claim

| Source | Claim | Read |
|---|---|---|
| [JevBench v1.2](https://benchmarkheaven.com/jev-models) | Jev 1.13.0 scores 75.4 on the four-axis JevBench Score (Intelligence 90.4, Calibration 82.7, Speed 83.3, Cost 52.0; $0.040 per 1,000 decisions). Tier accuracy: judge 94.5%, hard 74.1% | 2026-09-21 |
| JevBench v1.2 | classifier.dev scores 84.8 unranked, 87.6 Speed, ~$0.0033 per 1,000 decisions estimated; judge 97.3%, hard 70.5% | 2026-09-21 |
| JevBench v1.2 method | Score is the geometric mean of Intelligence, Calibration, Speed, Cost at 25% each; one decision averages 950 input tokens | 2026-09-21 |
| [classifier.dev](https://classifier.dev) | Confidence is a real forecast: on their sets, confidence >= 0.9 was right 82% (emotion) and 92% (news) of the time; < 0.5 was right 29% and 64% | 2026-09-21 |
| [jev-agent.com](https://jev-agent.com/benchmarks) | Jev is not more accurate: 27/27 on labelled tickets, tied with Mistral Small; confidence 0.979 on clear items vs 0.841 on ambiguous ones | 2026-09-21 |
| jev-agent.com | Five questions in one request cost the same server time as one (74 ms vs 70 ms) | 2026-09-21 |

## What this run adds

- The 91.2% remote column is consistent with the independent finding that a
  decision model is a strong but not infallible zero-shot labeler on short,
  idiomatic text, and with classifier.dev's own confidence table: most of its
  wins here came back at 0.90 to 1.00, and its unsure answers (0.50 to 0.62) are
  where the misses cluster.
- keel's routing decision does not need one network round trip per prompt: the
  curated tier is local, deterministic, and answers in microseconds, so at equal
  accuracy on this set the local path wins on speed by roughly four orders of
  magnitude. That is the same trade the remote service documents for itself
  (batch pre-filtering), not a claim about model quality.
- No JevBench Score is claimed here. Scoring keel on that axis set needs its
  task set and its calibration measurement; this page reports one task only.

## Reproduce

```
cargo run -p keel -- decision benchmark            # local column only, offline
cargo run -p keel -- decision benchmark --remote   # adds the live remote column
cargo run -p keel -- decision benchmark --json     # machine-readable rows
```

The remote column fails closed: if `curl`, the network, or the response shape is
unavailable, the report says `not run` and no remote number is printed.
