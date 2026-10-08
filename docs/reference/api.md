# API reference

Every SQL function pg_tviews provides, with its exact signature, who may call it and
the SQLSTATEs worth catching.

All objects live in the schema `tviews`. Call the functions schema-qualified, as this
page does, or put `tviews` on the `search_path`. The server must preload the library
(`shared_preload_libraries = 'pg_tviews'`, see the [Operator guide](../user-guides/operators.md)).

The examples on this page run in order in a fresh database. They use two tables:

```sql
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE TABLE tb_user (
    pk_user bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id      uuid NOT NULL DEFAULT gen_random_uuid(),
    name    text NOT NULL
);
CREATE TABLE tb_post (
    pk_post bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id      uuid NOT NULL DEFAULT gen_random_uuid(),
    fk_user bigint NOT NULL REFERENCES tb_user,
    title   text NOT NULL
);
INSERT INTO tb_user (name) VALUES ('Alice'), ('Bob');
INSERT INTO tb_post (fk_user, title) VALUES (1, 'Hello'), (1, 'Again'), (2, 'Hi');
```

## Function index

| Function | Returns | Who may call it |
|---|---|---|
| [`pg_tviews_create(tview_name text, select_sql text)`](#pg_tviews_create) | `text` | `CREATE` on the schema |
| [`pg_tviews_create_aggregate(tview_name text, select_sql text, group_keys jsonb)`](#pg_tviews_create_aggregate) | `text` | `CREATE` on the schema |
| [`pg_tviews_create_or_replace(tview_name text, query text, options jsonb DEFAULT '{}')`](#pg_tviews_create_or_replace) | `text` | owner (new: `CREATE` on the schema) |
| [`pg_tviews_drop(tview_name text, if_exists boolean DEFAULT false, cascade boolean DEFAULT false)`](#pg_tviews_drop) | `text` | owner |
| [`pg_tviews_reregister(tview_name text)`](#pg_tviews_reregister) | `text` | owner |
| [`pg_tviews_reregister_all(strict boolean DEFAULT false)`](#pg_tviews_reregister_all) | `TABLE(entity text, status text)` | operator, and owner of each TVIEW |
| [`pg_tviews_refresh(entity text)`](#pg_tviews_refresh) | `void` | owner |
| [`pg_tviews_refresh_time_dependent(tview text DEFAULT NULL)`](#pg_tviews_refresh_time_dependent) | `SETOF text` | owner |
| [`pg_tviews_refresh_all()`](#pg_tviews_refresh_all-and-pg_tviews_refresh_all_entities) | `jsonb` | operator |
| [`pg_tviews_refresh_all_entities()`](#pg_tviews_refresh_all-and-pg_tviews_refresh_all_entities) | `void` | operator |
| [`pg_tviews_show_cascade_path(entity text)`](#pg_tviews_show_cascade_path) | `TABLE(depth integer, entity_name text, depends_on text)` | anyone |
| [`pg_tviews_mapping_query(tview text, base_table oid)`](#pg_tviews_mapping_query) | `text` | anyone |
| [`pg_tviews_ensure_propagation_indexes(entity text DEFAULT NULL, dry_run boolean DEFAULT false)`](#pg_tviews_ensure_propagation_indexes) | `SETOF text` | operator, and owner of each TVIEW |
| [`pg_tviews_suspend_triggers()`](#suspending-refresh) | `void` | anyone (own session) |
| [`pg_tviews_resume_triggers()`](#suspending-refresh) | `void` | anyone (own session) |
| [`pg_tviews_is_suspended()`](#suspending-refresh) | `boolean` | anyone |
| [`pg_tviews_suspended_entities()`](#suspending-refresh) | `text[]` | anyone |
| [`pg_tviews_flush_and_report(max_entities integer DEFAULT 500, include_data boolean DEFAULT true, reset boolean DEFAULT true)`](#pg_tviews_flush_and_report) | `jsonb` | anyone (own transaction) |
| [`pg_tviews_set_typename(entity text, typename text)`](#pg_tviews_set_typename) | `void` | owner |
| [`pg_tviews_set_logged(entity text, logged boolean)`](#storage-replication-and-recovery) | `void` | operator, and owner |
| [`pg_tviews_is_replica_readable(entity text)`](#storage-replication-and-recovery) | `boolean` | anyone |
| [`pg_tviews_replication_status()`](#storage-replication-and-recovery) | `TABLE(entity text, persistence text, replica_readable boolean, is_empty boolean, needs_rebuild boolean)` | anyone |
| [`pg_tviews_rebuild_all(only_empty boolean DEFAULT true)`](#storage-replication-and-recovery) | `TABLE(entity text, rows bigint)` | operator |
| [`pg_tviews_recover_after_crash(entity_name text)`](#storage-replication-and-recovery) | `boolean` | owner |
| [`pg_tviews_health_check()`](#pg_tviews_health_check) | `TABLE(status text, component text, message text, severity text)` | anyone |
| [`pg_tviews_queue_stats()`](#pg_tviews_queue_stats) | `jsonb` | anyone |
| [`pg_tviews_debug_queue()`](#pg_tviews_debug_queue) | `jsonb` | anyone |
| [`pg_tviews_performance_stats()`](#pg_tviews_performance_stats) | `TABLE(entity text, table_size text, total_size text, row_count bigint, index_count integer)` | anyone |
| [`pg_tviews_profile(p_entity text DEFAULT NULL, fanout_warn bigint DEFAULT 1000)`](#pg_tviews_profile) | `TABLE(…)` | anyone |
| [`pg_tviews_version()`](#version-and-catalog) | `text` | anyone |
| [`pg_tviews_check_jsonb_delta()`](#version-and-catalog) | `boolean` | anyone |
| [`pg_tviews_catalog_revision()`](#version-and-catalog) | `integer` | anyone |
| [`contract_version()`](#version-and-catalog) | `integer` | anyone |

"Owner" and "operator" are defined in [Privileges](#privileges). Functions not listed
here are [internal](#internal-functions).

## Creating and changing TVIEWs

`tview_name` is `tv_<entity>`, `<entity>` or `schema.tv_<entity>`; an entity names one
TVIEW in the whole database. The definition is exactly one `SELECT` with a
`pk_<entity>` key column; see the [DDL reference](ddl.md) for what it may contain.
`CREATE TABLE tv_<entity> AS SELECT …` is the same as `pg_tviews_create`.

### pg_tviews_create

```text
tviews.pg_tviews_create(tview_name text, select_sql text) RETURNS text
```

Creates the TVIEW: a backing view `tviews.<schema>__tv_<entity>`, the `tv_<entity>`
table filled from it, its indexes and the triggers on the tables it reads. Returns
`TVIEW '<name>' created successfully`. An existing TVIEW is an error (`42P07`); use
`pg_tviews_create_or_replace` to change one.

```sql
SELECT tviews.pg_tviews_create('tv_user', $$
    SELECT pk_user, id, jsonb_build_object('id', id, 'name', name) AS data
    FROM tb_user $$);

SELECT tviews.pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user,
           jsonb_build_object('id', p.id, 'title', p.title, 'author', u.data) AS data
    FROM tb_post p JOIN tv_user u ON u.pk_user = p.fk_user $$);
```

Indexes created on the table:

| Index | Columns | Purpose |
|---|---|---|
| primary key | `pk_<entity>` | row identity |
| `idx_<tv>_id` | `id` | lookup by public UUID |
| `idx_<tv>_<column>_<pk>` | each column the plan looks rows up by (`fk_user` above), then `pk_<entity>` | propagation lookups; without it every cascade step scans the TVIEW |
| `idx_<tv>_data_gin` | `data` (GIN) | only with `data_gin_index`; makes every refresh a non-HOT update |

The table is created with `fillfactor = pg_tviews.fillfactor` (default 85) and
UNLOGGED unless `pg_tviews.unlogged_by_default` is off; see
[HOT updates](../operations/hot-updates.md) and [Replication](../operations/replication.md).

### pg_tviews_create_aggregate

```text
tviews.pg_tviews_create_aggregate(tview_name text, select_sql text, group_keys jsonb) RETURNS text
```

Creates a TVIEW with one row per `GROUP BY` key. `group_keys` maps each table the
definition reads to the column holding the group key. Same as
`pg_tviews_create_or_replace(…, '{"group_keys": …}')`. See
[Aggregate TVIEWs](../user-guides/aggregate-tviews.md).

```sql
SELECT tviews.pg_tviews_create_aggregate('tv_user_summary', $$
    SELECT p.fk_user AS pk_user_summary, u.id,
           jsonb_build_object('posts', count(*)) AS data
    FROM tb_post p JOIN tb_user u ON u.pk_user = p.fk_user
    GROUP BY p.fk_user, u.id $$,
    '{"tb_post": "fk_user", "tb_user": "pk_user"}');
```

### pg_tviews_create_or_replace

```text
tviews.pg_tviews_create_or_replace(tview_name text, query text, options jsonb DEFAULT '{}')
    RETURNS text  -- 'created' | 'unchanged' | 'altered' | 'replaced' | 'rebuilt'
```

Creates the TVIEW, or brings an existing one to `query` and `options` with the
smallest change: `unchanged`, `altered` (options only), `replaced` (same columns, the
backing view replaced in place) or `rebuilt` (new columns: view and table rebuilt).
The [contract for tools](read-contract.md) states exactly what each outcome changes.

`options` is a JSON object; an unknown key or a value of the wrong type is an error
(`22023`). An omitted option keeps the TVIEW's current value (on create: the default).

| Option | Value | Default | Meaning |
|---|---|---|---|
| `logged` | boolean | `NOT pg_tviews.unlogged_by_default` | LOGGED table (readable on standbys) or UNLOGGED |
| `fillfactor` | integer 10–100 | `pg_tviews.fillfactor` (85) | heap fillfactor of the table |
| `data_gin_index` | boolean | `pg_tviews.data_gin_index` (false) | GIN index on `data` |
| `group_keys` | object or `null` | `null` | `{"<table>": "<group key column>"}`: an aggregate TVIEW; `null`: a plain one |
| `uncascaded_policy` | `"error"`, `"full_refresh"`, `"warn"` | `pg_tviews.uncascaded_policy` (`error`) | what a write to a table no cascade reaches does: refuse the TVIEW, refresh it whole, or warn and leave rows stale ([details](ddl.md#tables-no-cascade-reaches)) |
| `uncascaded_tables` | object | `{}` | a policy per table, `{"public.tb_locale": "full_refresh"}`; a table the definition does not read, or whose writes are traced, is refused ([details](ddl.md#a-policy-per-table)) |
| `function_reads` | object | `{}` | the tables each non-immutable function reads, `{"public.label_suffix()": ["public.tb_setting"]}` (`[]` for none); functions are named with their argument types ([details](ddl.md#functions-that-read-tables)) |
| `time_refresh` | `"external"` or `null` | `pg_tviews.time_refresh` (`none`) | accept a definition reading the current time; `pg_tviews_refresh_time_dependent()` is called at the boundary ([details](ddl.md#time-dependent-tviews)) |

```sql
-- Options only: 'altered'
SELECT tviews.pg_tviews_create_or_replace('tv_user', $$
    SELECT pk_user, id, jsonb_build_object('id', id, 'name', name) AS data
    FROM tb_user $$, '{"fillfactor": 90, "logged": true}');

-- A function reading a table, that table refreshing the TVIEW whole
CREATE TABLE tb_setting (value text NOT NULL);
INSERT INTO tb_setting VALUES (' (draft)');
CREATE FUNCTION public.label_suffix() RETURNS text STABLE LANGUAGE sql
    AS $$ SELECT value FROM public.tb_setting $$;
SELECT tviews.pg_tviews_create_or_replace('tv_label', $$
    SELECT pk_post AS pk_label, id, title || public.label_suffix() AS label
    FROM tb_post $$, '{
      "function_reads": {"public.label_suffix()": ["public.tb_setting"]},
      "uncascaded_tables": {"public.tb_setting": "full_refresh"}}');
```

### pg_tviews_drop

```text
tviews.pg_tviews_drop(tview_name text, if_exists boolean DEFAULT false,
                      cascade boolean DEFAULT false) RETURNS text
```

Drops the table, its backing view, its triggers and its registration, like
`DROP TABLE tv_<entity>`. With `if_exists`, a missing TVIEW raises a NOTICE and
returns `TVIEW '<name>' does not exist, nothing dropped`; otherwise it is `42704`.
Without `cascade` the drop is RESTRICT: objects depending on the table (another
TVIEW embedding it, a view) make it fail; with `cascade` they are dropped too.
A qualified name must name the TVIEW's schema. Returns
`TVIEW '<name>' dropped successfully`.

```sql
SELECT tviews.pg_tviews_drop('tv_label');
SELECT tviews.pg_tviews_drop('tv_nonexistent', if_exists => true);
```

### pg_tviews_reregister

```text
tviews.pg_tviews_reregister(tview_name text) RETURNS text
```

Re-derives the TVIEW's stored propagation plan
([ADR 0203](../adr/0203-propagation-plan.md)) and its triggers from its definition with
the installed release, without touching its rows, and clears `needs_reregister`.
Returns `reregistered`. It is the repair a write or the health check names when a
TVIEW's plan does not decode.

```sql
SELECT tviews.pg_tviews_reregister('post');
```

### pg_tviews_reregister_all

```text
tviews.pg_tviews_reregister_all(strict boolean DEFAULT false)
    RETURNS TABLE(entity text, status text)
```

`pg_tviews_reregister` for every TVIEW, dependencies first, each in its own
subtransaction: `status` is `reregistered` or the error that stopped it. With
`strict`, it raises at the end when any failed. Run it after
`ALTER EXTENSION pg_tviews UPDATE` when the release notes say so or
`pg_tviews_health_check()` reports TVIEWs to re-register. Operator function.

```sql
SELECT * FROM tviews.pg_tviews_reregister_all();
```

## Refresh and repair

Writes refresh the TVIEWs by themselves. These functions repair what the triggers did
not see (`session_replication_role = replica`, disabled triggers) and refresh TVIEWs
that read the current time.

### pg_tviews_refresh

```text
tviews.pg_tviews_refresh(entity text) RETURNS void
```

Rebuilds `tv_<entity>` from its view, then every TVIEW whose view reads it, directly or
through others, dependencies first. Each rebuild is a `TRUNCATE` and an
`INSERT … SELECT` holding an `ACCESS EXCLUSIVE` lock on that TVIEW until the
transaction ends, run as the TVIEW's owner. Requires owning `tv_<entity>` (`42501`);
an unknown entity is `42704`.

```sql
SELECT tviews.pg_tviews_refresh('user');   -- tv_user, then tv_post, which embeds it
```

### pg_tviews_refresh_time_dependent

```text
tviews.pg_tviews_refresh_time_dependent(tview text DEFAULT NULL) RETURNS SETOF text
```

Refreshes in full the TVIEWs whose definitions read the current time
(`tviews.registry.time_dependent`): `tview`, or every such TVIEW the caller owns.
The TVIEWs reading them follow in the same flush. Returns the TVIEWs refreshed,
dependencies first. A named TVIEW that is not one, or reads no time, is `22023`.
Call it at the boundary the rows depend on, from pg_cron or the application
([Time-dependent TVIEWs](ddl.md#time-dependent-tviews)).

```sql
SELECT tviews.pg_tviews_create_or_replace('tv_recent_post', $$
    SELECT pk_post AS pk_recent_post, id,
           jsonb_build_object('title', title, 'today', CURRENT_DATE) AS data
    FROM tb_post $$, '{"time_refresh": "external"}');
SELECT * FROM tviews.pg_tviews_refresh_time_dependent();
```

### pg_tviews_refresh_all and pg_tviews_refresh_all_entities

```text
tviews.pg_tviews_refresh_all() RETURNS jsonb
tviews.pg_tviews_refresh_all_entities() RETURNS void
```

Rebuild every TVIEW once, dependencies first, each as its owner, and leave nothing
queued. `pg_tviews_refresh_all()` returns `{"refreshed_count", "order", "duration_ms"}`
and refuses to run while refresh is suspended (`55000`);
`pg_tviews_refresh_all_entities()` reports the count as an INFO message. Operator
functions.

```sql
SELECT tviews.pg_tviews_refresh_all() ->> 'refreshed_count' AS refreshed;
```

### pg_tviews_show_cascade_path

```text
tviews.pg_tviews_show_cascade_path(entity text)
    RETURNS TABLE(depth integer, entity_name text, depends_on text)
```

`entity` at depth 0, then the TVIEWs that read it, with their depth: what
`pg_tviews_refresh(entity)` rebuilds.

```sql
SELECT * FROM tviews.pg_tviews_show_cascade_path('user');
```

### pg_tviews_mapping_query

```text
tviews.pg_tviews_mapping_query(tview text, base_table oid) RETURNS text
```

The query that maps the rows a statement changed in `base_table`, read from a
relation named `pg_tviews_delta`, to keys of `tview`, from the TVIEW's stored plan.
NULL when writes to that table do not map through a query of their own (a table
propagated through an embed, or one refreshing every key), when the TVIEW does not
read the table, or when `tview` is not a TVIEW. See
[How a write finds the TVIEW rows to refresh](ddl.md#how-a-write-finds-the-tview-rows-to-refresh).

```sql
SELECT tviews.pg_tviews_mapping_query('tv_user_summary', 'tb_post'::regclass);
-- SELECT DISTINCT "fk_user" FROM pg_tviews_delta
```

### pg_tviews_ensure_propagation_indexes

```text
tviews.pg_tviews_ensure_propagation_indexes(entity text DEFAULT NULL,
                                            dry_run boolean DEFAULT false) RETURNS SETOF text
```

Creates the missing propagation index of every column the plan looks the TVIEW's rows
up by (`entity`, or every TVIEW when NULL). A column counts as covered when any index
leads with it. Returns one `CREATE INDEX IF NOT EXISTS …` per missing index, executed
unless `dry_run`. Idempotent. On large tables, take the dry-run output and run it with
`CREATE INDEX CONCURRENTLY`. Operator function; requires owning each TVIEW it looks at.

```sql
SELECT * FROM tviews.pg_tviews_ensure_propagation_indexes(dry_run => true);
```

## Suspending refresh

```text
tviews.pg_tviews_suspend_triggers() RETURNS void
tviews.pg_tviews_resume_triggers() RETURNS void
tviews.pg_tviews_is_suspended() RETURNS boolean
tviews.pg_tviews_suspended_entities() RETURNS text[]
```

`pg_tviews_suspend_triggers()` defers refreshes for a bulk load; calls nest and roll
back with a savepoint. When the outermost `pg_tviews_resume_triggers()` runs, every
TVIEW changed while suspended, and every TVIEW reading one of them, is rebuilt.
Resume without suspend is `55000`. Suspension ends with the transaction: an explicit
`COMMIT` catches up the same way, an implicit commit logs a WARNING naming the stale
TVIEWs. `pg_tviews_suspended_entities()` lists the TVIEWs changed so far.

```sql
BEGIN;
SELECT tviews.pg_tviews_suspend_triggers();
INSERT INTO tb_post (fk_user, title) SELECT 2, 'Bulk ' || g FROM generate_series(1, 1000) g;
SELECT tviews.pg_tviews_suspended_entities();
SELECT tviews.pg_tviews_resume_triggers();
COMMIT;
```

## Change reports

### pg_tviews_flush_and_report

```text
tviews.pg_tviews_flush_and_report(max_entities integer DEFAULT 500,
                                  include_data boolean DEFAULT true,
                                  reset boolean DEFAULT true) RETURNS jsonb
```

Flushes the queue, then reports the TVIEW rows this transaction changed, for a GraphQL
cascade response: each entry carries its type name, `id` and (with `include_data`) the
row's `data`; a deleted entry is `{"__typename", "id"}`. Past `max_entities` entries,
or when the journal overflowed `pg_tviews.report_max_tracked`, `truncated` is true and
`invalidated_types` lists the types left out. With `reset`, the next call reports only
later changes. See [GraphQL cascade](../user-guides/graphql-cascade.md).

```sql
BEGIN;
UPDATE tb_user SET name = 'Alice B.' WHERE pk_user = 1;
SELECT tviews.pg_tviews_flush_and_report(include_data => false);
COMMIT;
```

### pg_tviews_set_typename

```text
tviews.pg_tviews_set_typename(entity text, typename text) RETURNS void
```

Sets the GraphQL type name reported for `entity`; NULL resets it to the PascalCase of
the entity. Requires owning the TVIEW.

```sql
SELECT tviews.pg_tviews_set_typename('post', 'BlogPost');
```

## Storage, replication and recovery

```text
tviews.pg_tviews_set_logged(entity text, logged boolean) RETURNS void
tviews.pg_tviews_is_replica_readable(entity text) RETURNS boolean
tviews.pg_tviews_replication_status()
    RETURNS TABLE(entity text, persistence text, replica_readable boolean,
                  is_empty boolean, needs_rebuild boolean)
tviews.pg_tviews_rebuild_all(only_empty boolean DEFAULT true) RETURNS TABLE(entity text, rows bigint)
tviews.pg_tviews_recover_after_crash(entity_name text) RETURNS boolean
```

- `pg_tviews_set_logged` switches `tv_<entity>` to LOGGED (readable on standbys) or
  UNLOGGED: `ALTER TABLE … SET [UN]LOGGED` rewrites it under an `ACCESS EXCLUSIVE`
  lock. Operator function; requires owning the TVIEW.
- `pg_tviews_is_replica_readable` is true for a LOGGED TVIEW, false for an UNLOGGED
  one, NULL for an unknown entity. `pg_tviews_replication_status` reports every TVIEW
  and is safe on a standby.
- `pg_tviews_rebuild_all` refills the UNLOGGED TVIEWs a crash restart, promotion or
  restore left empty (every TVIEW with `only_empty => false`), dependencies first,
  each as its owner; it refuses to run during recovery. Operator function; it also
  reads each TVIEW as the caller (to find the empty ones and count rows), so the
  caller needs `SELECT` on the `tv_*` tables.
- `pg_tviews_recover_after_crash` does the same for one entity, returning whether it
  had to. Requires owning the TVIEW.

```sql
SELECT tviews.pg_tviews_set_logged('post', true);
SELECT * FROM tviews.pg_tviews_replication_status();
SELECT * FROM tviews.pg_tviews_rebuild_all();
SELECT tviews.pg_tviews_recover_after_crash('post');
```

See [Replication](../operations/replication.md).

## Monitoring

### pg_tviews_health_check

```text
tviews.pg_tviews_health_check()
    RETURNS TABLE(status text, component text, message text, severity text)
```

One row per check. `status` is `OK`, `WARNING` or `ERROR`; `severity` is `info`,
`warning` or `error`; `component` is one of `extension`, `jsonb_delta`, `catalog`,
`metadata`, `plans` (stored plans that do not decode), `reregister` (TVIEWs marked
`needs_reregister`), `triggers` and `tviews`. A check that cannot run is an `ERROR` row.

```sql
SELECT component, status, message FROM tviews.pg_tviews_health_check()
WHERE status <> 'OK';
```

### pg_tviews_queue_stats

```text
tviews.pg_tviews_queue_stats() RETURNS jsonb
```

Counters of the calling backend. Queue, flush and cache counters cover the current
transaction; the `direct_patch_*`, `view_recomputes`, `refresh_noop_skipped`,
`catalog_lookups` and `propagation_pruned` counters accumulate over the session. Read
it from the session doing the writes: another session sees its own, usually zero.

| Key | Meaning |
|---|---|
| `queue_size` | refreshes queued now |
| `flushes` | flushes that refreshed something in this transaction |
| `total_refreshes` | TVIEW rows refreshed by flushes |
| `total_iterations`, `max_iterations` | propagation rounds in all flushes, and in the longest |
| `total_timing_ms` | time spent flushing |
| `graph_cache_hits`, `graph_cache_misses`, `graph_cache_hit_rate` | dependency-graph cache |
| `table_cache_hits`, `table_cache_misses`, `table_cache_hit_rate` | catalog-row cache |
| `direct_patch_captured`, `direct_patches_applied`, `direct_patch_fallbacks` | direct patches captured, applied, and fallen back to a recompute |
| `view_recomputes` | rows recomputed from the backing view |
| `refresh_noop_skipped` | recomputed rows not rewritten because their content was unchanged |
| `catalog_lookups` | catalog reads by the row triggers |
| `propagation_pruned` | cascades skipped because the changed columns are not read by the parent |

```sql
BEGIN;
UPDATE tb_post SET title = 'Hello again' WHERE pk_post = 1;
SELECT tviews.pg_tviews_queue_stats() ->> 'flushes' AS flushes;
COMMIT;
```

### pg_tviews_debug_queue

```text
tviews.pg_tviews_debug_queue() RETURNS jsonb
```

The refresh queue of the current transaction, as `[{"entity": …, "pk": …}, …]`; empty
between statements, since every statement flushes.

```sql
SELECT tviews.pg_tviews_debug_queue();
```

### pg_tviews_performance_stats

```text
tviews.pg_tviews_performance_stats()
    RETURNS TABLE(entity text, table_size text, total_size text,
                  row_count bigint, index_count integer)
```

Size (`pg_size_pretty`), exact row count and index count of each TVIEW, largest
first. `row_count` is a `count(*)` run with the caller's privileges: NULL, with a
NOTICE, for a TVIEW the caller cannot read.

```sql
SELECT * FROM tviews.pg_tviews_performance_stats();
```

### pg_tviews_profile

```text
tviews.pg_tviews_profile(p_entity text DEFAULT NULL, fanout_warn bigint DEFAULT 1000)
    RETURNS TABLE(entity text, tview text, persistence text, replica_readable boolean,
                  rows_estimate bigint, heap_bytes bigint, index_bytes bigint,
                  toast_bytes bigint, avg_row_width integer, data_avg_width integer,
                  fillfactor integer, n_tup_upd bigint, n_tup_hot_upd bigint,
                  hot_ratio double precision, n_dead_tup bigint,
                  last_vacuum timestamptz, last_autovacuum timestamptz,
                  all_visible_fraction double precision, unused_indexes text[],
                  missing_propagation_indexes text[], fanout jsonb, warnings text[])
```

The per-TVIEW physical health report; see [profile.md](profile.md).

```sql
SELECT entity, persistence, hot_ratio, warnings FROM tviews.pg_tviews_profile();
```

### Version and catalog

```text
tviews.pg_tviews_version() RETURNS text            -- the library version
tviews.pg_tviews_check_jsonb_delta() RETURNS boolean -- jsonb_delta installed
tviews.pg_tviews_catalog_revision() RETURNS integer  -- catalog revision of the installed extension SQL
tviews.contract_version() RETURNS integer           -- version of the read contract (tviews.registry)
```

```sql
SELECT tviews.pg_tviews_version(), tviews.pg_tviews_check_jsonb_delta(),
       tviews.pg_tviews_catalog_revision(), tviews.contract_version();
```

## Privileges

pg_tviews follows the rules of `ALTER TABLE` and `REFRESH MATERIALIZED VIEW`
([ADR 0136](../adr/0136-tool-facing-surface.md), last amendment).

- **Owner.** Every function that acts on one TVIEW requires owning its `tv_*` table:
  being its owner or a member of the owning role, or of the role that owns the
  extension (superusers pass). The check runs before any lock is taken. Anyone else
  gets SQLSTATE `42501` (`must be owner of TVIEW tv_<entity>`). This covers
  `pg_tviews_create_or_replace` (on an existing TVIEW), `pg_tviews_drop`,
  `pg_tviews_reregister`, `pg_tviews_refresh`, `pg_tviews_refresh_time_dependent`,
  `pg_tviews_set_typename`, `pg_tviews_set_logged`, `pg_tviews_recover_after_crash`
  and `pg_tviews_ensure_propagation_indexes`.
- **Rebuilds run as the TVIEW's owner.** Every refresh, single or bulk, reads the
  backing view and writes the `tv_*` table as the owner of that `tv_*` table, in a
  security-restricted operation with `search_path = pg_catalog, pg_temp` and fixed
  rendering settings (`TimeZone` `UTC`, `DateStyle` `ISO, YMD`). A function the
  backing view calls never runs with the caller's privileges.
- **Operator.** The maintenance functions that act on every TVIEW are revoked from
  `PUBLIC`: `pg_tviews_refresh_all()`, `pg_tviews_refresh_all_entities()`,
  `pg_tviews_rebuild_all(boolean)`, `pg_tviews_reregister_all(boolean)`,
  `pg_tviews_set_logged(text, boolean)`,
  `pg_tviews_ensure_propagation_indexes(text, boolean)` and the internal
  `pg_tviews_invalidate_caches(oid)`. Superusers and the extension's owner may run
  them; any other role needs `GRANT EXECUTE` (an operator role, see
  [Operator role](../user-guides/operators.md#operator-role)). `pg_tviews_reregister_all`,
  `pg_tviews_set_logged` and `pg_tviews_ensure_propagation_indexes` also check
  ownership of each TVIEW they touch.
- **Creating** a TVIEW requires `CREATE` on the schema of its `tv_*` table; the caller
  becomes its owner.

A role that owns nothing is refused, and an operator is refused acting on one TVIEW
it does not own:

```sql
CREATE ROLE api_doc_operator;
GRANT EXECUTE ON FUNCTION tviews.pg_tviews_refresh_all() TO api_doc_operator;
SET ROLE api_doc_operator;
SELECT tviews.pg_tviews_refresh_all() ->> 'refreshed_count' AS refreshed;  -- allowed
DO $$
BEGIN
    PERFORM tviews.pg_tviews_refresh('post');                             -- not the owner
EXCEPTION WHEN insufficient_privilege THEN
    RAISE NOTICE '%', SQLERRM;                                            -- must be owner of TVIEW tv_post
END $$;
RESET ROLE;
DROP OWNED BY api_doc_operator;
DROP ROLE api_doc_operator;
```

## Errors

Every pg_tviews error carries a specific SQLSTATE, a one-line message, the query or
definition in `DETAIL` and the fix in `HINT`. Every message is listed in the
[error reference](../error-reference.md).

| SQLSTATE | Condition name | Raised when |
|---|---|---|
| `42704` | `undefined_object` | no such TVIEW (`pg_tviews_refresh`, `pg_tviews_drop` without `if_exists`, …) |
| `42P07` | `duplicate_table` | the TVIEW already exists (`pg_tviews_create`, `CREATE TABLE tv_* AS`) |
| `42601` | `syntax_error` | the definition does not parse, or is more than one `SELECT` |
| `0A000` | `feature_not_supported` | a definition pg_tviews cannot maintain (incl. refusals by the `error` uncascaded policy) |
| `42501` | `insufficient_privilege` | the caller does not own the TVIEW, or may not execute the function |
| `42P17` | `invalid_object_definition` | TVIEWs would read each other in a cycle |
| `42703` | `undefined_column` | a required column (`pk_<entity>`, …) is missing |
| `54001` | `statement_too_complex` | propagation or view nesting deeper than `pg_tviews.max_propagation_depth` / `max_dependency_depth` |
| `54000` | `program_limit_exceeded` | refresh queue full (`pg_tviews.max_queue_size`) |
| `55000` | `object_not_in_prerequisite_state` | resume without suspend, refresh-all while suspended, a commit with refresh work still queued |
| `42883` | `undefined_function` | `jsonb_delta` is missing |
| `22023` | `invalid_parameter_value` | an invalid argument or option |
| `21000` | `cardinality_violation` | two rows for one key of a UNION TVIEW under `union_duplicate_policy = 'error'` |
| `XX000` | `internal_error` | internal failures |

```sql
DO $$
BEGIN
    PERFORM tviews.pg_tviews_refresh('no_such_entity');
EXCEPTION WHEN undefined_object THEN
    RAISE NOTICE 'not a TVIEW: %', SQLERRM;
END $$;
```

## Catalog

- `tviews.registry`: the versioned read contract for tools, one row per TVIEW (schema,
  name, entity, normalized query, base tables, options, policies, `needs_reregister`,
  …). Stable under `contract_version()`; see [the contract for tools](read-contract.md).
- `tviews.pg_tview_meta`: the registration catalog. One row per entity: `view_oid`,
  `table_oid` (the `tv_*` table), `definition`, the stored propagation `plan` (jsonb,
  [ADR 0203](../adr/0203-propagation-plan.md)), `group_keys`, `identity`, the policy
  and declaration columns, `needs_reregister`. Written only by pg_tviews; read
  `tviews.registry` from tools.
- `tviews.pg_tview_audit_log`: one row per create, drop and refresh while
  `pg_tviews.audit_enabled` is on.

```sql
SELECT entity, view, base_tables, uncascaded_policy, needs_reregister
FROM tviews.registry ORDER BY entity;
```

## Configuration

The settings are listed, with types and defaults, in the
[README's configuration table](../../README.md#configuration). Those that change what
a new TVIEW is (`pg_tviews.uncascaded_policy`, `pg_tviews.time_refresh`,
`pg_tviews.unlogged_by_default`, `pg_tviews.fillfactor`, `pg_tviews.data_gin_index`)
are read when it is created and stored with it; the `options` of
`pg_tviews_create_or_replace` override them. The others apply to each session and can
be `SET` at any time. `pg_tviews.auto_rebuild_databases` is read at server start.

```sql
SET pg_tviews.uncascaded_policy = 'error';
SHOW pg_tviews.max_propagation_depth;
```

## Two-phase commit

pg_tviews refreshes the TVIEWs before `PREPARE TRANSACTION`, so the refresh writes
belong to the prepared transaction: `COMMIT PREPARED` applies them and
`ROLLBACK PREPARED` discards them. Prepared transactions require
`max_prepared_transactions > 0`.

```text
BEGIN;
INSERT INTO tb_post (fk_user, title) VALUES (1, 'Prepared');
PREPARE TRANSACTION 'post-42';   -- TVIEWs refreshed as part of the transaction
COMMIT PREPARED 'post-42';       -- or ROLLBACK PREPARED 'post-42'
```

## Internal functions

Called by triggers, event triggers and restore; do not call them:
`pg_tviews_audit_write` and `pg_tviews_invalidate_caches` (both revoked from
`PUBLIC`), `pg_tviews_defines_view`, `pg_tviews_handle_dropped`,
`pg_tviews_meta_changed`, `pg_tviews_meta_rebind`, the event-trigger functions
`pg_tviews_handle_ddl_event` and `pg_tviews_handle_drop_event`, and the trigger
functions `pg_tview_trigger_handler`, `pg_tview_delta_trigger`,
`pg_tview_flush_trigger` and `pg_tview_truncate_trigger`.

## See also

- [DDL reference](ddl.md)
- [Contract for tools](read-contract.md)
- [Operator guide](../user-guides/operators.md)
- [Monitoring](../operations/monitoring.md)
- [Troubleshooting](../operations/troubleshooting.md)
