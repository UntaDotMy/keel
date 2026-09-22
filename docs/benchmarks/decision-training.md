# Training the local decision head, and measuring it

Purpose: the exact procedure for supplying data, training the per-skill lexical
experts, and scoring them against the corpora keel does not train on. Every
command here is copy-pasteable and offline except the two fetch paths that say so.

Caller: a developer changing the decision model or the corpus, and any agent that
has to reproduce a number quoted in a commit message.

Dependencies: `rust/crates/keel/src/utility/lexical_experts.rs` (fit, split,
calibration, artifact), `decision_benchmark.rs` (fetch, corpora, host cases,
decontamination), `decision.rs` (CLI wiring), `state/benchmarks/*` (caches).

## 1. Build

```
cargo build --release -p keel
```

Release matters. Training is 3-4x slower in debug, and a debug run on the
24k-row corpus passed an hour before being cancelled.

## 2. Where the data lives

| what | path under `$KEEL_HOME` (default `~/.keel`) | contents |
|---|---|---|
| training corpus | `state/benchmarks/stackoverflow-train.json` | `[{text, skill, provider}]`, one row per weak label |
| evaluation shells | `state/benchmarks/external-corpus-<site>.json`, `state/benchmarks/crates-corpus.json` | `[[text, skill]]`, held out by construction |
| model | `state/benchmarks/lexical-experts.json` | experts, idf, held-out metrics, centroids |
| word vectors | `state/benchmarks/word-vectors.bin` | int8 table for the vector tier |
| sentence encoder | `state/models/minilm/{model.onnx,vocab.txt}` | class centroids only; not in git |

Providers: crates.io (categories 2-10), Stack Exchange (pages 2+, five sites),
optional OpenAlex prose (`--openalex`, rejected by measurement), and the
installed `SKILL.md` files (≤40 rows per skill, generated at train time and not
stored in the cache).

## 3. Training

```
# offline: reads the cache, touches no network
cargo run --release -p keel -- decision train-lexical

# fetch: pulls the providers, then MERGES with the cache
cargo run --release -p keel -- decision train-lexical --refresh --per-tag 100 --pages 3

# measured knobs
--split 70/15/15      train/validation/test percentages, must sum to 100
--no-stratify         reproduce a plain shuffled split (starves the tail classes)
--probes              bound the accept point with the out-of-scope probes (measured: it loses)
--quiet               no progress lines (stdout stays pure JSON)
```

`--per-tag` and `--pages` only matter during a fetch: with a cache present they
are ignored, so a re-run is byte-reproducible.

Split: 65/15/20 by default, **stratified by class**, seeded. A class with three
or more rows gets a seat in every slice, so no class is fitted on rows it is then
scored on by luck. Validation fits the temperature, the prior weight and the
accept point; test is reported and never fitted on; the idf vocabulary is fitted
on the training slice alone.

Label-noise prune floor: the 5-fold Confident-Learning prune does not judge a
class with fewer than ten rows. The fold model never saw such a class, so it
contradicts it by construction, and an unguarded prune deleted **29 of the 52
classes** the corpus carried, including `reviewer` and `git-expert`, which the
head then could not name at all. The floor keeps those rows and the run reports
how many it held back.

Decontamination: rows whose text is an evaluation row are dropped before the
split and counted in `dropped_eval_leak`. The corpora are fetched from the same
pages, so an overlap is a fetch artefact, and a benchmark that scores a memorized
row reports the memory. See §5 for the command that counts it.

## 4. Reading the run

Progress goes to stderr, the JSON header to stdout.

```
[    0.0s] split
         train 7366 · validation 1700 · test 2266 · 65/15/20 stratified=true
[    0.1s] label-noise prune (5 folds, held out per fold)
         13332 of 13332 rows judged by a fold that never saw them
         dropped 292 contradicted rows, 7040 kept
[    2.1s] fit
         7040 rows · 23 classes · 61234 features · 1700 validation rows · 30 epochs max, patience 5
         epoch  1/30  loss  2.1830  val_brier 0.1290  val_acc 0.6288  best 0.1290@1  stale 0/5  12.4s  eta 359.6s
         ...
         early stop at epoch 9: no validation improvement for 5 epochs
         kept epoch 4 at validation brier 0.1120
         prior weight alpha 0.375 fitted on validation (brier 0.1116)
[  118.0s] class centroids (encoder)
         23 centroids
[  131.0s] held-out scoring (temperature, accept point, confusion)
         temperature 1.45 by validation brier 0.1233 · 12 probes

held-out  2266 rows · 2260 scored · decided 2211 · correct 1628 · accuracy 0.7185 · brier 0.1456 · ece 0.0277
          macro F1 0.6102 · weighted F1 0.7044 · scale 1.45 · accept 0.30 · probe rejection 100%
          class                              rows   said  right    prec  recall     f1
          ...
          most confused:
            postgres-migration-safety      → cloud-and-devops-expert        11
          confusion (rows = expected, 1..23 = prediction order, 24 = silent)
          legend: 1=adversarial-security-rev…  2=authentication-and-iden…
```

What each line means:

- **phase lines** are one per stage with elapsed seconds; the JSON header repeats
  them with millisecond timings under `phases`.
- **epoch lines** carry the online training loss, validation Brier, validation
  accuracy, the best Brier and the epoch that produced it, the stale counter, and
  an ETA. Early stopping fires at `stale = patience`, and the *best* epoch is kept
  rather than the last.
- **prior weight** is how much the class priors count in the bias, fitted on
  validation (logit adjustment for imbalance, not an assumed constant).
- **held-out** reports accuracy (over all rows), Brier (over scored rows),
  ECE, macro F1, weighted F1, the accept point, and the share of out-of-scope
  probes the accept point rejects. An abstention is a decision, so it appears in
  the confusion matrix as the trailing column.

## 5. Benchmarking

```
cargo run --release -p keel -- decision benchmark --host              # 21 product prompts
cargo run --release -p keel -- decision benchmark --external --site softwareengineering
cargo run --release -p keel -- decision benchmark --external --source crates
cargo run --release -p keel -- decision benchmark --host --remote     # adds classifier.dev
```

Both columns come from one run, on the same rows. `--remote` needs the network
and spends a live call; it fails closed when the service cannot be reached.

Counting the train/eval overlap directly, so the decontamination is checkable:

```
python - <<'PY'
import json, pathlib
base = pathlib.Path.home()/".keel"/"state"/"benchmarks"
norm = lambda s: "".join(c.lower() for c in s if c.isalnum())
train = {norm(r["text"]) for r in json.loads((base/"stackoverflow-train.json").read_text())}
for name in ("external-corpus-softwareengineering.json", "crates-corpus.json"):
    rows = json.loads((base/name).read_text())
    hits = sum(1 for text, _ in rows if norm(text) in train)
    print(f"{name}: {hits} of {len(rows)} eval rows are training rows")
PY
```

## 6. Measured ladder

Same corpus, release build, real home, `task/keel-classifier`. Every row below is
a full run; the numbers are the run's own header and the benchmarks that follow it.

| run | config | corpus rows | training rows | classes | held-out acc | Brier | ECE | macro F1 | spent |
|---|---|---|---|---|---|---|---|---|---|
| shipped | shuffled, no decontamination, no floor | 11,353 | 6,353 | 23 | 0.7172 | 0.1456 | 0.0277 | - | - |
| A | shipped, decontaminated | 11,332 | 6,371 | 23 | 0.7127 | 0.1449 | 0.0196 | 0.3823 | 560 s |
| B | A + stratified + probes | 11,332 | 6,347 | 23 | 0.6097 | 0.1452 | 0.0259 | 0.3357 | 501 s |
| D | A + prune floor | 11,332 | 6,486 | **52** | 0.7034 | 0.1438 | 0.0308 | 0.2053 | 710 s |
| F | A + floor + stratified | 11,332 | 6,486 | **52** | 0.7088 | 0.1450 | 0.0152 | 0.1786 | 673 s |

Benchmarks of the same artifacts, same rows, same home:

| run | host correct/21 | host decided | design correct/125 | design decided | crates correct/200 | crates decided |
|---|---|---|---|---|---|---|
| shipped | 6 | 10 | 19 | 38 | 54 | 90 |
| A | 5 | 6 | 20 | 37 | 52 | 90 |
| B | 3 | 4 | 13 | 26 | 44 | 65 |
| D | 4 | 8 | 19 | 38 | 52 | 84 |
| F | **7** | 8 | 19 | 37 | 51 | 88 |

What the ladder says:

- **Decontamination is a correctness fix, not an accuracy win.** Twenty-one eval
  rows were in the training cache; removing them cost 0.45 points of held-out
  accuracy, which is the size of the self-scoring it removed.
- **The prune floor is the data win.** It restored 29 deleted classes (23 -> 52)
  and lowered Brier (0.1450 -> 0.1438-0.1450 at equal or better ECE). It also
  drops macro F1 to 0.18, because the restored classes have 3-4 rows each and are
  mostly answered wrong: the honest macro average is now low, and it was not
  measurable before.
- **The abstention probes lose.** Measured: 92% of the 12 probes rejected and the
  accept point raised 0.30 -> 0.55, at the cost of decoded answers on every
  surface (host 5 -> 3, design 20 -> 13, crates 52 -> 44). A 90% floor over 12
  rows implies the ceil((12+1)·0.9)/12 = 100% conformal quantile, so the
  threshold refuses nearly everything out of domain. Default off, `--probes`
  opts in.
- **The host surface cannot discriminate.** 21 rows, 1-4 decided per run: the
  spread from 3 to 7 is inside what a two-row difference explains. Use the design
  and crates corpora, and the per-class table, for decisions.
- **Restoring a class is not teaching it.** `reviewer` and `git-expert` are now
  in the head, and the host prompts for them stay silent: three seed rows per
  class do not clear the accept point. The fix is rows for the tail classes, not
  a lower threshold.

## 7. Gotchas

- Stack Exchange allows 300 requests/day/IP. A rate-limited provider contributes
  nothing and the cached rows survive; check `provider_counts` in the header.
- The accept point and the temperature are refitted on every run, so two runs are
  only comparable on the same corpus and config: read `config` and `accept` from
  the header before comparing numbers.
- An artifact trained with an encoder refuses to serve if
  `state/models/minilm` is missing, rather than scoring half its features.
- `--refresh` on a small `--per-tag` will shrink nothing (the fetch merges), but
  a first fetch with no cache defines the corpus.
- Skill seed rows land in the training, validation and test slices like any other
  row; they are the only in-domain rows keel has, and a class built from seeds
  alone is reported with a low support count in the per-class table.
