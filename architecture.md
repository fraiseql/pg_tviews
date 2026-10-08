# pg_tviews architecture

A TVIEW is a table, `tv_<entity>`, kept equal to what a SELECT over base tables
computes, row by row, inside the transaction that writes the base tables. This page
says how. The decisions behind it are in `docs/adr/`; the SQL surface is in
[docs/reference/api.md](docs/reference/api.md), and what tools may read is in
[docs/reference/read-contract.md](docs/reference/read-contract.md).

## Objects

| Object | What it is |
|---|---|
| `tv_<entity>` | The materialized table, in the schema the user chose. Its primary key is the TVIEW's identity column. UNLOGGED unless `pg_tviews.unlogged_by_default` is off. |
| Backing view `tviews.<schema>__tv_<entity>` | The definition, as a view. Every refresh reads it. Owned like its table, `SELECT` granted as on its table ([ADR 0136, #181 amendment](docs/adr/0136-tool-facing-surface.md)). |
| `tviews.pg_tview_meta` | One row per TVIEW: entity, OIDs, definition, identity, policies and declarations, and the propagation plan (`plan`). Internal: tools read `tviews.registry` instead. |
| Triggers on base tables | Installed per TVIEW from its plan (below). |

The extension lives in schema `tviews` and its version is the crate version, with one
upgrade script per release ([ADR 0136](docs/adr/0136-tool-facing-surface.md),
[docs/development/extension-versioning.md](docs/development/extension-versioning.md)).

## Registration

A TVIEW is created by `CREATE TABLE tv_<entity> AS SELECT …` (intercepted by the
`ProcessUtility` hook, `src/hooks/ctas.rs`), by `pg_tviews_create()`, or by
`pg_tviews_create_or_replace()` (`src/ddl/`). Each creates the backing view, then
analyses it.

### Analysis of the query tree

`src/lineage/` reads PostgreSQL's analysed query of the backing view (views, CTEs and
subqueries expanded); no SQL text is parsed to find a relationship
([ADR 0157](docs/adr/0157-cascade-key-mapping.md), [ADR 0203](docs/adr/0203-propagation-plan.md)).
`lineage/walk/` is the only code touching `pg_sys` nodes; it builds a graph of table
occurrences, the TVIEW key, and the predicates linking them. Each base table is then
classified:

| Kind | Meaning | Trigger |
|---|---|---|
| `local` | The key is a column of the changed row (the table holding the identity, or a table joined on the key) | row |
| `mapped` | A chain of predicates links the table to the key: a mapping query over the changed rows returns the keys | statement (delta) |
| `tview` | Another TVIEW's table read like a base table (#191) | delta only, fired inside the flush |
| `propagated` | Read only through a TVIEW this one embeds: parent propagation covers it | none |
| `all_keys` | Nothing selective links it to the key | statement; `uncascaded_policy` decides (`error` refuses the TVIEW, `warn`, `full_refresh`) |

Time-dependent definitions and calls to non-immutable functions are detected in the
same walk and go through the same policy, with the `time_refresh` and `function_reads`
declarations (`docs/reference/ddl.md`).

### Row identity

The identity is the output column naming the rows: `pk_<entity>`, or the `DISTINCT ON`
key of a TVIEW whose top level has one. It is derived from the tree, is the table's
primary key, and is the only key type at run time (`RefreshKey` with an integer or a
text value) ([ADR 0169](docs/adr/0169-tview-row-identity.md)).

### The stored plan

`lineage::plan` returns a `TviewPlan` (`src/catalog/plan.rs`), stored as one versioned
document in `pg_tview_meta.plan`:

- `tables`: per base table, its kind, mapping query template, the columns read, and the
  fan-out patch an UPDATE of it can write ([ADR 0078 outcome](docs/adr/0078-field-dependency-classes.md));
- `paths`: the tables whose changed rows carry a key, and in which column;
- `embeds`: each TVIEW this one embeds, the output columns holding the child's key
  (any name: the relationship is the equality in the definition), the embed kind
  (`nested`, `array`, `scalar`) and the path in `data`;
- `direct`: the direct-patch map, the identity table's columns copied into `data` and
  read nowhere else;
- `set_operation`: rows from UNION/INTERSECT/EXCEPT branches are recomputed, never
  patched.

The triggers and the flush read only the plan, decoded once per backend per catalog
generation. A plan names relations by OID and qualified name; a `BEFORE INSERT`
trigger on `pg_tview_meta` rebinds a restored row's OIDs and attnums from the names and
fails the insert, naming the TVIEW, when one no longer resolves. A plan of another
version is re-derived by `ALTER EXTENSION pg_tviews UPDATE`, never read.

A definition that makes TVIEWs read each other in a cycle is refused (42P17).

## Triggers on base tables

`src/dependency/triggers.rs` installs, per TVIEW and base table, the set the plan's
kind asks for. Every trigger function takes the TVIEW's entity as its argument.

- **Row trigger** (`src/trigger.rs`, `AFTER INSERT OR UPDATE OR DELETE FOR EACH ROW`):
  for `local` tables, and for partitioned `mapped` tables (PostgreSQL copies only row
  triggers onto partitions). It reads the key off the old and the new row by attribute
  number, captures a direct patch when every changed column is in the plan's direct
  map and the key is unchanged, and enqueues. No SPI per row.
- **Delta triggers** (`src/delta.rs`, one statement trigger per event with transition
  tables): for `mapped`, `all_keys` and `tview` tables. The handler runs the table's
  mapping query (prepared once per backend) over the changed rows, as the TVIEW's
  owner, and enqueues the keys; for an UPDATE it captures the fan-out patch instead
  when the plan has one. Under `full_refresh`, an `all_keys` table enqueues the whole
  TVIEW.
- **Flush trigger** (`AFTER INSERT OR UPDATE OR DELETE FOR EACH STATEMENT`, named to
  fire after the delta triggers): flushes the queue at the end of the statement.
- **Truncate trigger** (`AFTER TRUNCATE FOR EACH STATEMENT`): refreshes the whole TVIEW,
  once per statement.

Every partition of a partitioned base table also gets the flush and truncate
triggers, since statement triggers fire only on the table a statement names.

`pg_tviews_suspend_triggers()` / `pg_tviews.suspend_triggers` make the triggers record
which TVIEWs changed instead; resuming rebuilds them (`src/suspend.rs`).

## The queue

`src/queue/` holds the transaction's pending work in memory: refresh keys, the direct
and fan-out patches riding on them, and the rows refreshes changed (for
`pg_tviews_flush_and_report()`). A key whose change cannot be patched poisons its patch
chain, which then recomputes. A savepoint leaves the pending work in place and logs
changes made inside it; rolling it back undoes only those (`src/flush/savepoint.rs`).

## The flush engine

`src/flush/` applies the queue. It runs:

- at the end of each outermost writing statement, from the flush trigger. The executor
  hooks (`src/executor.rs`) defer the flush of a statement nested in another write to
  that write, so a trigger cascade refreshes each row once;
- before a top-level `COMMIT` or `PREPARE TRANSACTION`, from the `ProcessUtility` hook
  (`src/hooks/mod.rs`). SPI is not available in transaction callbacks, so nothing is
  flushed there.

The loop (`flush/drain.rs`) takes one entity per pass in dependency order (the
topological order of embeds and `tview` reads, `flush/graph.rs`), until neither the
flush nor the triggers its writes fire left work. For each entity
(`flush/apply.rs`), as the TVIEW's owner:

1. a whole-TVIEW key rebuilds it from its view;
2. keys carrying a usable direct patch are written straight into `tv_<entity>`
   (`src/refresh/direct.rs`); a row not yet materialised is recomputed;
3. other keys are recomputed from the backing view (`src/refresh/row.rs`,
   `bulk.rs`): an upsert by identity that skips unchanged rows and deletes rows the view
   no longer produces;
4. fan-out patches write a parent's changed columns into every child row in one
   `UPDATE`;
5. parents are found through each embed's lookup columns (`src/propagate.rs`), pruned
   when the child's refresh changed nothing they read, and queued: patched under the
   embed's path when a patch can be derived, recomputed otherwise.

A flush that fails fails the write. `pg_tviews.max_propagation_depth` bounds the
passes.

## Owner execution and render settings

The flush acts for whoever wrote a base table, so every read and write of a TVIEW runs
as its table's owner, in a security-restricted operation, with `search_path =
pg_catalog, pg_temp` (`src/owner.rs`), as `REFRESH MATERIALIZED VIEW` does. The writer
needs no privilege on the TVIEW or its view. Values are rendered under fixed settings
(`TimeZone` UTC, `DateStyle` ISO/YMD, `IntervalStyle` postgres, `extra_float_digits`
1, `bytea_output` hex), not the writer's.

Functions acting on one TVIEW require owning it (or the extension); maintenance
functions acting on every TVIEW are not granted to `PUBLIC`. Catalog writes run as the
extension owner after that check. No pg_tviews function is `SECURITY DEFINER`
([ADR 0136, maintenance-functions amendment](docs/adr/0136-tool-facing-surface.md)).

## Patching with jsonb_delta

Recomputing is always correct; patching skips the backing-view query. Both patch paths
need the `jsonb_delta` extension and `pg_tviews.direct_patch_enabled`, and call only
functions in jsonb_delta's own schema (`src/jsonb_delta.rs`):

- **Direct patch**: an UPDATE of identity-table columns that `data` only copies becomes
  `jsonb_smart_patch_scalar` on the TVIEW's own row, and `jsonb_smart_patch_nested`
  under the embed path in its parents.
- **Fan-out patch**: an UPDATE of a `mapped` table one equality away from a projected
  column becomes one `UPDATE tv_child SET data = jsonb_smart_patch_scalar(…) WHERE
  lookup = $key`.

Anything the plan cannot express (an opaque `data`, a set operation, an array or scalar
embed) recomputes. Why a whole `data` value is still written on every change is in
[ADR 0094](docs/adr/0094-large-document-refresh.md); why there is no automatic bulk
strategy is in [ADR 0077](docs/adr/0077-refresh-strategy-selection.md).

## Fail loud

- A commit with refresh work still queued fails with 55000 at `PRE_COMMIT`
  (`src/flush/xact.rs`); only a missing or disabled flush trigger leaves work queued.
- A write fails when its trigger cannot tell what to refresh: a plan, identity or
  policy that does not decode, a mapping stored without its query, a trigger naming no
  TVIEW. The error names the TVIEW and hints `pg_tviews_reregister`.
- An untraceable read is refused at create under the default `error` policy.
- Errors carry their SQLSTATE (`docs/error-reference.md`).

## DDL and lifecycle

The `ProcessUtility` hook (`src/hooks/`) also intercepts `DROP TABLE tv_*` (honouring
`CASCADE`), renames and other `ALTER TABLE tv_*`, `REFRESH MATERIALIZED VIEW` of a
matview a TVIEW reads, `DROP EXTENSION`, and `DISCARD ALL`, including statements run
by functions. Event triggers report a `CREATE TABLE tv_* AS` the hook did not see and
clean up after `DROP SCHEMA … CASCADE` / `DROP OWNED BY`. Per-backend caches check for
invalidations on every read and are cleared when a transaction aborts. An UNLOGGED
TVIEW emptied by a crash is refilled on its next flush (`src/lifecycle.rs`), and for
the databases in `pg_tviews.auto_rebuild_databases` a background worker repopulates
them once recovery ends (`src/rebuild_worker.rs`).

## Source map

| Path | Role |
|---|---|
| `src/lineage/` | Query-tree analysis, identity, mapping query templates |
| `src/catalog/` | `TviewMeta`, `TviewPlan`, catalog reads |
| `src/ddl/`, `src/hooks/` | Create, replace, drop, rename; utility hook |
| `src/dependency/` | Base-table discovery, trigger installation |
| `src/trigger.rs`, `src/delta.rs` | Row and statement triggers |
| `src/queue/` | Transaction-local pending work |
| `src/flush/`, `src/propagate.rs`, `src/executor.rs` | Flush engine, parents, statement nesting |
| `src/refresh/` | Recompute, bulk, direct patch |
| `src/owner.rs` | Owner execution, render settings, ownership checks |
| `src/admin.rs`, `src/health.rs`, `src/report.rs` | Maintenance, health check, change reports |
