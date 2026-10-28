# ADR 0260: `pg_tviews_facts`, an optional companion extension for multi-grain fact tables

- Status: **Proposed, parked until pulled** (2026-10-27; seam corrected against the code
  2026-10-28). The project governance allows the companion crate (2026-10-27);
  implementation waits for one of the pull conditions below.
- Related: [ADR 0077](0077-refresh-strategy-selection.md) (strategies),
  [ADR 0136](0136-tool-facing-surface.md) (registry), [ADR 0157](0157-cascade-key-mapping.md)
  (transition tables), [ADR 0207](0207-concurrent-maintenance.md) (value locks),
  [ADR 0232](0232-distribution.md) (distribution),
  [aggregate TVIEWs](../user-guides/aggregate-tviews.md); fraiseql #1459 (additivity),
  #1766 (exact numerics), #1793 (half-month, semester), #1794 (multi-grain planning),
  #1797 (Python declaration), #1798 (date buckets), #1799 (tenant scoping); pg_tviews #261.

## Context

Statistics schemas keep one fact table at several time grains (day, week, half-month,
month, quarter, semester, year) and would like rollups that drop the entity dimension
(category × month), so that a long or wide request reads a few rows instead of the day
grain. Today an ETL usually writes every grain itself, one function per grain, and the
grains drift: a late or corrected load reaches some and not others. Nothing records which
grains are current, and the only repair is a full rebuild.

A planner that recombines grains (whole periods from coarse tables, ragged edges from finer
ones: fraiseql #1794) is only correct if the grains agree. On a synthetic table of 2,000
entities × 2 series × 3 years (4.49 M day rows), a day-grain aggregate answers a 3-year
window in 2.2 s; covering a rolling year with the full lattice reads about 21 rows per
series instead of 365. The value of the planner, and so of this extension, grows with
entity count and window length.

Most pg_tviews users have no such tables. The capability must cost them nothing.

## Decision

1. **A companion extension, `pg_tviews_facts`, in this repository** (a second crate; the
   repository becomes a Cargo workspace whose root package stays `pg_tviews`, so its paths,
   `sql/` chain and artefacts are unchanged). Its control file says
   `requires = 'pg_tviews'`. A database that never runs `CREATE EXTENSION pg_tviews_facts`
   gets no objects, no library load and no catalog rows from it. Both extensions share one
   version and one tag; a release carries two signed tarballs; crates.io publishes
   `pg_tviews` only. The companion builds for PostgreSQL 18 only.
2. **pg_tviews stays the only owner of change capture on a source table.** Two extensions
   installing triggers on one table would race on firing order and flush their own queues.
   Instead pg_tviews gains one small, general seam: **change consumers**.
3. **The companion owns** the measure arithmetic, the derived tables, rebuild, verify and its
   catalog. pg_tviews owns capture, partitions, TRUNCATE, savepoints, suspend and the
   statement-end flush.
4. **The FraiseQL spec is the declaration.** The family, its grain links and rollups are
   declared beside the fact table in the API schema (`fact_tables` plus #1794's grain-link
   declaration; Python needs #1797 first). The companion's catalog is generated from it and
   drift-checked by `fraiseql compile --database`; it is never the source the compiler reads.

### What pg_tviews does today (why the seam is new code, not a reuse)

The seam cannot ride on the TVIEW triggers:

- **Capture is per TVIEW and keyed.** A table read by N TVIEWs carries N trigger sets, each
  with its TVIEW as argument. A `local` table, and any partitioned table, gets a row
  trigger that queues the TVIEW keys of the row; only a non-partitioned `mapped` or
  `all_keys` table gets statement triggers with transition tables, which run a mapping
  query and keep the keys. No row image survives the trigger.
- **Changes are filtered.** An `UPDATE` that changes no column a TVIEW reads is dropped
  before any query. A consumer needs every change.
- **A table no TVIEW reads has no triggers at all.** A fact table is often such a table.
- **pg_tviews applies changes at the end of every outermost writing statement**, not once
  per transaction: the flush trigger and the executor hook drain the queue there, and the
  `COMMIT` hook usually finds it empty. The pre-commit callback cannot run SPI; it only
  fails a transaction that left work queued.
- **TRUNCATE refreshes at once** (no flush trigger follows it). **ATTACH / DETACH /
  DROP of a partition** go through the `ProcessUtility` hook, not an event trigger (#261).
- **`pg_tviews_suspend_triggers()` is per transaction**, not per table.

### The seam in pg_tviews (the only cost to every user)

**Registration.**
`pg_tviews_register_consumer(source regclass, consumer name, on_change text, on_truncate text)`
and `pg_tviews_unregister_consumer(source, consumer)`.

- `on_change` is SQL over two relations, `pg_tviews_old` and `pg_tviews_new`, both always
  present (empty for an `INSERT` or a `DELETE`), with `TG_OP` as `$1`. `on_truncate` is SQL
  run on `TRUNCATE`. The companion generates both per family.
- A consumer registers **statements, not a function**: a function called from the trigger
  opens its own SPI connection and cannot see the caller's transition tables (verified on
  PostgreSQL 18). Statements also keep the whole consumer visible in the catalog.
- **Fail loud at registration:** pg_tviews prepares each statement against the source's row
  type with the two relations registered. A wrong column, invalid SQL or a missing privilege
  is refused then, not on the first write.
- Registering requires owning the source. The consumer runs as the role that registered it
  (recorded as its owner, as TVIEW refreshes run as the TVIEW's owner), so a writer needs no
  privilege on the derived tables.
- Recorded in a new table, `tviews.pg_tview_consumers (source, consumer, owner, on_change,
  on_truncate)`, dumped with `pg_extension_config_dump` and exposed as `tviews.consumers`.
  The registry (`tviews.registry`, one row per TVIEW) is a view and stays as it is. This is a
  catalog revision.

**Capture.** One trigger set per source, whatever the number of consumers, separate from the
TVIEW triggers, installed with the first consumer and removed with the last:

- `AFTER INSERT / UPDATE / DELETE` **statement** triggers with transition tables, and
  `AFTER TRUNCATE`.
- On a partitioned source, on the root **and on every partition**: a statement fires only
  the triggers of the table it names. Through the root, the transition tables carry every
  partition's rows in the root's row type, and a row moved across partitions appears once,
  as old and new; through a partition, that partition's triggers fire instead (verified on
  PostgreSQL 18, also for `MERGE` and `INSERT … ON CONFLICT`, which fire one trigger per
  action). Partitions created or attached later get them through the existing hook path.
- No change filter.
- A source with no flush trigger gets one, so the executor hook knows the statement writes.

**Application: once per source per statement.** Inside the capture trigger pg_tviews runs
each consumer's statement, in consumer-name order, with the transition tables registered on
that SPI connection (as the mapping queries of ADR 0157 run). Nothing is copied or kept past
the statement:

- savepoint rollback discards the consumer's writes with the statement's own;
- autocommit and explicit transactions behave the same;
- `PREPARE TRANSACTION` works (a per-transaction temp-table batch would forbid it).

A consumer error aborts the statement, naming the consumer and the source.

**TRUNCATE** runs `on_truncate`; a source whose consumer has none refuses `TRUNCATE` (a
consumer that cannot follow it must not go stale). **Suspend**: while suspended, capture
records the source and runs no consumer; at resume or `COMMIT` each consumer of a recorded
source runs `on_truncate` ("everything may have changed").

**Zero cost.** A source without consumers has no capture trigger, so a write to it, or to any
table, pays nothing new: the consumer lookup (a per-backend memo by table OID, invalidated
like the TVIEW catalog) runs only inside a capture trigger. **Acceptance: not measurable on
the write-path benchmark** (`docs/benchmarks/write-path.md`), including its write to a table
no TVIEW reads.

### Declaration (generated from the spec)

```sql
SELECT pg_tviews_facts.declare_family(
  family => 'sales', source => 'tf_sales_day', time_column => 'day', tenant => 'tenant_id',
  dimensions => ARRAY['entity_id', 'category'],
  measures => '{"amount":{"kind":"additive"},
                "balance":{"kind":"semi_additive","over":"day","using":"last","entity":["entity_id"]}}');
SELECT pg_tviews_facts.declare_grain('sales', 'month', parent => 'day');
SELECT pg_tviews_facts.declare_rollup('sales', 'sales_category_month',
                                      grain => 'month', dimensions => ARRAY['category']);
```

Refused by name: an unknown column, a broken lattice, a measure whose vocabulary is not
#1459's, an `additive` measure that is not integer or numeric, a rollup that drops the
tenant, a rollup that drops a measure's entity while carrying that measure.

### Lattice and time

- day ⊂ half-month ⊂ month ⊂ quarter ⊂ semester ⊂ year, with #1793's definitions:
  half-month periods start on the 1st and the 16th
  (`CASE WHEN EXTRACT(DAY FROM d) <= 15 THEN date_trunc('month', d)::date ELSE date_trunc('month', d)::date + 15 END`),
  semesters on 1 January and 1 July
  (`MAKE_DATE(EXTRACT(YEAR FROM d)::int, CASE WHEN EXTRACT(MONTH FROM d) <= 6 THEN 1 ELSE 7 END, 1)`).
- day ⊂ ISO week ⊂ ISO year: a separate chain (weeks do not nest in months).
- **A bucket of a `date` is a `date`**, computed without passing through `timestamptz`
  (`date_trunc('month', d::timestamp)::date`): `date_trunc(text, date)` resolves to the
  `timestamptz` form, local midnight of the session's `TimeZone`, and a bucket typed from it
  reads `2026-07-01 00:00:00+14` (#1798; M0 shows it). A `timestamptz` source must declare
  its zone.

### Measure algebra (#1459 vocabulary, as landed in FraiseQL's `fact-table.md`)

| kind | stored per derived row | maintenance |
|---|---|---|
| `additive` (integer or numeric only) | the sum, and a non-null count | signed deltas |
| every row | `_n`, the fact-row count | signed deltas; a bucket whose `_n` reaches 0 is **deleted** |
| `semi_additive {over, using: avg\|min\|max, entity}` | per entity and bucket (the grain keeps the entity): sum + count for `avg`, the value for `min`/`max` | `avg` by deltas; `min`/`max` recompute the touched buckets under value locks (ADR 0207) |
| `semi_additive {using: last\|first}` | per entity and bucket: the last (first) observed value **and its `over` value** | recompute the touched buckets; carrying values forward into later buckets is the planner's job, so a late row touches only its own bucket |
| `delta` | per entity and bucket: first and last observation with their `over` values | as `last`/`first` |
| `non_additive` | not stored in derived grains | (FraiseQL refuses every aggregate over it) |

Derived measures keep the source column's exact type: never a float for a numeric (#1766).
A rollup that drops an entity column carries **only additive measures**. Rollups never drop
the tenant. The planner rewrites `count` to `sum(_n)` and an average of an additive measure
to `sum / _n`.

### Paths that bypass triggers

| path | treatment |
|---|---|
| TRUNCATE | the capture trigger → `on_truncate` → the family is marked inconsistent for every tenant until rebuilt |
| ATTACH / DETACH / DROP of a partition | the `ProcessUtility` hook (as for TVIEWs, #261) tells the consumers through `on_truncate` semantics, so the family is marked inconsistent (range-scoped later) |
| `pg_tviews_suspend_triggers()` | `on_truncate` at resume |
| `session_replication_role = replica`, logical replication apply | not seen: documented, and `verify` is the backstop |

### Concurrency

- Changes are applied **once per source per statement** (above), inside the writing
  statement, rows sorted by (grain, bucket key) so two statements meet buckets in one order.
- A transaction-level advisory lock per `(family, tenant)`, taken in tenant order within a
  statement, before any derived row is written: concurrent loads of one tenant serialise
  on it, so two inserts of a new bucket never race. Across statements there is no global
  order: two transactions that load tenants A then B and B then A can deadlock, and
  PostgreSQL detects it and aborts one. Loads are one tenant per transaction in practice.
  M0's isolation specs show both sides: without the lock, two loads of one tenant writing
  two buckets in opposite orders deadlock; with it they serialise, and only the
  cross-tenant opposite order deadlocks (detected, grains exact).
- Recomputed measures (`min`, `max`, `last`, `first`, `delta`) take pg_tviews' value locks,
  the fix for the READ COMMITTED stale-row class (ADR 0207).
- M1's exit includes isolation specs for concurrent same-tenant loads: no deadlock, no stale
  bucket.

### Freshness

- Synchronous maintenance needs no version: one snapshot sees consistent grains (MVCC).
- One flag per `(family, tenant)`, `consistent`, is false during a rebuild, a load mode, or
  after a bypass path. While it is false, the planner reads the finest grain only.

### Rebuild, load mode, verify

- `rebuild(family, tenant, from, to)`: delete + insert of the range in one transaction under
  the family lock (no table swap: tables are shared by tenants), flag cleared at the end.
- Load mode = `pg_tviews_suspend_triggers()` for the loading transaction (it is per
  transaction), then one rebuild of the touched range.
- `verify(family, tenant, sample)`: recompute and report every mismatch; CI- and cron-friendly.

### Reading derived tables

Derived tables keep the tenant column. They are read through views that apply the same
tenant scoping as the source's read surface: today a session-variable filter
(`tenant_id = NULLIF(current_setting('app.tenant_id', true), '')::uuid`), because fraiseql
#1799 leaves aggregate roots unscoped by the server; once #1799 scopes them from the
declaration, the views follow the declaration. The backup catalog uses
`pg_extension_config_dump`; schema migration tools must ignore derived tables (a test).

## Pull conditions (any one)

1. fraiseql #1794 lands, or at least its grain-link declaration is designed.
2. A real application's measured window exceeds its budget on the finest grain (for
   example, a 3-year window over 1 s at p95).
3. An application replaces a hand-written per-grain ETL.

### Where they stand (2026-10-28)

1. **Not met.** #1794 asks for "a declaration (SDK/TOML) linking a coarse fact table to its
   finer grain" in prose: no syntax, no pull request. Its author's measurements (above) end:
   "an optimisation for long windows on large tables, not a correctness blocker, now that
   the day grain answers exactly". The earlier partial-period code was removed (#1519).
2. **Not met.** The 2.2 s window is a synthetic table; no application budget is reported.
3. **Not met.** #1793 reports a dashboard port that keeps hand-written per-grain *views* for
   half-month and semester; no ETL is being replaced.

## Milestones (once pulled)

| M | scope | exit |
|---|---|---|
| M0 (before any crate) | generated SQL: statement triggers + signed-delta upserts, additive only, day → month → year, one rollup, verify | verify clean under randomized insert/update/delete batches; overhead measured. **Done 2026-10-28** ([results](../benchmarks/facts-m0.md)): 7,000 randomized statements (moves across buckets and partitions, `MERGE`, `ON CONFLICT`, savepoints) verify clean; four isolation specs; about 5 µs and 235 B of WAL per derived table per inserted row |
| M1 | the consumer seam in pg_tviews; the companion with additive measures, `_n`, the full lattice, rollups, concurrency, bypass-path handling | isolation specs green; seam overhead unmeasurable; verify clean |
| M2 | semi-additive and delta measures, rebuild, load mode, `consistent` flag | verify clean under concurrent loads with late rows |
| M3 | spec-generated catalog + `compile --database` drift check (fraiseql side) | a family declared in the spec compiles, maintains and verifies end to end |

## Alternatives considered

| option | why not |
|---|---|
| A module inside `pg_tviews` (dormant until used) | zero runtime cost, but every install carries the code and its SQL surface; the companion gives the same seam with zero footprint for non-users |
| A delta strategy on aggregate TVIEWs, in core | closest to existing code (aggregate TVIEWs recompute their groups), but grows the core every user installs; the companion can still reuse that recompute path for `min`/`max`/`last` |
| Widen the TVIEW triggers to copy row images when a consumer exists | couples the seam to TVIEW lineage, delivers one copy per TVIEW and needs exceptions to the change filters |
| A per-transaction batch applied at `COMMIT` | the draft of this ADR assumed pg_tviews applies there; it applies at statement end. A temp table forbids `PREPARE TRANSACTION`; a tuplestore exposed as an ephemeral relation at `COMMIT` is possible (it keeps a global lock order) but adds a flush point that procedure `COMMIT` bypasses. Revisit if M0's deadlock measurement says so |
| The consumer as a function receiving a relation name | a called function cannot see the caller's transition tables; materialising them is the temp table above |
| A sibling extension in its own repository with its own triggers | two capture layers on one table (firing order, two queues); governance forbids new repos |
| TimescaleDB continuous aggregates | Timescale License, asynchronous refresh outside the writer's transaction, hypertables, no half-month or semester. Worth borrowing: hierarchical aggregates, and the real-time union of materialized and raw rows |
| pg_ivm | immediate maintenance with transition tables, but a relation-level lock per view (about 20× slower under concurrent writers, ADR 0207), base-table restrictions, no lattice or catalog |
| The ETL writes every grain | the drift this ADR exists to remove |
| Precedent | Oracle on-commit fast refresh + `CREATE DIMENSION` + query rewrite: the same split of maintenance (database) and planning (optimizer), and the same companion-count requirements |

## Non-goals

An asynchronous or durable queue; JSONB dimensions in rollups (rollup dimensions are
columns); ltree tree rollups (a later decision); PostgreSQL before 18 for the companion.
