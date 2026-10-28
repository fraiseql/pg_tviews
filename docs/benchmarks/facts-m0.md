# Fact families, M0

[ADR 0260](../adr/0260-facts-companion.md)'s M0: one fact family kept at several grains by
the SQL the companion would generate, on a database without it. It checks that the seam's
design (statement triggers with transition tables on the root and every partition, changes
applied once per statement, a per-tenant advisory lock) keeps the grains exact, and what it
costs. The harness is [`test/bench/facts_m0/`](../../test/bench/facts_m0/run.sh).

## The family

`tf_sales_day (tenant_id, entity_id, category, day, amount numeric(14,2), qty bigint)`,
range-partitioned by year (2025–2027), kept at three derived grains:

| table | grain | keys |
|---|---|---|
| `tf_sales_month` | month | tenant, entity, category |
| `tf_sales_year` | year | tenant, entity, category |
| `tf_sales_category_month` | month | tenant, category (the entity dropped, the tenant never) |

Each derived row holds per measure the sum (NULLs as 0) and the count of non-NULL values, and
`_n`, the fact-row count. `facts_m0.generate(declaration)` emits, from a declaration shaped like
ADR 0260's `declare_family`:

- the derived tables (a `NULLS NOT DISTINCT` key, a partial index on the emptied buckets);
- one trigger function per operation: the advisory locks of the batch's tenants in order, then
  per derived table one `INSERT … SELECT` of signed sums grouped by key, sorted, `ON CONFLICT DO
  UPDATE` adding them, and a `DELETE` of the buckets whose `_n` reached 0;
- `AFTER INSERT / UPDATE / DELETE` statement triggers with transition tables on the root and on
  every partition.

Buckets are `date_trunc(<unit>, day::timestamp)::date`: computed and stored as `date`, never
through `timestamptz`, so the session's `TimeZone` cannot move them (fraiseql #1798).

## Correctness

`run.sh correctness [rounds] [statements]`: each round is one transaction of randomized,
seeded statements, and `facts_m0.verify('sales')` recomputes every grain from the day grain
after it, reporting each row that differs in either direction. The statements: batch inserts
(10 % NULL amounts), updates moving rows across months, years and partitions, updates of the
category or of a measure, deletes, `INSERT … ON CONFLICT DO UPDATE`, `MERGE` (delete, update and
insert in one statement), writes naming a partition, and blocks rolled back to a savepoint.

| run | statements | day rows at the end | verify |
|---|---:|---:|---|
| 10 rounds × 100 | 1,000 | 1,580 | clean after every round |
| 20 rounds × 300 | 6,000 | 12,964 | clean after every round |
| the same load with triggers off | 100 | — | 351 rows wrong (the check catches drift) |

`facts_m0.check_zones('sales')` inserts a row dated 2026-07-01 under `Pacific/Kiritimati` and
`America/Adak`: every bucket reads `2026-07-01` (month) and `2026-01-01` (year). A first version
that typed buckets from `date_trunc('month', day)` stored `timestamptz` buckets that read
`2026-07-01 00:00:00+14` and `-09`.

## Concurrency

`run.sh isolation` (four `pg_isolation_regress` specs):

| spec | result |
|---|---|
| one tenant, two loads writing July then August and August then July | the second waits for the first at its first statement; exact. **Without the advisory lock: deadlock** (each waits on a bucket row the other inserted) |
| one tenant, two loads into the same new buckets | the second adds to the first's rows: 200 rows, no lost update |
| READ COMMITTED: an update moving a row to September while another session deletes it | the delete waits, then removes the updated row: the September bucket goes away, nothing stale |
| two transactions loading tenants 1 then 2 and 2 then 1 | deadlock, detected by PostgreSQL; one transaction is aborted, the other commits; exact |

The last row is the cost of applying changes per statement: locks are ordered within a
statement, not across a transaction's statements. A load touches one tenant per transaction,
so it does not meet it; a transaction that writes several tenants in arbitrary order can, and
retries. A per-transaction batch applied at `COMMIT` would order the whole transaction's locks
(ADR 0260, alternatives) at the price of a new flush point.

## Overhead

`run.sh overhead 15 1`: inserts into `tf_sales_day` with the family's triggers and with
triggers off, on fresh copies of a template seeded with 100,000 day rows across 10 tenants;
stock PostgreSQL 18.6, `synchronous_commit = off`, one client, three rounds (machine shared
with other builds, load 30–46: compare within a round).

| write | ms per transaction | of which per row | WAL per row | plain ms | plain WAL per row |
|---|---:|---:|---:|---:|---:|
| `insert_1` (one row) | 0.117–0.175 | — | 1,315–1,359 B | 0.067–0.080 | 230 B |
| `insert_1000` (one statement) | 15.5–17.9 | 15.5–17.9 µs | ~900 B | 1.3–2.4 | 191 B |

Keeping three derived grains costs about 5 µs and 235 B of WAL per derived table per inserted
row, in a batch where nearly every row reaches its own entity-level bucket (2,000 entities ×
3 categories × 3 months): the costly case, as every row rewrites a bucket row. A single-row
insert pays the statement's fixed part as well (the trigger, the lock, three upserts and three
deletes): about 0.05–0.1 ms.

## Found on the way

The randomized load trips an assertion in PostgreSQL's `heap_update`
(`HEAP_XMAX_IS_LOCKED_ONLY(infomask_lock_old_tuple)`) on assert-enabled builds, 16.11 and 18.1,
with no extension loaded: an `INSERT … ON CONFLICT DO UPDATE` after a block whose `DELETE` was
rolled back to a savepoint, in one transaction (seed 0.42, statement 70). Builds without
assertions run on, and `verify` stays clean. Run the harness on such a build.
