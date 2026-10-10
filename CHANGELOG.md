# Changelog

All notable changes to pg_tviews will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0-beta.27] - 2026-10-11

### Changed (breaking)

- **Maintenance functions that act on every TVIEW are no longer executable by
  `PUBLIC`**: `pg_tviews_refresh_all()`, `pg_tviews_refresh_all_entities()`,
  `pg_tviews_rebuild_all()`, `pg_tviews_reregister_all()`, `pg_tviews_set_logged()`,
  `pg_tviews_ensure_propagation_indexes()` and `pg_tviews_invalidate_caches()`. A role
  that is neither a superuser nor the extension's owner needs `GRANT EXECUTE`
  (`docs/user-guides/operators.md`), or gets 42501. A deploy or restore tool calling
  `pg_tviews_rebuild_all()` as such a role must be granted it before upgrading.
- **Every function acting on one TVIEW requires owning it** (or the extension), checked
  before any lock: `pg_tviews_set_logged()`, `pg_tviews_recover_after_crash()` and
  `pg_tviews_ensure_propagation_indexes(entity)` join `pg_tviews_refresh()`,
  `pg_tviews_reregister()` (which took the TVIEW's registration lock before checking)
  and the rest.
- **A commit with refresh work still queued fails** (55000), every time. It
  committed with a WARNING, shown once per backend, and the TVIEWs named stayed
  stale. Only a missing or disabled flush trigger leaves work queued.
- **A write fails when its row trigger cannot tell what to refresh**: a stored plan,
  identity or uncascaded policy of any TVIEW that does not decode (a catalog edited
  by hand, a restore out of step), a mapping stored without its query, a
  `pg_tviews` trigger that names no TVIEW. The error names the TVIEW, with the
  `pg_tviews_reregister` hint. The write committed with nothing queued, behind a
  WARNING, or the value was read as a default (`warn`, `pk_<entity>`).
- **Errors carry their SQLSTATE.** Every pg_tviews function reported its errors as
  22000 (`data_exception`) or XX000, whatever went wrong, so `WHEN undefined_object`
  or `WHEN sqlstate '42P07'` never matched. Now: no such TVIEW 42704, TVIEW already
  exists 42P07 (`pg_tviews_create` and `CREATE TABLE tv_* AS` alike), unreadable
  definition 42601, definition pg_tviews cannot maintain 0A000, not allowed 42501,
  TVIEWs reading each other in a cycle 42P17, nesting too deep 54001, refresh queue
  full 54000, resume without suspend or refresh while suspended 55000, `jsonb_delta`
  missing 42883, invalid argument 22023; internal failures stay XX000. Messages are
  one line, with the query or definition in DETAIL and the fix in HINT. Internal
  errors no longer reach the client as `SPI error: OpUnknown`.
- **A backend's caches follow DDL made in another backend.** A TVIEW replaced, or
  the extension dropped and created again, in one session left another session
  patching with the old column map: the row trigger's cache never checked for
  invalidations. Every cache now checks on every read, relation names come from the
  syscache, and the catalog is re-watched after the extension is created again.
- **A definition that makes TVIEWs read each other in a cycle is refused** (42P17)
  when it is created or replaced. Before, it was accepted and every later write to
  the tables involved failed.
- **`pg_tviews_refresh(entity)` requires owning the TVIEW, and every rebuild runs as
  the TVIEW's owner**, like `REFRESH MATERIALIZED VIEW`. A backing view runs the
  functions it calls as the querying role, so a rebuild run as the caller let a TVIEW
  owner's code run with the caller's privileges: a superuser's after the documented
  post-migration `pg_tviews_refresh_all()`. `pg_tviews_refresh_all()`,
  `pg_tviews_refresh_all_entities()`, `pg_tviews_rebuild_all()` and
  `pg_tviews_recover_after_crash()` now read each backing view as its owner (the
  emptiness checks of `pg_tviews_rebuild_all(true)` too); `pg_tviews_refresh(entity)`
  by a role that neither owns `tv_<entity>` nor the extension fails with 42501 (it
  rebuilt the requested TVIEW with the caller's privileges before).

### Changed

- **Writes may wait for concurrent writes to related rows** (ADR 0207). A write
  locks, exclusively, the join values of the rows it changed before it looks up the
  TVIEW rows they feed; a refresh locks, shared, the values the rows it computes
  read, and exclusively the keys of the rows it creates. The locks live in
  PostgreSQL's lock manager, show in `pg_locks` as advisory locks with `objsubid`
  21622 (a value or key) or 21623 (a relation), and are held to the end of the
  transaction. A write refreshing a whole TVIEW (`TRUNCATE`, the `full_refresh`
  policy, `pg_tviews_refresh()`) locks the TVIEW and the relations it reads. Not
  under `SERIALIZABLE`. The queries that find and compute TVIEW rows run with a
  fresh snapshot.
  Transactions taking these locks in opposite orders can deadlock (`40P01`); under
  `REPEATABLE READ`, contention on the same values fails transactions with `40001`, and
  each refreshed row is computed twice (measured: about a fifth fewer transactions per
  second than `READ COMMITTED`). Retry both errors; prefer `READ COMMITTED` for writes
  to TVIEW base tables ([Concurrency](docs/concurrency.md)).
- **A definition is read only from PostgreSQL's query tree** (ADR 0203). The
  text-pattern analysis that registration still used for columns, embeds and the
  direct-patch map is gone, and with it the spelling rules it imposed:
  - a parent may hold a child TVIEW's key in a column of any name (`author_pk`), and
    a TVIEW whose rows are another table's (`pk_order_summary` over `tb_order`) is
    accepted and refreshed (it was refused, #49);
  - a child's document embedded under an alias (`'author', u.data`) is patched into
    its parents instead of recomputing them;
  - a column whose name holds an apostrophe, `SELECT *` over such a view, and a
    JSON key holding a quote work; `SELECT * FROM t` lists the columns of the `t`
    the `search_path` resolves, not those of every same-named table;
  - a `fk_<entity>` column no longer makes a TVIEW depend on a TVIEW it does not read.
- **One stored propagation plan per TVIEW** (ADR 0203). `pg_tview_meta` keeps what
  registration derives from the query tree in one versioned `plan` document (how a
  write to each base table maps to keys, the tables whose rows carry a key, the TVIEWs
  it embeds, the direct-patch map) instead of thirteen columns. The triggers and the
  flush read only the plan: no table or column name is matched to find a
  relationship. `ALTER EXTENSION pg_tviews UPDATE` re-derives every TVIEW, and fails
  naming any TVIEW that no longer analyses (`docs/development/extension-versioning.md`).
  A TVIEW whose rows are another table's (`tv_purchase` over `tb_order`) now gets the
  direct patch (#56) like any other.
- **A definition is exactly one SELECT** (42601). A second statement after the
  SELECT was accepted and run.
- `CREATE TABLE tv_* AS` accepts a comment before the statement.
- **pg_tviews' index names on a TVIEW are its own** (#219). A user's `CREATE INDEX`
  or `ALTER INDEX … RENAME TO` on a TVIEW's table under a name pg_tviews uses there
  (`idx_<tv>_id`, `idx_<tv>_<fk>_<pk>`, `idx_<tv>_data_gin`…) is refused with
  `42939` (`reserved_name`): pg_tviews' `CREATE INDEX IF NOT EXISTS` would otherwise
  find the user's index in its place. A statement creating exactly pg_tviews' index
  under its name (what `pg_tviews_ensure_propagation_indexes(dry_run => true)`
  reports, run by hand, or what a dump restores) is accepted, and the index is
  pg_tviews'.
- **A `rebuilt` replace refuses while a user index on the TVIEW is invalid** (#219),
  naming it (`55000`), before it drops anything. A UNIQUE index left invalid by a
  failed `CREATE UNIQUE INDEX CONCURRENTLY` was re-created valid on the empty table,
  and the fill then failed on the duplicates, naming only the constraint.

### Added

- **`tviews.registry.managed_indexes regclass[]`** (#219, appended; `contract_version()`
  stays 1): the indexes pg_tviews created on the TVIEW's table and still owns,
  sorted by name, without the primary key; NULL when the table is gone. A TVIEW's
  user indexes are every other index on its table that backs no constraint. Renames
  and drops of these indexes are followed, and
  `pg_tviews_ensure_propagation_indexes()` records the indexes it creates.
- `pg_tviews.lock_escalation_threshold` (default 64): value locks a transaction takes
  on one relation before it locks the relation instead, so a bulk write never
  exhausts the shared lock table (0 always locks relations, -1 never does).
- `pg_tviews_queue_stats()` reports `value_locks`, `value_lock_escalations`,
  `value_lock_waits` and `value_lock_wait_ms` for the current transaction;
  `docs/operations/monitoring.md` shows the locks in `pg_locks` and who waits for whom.
- `tviews.pg_tviews_read_set_queries(tview, base_table)`: what a refresh of a
  TVIEW's rows reads of a table its mapping joins (the column, and the query from the
  TVIEW's keys to the values compared with it), from the plan, which now stores it
  (ADR 0207).
- `pg_tviews_queue_stats()` reports `flushes`: the flushes that refreshed something
  in the current transaction.

### Removed

- `pg_tviews_cascade()`, `pg_tviews_insert()` and `pg_tviews_delete()`: they found
  a TVIEW's rows by table-name conventions; a write to the base table already
  refreshes them, and `pg_tviews_refresh(entity)` repairs changes the triggers did
  not see.
- `pg_tviews_analyze_select(text)` and `pg_tviews_infer_types(text, text[])`. They ran
  the text-pattern analysis that registration is moving away from, and
  `pg_tviews_infer_types` built its catalog query from its arguments unquoted. The
  upgrade script drops both; `docs/DEPRECATION_WARNINGS.md` lists what replaces them
  and states the oldest release `ALTER EXTENSION pg_tviews UPDATE` starts from
  (0.1.0-beta.20).
- `pg_tview_meta` columns `cascade_paths`, `fk_columns`, `uuid_fk_columns`,
  `dependency_types`, `dependency_paths`, `array_match_keys`, `direct_map_columns`,
  `direct_map_keys`, `distinct_on_keys`, `distinct_on_output_keys`, `is_union`,
  `aggregate_embeds` and `key_mappings`: replaced by `plan`.
- `pg_tviews_rebind_cascade_paths()`, `pg_tviews_migrate_triggers()` and the
  migration of triggers installed by releases before 0.1.0-beta.20: the update drops
  those triggers and re-installs one per TVIEW.
- `pg_tviews_convert_existing_table()` and `pg_tviews_convert_table()`, which only
  raised errors; the event trigger reports a `CREATE TABLE tv_* AS` the hook did not
  intercept itself.
- The `pg_tviews.metrics_enabled` setting, which had no effect.
- `scripts/auto-convert/`: it turned existing `tv_*` tables into TVIEWs through the
  conversion functions removed above, and no longer ran.

### Fixed

- **`pg_tviews_create_or_replace()` no longer drops a user's index** (#218). A
  `rebuilt` replace left out a user index named `idx_<tv>_data_gin` on a TVIEW without
  pg_tviews' GIN, and turning `data_gin_index` off dropped every GIN index on `data`.
  pg_tviews now records the indexes it creates and removes or leaves out only those.
  `options.data_gin_index` in `tviews.registry` reports pg_tviews' GIN index, so a
  user's GIN index on `data` no longer reads as the option being on (the key's
  documented meaning was the option; `contract_version()` stays 1).
- **Concurrent writes no longer leave TVIEW rows stale under READ COMMITTED**
  (#207). A transaction creating or re-linking a TVIEW row (a new post, a post
  pointed at another user) while another changes a row it reads (its author's
  name) computed the row without that change, and the other's refresh couldn't see
  the row: once both committed, the row kept the old value. Each now waits for the
  other on the join values (ADR 0207): embedded TVIEWs, direct joins, fan-out
  patches, joins several hops away, outer joins to a row inserted concurrently, and
  tables under the `full_refresh` policy. Two transactions creating the same row
  (a child carrying the key in its own row with no foreign key, the first rows of a
  new group of an aggregate TVIEW) wait for each other on that key. Under
  `REPEATABLE READ`, where such a row was stale in every order (a snapshot can't see
  what committed after it), one of the transactions now fails with `40001` instead:
  it never waits for these locks, and checks what it refreshed and what it found
  against the latest snapshot, as foreign-key checks do.
- **Concurrent first writes into an UNLOGGED TVIEW no longer fail on a duplicate
  key** (#214), and a TVIEW that is merely empty is no longer refilled from its
  view. pg_tviews told a TVIEW reset by a crash restart or a promotion from an
  empty one only by "empty while its view has rows", so every new backend's first
  write into an empty TVIEW refilled it, and two of them at once both inserted the
  view's rows (`duplicate key value violates unique constraint "tv_…_pkey"`). The
  new UNLOGGED table `tviews.pg_tview_valid` holds a row per UNLOGGED TVIEW whose
  rows can be trusted; a reset empties it together with the TVIEWs. The first write
  to a TVIEW missing from it claims the row and fills the TVIEW, the TVIEWs it reads
  first; concurrent writers wait for the claim (under `REPEATABLE READ` they get a
  retryable `40001`). `needs_rebuild`, `pg_tviews_rebuild_all()`,
  `pg_tviews_recover_after_crash()` and the startup worker follow it. Its rows are
  not dumped: a restored UNLOGGED TVIEW is refilled once.
- A column copied into `data` that the definition also joins or filters on was
  patched in place, leaving the values that depend on it stale.
- A TVIEW embedding another one twice (an author and an editor, both `tv_user`) was
  refreshed only through the first column: renaming the editor left it stale. Every
  output column equal to the child's key is followed.
- A write to a table whose key column the plan no longer finds raises an ERROR naming
  the TVIEW, with the `pg_tviews_reregister` hint; it was a log line once per backend,
  and the TVIEW went stale.
- A restored catalog row whose plan names a table the restore did not create fails
  the insert, naming the TVIEW, instead of mapping nothing.
- A TVIEW that reads only other TVIEWs' tables (`SELECT … FROM tv_user`) got no
  trigger, only a "No base table dependencies" WARNING, and was never refreshed.
- A recomputed row's document replaces the stored one whole. A TVIEW whose embeds
  are all scalar, with `jsonb_delta` installed, merged the fresh document into the
  stored one: a NULL document stayed NULL, and a key the view no longer produced
  stayed in the document.
- `pg_tviews_drop()` recorded the drop twice in the audit log.
- A raw-SELECT definition expanded with a column name holding a backslash is quoted
  as PostgreSQL's `quote_literal()` would, whatever `standard_conforming_strings` is.
- `pg_tviews_ensure_propagation_indexes()` and `pg_tviews_profile()` find the columns a
  TVIEW's rows are looked up by in its plan (embed lookups, fan-out patch columns): a
  lookup column not called `fk_*` got no index and no fan-out estimate, and an `fk_*`
  column nothing looks up through got an index.
- `pg_tviews_create_or_replace()` with a new column set failed with "array contains
  NULL" on a TVIEW that embeds another one.
- `pg_tviews_reregister(entity)` re-derives a TVIEW whose stored plan does not
  decode; it failed on the plan it was meant to replace.
- `pg_tviews_health_check()` reports TVIEWs whose plan does not decode (component
  `plans`), and a count or catalog revision it cannot read as an ERROR row; a failed
  count read as 0 (healthy), and an unreadable revision as a 0.1.0 catalog.
- `pg_tviews_mapping_query()` raises the error that stops it (42704 for an unknown
  TVIEW) instead of returning NULL; the rebuild worker restarts on an error instead
  of idling with a WARNING.
- `pg_tviews_create_or_replace()` called by a superuser (or any member of the owner's
  role) on another role's TVIEW recomputed its rows as the caller: the owner's view
  functions and the triggers on its table ran with the caller's privileges. Rows are
  now reconciled, and a rebuilt table filled, as the TVIEW's owner.
- Work queued again during a flush, by a trigger on a TVIEW's table writing a base
  table, was dropped for a key the flush had already refreshed, and the TVIEW stayed
  stale with no error. It is refreshed again.
- `DROP TABLE tv_*` of a TVIEW whose plan does not decode treated it as a plain table
  and left its catalog row, backing view and triggers behind (and with them failing
  writes); it and `pg_tviews_drop()` drop it cleanly.
- `pg_tviews_health_check()` reports a disabled `pg_tviews` trigger, and a missing
  one, as an ERROR (it reported disabled ones as healthy, missing ones as a WARNING).
- A column named `fk_*` keeps the type its definition gives it: it was forced to
  `bigint`, and a TVIEW with a UUID or text `fk_*` column could not be created.
- `pg_tviews_show_cascade_path()` raises 42704 for an unknown TVIEW and its errors
  instead of returning no rows; an audit-log write that fails after a create or a
  drop fails it, as it fails any other statement.
- An operator granted `pg_tviews_rebuild_all()` could not run it without `SELECT` on
  every TVIEW: it checked and counted their rows as the caller.
- A `DROP TABLE tv_*` or a column rename run by a function that `EXECUTE` or
  `CREATE TABLE AS` calls is intercepted like any other: the TVIEW was left
  registered with no table, or its definition kept the old column name.
- `ALTER TABLE tv_*` that would leave the refreshes writing a column that no longer
  fits is refused with 42809 (#208): `RENAME COLUMN`, `DROP COLUMN`, and `ALTER
  COLUMN … TYPE` on `pk_<entity>`, `id` or `data` or to a type the backing view's
  column does not convert to. It was accepted, and every later write to a base
  table failed.
- A definition whose `pk_<entity>` is not an integer (`smallint`, `integer`,
  `bigint`, or a domain over one) is refused with 42804 before anything is created,
  naming the column and its type, with a hint to keep a uuid in `id` (#209).
  `pg_tviews_create` failed with PostgreSQL's raw 42804 from its fill query.
- `pg_tviews_flush_and_report(reset => true)` in a subtransaction that rolls back
  (`ROLLBACK TO SAVEPOINT`, a plpgsql `EXCEPTION` handler) no longer loses what it
  reported: the next call reports those rows again (#210).
- **A subtransaction that commits inside a writing statement no longer drops the
  refreshes queued before it.** A plpgsql `BEGIN … EXCEPTION … END` block (an audit
  trigger on a base table, a function in the `SET` list) took the whole pending queue
  aside when it started and threw it away when it committed: every row the statement
  wrote before the block stayed stale, without a warning. Savepoints now leave the
  pending work in place and undo only what was queued inside one that rolls back.
- **A direct patch never calls a function outside jsonb_delta's schema.** When
  jsonb_delta was dropped between capturing a patch and flushing it, the flush called
  `public.jsonb_smart_patch_scalar`, which any role with `CREATE` on `public` could
  have planted, as the TVIEW's owner. It now fails, naming jsonb_delta.
- An error message cut a long query at byte 100 even inside a multi-byte character,
  and the formatting panic replaced the real error.
- **An error raised under another library's executor or utility hook no longer
  stops refreshes for the rest of the session.** With `pg_stat_statements` (or any
  other hook) loaded before pg_tviews, an error inside a writing query, caught by an
  EXCEPTION block, skipped pg_tviews' bookkeeping of the running query: every later
  statement in the session deferred its refresh to a statement that no longer ran,
  and the work was dropped at commit. Calls to the previous hooks are now guarded,
  and a rolled-back subtransaction forgets the queries it ran.
- **`pg_tviews_suspend_triggers()` and `pg_tviews_resume_triggers()` roll back with
  a savepoint.** A suspension inside a savepoint that was rolled back stayed in force
  for the rest of the transaction.
- `COMMIT` of a transaction that already failed runs no catch-up or refresh: the
  server rolls it back.
- A refresh write that fires a base table's flush (a user trigger on a TVIEW's
  table writing a base table) no longer starts a second flush inside the running
  one; the running flush takes the work.
- `DROP TABLE tv_a, other` in a function called twice dropped `tv_a` only the first
  time: the TVIEW was taken out of the function's cached plan. The plan is copied
  before it is changed.
- A `TRUNCATE` run by a trigger of a writing statement leaves the refresh to that
  statement, as its other nested statements do.
- The query tree walkers report a stack-depth error as an ERROR, the view-query
  reader refuses a relation that is not a view, and the DDL pg_tviews runs from a
  trigger or a function no longer gets a connection that may end the transaction.
- The quick start works as written: it loads the library in
  `shared_preload_libraries` and puts `tviews` on the `search_path`. The
  troubleshooting guide no longer recommends `pg_tviews_convert_existing_table`
  (it always fails), and `pg_tviews_health_check()` is documented with its real
  columns `(status, component, message, severity)`.

### Upgrade notes

- `ALTER EXTENSION pg_tviews UPDATE` (from 0.1.0-beta.20 or later) re-derives every
  TVIEW from its query tree, dependencies first, into `pg_tview_meta.plan`, drops the
  thirteen columns the plan replaces and re-installs one trigger per TVIEW (the
  per-table triggers of earlier releases are dropped). Nothing needs to run
  afterwards. A TVIEW that no longer analyses fails the update, naming it: fix or drop
  it on beta.26, then update again. Older installs move with
  `scripts/migrate-from-0.1.0.sql`.
- The update drops `pg_tviews_analyze_select()`, `pg_tviews_infer_types()`,
  `pg_tviews_cascade()`, `pg_tviews_insert()`, `pg_tviews_delete()`,
  `pg_tviews_rebind_cascade_paths()`, `pg_tviews_migrate_triggers()`,
  `pg_tviews_convert_existing_table()` and `pg_tviews_convert_table()`, and the type
  `tviews.tviewschema` with `CASCADE`: a user function or column using that type is
  dropped with it.
- The update revokes `EXECUTE` from `PUBLIC` on `pg_tviews_refresh_all()`,
  `pg_tviews_refresh_all_entities()`, `pg_tviews_rebuild_all()`,
  `pg_tviews_reregister_all()`, `pg_tviews_set_logged()`,
  `pg_tviews_ensure_propagation_indexes()` and `pg_tviews_invalidate_caches()`.
  Grant them to the role a deploy or restore tool runs as, if it is neither a
  superuser nor the extension's owner (`docs/user-guides/operators.md`).
- Writers to TVIEW base tables may now get `40P01` (deadlock) and, under
  `REPEATABLE READ`, `40001`: retry both. A commit that still has refresh work queued
  fails with `55000`.
- `ALTER EXTENSION pg_tviews UPDATE` (and `scripts/migrate-from-0.1.0.sql`) records,
  for each TVIEW, the indexes that are exactly those pg_tviews creates, under their
  names, as pg_tviews' (`tviews.registry.managed_indexes`). Any other index on a
  TVIEW's table is the user's, including one under such a name with another
  definition (a `jsonb_path_ops` GIN named `idx_<tv>_data_gin`, say). The update
  warns about each of those: rename it, since its name is now reserved and a dump of
  the database would not restore it.
- `ALTER EXTENSION pg_tviews UPDATE` creates `tviews.pg_tview_valid` and records
  every UNLOGGED TVIEW that holds rows as trusted. An empty UNLOGGED TVIEW is filled
  from its view by its next write, once (nothing to do when its view is empty too).

## [0.1.0-beta.26] - 2026-10-09

### Changed (breaking)

- **Refreshes render values under fixed settings, not the writer's** (#200):
  `TimeZone` `UTC`, `DateStyle` `ISO, YMD`, `IntervalStyle` `postgres`,
  `extra_float_digits` `1`, `bytea_output` `hex`, on every path that computes a
  TVIEW's rows (creation, writes, full and time refreshes, `create_or_replace`).
  Before, a `timestamptz` in a JSONB document was stored with the offset of the
  session that wrote last, so one database held several renderings and a TVIEW
  differed from its definition read from another zone. `CURRENT_DATE` in a refresh is
  now the UTC day. Rows written before the upgrade keep their rendering until
  refreshed: run `SELECT tviews.pg_tviews_refresh_all()` once after upgrading if a
  TVIEW renders dates, times, intervals, floats or `bytea` as text.

- **A TVIEW that reads a materialized view goes through its `uncascaded_policy`**
  (#189). No trigger sees a matview's rows, so the matview is listed in
  `uncascaded_tables` and `cascade_kinds` (`all_keys`): under the default `error`
  policy the TVIEW is refused, under `warn` it is created with a WARNING. Before, it
  was created silently and `REFRESH MATERIALIZED VIEW` left it stale for good.
- **A definition that reads the current time is declared, never silent** (#193):
  `CURRENT_DATE`, `CURRENT_TIMESTAMP`, `LOCALTIMESTAMP` and the other SQL time values,
  `now()`, `clock_timestamp()`, `statement_timestamp()`, `transaction_timestamp()`,
  `timeofday()` and one-argument `age()`, in the definition or in a view, subquery or
  CTE it reads. Its rows change with no write, so under the `error` and
  `full_refresh` policies it is refused unless it declares `"time_refresh":
  "external"`; under `warn` it is created with a WARNING. Before, it was created
  silently and went stale at the boundary (midnight, for `CURRENT_DATE`).
- **A call to a function that may read tables goes through the policy** (#193): a
  non-immutable function outside `pg_catalog` (a `STABLE` lookup reading a settings
  table) is refused under `error` and `full_refresh` unless the `function_reads`
  option declares the tables it reads, and warned about under `warn`. Before, it got
  a NOTICE and writes to those tables left the TVIEW stale.

### Added

- **`REFRESH MATERIALIZED VIEW` refreshes the TVIEWs that read the matview** under
  `full_refresh` (#189), plain or `CONCURRENTLY`, in the REFRESH's transaction.
- **A read under window functions partitioned by a linked column is traced** (#187):
  `ROW_NUMBER() / RANK() / FIRST_VALUE() … OVER (PARTITION BY o.fk_customer …)` in a
  view, joined on `fk_customer`, maps like `DISTINCT ON (o.fk_customer)`, refreshing
  the partitions a write leaves and enters, instead of `all_keys` (refused under the
  default policy, stale or rebuilt in full under the others).
- **UNION branches keyed by their own tables** (#188): a branch may derive
  `pk_<entity>` from an immutable expression of its table's row (`-l.pk_order_line`,
  `l.pk_order_line + 1000000000`) when two entities have their own key spaces, and
  the UNION may sit in a view the definition reads. A write to a branch's table
  refreshes that branch's keys; a table joined to the union's output refreshes every
  branch's. Before, such a branch had no key and the TVIEW was refused ("can never
  be refreshed").

- **A TVIEW reading another TVIEW's table is maintained** (#191), directly or
  through views: a view aggregating `tv_line` per order, joined on `order_id`, or a
  correlated subquery on a column other than its key. A refresh of the inner TVIEW
  refreshes the rows of the outer one it reaches, in the same flush; a read nothing
  links to the key goes through the `uncascaded_policy`. Before, such a read had no
  cascade kind, was not listed as uncascaded, and the outer TVIEW went stale even
  under the `error` policy. An embed (`fk_<entity> = pk_<entity>`) is propagated as
  before, with no trigger on the inner table.
- **A per-table `uncascaded_policy`** (#195): option `uncascaded_tables` of
  `pg_tviews_create_or_replace()`, `{"public.tb_locale": "full_refresh"}`, gives a
  table no cascade reaches its own policy, so one reference table no longer moves a
  whole TVIEW to `full_refresh` and every untraced read added later is still refused.
  A named table the definition doesn't read, or whose writes are traced, is refused.
  `tviews.registry.uncascaded_table_policies` reports the map.
- **`function_reads`** (#193): `{"public.label_suffix()": ["public.tb_setting"]}`
  declares the tables a function reads (`[]` for none). They become reads of the TVIEW
  no cascade reaches, with triggers: their policy (`uncascaded_tables` or the TVIEW's)
  decides what a write does, `full_refresh` rebuilding the TVIEW.
  `tviews.registry.function_reads` reports them, `base_tables` lists the tables.
- **`time_refresh` and `tviews.pg_tviews_refresh_time_dependent()`** (#193):
  `"time_refresh": "external"` (or `SET pg_tviews.time_refresh = 'external'` before
  `CREATE TABLE … AS` / `pg_tviews_create()`) accepts a time-dependent TVIEW;
  `tviews.registry.time_dependent` reports it, and
  `pg_tviews_refresh_time_dependent([tview])` refreshes it, or every one the caller
  owns, for pg_cron or the application to call at the boundary.
- **A join through `unnest(<array>)::T` is traced** (#196), in a CTE or a subquery,
  and a cast of a `LATERAL unnest` element: the element cast one by one is an
  element of `(<array>)::T[]`, mapped like #182's spellings. Before, the cast made the
  join opaque and the TVIEW was refused as unlinked.
- **A table joined to another column of a first-row subquery is traced** (#194):
  the product of each customer's first order (`ROW_NUMBER() … rn = 1`, or `DISTINCT
  ON`), joined on `fk_product`, maps a product write to the orders carrying it, then
  through their partition key to the TVIEW. The subquery's own table still maps only
  through its partition key.

### Fixed

- **`DROP EXTENSION pg_tviews CASCADE` drops the backing views** (#199). They are not
  extension members (so `pg_dump` keeps them) and stayed in `tviews`, and
  re-creating the extension and a TVIEW then failed ("the backing view … is already
  taken by another relation"). The `tv_*` tables stay as plain tables. A backing view
  left by a drop in a session that never loaded the library is dropped by the next
  `pg_tviews_create()` of that TVIEW, with a NOTICE.
- **`pg_tviews_refresh_all()`, `pg_tviews_refresh()` and `pg_tviews_rebuild_all()` leave
  nothing queued** (#202). Rebuilding a TVIEW whose table another TVIEW reads (#191)
  queued refreshes of the reader that no flush followed, so the transaction committed
  with "N queued refreshes … not applied (missing flush trigger?)". They now refresh
  what their rebuilds queued before returning.
- **A trigger writing its own table refreshes the TVIEWs once** (#197). Each statement
  the trigger ran flushed the refresh queue, so a tree cascade (one nested `UPDATE`
  per level) recomputed a row at depth *d* about *d* times, from intermediate states:
  122 flushes and 2005 recomputes for a 364-node rename, now 1 and 364. A statement
  nested in a write to a TVIEW's base table now leaves its work to that write; a query
  that only reads (`SELECT f()`) still refreshes after each write in `f`.

- **A refresh that fails fails the write.** The statement-level flush trigger turned
  an error the flush returned (rather than one PostgreSQL raised), such as
  `pg_tviews.max_propagation_depth` exceeded, into a WARNING and let the write commit,
  leaving TVIEWs stale. It now raises it, as the refresh runbook says.
- **Comments in a TVIEW definition** (#192): an apostrophe in a `--` or `/* */`
  comment no longer hides the rest of the definition ("No FROM keyword found").
- **`DROP SCHEMA … CASCADE` and `DROP OWNED BY` drop a TVIEW's backing view** (#186).
  Since backing views moved to `tviews` (beta.25), a TVIEW dropped with its schema
  left its view there, and creating the TVIEW again failed ("tviews.s__tv_a is
  already taken by another relation"). `DROP OWNED BY` also left the registration.
- **Two rows for one key of a UNION TVIEW fail the write** (#188): the bulk refresh
  that mapped keys use upserted both rows, the second silently winning, and the
  `union_duplicate_policy` error of the single-key path reached the flush trigger as
  a WARNING. Both paths now raise `cardinality_violation` (or keep the first row
  under `union_duplicate_policy = 'first'`).
- **A create after a failed create of the same TVIEW** inserted the failed
  definition's columns (`column "qty" of relation "tv_attachment" does not exist`): the
  columns of a backing view were cached by name across a rolled-back create. Caches
  are now cleared when a transaction or subtransaction aborts.

### Upgrade notes

- `ALTER EXTENSION pg_tviews UPDATE` drops the backing views beta.25 left in
  `tviews` after a `DROP SCHEMA … CASCADE` (views named `<schema>__tv_*` that no TVIEW
  owns, unless something depends on them), and marks every TVIEW for
  re-registration: run `SELECT * FROM tviews.pg_tviews_reregister_all();`.
- A TVIEW that reads a materialized view under the `error` policy (the default
  since beta.25) is listed by `pg_tviews_reregister_all()` with the refusal and keeps
  its old registration (no refresh on `REFRESH MATERIALIZED VIEW`). Declare what a
  REFRESH does: `SELECT tviews.pg_tviews_create_or_replace('<schema>.tv_<entity>',
  <definition>, options => '{"uncascaded_policy": "full_refresh"}');` (`altered`, and
  re-registered).
- The same holds for a TVIEW whose definition reads the time or calls a
  non-immutable function outside `pg_catalog` (#193): under `error` or `full_refresh`
  `pg_tviews_reregister_all()` lists it with the refusal and it keeps refreshing on
  writes as before. Declare `"time_refresh": "external"` (then schedule
  `SELECT tviews.pg_tviews_refresh_time_dependent();`) and/or `function_reads` with
  `pg_tviews_create_or_replace()`; `uncascaded_tables` gives the declared tables a
  policy without changing the TVIEW's.
- Refreshes run with `search_path = pg_catalog, pg_temp`: a function a definition
  calls must qualify the tables it reads, or `SET search_path` itself.
- `tviews.registry` gains `uncascaded_table_policies`, `function_reads`,
  `time_dependent` and `time_refresh` (appended; `contract_version()` stays 1).

## [0.1.0-beta.25] - 2026-10-06

### Changed (breaking)

- **A TVIEW's backing view lives in pg_tviews' schema** (#181):
  `tviews.<schema>__tv_<entity>` instead of `<schema>.v_<entity>`. By fraiseql's naming
  convention `v_<entity>` is the application's query view, so a schema that already had
  one could not get the TVIEW (`relation "v_order" already exists`). The application
  schema now holds only its own objects; `tviews.registry.view` reports the backing
  view, which follows `ALTER TABLE tv_x RENAME` and `SET SCHEMA`, and stays owned by
  the TVIEW's owner. A definition that embedded another TVIEW through its `v_<entity>`
  reads its `tv_<entity>` table. Its privileges follow the TVIEW's table: whoever can
  `SELECT` from `tv_<entity>` can `SELECT` from the backing view (only `SELECT` is
  copied), kept so after every `GRANT` / `REVOKE` on tables, including `ON ALL TABLES
  IN SCHEMA`, and `ALTER TABLE tv_x OWNER TO` changes the view's owner too. A grant on
  the application's schema no longer reaches the view by itself.
- **A TVIEW that reads a table no cascade reaches is refused unless it declares a
  policy.** `pg_tviews.uncascaded_policy` now defaults to `error` (was `warn`, which
  created it with a WARNING and stale rows on such writes). The refusal names each
  table with its reason, and its HINT gives the option or the setting to use.
  Existing TVIEWs keep their policy.

### Added

- **`uncascaded_policy` option** of `pg_tviews_create_or_replace()`: a TVIEW declares
  what a write to a table no cascade reaches does (`"error"`, `"full_refresh"` or
  `"warn"`), whatever the session setting says; changing it is an `altered` change.

### Fixed

- **A view named `v_<entity>` in the TVIEW's schema no longer blocks it** (#181), and
  `pg_tviews_create('tv_order', 'SELECT * FROM v_order')` materializes the
  application's view.
- An aggregate TVIEW is recognised as embedded by the OID of what reads it, whatever
  its backing view is called, also in a correlated subquery.
- `pg_tviews_create_or_replace()` replacing a TVIEW in place refreshes the TVIEWs that
  read its table, not only those reading its view; `pg_tviews_rebuild_all()` and
  `pg_tviews_replication_status()` see an UNLOGGED TVIEW reading an emptied `tv_*`
  table as needing a rebuild after a promotion.
- **A hierarchy joined through an array of ids refreshes every row it changes**
  (#182). A TVIEW whose key passed through a subquery with a set-returning function
  in its select list (`unnest(string_to_array(n.path, '.')::bigint[]) AS node_id`)
  classified its own table `all_keys`: an update or soft delete of the entity's own
  row was lost under `warn`. The `= ANY (<array>)` spelling of the same join refreshed
  the row itself but not the rows whose path holds it. A set-returning function now
  hides only its own output, and the three spellings (`unnest` in a subquery,
  `LATERAL unnest`, `= ANY`) map the joined read through the array membership.
- **A view with `WITH RECURSIVE` is accepted** (#183). A TVIEW reading a recursive
  view was refused as "more than 32 levels deep", and recursion written in the
  definition itself was refused outright. The tables read inside a recursive CTE are
  `all_keys` ("read in a recursive CTE (<view>)") and follow the TVIEW's
  `uncascaded_policy`; the tables read outside it keep their mapping.

### Changed

- **More joins are `mapped` instead of `all_keys`**: array membership
  (`= ANY (<array>)`, `unnest`), and joins on a subquery column computed by an
  immutable expression (`upper(n.name) AS code`). A mapping that scans a large table
  by such an expression is reported at create time with the `CREATE INDEX` to run
  (GIN for an array, btree for a scalar expression).

### Upgrade notes

- Run `ALTER EXTENSION pg_tviews UPDATE` as a superuser (or as the extension's owner
  when it owns every backing view): it moves each backing view to
  `tviews.<schema>__tv_<entity>`. Tools that read `<schema>.v_<entity>` must read
  `tviews.registry.view` instead (confiture: the version pinned by this release).
- The update gives each moved backing view the `SELECT` grants of its TVIEW's table.
  A role that read `<schema>.v_<entity>` through `GRANT SELECT ON ALL TABLES IN SCHEMA
  <schema>` or default privileges keeps reading it when it can read `tv_<entity>`
  (fraisier's empty-TVIEW probe does both). A role granted the view but not the table
  loses it: grant it `SELECT` on `tv_<entity>` instead. Hosts with custom ACLs should
  check `SELECT has_table_privilege('<role>', view, 'SELECT') FROM tviews.registry`.
- Then run `SELECT * FROM tviews.pg_tviews_reregister_all();` so that existing TVIEWs
  pick up the new mappings.
- Definitions created or replaced from now on must declare a policy when they read a
  table no cascade reaches (option `uncascaded_policy`, or `SET
  pg_tviews.uncascaded_policy` before `CREATE TABLE … AS`); existing TVIEWs keep theirs.

## [0.1.0-beta.24] - 2026-10-05

### Fixed

- **A write that reaches a TVIEW through a virtual generated column refreshes it**
  (#179, PostgreSQL 18). A virtual column (`GENERATED ALWAYS AS (…)` without
  `STORED`, PostgreSQL 18's default) is NULL in the rows a trigger sees and in
  transition tables, so a change to its inputs was skipped (a joined table read
  through it), filtered out as unchanged (a table reached through several joins), or
  mapped to no key (a join on it, a `DISTINCT ON` key on it), with no warning. A
  TVIEW reading a virtual column now reads its inputs, the changed rows used for key
  mapping compute it, and a key on one is mapped instead of read off the row. The
  direct and fan-out patches never copy one.

### Changed

- The documentation and coverage builds target PostgreSQL 18, the release target;
  the coverage workflow now measures the unit tests (it measured nothing) (#178).

### Upgrade notes

- After `ALTER EXTENSION pg_tviews UPDATE`, run
  `SELECT * FROM tviews.pg_tviews_reregister_all();` so that TVIEWs reading virtual
  generated columns follow their inputs.

## [0.1.0-beta.23] - 2026-10-03

### Changed

- **A `DISTINCT ON` TVIEW is keyed on its `DISTINCT ON` key, read from the query
  tree** ([ADR 0169](docs/adr/0169-tview-row-identity.md), #170). A second key system,
  derived from the definition's text and matched by column name, ran `DISTINCT ON`
  TVIEWs; it is gone. Every TVIEW now has one identity: `pk_<entity>`, or its
  `DISTINCT ON` key, which is its table's primary key and is reported in the new
  `tviews.registry.identity` column. Writes, refreshes and propagation all go through
  it, with the key's own type (`bigint`, `uuid`, `text`, `numeric`, `date`…).
- **A `DISTINCT ON` TVIEW reads tables through joins whatever its key.** Keyed on
  anything but `pk_<entity>`, it was refused unless `pg_tviews.uncascaded_policy` was
  `full_refresh` (#164); the tables it reads now map to its key like any TVIEW's.
- **Refused at create, with the key named**: a composite `DISTINCT ON (a, b)` (a TVIEW
  row is one entity with one key; it already failed, on a duplicate key), and a
  `DISTINCT ON` expression (every write to it was skipped with a WARNING).
- A TVIEW's table always has its primary key on the identity column (a `DISTINCT ON`
  key named `identifier`, `fk_*` or `*_id` got none).

### Fixed

- **A statement writing rows of several `DISTINCT ON` groups refreshes them all**
  (#171). It refreshed none: the flush's bulk path dropped every `DISTINCT ON` key.
- **Changing a row's `DISTINCT ON` key removes the old group's row** (#172). The
  trigger read the new row only. Keys are now read from the old and the new row.
- **A TVIEW embedding a `DISTINCT ON` TVIEW follows it** (#173). Propagation skipped
  `DISTINCT ON` keys; parents are now found from the `pk_<entity>` of the child rows a
  refresh touched, before and after, so they also follow a new winning row.
- **`DISTINCT ON (o.id)` is accepted when another table read has an `id` column**
  (#169).
- **A `DISTINCT ON` refresh uses the base table's index** (#174). It filtered on
  `key::text = $1`, which no index serves and which cannot be pushed below the
  `DISTINCT ON`: every refresh deduplicated the whole view. On 50,000 rows a
  one-group update went from 18 ms to 1.4 ms.
- **Writes to a TVIEW's root table are no longer lost when it is not named
  `tb_<entity>`** (#175). The root table's key was found by stripping `tb_` from its
  name; it now goes through its key mapping like any other table's.
- **A parent row that an inner join dropped with its child comes back with it**
  (#177). Parents were looked up only in their own table; for child rows that
  reappear they are looked up in their backing view too.

### Upgrade notes

- After `ALTER EXTENSION pg_tviews UPDATE`, run
  `SELECT * FROM tviews.pg_tviews_reregister_all();`. It records each TVIEW's identity,
  keys its table on it (adding the primary key a table lacked) and drops the unique
  index on `pk_<entity>` that 0.1.0-beta.22 gave some `DISTINCT ON` TVIEWs. Until
  then, a `DISTINCT ON` TVIEW is refreshed in full on writes to its own table, and
  every other TVIEW refreshes as before.

## [0.1.0-beta.22] - 2026-10-02

### Fixed

- **Two READ COMMITTED transactions refreshing the same TVIEW row no longer lose a
  change.** The second writer waited on the first's row lock inside its own refresh,
  then wrote the document it had computed before the first committed. The refresh now
  locks the existing rows it recomputes before it reads the view, so the recompute
  sees the other writer's commit. REPEATABLE READ and SERIALIZABLE are unchanged
  (the second writer gets SQLSTATE `40001`).
- **A table linked to the TVIEW key through the nullable side of an outer join
  cascades** (#165). `tb_line l LEFT JOIN tb_order o ON l.fk_order = o.pk_order` in a
  view, then `v.order_id = o.id`, left `tb_line` reported as uncascaded. A line with
  no matching order yields NULLs there and matches no key, so the link is followed
  when the path then ends at the key or goes on by an equality (a TVIEW keyed on the
  nullable side included).
- **A table that feeds only view columns a TVIEW never reads is no longer tracked**
  (#166). Every write to it mapped its rows to TVIEW keys and recomputed them for
  nothing: on a real schema a 2,163-row insert took 646 ms instead of 104 ms. A view,
  subquery or CTE is now walked only for the columns the level above reads (a
  column used for sorting, grouping or `DISTINCT`, or returning a set, always
  counts), and the tables behind the others get no trigger.
- **A `DISTINCT ON` TVIEW keyed on `pk_<entity>` can read tables through joins
  again** (#164, regression in 0.1.0-beta.21). Every `DISTINCT ON` TVIEW with a table
  mapped through a join was refused, with a count of the wrong tables. Keyed on
  `pk_<entity>`, or on a unique NOT NULL column of its own table (the TVIEW then gets
  a unique index on `pk_<entity>`), those tables now map to its rows. Keyed on
  anything else, it is refused with the tables named, unless
  `pg_tviews.uncascaded_policy` is `full_refresh`, which refreshes it in full on
  writes to them.
- **A write to a TVIEW's own table refreshes its row again when another read of the
  table can't be traced** (#162, regression in 0.1.0-beta.21). One untraceable read
  of a table (here, inside a `DISTINCT ON` view) made the whole table `all_keys`, and
  under the default `warn` policy its writes refreshed nothing, the written row
  included. The reads that can be traced now keep refreshing the rows they reach;
  only the rest is left to `pg_tviews.uncascaded_policy`, and the WARNING says so.
  The repro's view itself now maps entirely: a `GROUP BY` or `DISTINCT ON` view
  column equal to the key through a join (`DISTINCT ON (l.fk_order) o.pk_order` with
  `l.fk_order = o.pk_order`) passes through like the key.
- **A view whose CTEs read each other three or more deep, or that defines a CTE it
  never uses, is accepted again** (#163, regression in 0.1.0-beta.21). The analyzer
  counted a CTE body's references from the level it was walking instead of the level
  that defines the CTE, so a third CTE in a chain lost its table and the create was
  refused ("not found in the view's query"). An unused CTE's tables are now known but
  not tracked: nothing they hold can change the TVIEW.

### Upgrade notes

- **After `ALTER EXTENSION pg_tviews UPDATE`, run
  `SELECT * FROM tviews.pg_tviews_reregister_all()`**: it re-derives every TVIEW with
  the fixes above (a TVIEW's own table refreshing again, unused CTEs and unread view
  columns no longer tracked, outer-join links, `DISTINCT ON`). The update marks every
  TVIEW for it.

## [0.1.0-beta.21] - 2026-10-02

### Added

- **`pg_tviews.uncascaded_policy`** (#157, #158): what a new TVIEW does about a base
  table it reads that nothing links to its key (an uncorrelated subquery, a window
  function). `warn` (default) names the tables in a WARNING, `error` refuses the
  TVIEW, `full_refresh` refreshes the whole TVIEW at flush on every write to such a
  table. The policy is read once at create time and stored with the TVIEW; the
  writer's session setting never matters.
- **`tviews.registry.uncascaded_tables` and `uncascaded_policy`** (#157, #158),
  appended; `contract_version()` stays 1. The upgrade script marks every TVIEW for
  re-registration (`SELECT * FROM tviews.pg_tviews_reregister_all()`), which fills
  `uncascaded_tables`.
- **`tviews.registry.cascade_kinds`** (ADR 0157): how a write to each base table maps
  to TVIEW keys (`local`, `mapped`, `propagated`, `all_keys`), read from PostgreSQL's
  query tree of the backing view (views, CTEs, subqueries and `UNION` branches
  followed) rather than from the SQL text. Registration fails when that analysis and
  `pg_depend` disagree on the tables the view reads, and warns about non-immutable
  functions the view calls (the tables they read are not tracked). Stored in the new
  `pg_tview_meta.key_mappings`, with, for each `mapped` table, the generated query that
  turns changed rows into keys (schema-qualified names and operators, so it resolves
  nothing through `search_path`). A mapping query that would scan a large table
  sequentially is reported at create time with the index to add.
  `tviews.pg_tviews_mapping_query(tview, base_table)` shows the query.

### Fixed

- **`pg_tviews_performance_stats()` works on a server built without libxml**. It
  counted rows through `xpath(query_to_xml(…))`, which fails there ("unsupported XML
  feature"). It now counts each TVIEW directly; a TVIEW the caller cannot read gets a
  NULL `row_count` and a NOTICE.
- **An UPDATE of a base table no longer fails when its TVIEW projects an
  extension-typed column** (#156, regression in 0.1.0-beta.20). The refresh's no-op
  guard compared rows with `IS DISTINCT FROM`, which looks `=` up by name; under the
  owner's `search_path = pg_catalog, pg_temp` (#141) the `=` of `ltree`, `citext`,
  `hstore` or a domain over them was not found. Columns of a type with no `=` at all
  (`json`, `point`) failed the same way before 0.1.0-beta.20, whatever the
  search_path. The guard, and the row comparison of `pg_tviews_create_or_replace()`,
  now compare record images (`*=`, schema-qualified), which need no per-type operator.
- **Writes to a table read in a subquery or through a view refresh the TVIEW**
  (#157, #158). A table read only by `ARRAY(SELECT … WHERE l.fk_order = o.pk_order)`,
  or through a plain view with `GROUP BY`, got triggers but no cascade, and its writes
  were dropped without a word (#158 named only the view). Writes now map to the keys
  through the condition that links them (ADR 0157); a table nothing links to the key
  is named at create time instead of being dropped silently.
- **TRUNCATE of a base table refreshes its TVIEWs**: it left them stale.
- **No WARNING on every write without jsonb_delta** (#159). Each refresh that would
  have used smart patching sent `WARNING: jsonb_delta extension not installed` to the
  client. Each backend now writes it once to the server log (`LOG`), `CREATE
  EXTENSION pg_tviews` warns once when jsonb_delta is absent, and
  `pg_tviews_health_check()` reports it as before. The `union_duplicate_policy =
  'first'` duplicate-row message and the "initial column not found" cascade message,
  also sent on every write, are logged once per backend the same way.

- **A TVIEW whose own table is partitioned refreshes on writes**. Every write to a
  partitioned `tb_<entity>`, through the root or to a partition, was dropped with a
  "not managed by pg_tviews" WARNING: the row trigger PostgreSQL clones onto each
  partition looked the entity up by the partition. It now uses the partition root.

- **Refresh work no longer carries over into the next transaction**. `COMMIT` cleared
  neither the refresh queue nor the recorded patches (`PREPARE` and `ROLLBACK` did), so
  work a transaction queued without flushing ran in whichever transaction wrote
  next. It is now dropped at `COMMIT`, with a WARNING (once per backend) naming the
  TVIEWs, so a missing flush shows up at once.
- **`pg_tviews_cascade()`, `pg_tviews_insert()` and `pg_tviews_delete()` refresh
  before returning in autocommit**. Outside a transaction block they queued the work
  and nothing flushed it. Inside one they still queue it for the transaction's next
  flush.

- **A TVIEW with a `bit(n)` column (n > 1) can be created**: the column was created
  as `bit(1)` and the create failed.
- **Writes and `TRUNCATE` that target a partition directly refresh the TVIEW;
  partitions created or attached later are covered**. Only the partitioned table
  had the flush and `TRUNCATE` triggers, so in autocommit a statement naming a
  partition left the TVIEW stale until a later write, and `TRUNCATE` of a partition
  until a full refresh. Every partition now gets them, including partitions created
  (also from a partition manager's function) or attached after the TVIEW; a
  detached one loses them, and `ATTACH`/`DETACH` refresh the TVIEWs over the table.
  `TRUNCATE` of the partitioned table refreshes each TVIEW once.
  `pg_tviews_health_check()` reports a missing partition trigger, and no longer
  reports one of ours on a partition as orphaned. Existing TVIEWs get the triggers
  from `pg_tviews_reregister_all()`, which the upgrade already asks for.
- **A refresh that fails after `TRUNCATE` aborts the `TRUNCATE`**: it only warned,
  leaving the TVIEW stale.

- **A window function, `LIMIT`/`OFFSET`, a set-returning function or `GROUPING SETS`
  in a TVIEW's top-level SELECT now goes through `uncascaded_policy`**. It was
  treated as row-local: a write refreshed only its own row, while `count(*) OVER ()`,
  a rank or a `LIMIT` changes others, and the TVIEW went stale with no WARNING, under
  `full_refresh` too. Its tables are now `all_keys`, named at create time with the
  reason. Re-registering an existing TVIEW of that shape reports them the same way.
- **An `INTERSECT` or `EXCEPT` TVIEW stays equal to its view on `UPDATE`**. A change to
  a column copied into `data` was patched straight into the TVIEW, even when the set
  operation no longer returned the row. Set operations of every kind are now
  recomputed, as `UNION` ones were.

### Changed

- **Writes are mapped to TVIEW keys from PostgreSQL's query tree** (ADR 0157). The
  cascade paths re-parsed from the definition's SQL text are replaced by an analysis
  of the backing view's query tree. A table whose key is a column of the changed row
  keeps its row trigger (and the direct-patch and fan-out fast paths). Any other
  table gets statement-level triggers that map all the rows a statement changed with
  one query over its transition tables, instead of one lookup per row and hop.
  Single-row writes cost the same; a single-row write two hops away is about 30%
  faster (`test/sql/real_benchmark/results/adr_0157`).
- **After `ALTER EXTENSION pg_tviews UPDATE`, run
  `SELECT * FROM tviews.pg_tviews_reregister_all()`**: it re-derives every TVIEW and
  installs the new triggers, including the flush and `TRUNCATE` triggers on every
  partition of a partitioned base table, and reclassifies the shapes this release
  maps differently (a top-level window function or `LIMIT`, INTERSECT/EXCEPT).
  Until then, a TVIEW keeps its old triggers, and a write
  its old metadata maps through more than one hop refreshes it in full (logged once
  per backend).
- **`pg_tviews_refresh(entity)` also rebuilds the TVIEWs that embed it**, in
  dependency order. It rebuilt only the one TVIEW, so a manual repair left every
  TVIEW embedding it stale until each was refreshed by hand. The requested TVIEW is
  rebuilt with the caller's privileges, the TVIEWs embedding it as their owners.
  `pg_tviews_refresh_all()` still rebuilds each TVIEW once.
- **New TVIEWs keep the backing view's column types** (enums, domains, composites,
  arrays of them, types in other schemas, typmods such as `varchar(5)` or
  `numeric(6,2)`). They were stored as `text`, or lost their typmod, so ordering by an
  enum, comparing with the enum type, reading a composite's field or a domain's check
  did not work on the TVIEW. Existing TVIEWs keep their types until
  `pg_tviews_create_or_replace()` is run with their definition: it converts each such
  column in place and returns `altered`. Tools that read TVIEW column types see the
  real types.
- **Supported PostgreSQL versions: 16, 17, 18.** The `pg13`–`pg15` build features
  are gone, CI builds, lints and runs every SQL suite on each supported version, and
  `CREATE EXTENSION pg_tviews` on an older server fails with
  `pg_tviews requires PostgreSQL 16 or later`.
- **The no-op guard uses binary equality** (#156). A change that a type's `=` treats
  as equal but that changes the stored bytes is now written to the TVIEW: `citext`
  `'A'` → `'a'`, `numeric` `1.0` → `1.00`, `json` whitespace. The TVIEW holds exactly
  what its view returns. NULL still equals NULL.

- **An unknown `pg_tviews.*` setting is refused**. The `pg_tviews` prefix is reserved,
  so `SET pg_tviews.<name>` for a name pg_tviews does not define raises an error
  instead of being accepted and ignored. A setting copied from the old concurrency
  page (`pg_tviews.lock_timeout_ms`, `debug_refresh`, `max_cascade_depth`, which
  never existed) now fails: remove it.

### Deprecated

- **`pg_tviews.metrics_enabled`** has no effect: refresh metrics are always
  collected (`pg_tviews_queue_stats()`). Setting it still works and logs once per
  backend; it will be removed in a later release.

### Removed

- **The views `pg_tviews_queue_realtime`, `pg_tviews_cache_stats` and
  `pg_tviews_performance_summary`, and the function `pg_tviews_hook_status()`**.
  They returned fixed placeholder values. Use `pg_tviews_queue_stats()`,
  `pg_tviews_health_check()` and `pg_tviews_performance_stats()`. A view or function
  of yours that depends on one of them makes `ALTER EXTENSION pg_tviews UPDATE` fail
  with a dependency error: drop it first.

### Documentation

- **The concurrency page describes what pg_tviews does.** It documented per-row
  advisory locks, a `pg_tviews.lock_timeout_ms` setting, a required REPEATABLE READ
  "to avoid dirty reads" and debug settings, none of which exist. It now describes
  the refresh inside the writer's transaction, the row locks, the registration lock,
  and the one case READ COMMITTED gets wrong: two transactions refreshing the same
  TVIEW row at once through a full recompute can leave it stale; use REPEATABLE READ
  and retry, or `pg_tviews_refresh()`.
- **Every relative link in the README and `docs/` resolves**, and a regression test
  keeps it that way (49 pointed at pages that never existed or had moved).
- **The DDL reference matches what ships**: INTERSECT/EXCEPT are maintained, window
  functions and `LIMIT` go through `uncascaded_policy`, partitions are covered, no
  10-table limit. The API reference documents every public function
  (`pg_tviews_refresh`, suspension, change reports, aggregates, replication) and lists
  the internal ones; the README lists `pg_tviews.report_max_tracked`.
- **The operations runbooks query only what pg_tviews has** (#150). The runbooks,
  their scripts and `docs/TROUBLESHOOTING.md` queried relations and columns that never
  existed (`pg_tviews_metadata`, `pg_tviews_queue`, `last_refreshed`, …). They now use
  `tviews.registry`, `pg_tviews_health_check()`, `pg_tviews_profile()`, `updated_at`
  of each TVIEW and the PostgreSQL statistics views, and describe the refresh queue
  as it is: in memory, inside each transaction. `queue-cleanup.sql` is archived (there
  is nothing to clean). A regression test runs every runbook script and rejects the
  phantom names.
- The bulk-load section of the README runs the suspend/resume pattern in one
  transaction: suspension ends with the transaction.

## [0.1.0-beta.20] - 2026-10-01

### Added

- **`tviews.registry.view`** (#151): the backing view of each TVIEW, a `regclass`
  appended as the last column (NULL when the view is gone). An addition:
  `contract_version()` stays 1.
- **Versioned extension SQL and upgrade scripts** (#137, ADR 0136). The extension
  version is now the release (`0.1.0-beta.20`), where every release so far installed
  as `0.1.0`. Each release ships `pg_tviews--<previous>--<release>.sql`, so
  `ALTER EXTENSION pg_tviews UPDATE` upgrades a database, and CI checks that an
  upgraded catalog equals a fresh install (`test/upgrade/`). The policy is in
  `docs/development/extension-versioning.md`.
- **Library/catalog guard** (#137). A library installed without `ALTER EXTENSION
  pg_tviews UPDATE` refuses to work against the older catalog: base-table writes and
  `pg_tviews_*` calls fail with `pg_tviews library catalog revision <n> does not match
  the installed extension (<m>)` and the command that fixes it, instead of running
  against a catalog it does not know. `pg_tviews_health_check()` reports it
  (`catalog`), and the rebuild worker logs it once and idles.
- **`pg_tviews_reregister(entity)` and `pg_tviews_reregister_all(strict => false)`**
  (#137) re-derive TVIEWs' metadata and base-table triggers from their stored
  definitions with the current release's analysis, in place, without touching their
  rows, and clear the new `pg_tview_meta.needs_reregister` flag. `reregister_all` runs
  dependencies first, one subtransaction per TVIEW, and returns each one's status.
  TVIEWs created before #120, #126 or #130 get fan-out patches, aggregate embeds and
  direct maps without being dropped. `pg_tviews_health_check()` reports TVIEWs to
  re-register (`reregister`).
- **`scripts/migrate-from-0.1.0.sql`** (#137) moves a `0.1.0` install (every release up
  to 0.1.0-beta.19) to this release in one transaction, keeping the TVIEWs and their
  rows: it refuses when objects outside the extension depend on it, saves the
  registrations, re-creates the extension in `tviews` and re-registers every TVIEW.
- **A read contract for tools** (#133, ADR 0136). `tviews.registry` has one row per
  TVIEW: `schema`, `name`, `entity`, the normalized `query`, `base_tables` (every
  table the backing view reads through views, sorted), `logged`, `options` (`logged`,
  `fillfactor`, `data_gin_index`, `group_keys`, read from the system catalogs) and
  `needs_reregister`. `tviews.contract_version()` (1) versions it: additions keep the
  number, anything else bumps it. Both are plain SQL, readable by every role, on a
  standby and without the library. `pg_tview_meta` and the other `pg_tview_*` tables
  are documented as internal (`docs/reference/read-contract.md`).
- **`pg_tviews_create_or_replace(name, query, options)`** (#134, ADR 0136). Creates a
  TVIEW, or brings an existing one to the definition and options with the smallest
  change, and says which: `created`, `unchanged` (same definition as rendered by
  `pg_get_viewdef`, same options), `altered` (only `logged`, `fillfactor` or
  `data_gin_index` differ: changed in place, rows kept), `replaced` (a definition with
  the same columns: the view replaced and the rows reconciled in place, writing only
  rows that change, dependents and indexes untouched; the TVIEWs reading its view are
  re-registered and reconciled the same way, as their owners) or `rebuilt` (other columns or `group_keys`; the table's
  owner, privileges and comment, the GraphQL type name and user indexes carried over;
  refused, naming the reason, when something depends on the TVIEW or it has what a
  rebuild cannot carry). `options` keys omitted keep their current value; unknown or
  wrongly typed keys are errors. Names can be `tv_post`, `post` or `app.tv_post`, must
  match the definition's key, and name one TVIEW per entity in the database. An invalid
  definition raises its error. The DDL runs as the caller: replacing or dropping a
  TVIEW requires owning it, and no superuser is needed. Calls for one entity are
  serialized with an advisory lock (also taken by `pg_tviews_create`, `pg_tviews_drop`
  and `pg_tviews_reregister`). Works in any transaction, `DO` block or batch.
- **confiture's TVIEW suites run in CI** (#135). `.github/workflows/confiture.yml`
  builds the commit under test, preloads it on PostgreSQL 18 and runs confiture's
  TVIEW suites at a pinned confiture commit: informational on pull requests,
  required by the release workflow before a tag is published.
- **Release tarball layout** (#137): `lib/pg_tviews.so` and `extension/` (control file,
  install and upgrade scripts), to copy into `pg_config --pkglibdir` and
  `pg_config --sharedir`/extension.

### Changed

- **`CREATE TABLE tv_* AS SELECT` runs the shared create code** (#134). The
  ProcessUtility hook used to let PostgreSQL create a plain table, drop it from the
  event trigger, rebuild it as a TVIEW and populate it after the statement; it now
  creates the TVIEW itself before PostgreSQL creates anything, with the code behind
  `pg_tviews_create_or_replace()` and `CREATE TABLE AS` semantics: an existing TVIEW
  is an error, `IF NOT EXISTS` makes it a notice, and the command tag reports the rows
  (`SELECT n`). `CREATE UNLOGGED TABLE` and `WITH (fillfactor = n)` are honoured.
  What a TVIEW cannot honour is refused with a hint to `pg_tviews_create_or_replace()`:
  `SELECT … INTO`, `TEMP`, a column list, `TABLESPACE`, `USING`, other storage
  parameters, `WITH NO DATA`, `AS EXECUTE`, a query with parameters (PL/pgSQL
  variables), and `EXPLAIN [ANALYZE] CREATE TABLE tv_* AS`. `CREATE MATERIALIZED VIEW
  tv_*` is left to PostgreSQL. The statement is only inspected inside the hook's panic
  guard; the creation runs outside it.
- **`pg_tviews_create()` and `pg_tviews_create_aggregate()` share that code** (#134):
  they are create-only, and the name must match the definition's key (they used to
  name the table after the definition's `pk_*` column, whatever the name passed).
- **`pg_tviews_drop()` accepts a schema-qualified name** (#134) and requires owning
  the TVIEW. `DROP TABLE tv_*` is now handled outside the ProcessUtility hook's panic
  guard, so its errors reach the client as PostgreSQL raised them.

- **The extension lives in schema `tviews`** (#136, ADR 0136). `CREATE EXTENSION
  pg_tviews` used to install into the first schema on `search_path`, with
  `pg_tviews_performance_summary` and the audit-log index hardcoded to `public`; every
  object is now in `tviews`, which `CREATE EXTENSION` creates when missing. `WITH
  SCHEMA` is refused, and so is an existing `tviews` schema owned by another role. The
  install script no longer uses `CREATE OR REPLACE` or `IF NOT EXISTS`, so an object
  planted under one of its names fails the install instead of being reused. Function
  names keep their `pg_tviews_` prefix: call them as `tviews.pg_tviews_create(…)` or
  add `tviews` to `search_path`. Nothing pg_tviews does at run time needs it there:
  base-table and event triggers call the extension's functions qualified.

- **Ordinary roles can use a database with pg_tviews** (#136, ADR 0136). Everything
  pg_tviews did ran as the current role, so only the extension owner could write to
  a base table (`permission denied for table pg_tview_meta`). Now:
  - the flush reads and writes each TVIEW as the **owner of its `tv_*` table**, as
    `REFRESH MATERIALIZED VIEW` does: in a security-restricted operation, with
    `search_path` set to `pg_catalog, pg_temp`. A writer needs only its privileges on
    the base tables it writes; application roles can get `SELECT` only on `tv_*`.
    A multi-hop cascade reads the intermediate tables as the TVIEW's owner as well;
  - `PUBLIC` gets `USAGE` on schema `tviews` and `SELECT` on `pg_tview_meta` and
    `pg_tview_helpers` (view definitions, which `pg_views` already shows). The audit
    log gets no grant;
  - a `DROP … CASCADE` that takes the backing view of a TVIEW another role owns
    deregisters that TVIEW instead of aborting; its table is dropped only if the
    dropping role owns it, and kept as a plain table otherwise;
  - pg_tviews' own base-table triggers are dropped as the tables' owners, registrations
    and audit rows are written as the extension's owner (`performed_by` is the session
    user), and no SQL function lets another role write them.

### Fixed

- **`CREATE EXTENSION pg_tviews` works without `shared_preload_libraries`** (#134).
  Loading the library lazily defined the postmaster-level
  `pg_tviews.auto_rebuild_databases` setting after startup, which ended the session
  with `FATAL: cannot create PGC_POSTMASTER variables after startup`. The setting (and
  the rebuild worker it configures) now exists only when the library is preloaded; a
  lazily loaded library creates, replaces and refreshes TVIEWs. CI runs
  `test/no_preload/run.sh` on a cluster without the preload.
- **The docs name only functions pg_tviews has** (#138). README, INTEGRATION_GUIDE and
  `docs/` described 33 functions no release shipped (`pg_tviews_install_stmt_triggers`,
  `pg_tviews_refresh_one`, `pg_tviews_commit_prepared`, `pg_tviews_metadata`, …). Their
  passages now use the real functions (statement-level triggers are installed with
  every TVIEW; prepared transactions need no call; `tviews.registry` replaces the metadata
  function) or are gone; the never-implemented v2.0 plans moved to `docs/archive/`.
  `test/sql/regress/docs/regress_documented_functions.sql` fails CI when a published doc
  (Markdown or JSON) names a `pg_tviews_*` / `pg_tview_*` object that `CREATE EXTENSION`
  does not create, apart from names users choose and the relations #150 tracks.
- **`pg_tviews_drop(name, if_exists => true)` on a missing TVIEW** (#152) returned
  "dropped successfully". It now raises `NOTICE: TVIEW "<name>" does not exist,
  skipping`, like `DROP TABLE IF EXISTS`, and returns `TVIEW '<name>' does not exist,
  nothing dropped`.
- **`pg_tviews_health_check()` checks pg_tviews' own triggers** (#139). Its
  orphaned-trigger check matched `tview_%`, which no pg_tviews trigger is named, and
  looked each TVIEW's base table up as `('tb_' || entity)::regclass`: wrong for an
  aggregate TVIEW or one reading several tables, and an error (`relation "tb_post"
  does not exist`) for a TVIEW off the `search_path`, failing the whole call. A
  trigger now counts when it calls a pg_tviews trigger function, and is orphaned
  when the entity it carries is not registered or its backing view does not read the
  trigger's table; the copies PostgreSQL makes on partitions are not counted. The
  check also reports missing triggers (a table a TVIEW reads without its row or flush
  trigger) and triggers without an entity, and lists up to ten of each.
- **Long or multibyte trigger names** (#136). A base-table trigger is named
  `trg_tview[_flush]_<entity>_on_<schema>_<table>`, which PostgreSQL truncates to 63
  bytes: two TVIEWs whose names share their first 53 characters could not both
  trigger on one table (`trigger … already exists`), and dropping a TVIEW over a table
  in a schema with multibyte characters left its triggers behind, because removal cut
  the name to 63 characters. Over-long names are now shortened with a hash, the way
  index names are, and a TVIEW's triggers are found by their function and the entity
  they carry as argument, so a renamed table or schema no longer hides them either.

### Upgrade notes

- **From 0.1.0-beta.19 or any earlier release** (all installed as `0.1.0`): install
  the new package, restart PostgreSQL, then run `scripts/migrate-from-0.1.0.sql` in
  each database (`psql -v ON_ERROR_STOP=1 -d <db> -f scripts/migrate-from-0.1.0.sql`).
  It keeps the TVIEWs and their rows; the audit log is not carried over. Until it
  runs, writes to the TVIEWs' base tables fail with the catalog revision error.
- **From this release on**: install the new package, restart, `ALTER EXTENSION
  pg_tviews UPDATE` in each database, and `SELECT * FROM
  tviews.pg_tviews_reregister_all()` when the release notes say so.
- The extension moves to schema `tviews`. Unqualified calls to `pg_tviews_*`
  functions need `tviews` on `search_path` (for example `ALTER DATABASE … SET
  search_path = "$user", public, tviews`). `DROP EXTENSION pg_tviews` leaves the
  `tviews` schema behind, empty.
- The refresh runs with `search_path = pg_catalog, pg_temp`, as `REFRESH MATERIALIZED
  VIEW` does. A function the backing view calls that names objects without a schema
  must set its own `search_path` (`ALTER FUNCTION … SET search_path = …`).
- A TVIEW's owner now needs `SELECT` on what its backing view reads, and write
  access to its `tv_*` table, as it always did to create it; the roles writing to
  base tables no longer need any privilege on TVIEWs. Replacing, dropping,
  re-registering or renaming (`pg_tviews_set_typename`) a TVIEW requires owning it.

## [0.1.0-beta.19] - 2026-09-30

### Added

- **Parent columns are written into all children at once** (#120, ADR 0078 class C).
  When a TVIEW copies a joined parent's column into its `data` (`'author_name',
  u.name`), an UPDATE of that column is applied to every child in one statement
  keyed by the child's `fk_*` column, instead of recomputing each child from its
  view: 1.8–2.7× faster per parent update from 10 to 10 000 children. Controlled by
  `pg_tviews.direct_patch_enabled`. Cascade paths record it as `fanout`, so TVIEWs
  created before this release keep recomputing until they are re-created.
- **`pg_tviews_flush_and_report()`** (#76): flushes pending refreshes and returns the
  TVIEW rows the transaction changed in the GraphQL Cascade shape (`updated` with
  `__typename`, `id`, `operation` and fresh `data`; `deleted`; `truncated`;
  `invalidated_types`). Every refresh write journals the rows it really changed, so
  cascaded rows are included and no-op refreshes and rolled-back savepoints are not.
  `pg_tviews_set_typename()` overrides the reported type name (new
  `pg_tview_meta.graphql_typename` column); `pg_tviews.report_max_tracked` bounds the
  journal. See `docs/user-guides/graphql-cascade.md`.
- **`pg_tviews_profile(entity DEFAULT NULL, fanout_warn DEFAULT 1000)`** (#74): per-TVIEW
  physical health from the catalogs and statistics views (sizes, TOAST, HOT ratio,
  fillfactor, dead tuples, unused and missing propagation indexes, estimated fan-out
  per `fk_*`) with a `warnings` column. Read-only and callable on a standby; the columns
  are a stable contract (`docs/reference/profile.md`).
- **Aggregate TVIEWs** (#58): `pg_tviews_create_aggregate(tview_name, select_sql,
  group_keys)` materializes the `GROUP BY` groups of its source tables, keyed by
  `pk_<entity>`, with no `tb_<entity>`. `group_keys` names, per source table, the column
  whose value is the group key; each write refreshes the groups of its row (both when
  it moves), inserting new groups and deleting emptied ones. Window functions and
  expression keys are rejected. See `docs/user-guides/aggregate-tviews.md`.

- **Replication support for UNLOGGED TVIEWs** (#75). A hot standby cannot read
  an UNLOGGED table, and promotion or a crash restart empties it. Before, such a
  TVIEW stayed empty until something wrote to its base tables. New:
  - `pg_tviews_is_replica_readable(entity)` and
    `pg_tviews_replication_status()` show which TVIEWs a standby can serve. Both
    are read-only and callable on a standby.
  - `pg_tviews_rebuild_all(only_empty DEFAULT true)` rebuilds emptied UNLOGGED
    TVIEWs, dependencies first, and returns each entity with its row count.
  - `pg_tviews_set_logged(entity, logged)` switches a TVIEW between LOGGED and
    UNLOGGED.
  - The `pg_tviews.auto_rebuild_databases` GUC (postmaster, default empty)
    starts one background worker per listed database. The worker runs
    `pg_tviews_rebuild_all()` whenever the server leaves recovery: at startup,
    after a crash restart, and on promotion.
  - `docs/operations/replication.md` states the contract.
  - CI runs `test/replication/promote_rebuild.sh` against a real standby.

### Changed

- **Propagation stops at a row whose refresh changed nothing** (#85). Along an edge where a
  parent embeds the child's computed document (a nested object or array of `v_<child>.data`),
  parents are no longer looked up and recomputed when the child's row came out unchanged.
  A no-op update of a user with 20 posts and 60 comments now recomputes 1 row instead of 81.
  Scalar embeds that follow the child's FK to a deeper relationship still propagate.
  `propagation_pruned` in `pg_tviews_queue_stats()` counts the skipped edges.

- **Less fixed cost per refresh** (#91). TVIEW metadata is cached per backend and a warm
  refresh makes no catalog queries (`catalog_lookups` in `pg_tviews_queue_stats()`
  counts the misses); the per-row recompute upserts straight from the backing view,
  which is now evaluated once instead of twice; the UNLOGGED crash probe runs once per
  backend and TVIEW instead of once per transaction. 2000 single-row refreshes: direct
  patch ~665 ms → ~485 ms, per-row recompute ~1390 ms → ~605 ms. The caches follow
  other sessions' changes: DDL on a TVIEW's table or view, and every write to
  `pg_tview_meta`, invalidate them in every backend.

- **Breaking: `pg_tview_meta.view_oid` and `table_oid` are `regclass`**, not
  `oid`, so a dump stores them as names (#96). They now print as relation names;
  cast with `::oid` to get the number. Comparisons with an `oid` still work.

### Deprecated

- **`pg_tviews_convert_existing_table()` now raises a deprecation error** (#90). It failed
  on PG18 with a Datum type error and, by design, replaced the table with a view over a
  literal `VALUES` snapshot (no triggers, no refresh). Use `pg_tviews_create()` or
  `CREATE TABLE tv_<entity> AS SELECT ...`. The function is removed in the next breaking
  release.

### Removed

- `src/refresh/array_ops.rs` (#93): its element-level array functions had no caller since
  array dependencies moved to full replacement (#50), and they interpolated values into SQL.

### Fixed

- **A TVIEW embedding an aggregate TVIEW follows it** (#126). Propagation finds parent
  rows by `fk_<child>`, which no parent has for an aggregate, so `tv_user LEFT JOIN
  v_user_summary` kept the old summary after every order write (and `null` for a new
  group). The create now records the output column joined to `pk_<aggregate>` (new
  `pg_tview_meta.aggregate_embeds`, indexed unless it is the primary key) and
  propagation looks parent rows up by it. A definition that reads an aggregate but
  projects no column carrying its key is rejected with an explanation.
- **DDL works in databases without the extension** (#128). The hook is loaded
  cluster-wide through `shared_preload_libraries` and looked tables up in
  `pg_tview_meta` even where `pg_tviews` was not installed, so `DROP TABLE` failed there
  with `relation "pg_tview_meta" does not exist`. The hook now leaves such databases
  alone.
- **Direct patch no longer leaves expression keys stale** (#130). With
  `jsonb_build_object('bio', bio, 'ub', upper(bio))`, an update of `bio` patched only
  `bio` and left `ub` at its old value. A column that also feeds another value of the
  `data` builder (an expression, a nested object, a key the map cannot hold) is now
  left out of the direct map, so an update to it recomputes the row.
- **Refresh no longer depends on `search_path`** (#122). DML on the base table of a
  TVIEW whose schema was not on `search_path` failed with `relation "tv_post" does not
  exist`: the refresh named `tv_<entity>`, `v_<entity>` and the extension's
  `pg_tview_meta` unqualified. Every relation the refresh, propagation and catalog code
  names is now schema-qualified through its catalog OID or the extension's schema, and
  `jsonb_smart_patch_*` calls are qualified with `jsonb_delta`'s schema, so refresh also
  works under `search_path = pg_catalog`. `pg_tviews_performance_stats()` reads each
  TVIEW through its OID (it used to fail on its own row count query).
- **TVIEWs refresh dependencies first** (#124). `topo_order` came out dependents-first
  and the flush regrouped keys into a hash map, so when one flush held keys of a TVIEW
  and of a TVIEW whose view reads its `tv_*` table (e.g. an application trigger on
  `tb_post` updating `tb_user`), the reader could be refreshed from the old row and was
  then skipped as already processed: 2 of 8 such updates left `tv_post` stale. The
  flush now refreshes one entity at a time in dependency order, so every key is
  refreshed once, after everything it reads. `pg_tviews_resume_triggers()` rebuilds in
  that order too, and so does `pg_tviews_refresh_all_entities()` (it used catalog order).
- **`pg_tviews_refresh_all()` works** (#124). It read a `pg_tview_refresh_queue` table
  that does not exist and always failed. It now rebuilds every TVIEW, dependencies
  first, and returns `refreshed_count`, `order` and `duration_ms` (`queued_count` is gone).
- **`pg_tviews_suspend_triggers()` / `pg_tviews_resume_triggers()` work** (#44). The row
  trigger only honoured the `pg_tviews.suspend_triggers` GUC, so the functions suspended
  nothing, and resuming enqueued pk 0, which refreshes nothing. Suspension now skips
  refresh; the outermost resume, or a COMMIT while still suspended, rebuilds every TVIEW
  the suspended writes touched and every TVIEW embedding one of them. An implicit commit
  while suspended warns which TVIEWs are stale. Updating 10 000 of 20 000 rows: 1 493 ms
  before (refreshed row by row), 135 ms now.
- **Cascades through complex CTEs** (#60). A base table reachable only through a CTE
  chain (a CTE reading an earlier one), a CTE body joining several tables, or a
  UNION-bodied CTE got no cascade path, so its changes never reached the TVIEW. A
  resolved CTE now inlines its base tables, their join edges and a per-column map into
  the outer join graph, so each of those tables gets its own (possibly multi-hop) path.
- **Moving a row to another parent refreshes both parents.** The row trigger followed
  cascade paths from the new row image only, so an UPDATE that changed a child's FK
  (a comment moved to another post) refreshed the new parent and left the old one still
  showing the child. On UPDATE both images are now followed.

- **`PREPARE TRANSACTION` works with pending TVIEW refreshes** (#59). It was rejected;
  the queue is now flushed first, as before `COMMIT`, so the TVIEW writes belong to the
  prepared transaction and `COMMIT PREPARED` / `ROLLBACK PREPARED` apply or discard them.
  The never-built GID queue scaffolding (`pg_tview_pending_refreshes`) is gone.
- **The first write to an empty UNLOGGED TVIEW no longer locks out readers.** It looked like
  a crash-reset table, and the repopulation used `TRUNCATE`, holding ACCESS EXCLUSIVE until
  the transaction ended (until `COMMIT PREPARED` under 2PC). An empty TVIEW is now filled
  with a plain `INSERT … SELECT`; `pg_tviews_rebuild_all()` does the same.
- `pg_tviews_cascade()` / `pg_tviews_insert()` / `pg_tviews_delete()` failed with
  `SpiError(NoAttribute)`: their catalog query lacked columns the loader reads.

- **Normal DDL is quiet again (#92).** `CREATE TABLE tv_*`, CTAS and `pg_tviews_create` no
  longer print `EVENT TRIGGER` banners, `DEBUG:` lines or `spi_run_ddl()` INFO output. The
  diagnostics are `DEBUG1` messages (`client_min_messages = debug1`), or NOTICEs with
  `SET pg_tviews.log_level = 'debug'`.
- **Dropping any object a TVIEW reads now deregisters that TVIEW** (#57). Before,
  only the eponymous case was handled: dropping `tb_<entity>` deregistered
  `tv_<entity>`. A TVIEW that read the dropped table under another name, for
  example through a join, lost its backing view to `CASCADE`. Its `tv_*` table,
  its `pg_tview_meta` row and its triggers on the surviving base tables were
  left behind. The `sql_drop` handler now matches any TVIEW whose backing view
  or table is dropped as a dependent of a base table, a helper view or a schema.
- `pg_tviews_drop` finds a TVIEW's triggers by name instead of through the
  backing view. It no longer leaves them behind when the view is already gone.
- **A column rename on a base table no longer leaves TVIEW metadata stale**
  (#81). `pg_tview_meta.definition` kept the old column name. Everything derived
  from it at creation did too, so propagation broke silently: updates to a
  renamed joined column no longer cascaded, and a renamed FK stopped the cascade.
  After `ALTER … RENAME COLUMN`, each TVIEW whose backing view reads the column
  now has its definition rewritten in place: the author's text is kept and only
  the renamed references change. A bare select item gets `AS <old name>`, so the
  TVIEW's columns keep their names. Its metadata is then re-derived. If the
  rewrite does not define exactly the renamed backing view, the definition falls
  back to `pg_get_viewdef` text (with a NOTICE).
- The per-transaction cascade-path cache was only cleared on abort, so a
  committed metadata change could be served stale paths by the same session.
- **`pg_dump` / `pg_restore` round-trips TVIEWs** (#96). The catalog tables
  `pg_tview_meta` and `pg_tview_helpers` are marked with
  `pg_extension_config_dump`, so their rows are dumped; before, a restored
  database had `tv_*`, `v_*` and the triggers but no registered TVIEW, and writes
  to the base tables no longer reached the TVIEW. On restore, an insert trigger
  on `pg_tview_meta` rebinds the relation OIDs stored in `cascade_paths`.
  Databases whose extension was created on beta.18 or earlier do not get the
  marking and must re-register their TVIEWs after a restore.
- `CREATE TABLE tv_* AS SELECT ...` under a `search_path` without the extension
  schema (for example `search_path = ''`, as in `pg_dump` scripts) no longer
  fails with `function pg_tviews_convert_table(text, text) does not exist` (#96).
- A CTAS TVIEW that joins a table in another schema no longer fails at creation
  with `relation "<tview schema>.<table>" does not exist`.
- The README said `ALTER TABLE … SET LOGGED` truncates the TVIEW. It keeps the
  rows.

### Upgrade notes

- The extension's SQL version stays `0.1.0` and there is no upgrade script. This
  release changes `pg_tview_meta` (`view_oid`/`table_oid` become `regclass`; new
  `graphql_typename`, `group_keys` and `aggregate_embeds` columns) and adds SQL
  functions, so an existing database must re-create the extension after installing
  the new library: save each TVIEW's definition
  (`SELECT entity, definition, group_keys FROM pg_tview_meta`), drop each TVIEW
  (`DROP TABLE tv_<entity> CASCADE`), `DROP EXTENSION pg_tviews`, `CREATE EXTENSION
  pg_tviews`, and re-create the TVIEWs (`pg_tviews_create` /
  `pg_tviews_create_aggregate`), dependencies first.
- Re-creating also records the new per-TVIEW metadata: the fan-out patch (#120), the
  aggregate embeds (#126) and the direct map without shared columns (#130).
- `pg_tviews_refresh_all()` returns `{refreshed_count, order, duration_ms}`;
  `queued_count` is gone (#124).

## [0.1.0-beta.18] - 2026-09-30

### Added

- `refresh_noop_skipped` in `pg_tviews_queue_stats()`: session-cumulative count
  of refresh writes skipped because nothing changed.
- `pg_tviews_ensure_propagation_indexes(entity DEFAULT NULL, dry_run DEFAULT false)`
  adds the missing propagation indexes to existing TVIEWs and returns the DDL.
- GUCs `pg_tviews.data_gin_index` (default `off`) and `pg_tviews.fillfactor`
  (default `85`), applied at TVIEW creation.

### Changed

- **Refreshes no longer rewrite unchanged rows** (#72). Every refresh path (bulk
  recompute, per-row upsert, smart patch, direct patch, DISTINCT ON, array ops)
  now skips the write when the recomputed row equals the stored one
  (`IS DISTINCT FROM` guard). An unchanged row gets no new tuple version, no index
  entries and no dead tuple. On the beta.17 baseline a no-op `UPDATE` over 10 000
  rows rewrote all 10 000 TVIEW rows (13 MB of WAL on a logged TVIEW).
- **Breaking: `updated_at` now means "last content change"**. It moves only when
  the row's content changes, no longer on every refresh that touched the key.
  Code that used `updated_at` as "last refreshed" must stop doing so; code using
  it for cache validation / ETags gets correct values now.
- **Default change: new TVIEWs get no GIN index on `data`, and fillfactor 85**
  (#70, #73). Nearly every refresh rewrites `data`, so the GIN made every refresh a
  non-HOT update (0 % HOT across the beta.17 physical baseline), and fillfactor 100
  left no room for the new row version on its page. New TVIEWs now measure 100 %
  HOT on single-row refreshes. Existing TVIEWs are untouched. Opt back in per
  TVIEW with `SET LOCAL pg_tviews.data_gin_index = on` /
  `SET LOCAL pg_tviews.fillfactor = 100`.

### Fixed

- **Quoted output columns and mixed-case entities work (#89).** `AS "order"` /
  `AS "Label"` was recorded with its quotes, so `pg_tviews_create` built a column
  literally named `"order"` and failed; unquoted aliases are now folded to lower case
  as PostgreSQL does. Every refresh and propagation statement now quotes the column,
  table and key names it interpolates, so a TVIEW like `tv_Mixed` refreshes instead
  of failing on every write to its base table.
- **Single-row refresh keeps every projected column in sync (#98).** For a TVIEW
  that joins a parent table, a change to one base row only rewrote `data`; other
  projected columns (`qty AS qty_alias`) stayed stale. The direct-patch fast path
  now also declines when a changed column feeds a projected column outside `data`.
- **Cascade propagation no longer scans the whole parent TVIEW** (#71). Propagation
  finds parent rows with `WHERE fk_<child> = ANY($1)`, but integer `fk_*` columns
  had no index. Every new TVIEW now gets a required `(fk_<x>, pk_<entity>)` btree
  per integer FK, so the lookup is index-only. On the beta.17 baseline a p99 user
  cascade read 8 651 / 25 805 buffers to find 83 / 216 rows.
- **`CREATE TABLE tv_* AS` is converted in a multi-statement batch, in `DO` blocks and in
  functions (#80), and only the statement's own SELECT is used (#95).** The hook skipped a
  whole batch whose text mentioned `create extension`, `DO` held the internal reentrancy guard
  for its nested statements, and the SELECT was cut from the raw batch text. It now decides
  extension statements by node type, releases the guard for `DO`/`CALL`, slices the statement
  by `stmt_location`/`stmt_len`, and resets the guard and pending state when a (sub)transaction
  aborts. A plain `CREATE TABLE tv_x (cols…)` is never converted.
- **A `CREATE TABLE tv_* AS` that pg_tviews did not intercept now fails instead of leaving a
  silent plain table (#80).** When the hook did not see the statement (pg_tviews not in
  `shared_preload_libraries`), the event trigger raises an error naming the table, the reason
  and the fix. `pg_tviews_convert_table(table_name, command_tag)` gained an optional
  `command_tag`; the new `pg_tviews.test_skip_ctas_intercept` GUC exists for tests only.
- **`CREATE TABLE IF NOT EXISTS tv_x AS …` no longer deletes an existing `tv_x` (#79).**
  PostgreSQL skipped the create but the fallback conversion still ran
  `DROP TABLE tv_x CASCADE`, leaving `pg_tview_meta` dangling. The hook now passes an
  `IF NOT EXISTS` over an existing relation through untouched, and the fallback only
  replaces a relation the statement itself created (never a pre-existing table or a
  registered TVIEW), including after a failed `CREATE TABLE tv_x AS` that left a pending entry.
- **`DROP TABLE [IF EXISTS] tv_x` on a `tv_*` table that isn't a registered TVIEW now
  behaves like PostgreSQL (#82).** It was claimed by the hook and silently kept. The hook now
  claims a name only if it resolves, schema-aware, to a relation registered in
  `pg_tview_meta`; plain tables, missing names and mixed lists go to the standard handler.

### Upgrade notes

- To move an existing TVIEW to the new defaults (see
  `docs/operations/hot-updates.md`):
  ```sql
  -- GIN indexes on data that no query uses
  SELECT indexrelid::regclass FROM pg_stat_user_indexes
   WHERE indexrelname LIKE 'idx_tv_%_data_gin' AND idx_scan = 0;
  DROP INDEX idx_tv_post_data_gin;             -- per unused index
  ALTER TABLE tv_post SET (fillfactor = 85);   -- newly written pages only
  ```

- TVIEWs created before this release lack the propagation indexes. After
  upgrading, run `SELECT * FROM pg_tviews_ensure_propagation_indexes();` (or run
  its `dry_run` output with `CREATE INDEX CONCURRENTLY` on large tables).

## [0.1.0-beta.17] - 2026-07-24

### Fixed

- **Release workflow: the signed source tarball now attaches to the GitHub Release.**
  The `Generate provenance` step (`actions/attest-build-provenance`) requires
  `attestations: write`, which `release.yml`'s permissions block did not grant, so it
  failed with "Resource not accessible by integration" before the `Create release` step
  could attach the tarball. No runtime or API changes — release-pipeline fix only.

## [0.1.0-beta.16] - 2026-07-24

### Changed

- **Multi-hop column-aware refresh**: flush-time entity propagation no longer
  recomputes a parent that embeds only a child's *own* scalar columns. Such an embed
  (e.g. a comment embedding a post's `{title}`) is already covered by the column-aware
  `tb_<child>` cascade path added in beta.15, so the extra propagation was redundant —
  and it fired even when the child was recomputed for an *unrelated* deeper embed (a
  post recomputed because its author's bio changed dragged every comment on that post
  through a data-preserving recompute). `EntityDepGraph` now drops such a `parents`
  edge only when a covering cascade path provably exists (non-empty, all-non-FK
  `source_columns` from `pg_depend`); `nested_object`/`array` embeds, scalar embeds
  that follow a child FK, and any ambiguous case keep the edge — the safe default.
  Measured on the lean benchmark schema: `updateUser(bio)` drops from 110 to ~11
  recomputes, `updateUser(username)` from 110 to ~61.

## [0.1.0-beta.15] - 2026-07-23

### Fixed

- **Stale scalar/base-table embeds when the embedded table is itself a TVIEW source
  (#63)**: a base table that backs its own TVIEW (`tb_user` → `tv_user`) and is *also*
  embedded into other TVIEWs via a direct JOIN to its columns (a `scalar`/base-table
  dependency, not the `v_<entity>.data` nested-object form) was left **silently stale**
  on `UPDATE`. The row trigger resolved the table to its own entity and returned before
  walking the base-table cascade paths, and commit-time entity propagation
  (`find_parents_batch`) only follows nested-object references — so a `tb_user` rename
  never reached `tv_post.author` / `tv_comment.author` (P1 silent data divergence). The
  row trigger now falls through to `enqueue_cascade_parents` for direct sources too;
  tables with no cascade paths hit the existing empty-paths early return, so the added
  cost is a single cache lookup.

### Added

- **Column-aware cascade refresh**: an `UPDATE` to a base-table column that a dependent
  TVIEW does not project can no longer trigger a wasted cascade recompute. `CascadePath`
  now records the source-table columns the target's backing view actually depends on —
  derived from PostgreSQL's column-level `pg_depend` records on `v_<entity>` (exact, and
  robust to expressions, `CASE`/`WHERE` qualifiers, `SELECT *`, and repeated/self joins,
  unlike parsing the SELECT text) — and a cascade path is skipped when the UPDATE's
  changed columns are disjoint from it. `INSERT`/`DELETE` (membership changes) and
  multi-hop / unknown paths always cascade — the safe default. On a lean read model this
  drops the write fan-out of a volatile-but-unembedded column (e.g. `tb_user.bio`) to
  zero, while embedded summary fields still fan out correctly.

## [0.1.0-beta.14] - 2026-07-23

### Added

- **Direct-patch fast path (#56)**: an eligible row-level `UPDATE` whose changed
  columns all map identity-style to top-level `data` keys now patches
  `tv_<entity>` (and nested-object parents) directly via `jsonb_smart_patch_*`,
  skipping the backing-view recompute entirely (zero view queries, counter-proven).
  Anything outside the eligibility boundary falls back to the recompute path, which
  remains the source of truth; the fast-path `data` output is byte-identical to a
  recompute. New GUC `pg_tviews.direct_patch_enabled` (bool, default `on`) is a
  kill-switch. New `pg_tviews_queue_stats()` counters: `direct_patch_captured`,
  `direct_patches_applied`, `direct_patch_fallbacks`, `view_recomputes`.
  `pg_tview_meta` gains `direct_map_columns` / `direct_map_keys`.
  **Upgrade note:** the column→key map is extracted at `pg_tviews_create` time, so
  tviews created before this version have an empty map and stay on the recompute
  path until re-created (`pg_tviews_drop` + `pg_tviews_create`).

## [0.1.0-beta.13] - 2026-07-22

### Fixed

- **Incremental refresh silently dropped INSERTs and failed DELETE (#48)**: After the
  first change statement per entity, subsequent INSERTs were silently lost and DELETEs
  left stale rows. Three write sites were UPDATE-only (a not-yet-materialized row matched
  nothing and was dropped), and a deleted base row raised a swallowed SPI error instead of
  removing the tview row. `apply_patch` and `refresh_bulk` now UPSERT
  (`INSERT … ON CONFLICT DO UPDATE`), and a missing backing-view row now deletes the tview
  row. Fixed in both jsonb_delta modes.
- **`jsonb_smart_patch_array` signature mismatch (#50, #24)**: pg_tviews emitted a 4-arg
  call that jsonb_delta 0.1.0 never exported (a test stub masked it), so every array
  dependency errored against the real extension. Array-dependency tviews are now recomputed
  via full replacement — correct for array element insert/update/delete and for the entity's
  own-column changes. This also resolves the array test failures reported as #24. The
  jsonb_delta availability latch is now invalidated on `CREATE/DROP EXTENSION jsonb_delta`.

### Added

- **Create-time refreshability validation (#49)**: `pg_tviews_create` now rejects a
  definition whose derived entity can never refresh — no `tb_<entity>` base table **and** no
  cascade path routing changes to it — instead of registering a permanently-stale tview that
  can silently shadow a correctly-named sibling on the same base table.
- **New tunable GUCs (#27)**: `pg_tviews.max_dependency_depth` (was a compile-time constant),
  `pg_tviews.batch_size` (bulk refresh is now chunked), and `pg_tviews.cache_size` (bounds
  the previously unbounded per-session metadata caches). Existing `pg_tviews.max_queue_size`
  already covered the issue's `queue_depth`.

### Changed

- **jsonb_delta 0.3.0 is the documented drop-in minimum**: CI and docs now target
  jsonb_delta **0.3.0**. The upgrade is zero-code for pg_tviews — the extension is
  detected by presence only (no version pin), and the SQL contract is byte-identical
  to 0.1.0/0.2.0. 0.3.0 installs cleanly over an existing extension via
  `ALTER EXTENSION jsonb_delta UPDATE TO '0.3.0'`. pg_tviews' only live runtime call
  into jsonb_delta is the stable `jsonb_smart_patch_scalar` entry point; benchmarking
  0.3.0 against the native fallback shows the two at parity for pg_tviews' refresh
  workload (see `docs/benchmarks/results.md`), so this is a maintenance/compatibility
  bump, not a performance change. Resolves jsonb_delta #12 on the consumer side
  (Option A: contract test only).

### Documentation

- **Benchmark docs rewritten to the real harness**: the entire `docs/benchmarks/`
  section now documents `test/sql/real_benchmark/` (the real `pg_tviews_create`
  API) instead of the removed `comprehensive_benchmarks/` harness. Every figure
  traces to a measured run in `docs/benchmarks/results.md`. Removed the dead
  Docker/podman benchmark tooling (`docker/dockerfile-benchmarks`, its helper
  scripts, and `scripts/{01..06,master}.sh`), which targeted the removed harness.

## [0.1.0-beta.12] - 2026-06-15

### Added

- **Pause/resume API for bulk INSERT operations (#44)**: Suspend row-level refresh
  triggers during bulk loads, then refresh affected TVIEWs in dependency order on resume.
  Adds `pg_tviews_suspend_triggers()`, `pg_tviews_resume_triggers()`, and
  `pg_tviews_refresh_all()`, the `pg_tviews.suspend_triggers` GUC, nested suspend/resume
  depth tracking, and auto-resume on COMMIT/ABORT to prevent orphaned state.
- **Automatic `tv_*` to TVIEW conversion**: SQL helpers (`pg_tviews_auto_convert()` and
  `pg_tviews_auto_convert_plan()` dry-run) plus a `convert_tviews.sh` CLI to detect `tv_*`
  tables created via bulk DDL and convert them to TVIEWs after schema creation.
- **`cascade` argument for `pg_tviews_drop()`**: Optional third argument to drop dependent
  objects along with the TVIEW.

### Fixed

- **`DROP TABLE tv_* CASCADE` panics in ProcessUtility hook (#47)**: The hook ignored
  `DropStmt.behavior`, so the internal drop was always RESTRICT — dropping a TVIEW with
  any dependent object raised "cannot drop ... because other objects depend on it"
  internally, which `catch_unwind` degraded to the opaque
  `PANIC in ProcessUtility hook: Any { .. }` message. The hook now honors CASCADE/RESTRICT
  and re-raises caught PostgreSQL errors faithfully (preserving SQLSTATE, detail, and hint)
  instead of mislabeling them as a pg_tviews bug.
- **Schema collision in `VIEW_COLUMNS_CACHE`**: The cache was keyed by view name only, so
  identically named backing views in different schemas (e.g. `public.v_machine` and
  `app.v_machine`) returned the wrong columns. The cache is now keyed by
  `{schema}.{view_name}`, fixing multi-schema deployments.
- **TVIEW conversion fallback when event triggers don't fire**: During bulk SQL, the
  `ddl_command_end` event trigger may not fire for every statement. The ProcessUtility hook
  now drains any pending unconverted `tv_*` entries after each statement so registration
  still succeeds.
- **Schema inference prioritizes the `pk` column over `id`**: A `SELECT` exposing both `pk`
  (BIGINT) and `id` (UUID) incorrectly chose `id` as the primary key, causing type-mismatch
  errors when populating the materialized table. PK detection now prefers an explicit `pk`
  column, then integer/serial types, and falls back to `id` last.
- **PL/pgSQL syntax in the auto-convert functions**: Loop variables in
  `pg_tviews_auto_convert()` / `pg_tviews_auto_convert_plan()` are now declared as record
  types so both functions parse correctly.

### Documentation

- **`INTEGRATION_GUIDE` for automatic TVIEW conversion**: End-to-end guide for wiring
  `tv_*` detection and conversion into schema build workflows, including shell-script usage,
  custom backing views, multi-database builds, and troubleshooting.

## [0.1.0-beta.11] - 2026-04-19

### Fixed

- **`spi_batch_lookup` OID cast fails in FROM clause (#010)**: Cast expressions like
  `(52276294::regclass)` are not valid PostgreSQL FROM-clause table references. The
  function now resolves OIDs to schema-qualified names via `pg_class + pg_namespace`
  (`quote_ident(nspname) || '.' || quote_ident(relname)`) before building the query,
  making cascade traversal work correctly for tables in any schema.
- **`cascade_paths` column serialization**: Changed from `JSONB[]` to `TEXT[]` throughout
  the runtime schema to fix pgrx deserialization failures; added `pg_array_elem` for
  proper `TEXT[]` serialization in `CREATE TVIEW`.

### Added

- **Multi-hop cascade integration tests**: SQL test suite covering transitive FK cascade
  paths across three or more hops (e.g. `tb_currency` → `tv_contract` → `tv_invoice`).

## [0.1.0-beta.10] - 2026-04-01

### Added

- **Multi-hop cascade path support**: Full end-to-end cascade path computation,
  storage, and traversal across arbitrary FK chains. TVIEWs now automatically
  propagate refreshes through transitive dependencies.
- **SQL JOIN parser**: Extracts FK relationships from TVIEW `SELECT` definitions
  to build the cascade path graph at registration time.
- **`cascade_paths` catalog column**: Stores serialized hop sequences per TVIEW
  for O(1) lookup during trigger processing.
- **Error message improvements**: Enhanced error messages for missing rows during refresh with:
  - Entity name and view name context
  - Actual SQL query being executed
  - Possible causes guidance (cascading delete, UNION ALL conditions, view filters)
- **Audit logging integration**: `log_refresh()` now called after successful refresh operations
- **GUC parameter**: `pg_tviews.max_queue_size` for queue backpressure enforcement
- **Regex caching**: LazyLock static patterns for parser and analyzer regexes
- `InvalidInput` error variant (SQLSTATE `22023`) for input validation errors

### Removed

- **Prepared-transaction (2PC) infrastructure**: Removed unimplemented 2PC support
  (`pg_tviews_commit_prepared`, `pg_tviews_rollback_prepared`, `src/twophase.rs`,
  `src/queue/persistence.rs`, `src/refresh/cache.rs`). Implicit transaction commit
  via statement-level trigger flushing supersedes the 2PC design.

### Security

- **SQL injection prevention**: Parameterized all user-controlled string inputs
  previously embedded via `format!()` in `pg_tviews_show_cascade_path()`,
  `entity_for_table_uncached()`, and all three audit log functions.
- **Privilege escalation**: Removed unnecessary `SECURITY DEFINER` from
  `pg_tviews_debug_queue()` in `pg_tviews_monitoring.sql`.

## [0.1.0-beta.9] - 2026-03-01

### Fixed

- **SIGABRT on UPDATE of tview-tracked base table (#31)**: Any `UPDATE` on a
  base table tracked by a TVIEW caused a PostgreSQL backend crash (SIGABRT)
  due to recursive SPI connections inside the trigger context.
  `pg_tviews_cascade()` now enqueues `(entity, pk)` pairs into the
  transaction-level refresh queue and lets the existing PRE_COMMIT handler
  process them iteratively in a clean SPI context. `refresh_pk()` no longer
  calls `propagate_from_row()` — parent discovery is handled exclusively by
  `find_parents_for()` in the queue's commit callback.
- Tests: all `refresh_pk()` tests now use correct TVIEW OIDs (`tv_user` instead of
  `tb_user`) and create dependency TVIEWs before parent TVIEWs; the metadata query
  column name in test assertions is corrected (`entity_name` → `entity`).

### Removed

- **`propagate_from_row()`**: Recursive propagation function that caused the
  nested SPI crash. Replaced by iterative queue processing.
- **`src/refresh/batch.rs`**: Batch refresh module (only consumer was the
  removed `propagate_from_row`; `src/refresh/bulk.rs` covers batch needs).
- **`clear_queue_and_reset()`**: Unused queue helper.
- **`extract_pk()` dead-code annotation**: Function is actively used by
  `trigger.rs`; stale `#[allow(dead_code)]` removed.
- **`get_relkind()`**: Unused dependency graph helper.
- Stale `#[allow(dead_code)]` annotations on five queue functions now
  actively called from the trigger path.

## [0.1.0-beta.8] - 2026-02-24

### Fixed

- **`dependency_paths` column type `TEXT[][]` → `TEXT[]` (#24)**: The column
  declaration was aspirational `TEXT[][]` but the write path already stored
  dot-separated strings in a flat `TEXT[]` (e.g. `{author}`,
  `{book.author}`). pgrx 0.16.1 cannot extract multidimensional arrays from
  SPI results, so all three SPI read paths returned empty paths, breaking
  smart JSONB patching for `NestedObject` and `Array` dependency types. The
  column type is now `TEXT[]` in both `pg_tview_meta` DDL and the test schema;
  a new `parse_dep_paths()` helper splits each element on `'.'` to reconstruct
  the key sequence, and all four read sites (`load_for_source`,
  `load_by_entity`, `from_spi_row`, `find_dependent_tviews`) now call it.

## [0.1.0-beta.7] - 2026-02-24

### Fixed

- **Cascade refresh for array aggregation TVIEWs**: Rewrote
  `find_affected_tview_rows` in `src/lib.rs` to handle three cases: direct
  column match, scalar FK column, and array aggregation (GROUP BY TVIEWs
  where the child table's FK is not an output column of the backing view)
- **UPSERT for new rows**: `apply_full_replacement` now uses
  `INSERT ... ON CONFLICT DO UPDATE` so rows inserted after TVIEW creation
  are handled correctly instead of being silently skipped
- **Test SQL fixes**: Added `FILTER (WHERE ... IS NOT NULL)` to `jsonb_agg`
  calls in tests 52 and 53 to prevent null-object elements from LEFT JOINs;
  added missing extension loading to test 50

### Changed

- **Removed 118 diagnostic `info!()` calls** across 11 source files
- **Removed dead code**: `pg_tviews_debug_ddl`, `pg_tviews_debug_sequence`,
  `with_hook_bypassed`, `peek_pending_tview_select`
- Removed empty `if let` blocks left over from logging removal
- Removed development markers (TODO / FIXME) from source and test files
- Zero compiler warnings

## [0.1.0-beta.6] - 2026-02-24

### Fixed

- **`cargo pgrx test` missing `crate::pg_test` module (#30)**: Added the
  required `pub mod pg_test` boilerplate to `src/lib.rs`. The `#[pg_test]`
  proc macro expands to calls to `crate::pg_test::setup()` and
  `crate::pg_test::postgresql_conf_options()`, which must exist at the crate
  root. This module is normally generated by `cargo pgrx new` and was absent.

## [0.1.0-beta.5] - 2026-02-24

### Fixed

- **`cargo pgrx test` compilation (#28, #29)**: Removed spurious
  `use pgrx_tests::pg_test` import (gated behind `cfg(test)` in pgrx-tests
  0.16.1, unavailable during cdylib builds). Applied the standard pgrx test
  module pattern across all 9 source files: module gate changed to
  `#[cfg(any(test, feature = "pg_test"))]`, `use pgrx::prelude::*` made
  unconditional inside test modules so the `#[pg_test]` proc macro attribute
  is always in scope, and redundant `#[cfg(feature = "pg_test")]` guards
  removed from individual test functions. Contributors can now run
  `cargo pgrx test` locally without E0432/E0433 errors.

## [0.1.0-beta.4] - 2026-02-23

This release also contains the changes prepared as 0.1.0-beta.3, which was never tagged.

### Changed

- **Actionable `pk_<entity>` error**: `CREATE TVIEW` on a view without the expected
  `pk_<entity>` column now explains the naming convention instead of failing tersely
  (#26).
- **`jsonb_delta` naming**: The companion JSONB patching extension is referred to as
  `jsonb_delta` (formerly `jsonb_ivm`) in all documentation and install instructions.
  Its API is unchanged.
- Package metadata (repository, homepage, documentation URLs) points to
  `github.com/fraiseql/pg_tviews`.

### Fixed

- **Schema-aware DDL**: `CREATE TVIEW` resolves the target schema from `search_path`
  instead of assuming `public`; `DROP TVIEW` resolves qualified names by OID; the
  extension's catalog objects are created in the extension schema rather than `public`.
- **`pg_tviews_refresh()`** uses an explicit column list, avoiding column-order
  mismatches.
- **`pg_tviews_version()`** returns the packaged version.
- **Build failure from a duplicate `pg_tviews_refresh` definition** (#21).
- **Backend crash (SIGABRT) on string SPI lookups** (#22): replaced with a safe wrapper.
- **Invalid array literals** such as `'{,}'` when storing empty dependency paths (#23).
- **Backend crash on a poisoned internal lock** (#25): caches recover instead of
  aborting. Missing `id`, `data` or `pk_*` columns now return an error instead of
  panicking (#26).

## [0.1.0-beta.2] - 2025-12-16

### Changed

- Internal refactoring for strict Clippy compliance: dependency-graph helpers
  extracted, consistent error-module patterns, `#[must_use]` on value-returning
  functions, completed error documentation.
- Replaced the deprecated `once_cell::Lazy` with `std::sync::LazyLock`.
- Pre-commit hooks moved from bash scripts to prek; CI fixes for PostgreSQL version
  handling, feature flags, security audit and coverage reporting.
- CI verifies the extension with `cargo build`, since `#[pg_test]` unit tests need the
  pgrx test framework; the SQL suites under `test/sql/` provide behavioural coverage.

## [0.1.0-beta.1] - 2025-12-10

First beta: a feature-complete transactional materialized JSONB view system.

### Added

- **Queue-based refresh**: changes enqueue `(entity, pk)` pairs in a per-transaction,
  deduplicated queue that is flushed before commit in dependency (topological) order.
  `ROLLBACK TO SAVEPOINT` is honoured, a refresh failure aborts the transaction,
  circular dependencies are detected, and parent entities are discovered for cascades.
- **Statement-level triggers** reading transition tables, with a bulk enqueue API
  (`enqueue_refresh_bulk()`), cutting trigger overhead 100-500x for bulk writes.
- **Bulk refresh**: N rows refreshed with 2 queries (`ANY($1)`,
  `UPDATE ... FROM unnest()`), grouped by entity.
- **Query plan caching** with prepared statements, invalidated on schema changes and
  on `DISCARD ALL`.
- **Connection pooling safety**: session state is cleared on `DISCARD ALL` and at
  transaction start, so queues never leak between transactions.
- **Prepared transactions (2PC)**: the queue is persisted on `PREPARE TRANSACTION`
  (`pg_tview_pending_refreshes`, compressed binary encoding), replayed on
  `COMMIT PREPARED` and discarded on `ROLLBACK PREPARED`;
  `pg_tviews_recover_prepared_transactions()` recovers pending entries.
- **Caching** of the entity dependency graph and table OIDs; an iteration limit
  prevents runaway propagation.
- **Monitoring**: views `pg_tviews_queue_realtime`, `cache_stats` and
  `performance_summary`, a metrics history table, `pg_tviews_health_check()`,
  `pg_tviews_debug_stats()`, `pg_tviews_debug_queue()`, and `pg_stat_statements`
  integration.

### Changed

- **Error handling**: no `unwrap()` left in the extension; every SPI result is
  NULL-checked; new error variants (`ConfigError`, `CacheError`, `CallbackError`,
  `MetricsError`) and conversions from serde_json, bincode, regex and I/O errors;
  error messages carry context.
- **Panic safety**: transaction, transaction-start and subtransaction callbacks are
  guarded against panics and log them instead of crashing the backend.
- Module and architecture documentation; `cargo clippy -- -D warnings` enforced in CI.

## 0.1.0-alpha - 2025-12-09

### Added

- **`CREATE TVIEW` / `DROP TVIEW`** through DDL hooks, with management of the
  materialized backing table and parsing of the view definition.
- **Schema inference**: column types (including `UUID[]`, `TEXT[]`, `INTEGER[]`
  arrays), `ARRAY(...)` and `jsonb_agg()` patterns, and relationships between tables.
- **Dependency tracking**: automatic dependency graph, trigger installation,
  circular-dependency detection and metadata tables, including array-aggregation
  dependencies (`jsonb_agg(v_table.data)`).
- **Cascade propagation** of refreshes, transaction-isolated and safe under
  concurrency.
- **Smart JSONB patching** with `jsonb_delta`: surgical updates instead of full
  document replacement, about 2x faster on the medium-cascade benchmark
  (7.55 ms to 3.72 ms).
- **Array element INSERT/DELETE** (`insert_array_element()`,
  `delete_array_element()`) with automatic type inference.
- **Batch refresh** for cascades of 10 rows or more (3-5x faster), switching
  automatically between per-row and batch updates.
- Array handling guide in `docs/arrays.md`.

## 0.0.1-alpha - 2025-11-01

### Added

- Initial project structure on pgrx.

[Unreleased]: https://github.com/fraiseql/pg_tviews/compare/v0.1.0-beta.25...HEAD
[0.1.0-beta.25]: https://github.com/fraiseql/pg_tviews/compare/v0.1.0-beta.24...v0.1.0-beta.25
[0.1.0-beta.24]: https://github.com/fraiseql/pg_tviews/compare/v0.1.0-beta.23...v0.1.0-beta.24
[0.1.0-beta.23]: https://github.com/fraiseql/pg_tviews/compare/v0.1.0-beta.22...v0.1.0-beta.23
[0.1.0-beta.22]: https://github.com/fraiseql/pg_tviews/compare/v0.1.0-beta.21...v0.1.0-beta.22
[0.1.0-beta.21]: https://github.com/fraiseql/pg_tviews/compare/v0.1.0-beta.20...v0.1.0-beta.21
[0.1.0-beta.20]: https://github.com/fraiseql/pg_tviews/compare/v0.1.0-beta.19...v0.1.0-beta.20
[0.1.0-beta.19]: https://github.com/fraiseql/pg_tviews/compare/v0.1.0-beta.18...v0.1.0-beta.19
[0.1.0-beta.18]: https://github.com/fraiseql/pg_tviews/compare/v0.1.0-beta.17...v0.1.0-beta.18
[0.1.0-beta.17]: https://github.com/fraiseql/pg_tviews/compare/v0.1.0-beta.16...v0.1.0-beta.17
[0.1.0-beta.16]: https://github.com/fraiseql/pg_tviews/compare/v0.1.0-beta.15...v0.1.0-beta.16
[0.1.0-beta.15]: https://github.com/fraiseql/pg_tviews/compare/v0.1.0-beta.14...v0.1.0-beta.15
[0.1.0-beta.14]: https://github.com/fraiseql/pg_tviews/compare/v0.1.0-beta.13...v0.1.0-beta.14
[0.1.0-beta.13]: https://github.com/fraiseql/pg_tviews/compare/v0.1.0-beta.12...v0.1.0-beta.13
[0.1.0-beta.12]: https://github.com/fraiseql/pg_tviews/compare/v0.1.0-beta.11...v0.1.0-beta.12
[0.1.0-beta.11]: https://github.com/fraiseql/pg_tviews/compare/b3748ab5...v0.1.0-beta.11
[0.1.0-beta.10]: https://github.com/fraiseql/pg_tviews/compare/v0.1.0-beta.9...b3748ab5
[0.1.0-beta.9]: https://github.com/fraiseql/pg_tviews/compare/462a2b18...v0.1.0-beta.9
[0.1.0-beta.8]: https://github.com/fraiseql/pg_tviews/compare/v0.1.0-beta.7...462a2b18
[0.1.0-beta.7]: https://github.com/fraiseql/pg_tviews/compare/64886669...v0.1.0-beta.7
[0.1.0-beta.6]: https://github.com/fraiseql/pg_tviews/compare/af562d01...64886669
[0.1.0-beta.5]: https://github.com/fraiseql/pg_tviews/compare/v0.1.0-beta.4...af562d01
[0.1.0-beta.4]: https://github.com/fraiseql/pg_tviews/compare/v0.1.0-beta.2...v0.1.0-beta.4
[0.1.0-beta.2]: https://github.com/fraiseql/pg_tviews/compare/v0.1.0-beta.1...v0.1.0-beta.2
[0.1.0-beta.1]: https://github.com/fraiseql/pg_tviews/releases/tag/v0.1.0-beta.1
