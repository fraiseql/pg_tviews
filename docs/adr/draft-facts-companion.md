# ADR (draft): `pg_tviews_facts`, an optional companion extension for multi-grain fact tables

- Status: **Proposed, parked until pulled** (2026-10-27). Not to be implemented before one
  of the pull conditions below holds and the governance amendment is accepted.
- Related: [ADR 0077](0077-refresh-strategy-selection.md) (strategies),
  [ADR 0136](0136-tool-facing-surface.md) (registry), [ADR 0207](0207-concurrent-maintenance.md)
  (value locks), [ADR 0232](0232-distribution.md) (distribution),
  [aggregate TVIEWs](../user-guides/aggregate-tviews.md); fraiseql #1459 (additivity),
  #1793 (half-month, semester), #1794 (multi-grain planning), #1797, #1799.

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
   repository becomes a Cargo workspace). Its control file says `requires = 'pg_tviews'`.
   A database that never runs `CREATE EXTENSION pg_tviews_facts` gets no objects, no
   library load and no catalog rows from it.
2. **pg_tviews stays the only owner of change capture on a source table.** Two extensions
   installing triggers on one table would race on firing order and flush their own queues;
   pg_tviews already orders its flush after its delta triggers on purpose. Instead pg_tviews
   gains one small, general seam: **change consumers**.
3. **The companion owns** the measure arithmetic, the derived tables, rebuild, verify and its
   catalog. pg_tviews owns capture, partitions, TRUNCATE, savepoints, suspend and the flush
   points.
4. **The FraiseQL spec is the declaration.** The family, its grain links and rollups are
   declared beside the fact table in the API schema (`fact_tables` plus #1794's grain-link
   declaration). The companion's catalog is generated from it and drift-checked by
   `fraiseql compile --database`; it is never the source the compiler reads.

### The seam in pg_tviews (the only cost to every user)

- `pg_tviews_register_consumer(source regclass, consumer regprocedure)` and its inverse,
  recorded in the existing registry (ADR 0136).
- At its existing flush points (statement end, before COMMIT), pg_tviews calls each
  registered consumer **once per source per flush** with that flush's change batch (old and
  new row sets, as a transient relation name), and with a `truncate` event for TRUNCATE.
- Capture keeps pg_tviews' current rules: statement triggers with transition tables, row
  triggers on partitioned sources (`src/delta.rs`), the TRUNCATE trigger, savepoint-aware
  queueing, `pg_tviews_suspend_triggers()`.
- With no consumer registered, the cost is one registry check per flush. **Acceptance: not
  measurable on the existing write-path benchmark** (`docs/benchmarks/write-path.md`).

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
#1459's, a rollup that drops the tenant, a rollup that drops a measure's entity while
carrying that measure.

### Lattice and time

- day ⊂ half-month (1st–15th, 16th–end) ⊂ month ⊂ quarter ⊂ semester (from 1 Jan and
  1 Jul) ⊂ year: #1793's definitions.
- day ⊂ ISO week ⊂ ISO year: a separate chain (weeks do not nest in months).
- Buckets are computed on the stored `date`. A `timestamptz` source must declare its zone.

### Measure algebra (#1459 vocabulary, verbatim)

| kind | stored per derived row | maintenance |
|---|---|---|
| `additive` (integer or numeric only) | the sum, and a non-null count | signed deltas |
| every row | `_n`, the fact-row count | signed deltas; a bucket whose `_n` reaches 0 is **deleted** |
| `semi_additive {over, using: avg\|min\|max, entity}` | per entity and bucket (the grain keeps the entity): sum + count for `avg`, the value for `min`/`max` | `avg` by deltas; `min`/`max` recompute the touched buckets under value locks (ADR 0207) |
| `semi_additive {using: last\|first}` | per entity and bucket: the last (first) observed value **and its `over` value** | recompute the touched buckets; carrying values forward into later buckets is the planner's job, so a late row touches only its own bucket |
| `delta` | per entity and bucket: first and last observation with their `over` values | as `last`/`first` |
| `non_additive` | not stored in derived grains | (FraiseQL refuses every aggregate over it) |

A rollup that drops an entity column carries **only additive measures**. Rollups never drop
the tenant. The planner rewrites `count` to `sum(_n)` and an average of an additive measure
to `sum / _n`.

### Paths that bypass triggers

| path | treatment |
|---|---|
| TRUNCATE | pg_tviews' trigger → consumer event → the family is marked inconsistent for every tenant until rebuilt |
| ATTACH / DETACH / DROP PARTITION | an event trigger marks the family inconsistent for the affected range |
| `session_replication_role = replica`, logical replication apply | not seen: documented, and `verify` is the backstop |

### Concurrency

- Changes are applied **once per transaction**, at pg_tviews' pre-commit flush: global lock
  order, no hot-row rewrite per statement.
- A transaction-level advisory lock per `(family, tenant)` around the application.
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
- Load mode = pg_tviews' suspend for the source, then one rebuild of the touched range.
- `verify(family, tenant, sample)`: recompute and report every mismatch; CI- and cron-friendly.

### Reading derived tables

Derived tables are read through views that apply the same tenant scoping as the source's
read surface (a session-variable filter, as long as fraiseql #1799 leaves aggregates
unscoped by the server). The backup catalog uses `pg_extension_config_dump`; schema
migration tools must ignore derived tables (a test).

## Pull conditions (any one)

1. fraiseql #1794 lands, or at least its grain-link declaration is designed.
2. A real application's measured window exceeds its budget on the finest grain (for
   example, a 3-year window over 1 s at p95).
3. An application replaces a hand-written per-grain ETL.

## Milestones (once pulled)

| M | scope | exit |
|---|---|---|
| M0 (optional, before any crate) | generated SQL: statement triggers + signed-delta upserts, additive only, day → month → year, one rollup, verify | verify clean under randomized insert/update/delete batches; overhead measured |
| M1 | the consumer seam in pg_tviews; the companion with additive measures, `_n`, the full lattice, rollups, concurrency, bypass-path handling | isolation specs green; seam overhead unmeasurable; verify clean |
| M2 | semi-additive and delta measures, rebuild, load mode, `consistent` flag | verify clean under concurrent loads with late rows |
| M3 | spec-generated catalog + `compile --database` drift check (fraiseql side) | a family declared in the spec compiles, maintains and verifies end to end |

## Alternatives considered

| option | why not |
|---|---|
| A module inside `pg_tviews` (dormant until used) | zero runtime cost, but every install carries the code and its SQL surface; the companion gives the same seam with zero footprint for non-users |
| A delta strategy on aggregate TVIEWs, in core | closest to existing code (aggregate TVIEWs recompute their groups), but grows the core every user installs; the companion can still reuse that recompute path for `min`/`max`/`last` |
| A sibling extension in its own repository with its own triggers | two capture layers on one table (firing order, two queues); governance forbids new repos |
| TimescaleDB continuous aggregates | Timescale License, asynchronous refresh outside the writer's transaction, hypertables, no half-month or semester. Worth borrowing: hierarchical aggregates, and the real-time union of materialized and raw rows |
| pg_ivm | immediate maintenance with transition tables, but a relation-level lock per view (about 20× slower under concurrent writers, ADR 0207), base-table restrictions, no lattice or catalog |
| The ETL writes every grain | the drift this ADR exists to remove |
| Precedent | Oracle on-commit fast refresh + `CREATE DIMENSION` + query rewrite: the same split of maintenance (database) and planning (optimizer), and the same companion-count requirements |

## Non-goals

An asynchronous or durable queue; JSONB dimensions in rollups (rollup dimensions are
columns); ltree tree rollups (a later decision); PostgreSQL before 18.
