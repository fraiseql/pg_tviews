# `pg_tviews_profile()`: per-TVIEW physical health

```sql
SELECT * FROM pg_tviews_profile();              -- every TVIEW
SELECT * FROM pg_tviews_profile('post');        -- one TVIEW (post, tv_post or public.tv_post)
SELECT * FROM pg_tviews_profile(NULL, 5000);    -- fan-out warning threshold
```

Reports what refreshes cost PostgreSQL for each TVIEW, from the catalogs and the
statistics views only: it never scans a TVIEW, writes nothing and works on a hot
standby. Intended for monitoring scrapes (`/metrics`, `doctor`) and lint tools.
`tview` names one TVIEW in any form the API accepts
([naming a TVIEW](api.md#naming-a-tview)); NULL reports every TVIEW. What refreshes did
is in [`tviews.stats`](read-contract.md#tviewsstats).

**Stability:** the columns below are a contract. New columns may be added at the end;
existing ones are never renamed or retyped. Warning texts may be reworded.

| column | type | meaning |
|---|---|---|
| `entity` | text | TVIEW entity |
| `schema` | text | schema of the `tv_*` table |
| `name` | text | `tv_*` table name, unquoted |
| `persistence` | text | `logged` or `unlogged` |
| `replica_readable` | boolean | whether a hot standby can read it (LOGGED) |
| `rows_estimate` | bigint | `pg_class.reltuples`; NULL before the first ANALYZE |
| `heap_bytes`, `index_bytes`, `toast_bytes` | bigint | main fork, all indexes, TOAST data |
| `avg_row_width`, `data_avg_width` | integer | from `pg_stats` (sum of columns; `data` alone) |
| `fillfactor` | integer | heap fillfactor (100 when unset) |
| `n_tup_upd`, `n_tup_hot_upd` | bigint | updates and HOT updates since the stats reset |
| `hot_ratio` | double precision | `n_tup_hot_upd / n_tup_upd`; NULL without updates |
| `n_dead_tup`, `last_vacuum`, `last_autovacuum` | | from `pg_stat_all_tables` |
| `all_visible_fraction` | double precision | from `pg_visibility_map_summary()` when the `pg_visibility` extension is installed, else NULL |
| `unused_indexes` | text[] | never-scanned indexes, excluding the primary key, unique indexes and propagation indexes |
| `missing_propagation_indexes` | text[] | integer `fk_*` columns no index leads with |
| `fanout` | jsonb | per `fk_*` column, estimated rows per key: `{"fk_user": {"p50", "p99", "max"}}`, from planner statistics (run ANALYZE) |
| `warnings` | text[] | advice, below |

## Warnings

| condition | advice |
|---|---|
| an `fk_*` column has no leading index | cascades scan the whole TVIEW: run `pg_tviews_ensure_propagation_indexes(tview)` |
| more than 1000 updates, HOT ratio under 50%, an index on `data` or `updated_at` | that index prevents HOT updates |
| a GIN index on `data` never scanned since the statistics reset | it costs every refresh for no reader |
| fillfactor 100 and more updates than rows | refreshed rows cannot stay on their page (option `fillfactor`, default 85) |
| TOAST data over 30% of the table | each refresh rewrites whole documents ([ADR 0094](../adr/0094-large-document-refresh.md)) |
| estimated p99 fan-out of an `fk_*` over `fanout_warn` (default 1000) | one parent change refreshes that many rows |
| UNLOGGED | not readable on standbys, empty after promotion or a crash restart (option `logged`, [replication](../operations/replication.md)) |
| dead tuples over 20% of the rows | autovacuum is behind |

Counters come from the cumulative statistics system and cover the time since the
last statistics reset.
