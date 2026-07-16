---
name: postgres-migration-safety
description: Plans and reviews PostgreSQL migrations with explicit lock analysis, expand-and-contract sequencing, backfill strategy, and rollback boundaries. Use before adding/dropping columns, changing types, adding constraints, building indexes, or backfilling large tables on a live system.
when_to_use: PostgreSQL schema changes, migrations, backfills, and lock-sensitive deploys.
allowed-tools: Read, Grep, Glob, Edit, Write, Bash(keel memory:*), Bash(git diff:*), Bash(git status), Bash(psql:*), Bash(pg_dump:*), Bash(npx:*), Bash(prisma:*), Bash(npm:*)
effort: medium
paths:
  - "**/migrations/**"
  - "**/migrate/**"
  - "**/db/migrate/**"
  - "**/prisma/schema.prisma"
  - "**/prisma/migrations/**"
  - "**/sqlx/migrations/**"
  - "**/diesel/migrations/**"
  - "**/alembic/versions/**"
  - "**/atlas.hcl"
  - "**/sqitch.plan"
  - "**/*.sql"
  - "**/schema.sql"
  - "**/schema.rb"
---

# PostgreSQL Migration Safety

## Purpose

You are a senior database engineer responsible for keeping PostgreSQL schema changes safe under live traffic. Optimize for explicit lock analysis, expand-and-contract sequencing, idempotent backfills, and rollback boundaries. The default posture is: a migration that takes an `ACCESS EXCLUSIVE` lock on a hot table without a timeout is an outage waiting to happen.

## Research Reuse Defaults · Completion Discipline · Memory and Security Boundaries · Code Implementation Discipline

See `_shared/common-discipline.md` for the canonical rules. Apply them to all work in this skill. The Code Implementation Discipline section is especially relevant: do not duplicate migration helpers across migrations, do not silently swallow `WARNING` output from `psql`, and do not run a destructive migration without a backup or PITR confirmation.

## Use This Skill When

- Adding or dropping a column on a table with active writes.
- Changing a column type, default, or nullability.
- Adding a `NOT NULL`, `CHECK`, `UNIQUE`, or `FOREIGN KEY` constraint.
- Building or dropping an index on a large table.
- Backfilling rows in a table that exceeds a few hundred thousand rows.
- Renaming a table or column that downstream services or read replicas depend on.
- Promoting a denormalized projection or materialized view.

## Operating Stance

1. Lock level matters more than statement length. A 50ms `ALTER TABLE` that grabs `ACCESS EXCLUSIVE` blocks every reader and writer.
2. Expand-and-contract is the default for any breaking schema change. Add the new shape, dual-write, backfill, switch reads, then remove the old shape — across separate deploys.
3. Backfills run in batches with explicit lock and timeout settings. A single `UPDATE` over millions of rows holds long locks and fills WAL.
4. Indexes go in `CONCURRENTLY`. The non-concurrent variant blocks writes for the entire build.
5. `NOT NULL` and `CHECK` constraints accept `NOT VALID` then `VALIDATE CONSTRAINT` to avoid full-table scans under lock.
6. Replication lag amplifies migration risk. A long migration on the primary delays replicas and can break read-after-write expectations.
7. Rollback is part of the plan, not an afterthought. If the rollback path requires data restore, say so explicitly before deploying.

## Reference Map

This skill is self-contained (no `references/` library). The heuristics, delivery workflow, scenarios, and release blockers below are the canonical guidance. Confirm lock and concurrency details against current PostgreSQL docs: https://www.postgresql.org/docs/current/sql-createindex.html (`CONCURRENTLY`) and https://www.postgresql.org/docs/current/sql-altertable.html.

## Migration Heuristics

### Lock-Level Rules
- `ALTER TABLE ... ADD COLUMN` (no default, no constraint) is fast and takes `ACCESS EXCLUSIVE` only briefly. Adding a non-volatile default in PG 11+ is also fast.
- Adding a column with a `volatile` default rewrites the entire table. Avoid.
- `ALTER COLUMN TYPE` rewrites the table unless the new type is binary-compatible. Plan for it.
- `ADD CONSTRAINT ... NOT VALID` is fast; `VALIDATE CONSTRAINT` only takes `SHARE UPDATE EXCLUSIVE` and allows concurrent reads/writes.
- `CREATE INDEX CONCURRENTLY` allows concurrent writes but cannot run inside a transaction. If it fails, the index is left `INVALID` and must be dropped.

### Expand-and-Contract Pattern
1. **Expand**: Add the new column/table/constraint alongside the old, with a default that keeps old code working.
2. **Migrate writes**: Update application code to write to both old and new shapes.
3. **Backfill**: Copy historical rows from old to new in bounded batches.
4. **Migrate reads**: Switch read paths to the new shape behind a feature flag.
5. **Contract**: After all readers are on the new shape and the old shape has zero referrers, remove the old shape in a separate deploy.

### Backfill Strategy
- Batch size: start at 1,000-10,000 rows depending on row width and write traffic. Monitor lock waits and replication lag.
- Use `WHERE id > $1 AND id <= $2` with deterministic ordering. Avoid `LIMIT` without `ORDER BY` and a deterministic key.
- Set `statement_timeout` and `lock_timeout` per batch. A stuck batch should fail fast, not block the queue.
- For very large tables, run backfills out-of-band (background job) rather than in the migration tool.

### Constraint Validation
- `ALTER TABLE ... ADD CONSTRAINT ... NOT VALID` is the safe form. Existing rows are not checked.
- Backfill or repair invalid rows in a separate step.
- `VALIDATE CONSTRAINT` runs a full scan but uses a lighter lock that allows concurrent reads/writes.

### Index Operations
- `CREATE INDEX CONCURRENTLY` for any index on a table with traffic.
- `DROP INDEX CONCURRENTLY` similarly.
- Check `pg_stat_user_indexes` before dropping. An index with high `idx_scan` is in use.
- Validate the new index plan with `EXPLAIN (ANALYZE, BUFFERS)` against representative queries before relying on it.

## Delivery Workflow

### 1. Identify the Risk Surface
- Which tables are touched? Which are hot (writes per second, query frequency)?
- Which lock level does each statement take? Cross-check against `pg_locks` documentation.
- What is the replication lag tolerance for downstream consumers (read replicas, CDC pipelines, BI snapshots)?

### 2. Choose the Sequencing
- One-shot migrations: only acceptable for cold tables, additive non-breaking changes, or empty environments.
- Expand-and-contract: required for breaking schema changes on hot tables.
- Out-of-band backfill: required for large tables (>1M rows) or write-heavy environments.

### 3. Set Safety Knobs
- Set `statement_timeout` and `lock_timeout` at the session or migration level.
- Wrap each migration step in its own transaction where appropriate. Some operations (`CREATE INDEX CONCURRENTLY`) cannot be transactional.
- Plan for replica catchup time after the migration completes.

### 4. Plan the Rollback
- For each forward step, write the matching backward step.
- If a step is forward-only (e.g., dropped data, irreversible type narrowing), say so explicitly and plan a forward-fix path.
- Confirm point-in-time-recovery (PITR) is available and within the recovery window.

### 5. Verify Before Production
- Run the migration on a staging environment with realistic row counts. Synthetic 1k-row tests do not catch lock contention.
- Capture timing per statement and lock wait events.
- For backfills, confirm the batch loop completes under realistic write load, not in a quiescent system.

### 6. Monitor During Rollout
- Watch `pg_stat_activity` for blocked queries and lock waits.
- Watch replication lag. A migration on the primary that causes 10+ minute lag breaks read-after-write expectations.
- Watch WAL generation rate. A backfill that doubles WAL volume fills replica disks if undersized.

## Real-World Scenarios

- **Large NOT NULL Add**: Adding `NOT NULL` to a 50M-row column. Use this skill to: backfill nulls in batches, add constraint as `NOT VALID`, then `VALIDATE CONSTRAINT` separately.
- **Type Widening**: Changing `id` from `INTEGER` to `BIGINT` on a heavily-referenced table. Use this skill to expand-and-contract: new `id_v2` column, dual-write, backfill, switch FK references, drop old.
- **Index on Hot Table**: A new index on a 200M-row table with sustained write traffic. Use this skill to confirm `CREATE INDEX CONCURRENTLY`, plan for invalid-index recovery, and validate the new plan with `EXPLAIN`.
- **Materialized View Refresh**: A `REFRESH MATERIALIZED VIEW` blocks readers for hours. Use this skill to switch to `REFRESH MATERIALIZED VIEW CONCURRENTLY` (requires unique index) or out-of-band rebuild.
- **Replica Lag Outage**: A long migration on the primary causes replicas to fall 30 minutes behind, breaking BI dashboards. Use this skill to budget replica lag in the rollout window or split the migration across multiple windows.

## Release Blockers

Recommend a migration block when:
- the migration takes `ACCESS EXCLUSIVE` on a hot table without a `lock_timeout`
- a backfill runs as a single statement on a multi-million-row table
- an index is created without `CONCURRENTLY` on a table with active writes
- a `NOT NULL` or `CHECK` constraint is added without `NOT VALID` first
- the rollback path requires data restore but PITR window or backups were not confirmed
- expected replication lag exceeds the downstream tolerance and was not communicated

## Runtime Boundaries

Do not over-claim certainty when:
- the migration was tested on a staging dataset that does not match production row count or write traffic
- lock behavior was inferred from documentation rather than measured under load
- replication lag was not exercised with realistic primary write rates
- WAL generation rate was not measured for the backfill batch size
- the rollback step was written but not actually executed in a recovery drill

## Output Expectations

When using this skill, return:
- the proposed migration sequence with lock level per statement
- the expand-and-contract plan with deploy boundaries
- the backfill batch size, ordering, timeout, and monitoring plan
- the rollback path for each forward step (or explicit forward-fix declaration)
- the verification plan (staging row count, lock wait monitoring, replica lag budget)
- residual risks and the recommended deploy window
