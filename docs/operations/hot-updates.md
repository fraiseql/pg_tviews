# HOT Updates and TVIEW Storage

Nearly every refresh rewrites a TVIEW row's `data` and `updated_at`. Whether that
rewrite is a **HOT** (heap-only tuple) update decides most of what a refresh
costs physically. This page explains how TVIEW tables are set up to keep
refreshes HOT, and which choices break that.

## Why HOT matters

PostgreSQL can apply an `UPDATE` as a HOT update when:

1. **no indexed column changes**, and
2. the new row version **fits on the same heap page**.

A HOT update writes the new version next to the old one and touches no index.
The old version is reclaimed by in-page pruning, without VACUUM. A non-HOT update
inserts a new entry into **every** index on the table, leaves a dead tuple that
needs a full index-cleanup VACUUM, and clears the page's visibility-map bit, which
disables index-only scans on that page until the next VACUUM.

On the beta.17 defaults, TVIEW refreshes were **0 % HOT** in every scenario of the
[physical baseline](../benchmarks/physical-baseline-beta17.md): a GIN index on
`data` failed condition 1 on every refresh, and fillfactor 100 often failed
condition 2.

## Defaults for new TVIEWs

| Setting | Default | Effect |
|---|---|---|
| `pg_tviews.data_gin_index` | `off` | no GIN index on `data` (it would block HOT on every refresh) |
| `pg_tviews.fillfactor` | `85` | `CREATE TABLE … WITH (fillfactor = 85)`: 15 % of each page stays free for new row versions |

The indexes a TVIEW does get (`pk_<entity>`, `id`, UUID FKs, and the
`(fk_<x>, pk_<entity>)` propagation indexes) are all on columns that a `data`
refresh doesn't change. `updated_at` is never indexed. With these defaults,
single-row refreshes measure **100 % HOT** (`regress_hot_defaults.sql`).

Both settings apply when a TVIEW is created, through `pg_tviews_create()` or
`CREATE TABLE tv_* AS SELECT …`. **Existing TVIEWs are not changed.**

## Per-TVIEW overrides

Both GUCs are user-settable, so scope them to one creation with `SET LOCAL`:

```sql
BEGIN;
SET LOCAL pg_tviews.fillfactor = 100;        -- append-mostly: rows are rarely refreshed
SET LOCAL pg_tviews.data_gin_index = on;     -- top-level containment queries needed
SELECT pg_tviews_create('tv_event_log', $$ … $$);
COMMIT;
```

### When fillfactor 100 is right

A TVIEW whose rows are written once and rarely refreshed (event logs, archives)
gains nothing from free space. Fillfactor 85 costs about 15 % more heap in
exchange for HOT updates, fewer index insertions, less VACUUM work and a stable
visibility map, which only pays off when rows get refreshed.

## Indexes on `data`

**Any** index whose key includes a column that refreshes rewrite (`data`, or an
expression over it) turns every refresh of that table into a non-HOT update. This
includes expression indexes such as `((data->>'email'))`: the expression's input
column changes, so HOT is impossible even when the extracted value is unchanged.

Before adding one, check that queries actually use it. A top-level GIN index on
`data` only serves top-level containment (`data @> '{"k": …}'`, `?`, `?|`, `?&`).
FraiseQL emits none of these; its filter shapes are:

| FraiseQL filter | Predicate | Served by a GIN on `data`? |
|---|---|---|
| equality | `data->>'email' = $1` | no |
| array contains | `(data->'tags')::jsonb @> $1` | no |

When a filter really needs an index, prefer an expression btree on exactly the
extracted path, and accept that it costs HOT for that TVIEW:

```sql
CREATE INDEX idx_tv_user_email ON tv_user ((data->>'email'));
```

To find existing indexes that cost HOT without serving any query:

```sql
SELECT indexrelid::regclass AS index, idx_scan
FROM pg_stat_user_indexes
WHERE relname LIKE 'tv\_%' AND indexrelname LIKE '%\_data\_gin' AND idx_scan = 0;
```

## Existing TVIEWs

TVIEWs created before these defaults keep their GIN index and fillfactor 100. To
move one to the new defaults:

```sql
-- only if the index is unused (idx_scan = 0 above)
DROP INDEX idx_tv_post_data_gin;

-- applies to newly written pages only
ALTER TABLE tv_post SET (fillfactor = 85);
```

`ALTER TABLE … SET (fillfactor)` only affects pages written afterwards. Existing
full pages gain free space as refreshes move rows, or all at once with a rewrite
(`VACUUM FULL tv_post` or `CLUSTER`, both taking an `ACCESS EXCLUSIVE` lock).

## Checking HOT on a live TVIEW

```sql
SELECT relname, n_tup_upd, n_tup_hot_upd,
       round(100.0 * n_tup_hot_upd / nullif(n_tup_upd, 0), 1) AS hot_pct,
       n_tup_newpage_upd, n_dead_tup
FROM pg_stat_user_tables
WHERE relname LIKE 'tv\_%'
ORDER BY n_tup_upd DESC;
```

A low `hot_pct` with a high `n_tup_newpage_upd` points to fillfactor. A `hot_pct`
near 0 with few new-page updates points to an index on a rewritten column.
