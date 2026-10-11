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
| [`pg_tviews_create(tview text, query text, options jsonb DEFAULT '{}')`](#pg_tviews_create) | `text` | `CREATE` on the schema |
| [`pg_tviews_create_or_replace(tview text, query text, options jsonb DEFAULT '{}')`](#pg_tviews_create_or_replace) | `text` | owner (new: `CREATE` on the schema) |
| [`pg_tviews_drop(tview text, if_exists boolean DEFAULT false, cascade boolean DEFAULT false)`](#pg_tviews_drop) | `text` | owner |
| [`pg_tviews_reregister(tview text)`](#pg_tviews_reregister) | `text` | owner |
| [`pg_tviews_reregister_all(strict boolean DEFAULT false)`](#pg_tviews_reregister_all) | `TABLE(entity text, status text)` | operator, and owner of each TVIEW |
| [`pg_tviews_refresh(tview text)`](#pg_tviews_refresh) | `void` | owner |
| [`pg_tviews_refresh_time_dependent(tview text DEFAULT NULL)`](#pg_tviews_refresh_time_dependent) | `SETOF text` | owner |
| [`pg_tviews_refresh_all()`](#pg_tviews_refresh_all) | `jsonb` | operator |
| [`pg_tviews_show_cascade_path(tview text)`](#pg_tviews_show_cascade_path) | `TABLE(depth integer, entity text, depends_on text)` | anyone |
| [`pg_tviews_mapping_query(tview text, base_table oid)`](#pg_tviews_mapping_query) | `text` | anyone |
| [`pg_tviews_read_set_queries(tview text, base_table oid)`](#pg_tviews_read_set_queries) | `TABLE(column_name text, query text)` | anyone |
| [`pg_tviews_ensure_propagation_indexes(tview text DEFAULT NULL, dry_run boolean DEFAULT false)`](#pg_tviews_ensure_propagation_indexes) | `SETOF text` | operator, and owner of each TVIEW |
| [`pg_tviews_entity_of(tview text)`](#pg_tviews_entity_of) | `text` | anyone |
| [`pg_tviews_suspend_triggers()`](#suspending-refresh) | `void` | anyone (own session) |
| [`pg_tviews_resume_triggers()`](#suspending-refresh) | `void` | anyone (own session) |
| [`pg_tviews_is_suspended()`](#suspending-refresh) | `boolean` | anyone |
| [`pg_tviews_suspended_entities()`](#suspending-refresh) | `text[]` | anyone |
| [`pg_tviews_flush_and_report(max_entities integer DEFAULT 500, include_data boolean DEFAULT true, reset boolean DEFAULT true)`](#pg_tviews_flush_and_report) | `jsonb` | anyone (own transaction) |
| [`pg_tviews_replication_status()`](#storage-replication-and-recovery) | `TABLE(entity text, persistence text, replica_readable boolean, is_empty boolean, needs_rebuild boolean)` | anyone |
| [`pg_tviews_rebuild_all(only_empty boolean DEFAULT true)`](#storage-replication-and-recovery) | `TABLE(entity text, rows bigint)` | operator |
| [`pg_tviews_health_check()`](#pg_tviews_health_check) | `TABLE(status text, component text, message text, severity text)` | anyone |
| [`pg_tviews_stats_reset(tview text DEFAULT NULL)`](#tviewsstats-and-pg_tviews_stats_reset) | `void` | operator |
| [`pg_tviews_queue_stats()`](#pg_tviews_queue_stats) | `jsonb` | anyone |
| [`pg_tviews_debug_queue()`](#pg_tviews_debug_queue) | `jsonb` | anyone |
| [`pg_tviews_profile(tview text DEFAULT NULL, fanout_warn bigint DEFAULT 1000)`](#pg_tviews_profile) | `TABLE(…)` | anyone |
| [`pg_tviews_version()`](#version-and-catalog) | `text` | anyone |
| [`pg_tviews_check_jsonb_delta()`](#version-and-catalog) | `boolean` | anyone |
| [`pg_tviews_catalog_revision()`](#version-and-catalog) | `integer` | anyone |
| [`contract_version()`](#version-and-catalog) | `integer` | anyone |

"Owner" and "operator" are defined in [Privileges](#privileges). Functions not listed
here are [internal](#internal-functions).

## Naming a TVIEW

Every function acting on one TVIEW takes it as its first parameter, `tview`
([ADR 0211](../adr/0211-api-surface.md)). A TVIEW's table is always `tv_<entity>`, and
an entity names one TVIEW in the whole database, so all these spellings name the same
TVIEW:

| Form | Example |
|---|---|
| the entity | `post` |
| the table | `tv_post` |
| the schema-qualified table | `public.tv_post` |
| quoted | `"public"."tv_post"`, `"tv_post"` |

A schema, when written, must be the TVIEW's; `search_path` plays no part. `tb_` and
`v_` names are never accepted. A name that names no TVIEW fails with `42704` and
`TVIEW <name> does not exist`. Messages name a TVIEW by its table
(`TVIEW public.tv_post created`).

## Creating and changing TVIEWs

The definition is exactly one `SELECT` with a `pk_<entity>` key column; see the
[DDL reference](ddl.md) for what it may contain. `pg_tviews_create` and
`pg_tviews_create_or_replace` take the TVIEW to create in the forms above; an
unqualified new TVIEW goes in `current_schema()`.
`CREATE TABLE tv_<entity> AS SELECT …` is the same as `pg_tviews_create` with no
options, and `CREATE UNLOGGED TABLE tv_<entity> AS SELECT …` the same with
`logged: false`.

A TVIEW is its definition and its options: no setting changes what it is
([ADR 0220](../adr/0220-settings.md)).

### Options

`options` is a JSON object; an unknown key or a value of the wrong type is an error
(`22023`). The options passed are the whole declaration: an option not passed is at
its default, also on an existing TVIEW. `tviews.registry.options` lists every option
of a TVIEW, defaults included.

| Option | Value | Default | Meaning |
|---|---|---|---|
| `logged` | boolean | `true` | LOGGED table (crash-safe, readable on standbys) or UNLOGGED |
| `fillfactor` | integer 10–100 | `85` | heap fillfactor of the table |
| `data_gin_index` | boolean | `false` | GIN index on `data` |
| `group_keys` | object or `null` | `null` | `{"<table>": "<group key column>"}`: an aggregate TVIEW; `null`: a plain one |
| `uncascaded_policy` | `"error"`, `"full_refresh"`, `"warn"` | `"error"` | what a write to a table no cascade reaches does: refuse the TVIEW, refresh it whole, or warn and leave rows stale ([details](ddl.md#tables-no-cascade-reaches)) |
| `uncascaded_tables` | object | `{}` | a policy per table, `{"public.tb_locale": "full_refresh"}`; a table the definition does not read, or whose writes are traced, is refused ([details](ddl.md#a-policy-per-table)) |
| `function_reads` | object | `{}` | the tables each non-immutable function reads, `{"public.label_suffix()": ["public.tb_setting"]}` (`[]` for none); functions are named with their argument types ([details](ddl.md#functions-that-read-tables)) |
| `time_refresh` | `"external"` or `null` | `null` | accept a definition reading the current time; `pg_tviews_refresh_time_dependent()` is called at the boundary ([details](ddl.md#time-dependent-tviews)) |
| `typename` | string or `null` | `null` (the PascalCase of the entity) | the GraphQL type name [`pg_tviews_flush_and_report()`](#pg_tviews_flush_and_report) reports |

### pg_tviews_create

```text
tviews.pg_tviews_create(tview text, query text, options jsonb DEFAULT '{}') RETURNS text
```

Creates the TVIEW: a backing view `tviews.<schema>__tv_<entity>`, the `tv_<entity>`
table filled from it, its indexes and the triggers on the tables it reads. Takes the
[options](#options) of `pg_tviews_create_or_replace`. Returns
`TVIEW <schema>.tv_<entity> created`. An existing TVIEW is an error (`42P07`); use
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

An aggregate TVIEW has one row per `GROUP BY` key; `group_keys` maps each table the
definition reads to the column holding the group key. See
[Aggregate TVIEWs](../user-guides/aggregate-tviews.md).

```sql
SELECT tviews.pg_tviews_create('tv_user_summary', $$
    SELECT p.fk_user AS pk_user_summary, u.id,
           jsonb_build_object('posts', count(*)) AS data
    FROM tb_post p JOIN tb_user u ON u.pk_user = p.fk_user
    GROUP BY p.fk_user, u.id $$,
    '{"group_keys": {"tb_post": "fk_user", "tb_user": "pk_user"}}');
```

Indexes created on the table:

| Index | Columns | Purpose |
|---|---|---|
| primary key | `pk_<entity>` | row identity |
| `idx_<tv>_id` | `id` | lookup by public UUID |
| `idx_<tv>_<column>_<pk>` | each column the plan looks rows up by (`fk_user` above), then `pk_<entity>` | propagation lookups; without it every cascade step scans the TVIEW |
| `idx_<tv>_data_gin` | `data` (GIN) | only with `data_gin_index`; makes every refresh a non-HOT update |

The table's fillfactor and persistence come from the `fillfactor` and `logged`
options; see [HOT updates](../operations/hot-updates.md) and
[Replication](../operations/replication.md).

### pg_tviews_create_or_replace

```text
tviews.pg_tviews_create_or_replace(tview text, query text, options jsonb DEFAULT '{}')
    RETURNS text  -- 'created' | 'unchanged' | 'altered' | 'replaced' | 'rebuilt'
```

Creates the TVIEW, or brings an existing one to `query` and `options` with the
smallest change: `unchanged`, `altered` (options only), `replaced` (same columns, the
backing view replaced in place) or `rebuilt` (new columns: view and table rebuilt).
The [contract for tools](read-contract.md) states exactly what each outcome changes.
`options` is the whole declaration ([Options](#options)): pass every option the TVIEW
should have. An unqualified name changes the existing TVIEW wherever it lives.

```sql
-- Options only: 'altered'
SELECT tviews.pg_tviews_create_or_replace('tv_user', $$
    SELECT pk_user, id, jsonb_build_object('id', id, 'name', name) AS data
    FROM tb_user $$, '{"fillfactor": 90}');

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

Switching `logged` rewrites the table under an `ACCESS EXCLUSIVE` lock, like
`ALTER TABLE tv_<entity> SET [UN]LOGGED`, which does the same. Either fills a reset
UNLOGGED TVIEW before it becomes LOGGED.

### pg_tviews_drop

```text
tviews.pg_tviews_drop(tview text, if_exists boolean DEFAULT false,
                      cascade boolean DEFAULT false) RETURNS text
```

Drops the table, its backing view, its triggers and its registration, like
`DROP TABLE tv_<entity>`. With `if_exists`, a missing TVIEW raises a NOTICE and
returns `TVIEW <name> does not exist, nothing dropped`; otherwise it is `42704`.
Without `cascade` the drop is RESTRICT: objects depending on the table (another
TVIEW embedding it, a view) make it fail; with `cascade` they are dropped too.
Returns `TVIEW <schema>.tv_<entity> dropped`.

```sql
SELECT tviews.pg_tviews_drop('tv_label');
SELECT tviews.pg_tviews_drop('tv_nonexistent', if_exists => true);
```

### pg_tviews_reregister

```text
tviews.pg_tviews_reregister(tview text) RETURNS text
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
tviews.pg_tviews_refresh(tview text) RETURNS void
```

Rebuilds the TVIEW from its view, then every TVIEW whose view reads it, directly or
through others, dependencies first. Each rebuild is a `TRUNCATE` and an
`INSERT … SELECT` holding an `ACCESS EXCLUSIVE` lock on that TVIEW until the
transaction ends, run as the TVIEW's owner. Requires owning the TVIEW (`42501`).

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
dependencies first. A TVIEW that reads no time is `22023`.
Call it at the boundary the rows depend on, from pg_cron or the application
([Time-dependent TVIEWs](ddl.md#time-dependent-tviews)).

```sql
SELECT tviews.pg_tviews_create_or_replace('tv_recent_post', $$
    SELECT pk_post AS pk_recent_post, id,
           jsonb_build_object('title', title, 'today', CURRENT_DATE) AS data
    FROM tb_post $$, '{"time_refresh": "external"}');
SELECT * FROM tviews.pg_tviews_refresh_time_dependent();
```

### pg_tviews_refresh_all

```text
tviews.pg_tviews_refresh_all() RETURNS jsonb
```

Rebuilds every TVIEW once, dependencies first, each as its owner, and leaves nothing
queued. Returns `{"refreshed_count", "order", "duration_ms"}` and refuses to run while
refresh is suspended (`55000`). Operator function.

```sql
SELECT tviews.pg_tviews_refresh_all() ->> 'refreshed_count' AS refreshed;
```

### pg_tviews_show_cascade_path

```text
tviews.pg_tviews_show_cascade_path(tview text)
    RETURNS TABLE(depth integer, entity text, depends_on text)
```

The TVIEW at depth 0, then the TVIEWs that read it, with their depth: what
`pg_tviews_refresh(tview)` rebuilds.

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
read the table. See
[How a write finds the TVIEW rows to refresh](ddl.md#how-a-write-finds-the-tview-rows-to-refresh).

```sql
SELECT tviews.pg_tviews_mapping_query('tv_user_summary', 'tb_post'::regclass);
-- SELECT DISTINCT "fk_user" FROM pg_tviews_delta
```

### pg_tviews_read_set_queries

```text
tviews.pg_tviews_read_set_queries(tview text, base_table oid)
    RETURNS TABLE(column_name text, query text)
```

What a refresh of `tview`'s rows reads of `base_table`, from the TVIEW's stored plan:
for each column of the table its mapping joins on, the query from the TVIEW's keys
(`$1`, an array of its identity's type) to the values that column is compared with.
A refresh takes a shared lock on each value before it computes the rows; a write to
the table takes an exclusive lock on its rows' values of the column
([Concurrency](../concurrency.md)). `column_name` and `query` are NULL when the table
is joined by no equality: the table is locked as a whole. No rows when writes to the
table map through no query of their own.

```sql
SELECT * FROM tviews.pg_tviews_read_set_queries('tv_post', 'tb_user'::regclass);
--  column_name | query
--  pk_user     | SELECT DISTINCT (o1.fk_user)::pg_catalog.text FROM public.tb_post o1
--              |   WHERE o1.pk_post OPERATOR(pg_catalog.=) ANY ($1::pg_catalog.int8[])
```

### pg_tviews_ensure_propagation_indexes

```text
tviews.pg_tviews_ensure_propagation_indexes(tview text DEFAULT NULL,
                                            dry_run boolean DEFAULT false) RETURNS SETOF text
```

Creates the missing propagation index of every column the plan looks the TVIEW's rows
up by (`tview`, or every TVIEW when NULL). A column counts as covered when any index
leads with it. Returns one `CREATE INDEX IF NOT EXISTS …` per missing index, executed
unless `dry_run`. Idempotent. On large tables, take the dry-run output and run it with
`CREATE INDEX CONCURRENTLY`. Operator function; requires owning each TVIEW it looks at.

```sql
SELECT * FROM tviews.pg_tviews_ensure_propagation_indexes(dry_run => true);
```

### pg_tviews_entity_of

```text
tviews.pg_tviews_entity_of(tview text) RETURNS text
```

The entity a TVIEW name names, in any of the [accepted forms](#naming-a-tview); `42704`
when it names no TVIEW. For tools that key on the entity.

```sql
SELECT tviews.pg_tviews_entity_of('public.tv_post');   -- post
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
row's `data`; a deleted entry is `{"__typename", "id"}`. The type name is the TVIEW's
`typename` option, or the PascalCase of its entity. Past `max_entities` entries,
or when the journal overflowed `pg_tviews.report_max_tracked`, `truncated` is true and
`invalidated_types` lists the types left out. With `reset`, the next call reports only
later changes; a reset made in a subtransaction that rolls back is undone. See [GraphQL cascade](../user-guides/graphql-cascade.md).

```sql
BEGIN;
UPDATE tb_user SET name = 'Alice B.' WHERE pk_user = 1;
SELECT tviews.pg_tviews_flush_and_report(include_data => false);
COMMIT;
```

## Storage, replication and recovery

```text
tviews.pg_tviews_replication_status()
    RETURNS TABLE(entity text, persistence text, replica_readable boolean,
                  is_empty boolean, needs_rebuild boolean)
tviews.pg_tviews_rebuild_all(only_empty boolean DEFAULT true) RETURNS TABLE(entity text, rows bigint)
```

- TVIEWs are LOGGED unless declared `logged: false`. Switch one with the `logged`
  option or `ALTER TABLE tv_<entity> SET [UN]LOGGED`.
- `pg_tviews_replication_status` reports every TVIEW, `replica_readable` true for a
  LOGGED one, and is safe on a standby.
- `pg_tviews_rebuild_all` refills the UNLOGGED TVIEWs a crash restart, promotion or
  restore reset (`needs_rebuild`; every TVIEW with `only_empty => false`),
  dependencies first, each as its owner, and counts their rows; it refuses to run
  during recovery. A TVIEW that is merely empty is not a reset one. Operator
  function: the caller needs no privilege on the `tv_*` tables.
- After a crash restart or a promotion, a background worker does the same in every
  database listed by `pg_tviews.auto_rebuild_databases` (`*`, the default: all). A
  reset TVIEW not yet refilled is filled by its first write.
- `needs_rebuild` is true for an UNLOGGED TVIEW missing from `tviews.pg_tview_valid`,
  the UNLOGGED table that a reset empties together with the TVIEWs
  ([Replication](../operations/replication.md#rebuilding-after-promotion-a-crash-or-a-restore)).

```sql
SELECT * FROM tviews.pg_tviews_replication_status();
SELECT * FROM tviews.pg_tviews_rebuild_all();
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

### tviews.stats and pg_tviews_stats_reset

```text
tviews.stats  -- view: schema, name, entity, view_recomputes, noop_skipped,
              --   patch_captured, patch_applied, patch_fallbacks, propagation_pruned,
              --   rows_written, rows_deleted, full_refreshes, refresh_ms,
              --   stats_reset, untracked
tviews.pg_tviews_stats_reset(tview text DEFAULT NULL) RETURNS void
```

Refresh statistics per TVIEW of the database, readable from any session by any role
([ADR 0221](../adr/0221-observability.md)). Counters are cumulative since the server
started or the TVIEW's last reset, and counted when each transaction ends. They live in
shared memory (4096 TVIEWs per cluster): reading the view without
`shared_preload_libraries = 'pg_tviews'` fails with a hint, and a TVIEW that finds the
table full is `untracked`, with NULL counters. The columns are described in the
[contract for tools](read-contract.md#tviewsstats).
`pg_tviews_stats_reset` zeroes one TVIEW, or every TVIEW of the database when `tview`
is NULL. Operator function.

```sql
SELECT entity, view_recomputes, rows_written, full_refreshes FROM tviews.stats
ORDER BY entity;
SELECT tviews.pg_tviews_stats_reset('post');
```

### pg_tviews_queue_stats

```text
tviews.pg_tviews_queue_stats() RETURNS jsonb
```

Counters of the calling backend, for debugging. Queue, flush and cache counters cover
the current transaction; the `direct_patch_*`, `view_recomputes`,
`refresh_noop_skipped`, `catalog_lookups` and `propagation_pruned` counters accumulate
over the session. Read it from the session doing the writes: another session sees its
own, usually zero. Per-TVIEW counters for every session are in `tviews.stats`.

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
| `value_locks`, `value_lock_escalations` | value and key locks this transaction took, and relations it locked in place of their values ([Concurrency](../concurrency.md)) |
| `value_lock_waits`, `value_lock_wait_ms` | of those, the locks that weren't granted at once, and the time spent waiting for them |

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

### pg_tviews_profile

```text
tviews.pg_tviews_profile(tview text DEFAULT NULL, fanout_warn bigint DEFAULT 1000)
    RETURNS TABLE(entity text, schema text, name text, persistence text,
                  replica_readable boolean, rows_estimate bigint, heap_bytes bigint,
                  index_bytes bigint, toast_bytes bigint, avg_row_width integer,
                  data_avg_width integer, fillfactor integer, n_tup_upd bigint,
                  n_tup_hot_upd bigint, hot_ratio double precision, n_dead_tup bigint,
                  last_vacuum timestamptz, last_autovacuum timestamptz,
                  all_visible_fraction double precision, unused_indexes text[],
                  missing_propagation_indexes text[], fanout jsonb, warnings text[])
```

The per-TVIEW physical health report (`tview`, or every TVIEW when NULL); see
[profile.md](profile.md).

```sql
SELECT entity, persistence, hot_ratio, warnings FROM tviews.pg_tviews_profile();
```

### Version and catalog

```text
tviews.pg_tviews_version() RETURNS text            -- the library version
tviews.pg_tviews_check_jsonb_delta() RETURNS boolean -- jsonb_delta installed
tviews.pg_tviews_catalog_revision() RETURNS integer  -- catalog revision of the installed extension SQL
tviews.contract_version() RETURNS integer           -- version of the read contract (2)
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
  `pg_tviews_reregister`, `pg_tviews_refresh`, `pg_tviews_refresh_time_dependent`
  and `pg_tviews_ensure_propagation_indexes`.
- **Rebuilds run as the TVIEW's owner.** Every refresh, single or bulk, reads the
  backing view and writes the `tv_*` table as the owner of that `tv_*` table, in a
  security-restricted operation with `search_path = pg_catalog, pg_temp` and fixed
  rendering settings (`TimeZone` `UTC`, `DateStyle` `ISO, YMD`). A function the
  backing view calls never runs with the caller's privileges.
- **Operator.** The maintenance functions are revoked from `PUBLIC`:
  `pg_tviews_refresh_all()`, `pg_tviews_rebuild_all(boolean)`,
  `pg_tviews_reregister_all(boolean)`,
  `pg_tviews_ensure_propagation_indexes(text, boolean)`,
  `pg_tviews_stats_reset(text)` and the internal
  `pg_tviews_invalidate_caches(oid)`. Superusers and the extension's owner may run
  them; any other role needs `GRANT EXECUTE` (an operator role, see
  [Operator role](../user-guides/operators.md#operator-role)).
  `pg_tviews_reregister_all` and `pg_tviews_ensure_propagation_indexes` also check
  ownership of each TVIEW they touch.
- **Creating** a TVIEW requires `CREATE` on the schema of its `tv_*` table; the caller
  becomes its owner.
- **Reading.** `tviews.registry` and `tviews.stats` are readable by any role.

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
| `42704` | `undefined_object` | a `tview` argument names no TVIEW (`pg_tviews_drop` without `if_exists`, …) |
| `42P07` | `duplicate_table` | the TVIEW already exists (`pg_tviews_create`, `CREATE TABLE tv_* AS`) |
| `42601` | `syntax_error` | the definition does not parse, or is more than one `SELECT` |
| `0A000` | `feature_not_supported` | a definition pg_tviews cannot maintain (incl. refusals by the `error` uncascaded policy) |
| `42501` | `insufficient_privilege` | the caller does not own the TVIEW, or may not execute the function |
| `42P17` | `invalid_object_definition` | TVIEWs would read each other in a cycle |
| `42703` | `undefined_column` | a required column (`pk_<entity>`, …) is missing |
| `54001` | `statement_too_complex` | propagation or view nesting deeper than `pg_tviews.max_propagation_depth` / `max_dependency_depth` |
| `54000` | `program_limit_exceeded` | refresh queue full (`pg_tviews.max_queue_size`) |
| `55000` | `object_not_in_prerequisite_state` | resume without suspend, refresh-all while suspended, a commit with refresh work still queued |
| `25000` | `invalid_transaction_state` | `PREPARE TRANSACTION` of a transaction that refilled a reset UNLOGGED TVIEW |
| `42883` | `undefined_function` | `jsonb_delta` is missing |
| `22023` | `invalid_parameter_value` | an invalid argument or option |
| `21000` | `cardinality_violation` | two rows for one key of a UNION TVIEW (ADR 0216) |
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
  name, entity, normalized query, every option, base tables, `needs_reregister`, …).
  Stable under `contract_version()`; see [the contract for tools](read-contract.md).
- `tviews.stats`: per-TVIEW refresh statistics, part of the same contract; see
  [above](#tviewsstats-and-pg_tviews_stats_reset).
- `tviews.pg_tview_meta`: the registration catalog. One row per entity: `view_oid`,
  `table_oid` (the `tv_*` table), `definition`, the stored propagation `plan` (jsonb,
  [ADR 0203](../adr/0203-propagation-plan.md)), `group_keys`, `identity`, the policy
  and declaration columns, `needs_reregister`. Written only by pg_tviews; read
  `tviews.registry` from tools.
- `tviews.pg_tview_audit_log`: one row per create, drop and refresh while
  `pg_tviews.audit_enabled` is on.

```sql
SELECT entity, view, base_tables, options->>'uncascaded_policy' AS uncascaded_policy,
       needs_reregister
FROM tviews.registry ORDER BY entity;
```

## Configuration

The settings are listed, with types and defaults, in the
[README's configuration table](../../README.md#configuration). None changes what a
TVIEW is: that comes only from its definition and its [options](#options). The
settings that decide whether a write or a creation succeeds
(`pg_tviews.max_propagation_depth`, `max_dependency_depth`, `max_queue_size`,
`lock_escalation_threshold`) and `pg_tviews.audit_enabled` are a superuser's.
`batch_size`, `cache_size` and `report_max_tracked` apply to each session.
`pg_tviews.auto_rebuild_databases` is read at server start. Diagnostics are `DEBUG1`
messages: `SET client_min_messages = debug1` shows them.

```sql
SHOW pg_tviews.max_propagation_depth;
```

## Two-phase commit

pg_tviews refreshes the TVIEWs before `PREPARE TRANSACTION`, so the refresh writes
belong to the prepared transaction: `COMMIT PREPARED` applies them and
`ROLLBACK PREPARED` discards them. Prepared transactions require
`max_prepared_transactions > 0`. A transaction that refilled a reset UNLOGGED TVIEW
cannot be prepared (`25000`).

```text
BEGIN;
INSERT INTO tb_post (fk_user, title) VALUES (1, 'Prepared');
PREPARE TRANSACTION 'post-42';   -- TVIEWs refreshed as part of the transaction
COMMIT PREPARED 'post-42';       -- or ROLLBACK PREPARED 'post-42'
```

## Internal functions

Called by triggers, event triggers, views and restore; do not call them:
`pg_tviews_audit_write` and `pg_tviews_invalidate_caches` (both revoked from
`PUBLIC`), `pg_tviews_defines_view`, `pg_tviews_handle_dropped`,
`pg_tviews_meta_changed`, `pg_tviews_meta_rebind`, `pg_tviews_stats_rows` (behind
`tviews.stats`), the event-trigger functions
`pg_tviews_handle_ddl_event` and `pg_tviews_handle_drop_event`, and the trigger
functions `pg_tview_trigger_handler`, `pg_tview_delta_trigger`,
`pg_tview_flush_trigger` and `pg_tview_truncate_trigger`.

## See also

- [DDL reference](ddl.md)
- [Contract for tools](read-contract.md)
- [Operator guide](../user-guides/operators.md)
- [Monitoring](../operations/monitoring.md)
- [Troubleshooting](../operations/troubleshooting.md)
