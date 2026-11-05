# Upgrading pg_tviews

A release updates from the release before it, with one `ALTER EXTENSION pg_tviews
UPDATE`. Between installing the new package and running that update, pg_tviews refuses
every write to the TVIEWs' base tables: plan a short write outage per database.

## Why the window refuses writes

The library and each database's catalog carry a catalog revision. A library swapped in
before `ALTER EXTENSION pg_tviews UPDATE` finds a catalog of another revision, whose
plans, functions and triggers it does not read, and refuses to work with it rather than
refresh TVIEWs wrongly. Every write to a base table, every pg_tviews function and every
`CREATE TABLE tv_* AS` then fails with SQLSTATE `55000`:

```
ERROR:  pg_tviews library catalog revision 11 does not match the installed extension (10)
HINT:  run ALTER EXTENSION pg_tviews UPDATE
```

Reads of the TVIEWs keep working: their rows are those of the last committed write.
A function the new release removed still exists in the old catalog; calling it raises
the same error, never PostgreSQL's `could not find function … in file`.

## From the previous release

Do it in each database that has the extension.

1. **Check before.** jsonb_delta is installed (`requires = 'jsonb_delta'`: the update
   refuses without it, `required extension "jsonb_delta" is not installed`); writers run
   at READ COMMITTED; the operator role's grants are noted (step 5). A dump of each
   database is the way back.
2. **Pause the writers** (the application, workers, scheduled jobs). Readers may stay.
3. **Install the package and restart PostgreSQL.** The library is preloaded
   (`shared_preload_libraries`): a running server keeps the old one until it restarts.
   Overwriting a loaded library crashes its backends; stop first.
4. **Update every database:**
   ```sql
   ALTER EXTENSION pg_tviews UPDATE;
   ```
   It re-derives every TVIEW's plan and triggers from its definition inside the update,
   each in its owner's frame ([owner frame](../security/architecture.md#owner-frame)):
   the role running the update runs none of a TVIEW's code. The rows are not touched. A TVIEW that no longer analyses
   fails the whole update, named with its error, and nothing is changed: fix or drop it
   and update again. `tviews.registry.needs_reregister` is false for every TVIEW after it.
   Before it re-derives anything, the update refuses, naming every case in one error:
   GraphQL type names shared by several TVIEWs (`42710`: give all but one a `typename`
   option), and TVIEW or view owners that may no longer read a relation their view reads
   (`42501`, naming the TVIEW, the owner and the privilege: grant it or drop the TVIEW).
   Re-deriving also takes every privilege on a backing view (its columns included) from
   the roles other than its owner, and warns about each function a refresh runs whose
   `search_path` does not end in `pg_temp` (the health check's `function_paths`).
   0.1.0-beta.35 also replaces every per-TVIEW trigger on the base tables by one set per
   table (`trg_tview_<role>_on_<schema>_<table>`); a per-TVIEW trigger restored later from
   an older dump fails the writes to its table until `pg_tviews_reregister_all()`.
5. **Restore grants** that name a function the release replaced (the release notes list
   them; 0.1.0-beta.35: `pg_tviews_ensure_propagation_indexes_all(boolean)` and
   `pg_tviews_reset_stats_all()` for the operator role).
6. **Check, then resume the writers:**
   ```sql
   SELECT * FROM tviews.pg_tviews_check_health() WHERE status <> 'OK';
   ```

How long step 4 takes, on 0.1.0-beta.34 → 0.1.0-beta.35 (stock PostgreSQL 18, a chain
of TVIEWs each embedding the previous one, 1,000 rows each): 1 TVIEW 50 ms, 50 TVIEWs
1.2 s, 200 TVIEWs 2.2 s. The time follows the number of TVIEWs, not their rows.

## From an older release

There is no update from a release older than the previous one: the window's error says
`this release updates only from <previous>`. Drop the extension and create the TVIEWs
again from their definitions (the schema files a tool such as confiture applies). The
base tables keep their rows; the TVIEWs are refilled from them.

```sql
DROP EXTENSION pg_tviews CASCADE;  -- the triggers, the catalog, the backing views
DROP SCHEMA IF EXISTS tviews CASCADE;
-- each former TVIEW's table (tv_<entity>), and its v_<entity> backing view from
-- releases that kept it beside the table, is now an ordinary relation: drop it
DROP TABLE public.tv_post, public.tv_user;
CREATE EXTENSION pg_tviews;
-- then create each TVIEW again (CREATE TABLE tv_<entity> AS …, or
-- tviews.pg_tviews_create_or_replace(…)), in dependency order
```

Writes to the base tables are refused (the window's error) from the restart until
`DROP EXTENSION`, and are not followed between `DROP EXTENSION` and the TVIEWs'
creation: keep the writers paused until the last TVIEW exists.

## Rehearse

`test/upgrade/upgrade_check.sh` does both procedures on fixture TVIEWs (CI runs them
from the previous release on PostgreSQL 16 and 18, and from 0.1.0-beta.25): the window
refuses row, statement, partition and `TRUNCATE` writes, every old function reaches
pg_tviews' code, and the updated catalog equals a fresh install's.
