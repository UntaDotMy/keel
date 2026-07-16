# Data/ML deep practices

## Delivery Workflow

### 1. Trace the Data Flow and Ownership
- Identify the source of truth, the producers, and every downstream consumer of the asset you are changing.
- Map the read/write paths, freshness SLAs, and which consumers (BI, ML, reverse-ETL) depend on the output.
- Distinguish what this skill owns (analytical/ML flow) from what `backend-and-data-architecture` owns (OLTP schema and migrations).

### 2. Define the Contract and Quality Bar
- Specify schema, types, nullability, accepted values, freshness, and volume expectations for inputs and outputs.
- Decide the quality assertions that gate each stage and what happens to rejected rows.
- Document schema-evolution rules and how a breaking change is staged across consumers.

### 3. Choose Processing Strategy
- Batch vs streaming: pick streaming only when latency requires it; otherwise batch is cheaper and easier to reason about.
- Incremental vs full-refresh: declare the default and the conditions that force a full rebuild.
- Define the partitioning, watermark, and backfill window before writing transformation logic.

### 4. Build Idempotent, Reproducible Steps
- Make every load idempotent and every transformation a pure function of its inputs.
- Wire orchestration with explicit dependencies, retries with backoff, and parameterized backfill runs.
- For ML, pin data and code versions and log the run so it can be reproduced.

### 5. Verify Before Promotion
- Run on a realistic data volume and a representative window, not a toy sample that hides skew and late data.
- For pipelines, confirm a re-run and a backfill produce identical, non-duplicated results.
- For models, confirm the evaluation bar is met on the held-out set and on the slices that matter.

### 6. Monitor After Rollout
- Watch freshness, row-count bounds, quality-check pass rates, and pipeline run durations.
- For models, monitor input-feature drift, prediction drift, and online metric regressions against the training reference.
- Define alerts and the containment or rollback step (revert to prior model version or last-good partition) before going live.

## Real-World Scenarios

- **Non-Idempotent Backfill**: A daily pipeline appends rows, so re-running a failed day double-counts revenue. Use this skill to switch to partition-scoped delete-and-insert or `MERGE` on a deterministic key, and to add a parameterized backfill window.
- **Incremental Drift**: An incremental dbt model misses late-arriving events and slowly diverges from a full refresh. Use this skill to add a grace window on the watermark, reconcile with a periodic full-refresh, and document when one is required.
- **Producer Contract Break**: An upstream service renames a field and silently breaks every downstream mart. Use this skill to add a data contract with type and accepted-value checks at ingestion, fail loudly, and stage the change across consumers.
- **Train/Serve Skew**: A model scores well offline but degrades in production because serving computes a feature differently than training. Use this skill to unify feature definitions, add point-in-time correctness, and monitor feature distributions.
- **Unreproducible Model**: A model in production cannot be regenerated because its training data and parameters were not pinned. Use this skill to version the dataset, track the run, and wire the registry so promotions are auditable.

## Release Blockers

Recommend a block when:
- a pipeline load is not idempotent and a retry or backfill would duplicate or corrupt data
- there is no defined backfill path for reprocessing a historical window
- a data contract or quality gate is missing on a consumer-facing asset, or violations are dropped silently
- an incremental model has no late-data handling and no full-refresh reconciliation path
- a model is promoted to serving without clearing an evaluation bar on a held-out or shadow set
- training and serving feature computation diverge, or the training run cannot be reproduced
- drift and freshness monitoring are not wired up before a model or pipeline goes live

## Runtime Boundaries

Do not over-claim certainty when:
- the pipeline was tested on a sample that does not match production volume, skew, or late-arriving-data behavior
- idempotency and backfill were reasoned about but not actually exercised with a real re-run
- the evaluation set may overlap training data or may not represent production slices
- feature parity between training and serving was inferred from code rather than measured on shared inputs
- drift thresholds were set without a production baseline, so alerts may be noisy or silent
- cost and partitioning behavior were estimated rather than observed on a full-scale run

## Output Expectations

When using this skill, return:
- the data-flow map: sources, transformations, consumers, and ownership boundary with `backend-and-data-architecture`
- the contract and quality gates for inputs and outputs, with the failure and quarantine behavior
- the processing strategy: batch vs streaming, incremental vs full-refresh, partitioning, watermark, and backfill window
- the idempotency and reproducibility plan (keys, versioning, experiment tracking)
- for ML work, the evaluation bar, the slices evaluated, and the drift-monitoring plan
- the verification plan, the rollback or containment step, and the residual risks
