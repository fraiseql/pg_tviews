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
   database is the way back. Learn which TVIEWs the update will leave to re-register
   ([below](#what-the-update-leaves-behind)): run the [pre-check](#pre-check) with the
   release still installed.
2. **Pause the writers** (the application, workers, scheduled jobs). Readers may stay.
3. **Install the package and restart PostgreSQL.** The library is preloaded
   (`shared_preload_libraries`): a running server keeps the old one until it restarts.
   Overwriting a loaded library crashes its backends; stop first.
4. **Update every database:**
   ```sql
   ALTER EXTENSION pg_tviews UPDATE;
   ```
   Re-deriving a TVIEW locks its backing view. The update waits for that lock at most
   the session's `lock_timeout`, 10 s when the session sets none: a TVIEW whose lock is
   not granted in time is left to re-register and named like any other. A
   `statement_timeout` that runs out, or a cancel, aborts the whole update instead.
   It re-derives every TVIEW's plan and triggers from its definition inside the update,
   each in its owner's frame ([owner frame](../security/architecture.md#owner-frame)):
   the role running the update runs none of a TVIEW's code. The rows are not touched.
   The update does not depend on any TVIEW: one that cannot be re-derived is left to
   re-register, and the update completes and names it in a WARNING
   ([below](#what-the-update-leaves-behind)).
   GraphQL type names are unique in the catalog, so no two TVIEWs share one.
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
   The `reregister` component names every TVIEW the update left to re-register.

### What the update leaves behind

A TVIEW the update cannot re-derive keeps `tviews.registry.needs_reregister`, and the
update's WARNING names it with the first line of its error:

```
WARNING:  pg_tviews: 2 TVIEW(s) could not be re-derived and need re-registration: tv_lost: Failed to re-register TVIEW lost: the roles reading for it may no longer read what they read: tv_lost (app_owner lacks SELECT on public.tb_lost); …; tv_rls: Failed to re-register TVIEW rls: row security applies to role app_owner on table public.tb_rls, which tv_rls reads
```

Until it is fixed and re-registered:

- its plan is the earlier release's, and nothing it says is trusted: every write to a
  table it reads refreshes its rows from its backing view, with no column skipped and
  nothing patched;
- a refresh still fails where its definition no longer holds: `42501` when a role
  reading for it may no longer read a relation, `42P16` when row security applies to
  such a role on a table it reads ([security](security.md#row-security-on-the-tables-a-tview-reads)).
  The writing statement fails; the TVIEW's rows stay those of the last refresh;
- `pg_tviews_check_health()` names it (component `reregister`), with why when this
  release's checks know.

The usual causes, and the fix: a role reading for the TVIEW lost a privilege (grant it
again; the health check's `owner_reads` names it); row security applies on a table it
reads to a role it reads that table as (exempt the role, or disable row security on the
table); a view or function its definition reads changed into something the analysis
refuses (restore it, or replace the TVIEW). Then:

```sql
SELECT tviews.pg_tviews_reregister('tv_lost');   -- or pg_tviews_reregister_all()
```

Or drop the TVIEW. A TVIEW left behind blocks nothing else: the other TVIEWs, and
writes to tables it does not read, work as usual.

### Pre-check

With the release you update **from** still installed, and in each database:

<!-- upgrade-precheck:begin -->
```sql
-- What the installed release itself cannot re-derive now (a privilege lost, a
-- view or function changed): its own re-derivation, rolled back. It runs each
-- TVIEW's analysis as its owner and takes its locks for the transaction.
BEGIN;
SELECT entity, status FROM tviews.pg_tviews_reregister_all() WHERE status <> 'reregistered';
ROLLBACK;

-- What 0.1.0-beta.40 also refuses: row security that applies, on a table a TVIEW
-- reads, to a role it reads that table as (its owner, the owner of each view it
-- reads through, the owner of each declared SECURITY DEFINER function).
WITH reading(entity, role) AS (
    SELECT m.entity, t.relowner
    FROM tviews.pg_tview_meta m JOIN pg_catalog.pg_class t ON t.oid = m.table_oid
  UNION
    SELECT r.entity, v.relowner
    FROM tviews.reads r JOIN pg_catalog.pg_class v ON v.oid = r.relid AND v.relkind = 'v'
  UNION
    SELECT m.entity, p.proowner
    FROM tviews.pg_tview_meta m,
         pg_catalog.unnest(m.function_read_functions) AS f(signature),
         pg_catalog.pg_proc p JOIN pg_catalog.pg_namespace n ON n.oid = p.pronamespace
    WHERE p.prosecdef
      AND pg_catalog.format('%s.%s(%s)', pg_catalog.quote_ident(n.nspname),
                            pg_catalog.quote_ident(p.proname),
                            pg_catalog.oidvectortypes(p.proargtypes)) = f.signature
)
SELECT DISTINCT m.table_oid::pg_catalog.regclass AS tview,
       pg_catalog.pg_get_userbyid(g.role) AS role, t.oid::pg_catalog.regclass AS "table"
FROM tviews.reads r
JOIN tviews.pg_tview_meta m ON m.entity = r.entity
JOIN pg_catalog.pg_class t ON t.oid = r.relid AND t.relrowsecurity
JOIN reading g ON g.entity = r.entity
JOIN pg_catalog.pg_roles a ON a.oid = g.role
WHERE NOT a.rolsuper AND NOT a.rolbypassrls
  AND (t.relforcerowsecurity OR NOT pg_catalog.pg_has_role(g.role, t.relowner, 'USAGE'))
ORDER BY 1, 2, 3;

-- What 0.1.0-beta.40 also checks column by column: each view a TVIEW reads through
-- (its backing view among them) reads, as its owner, every column it names.
WITH viewing(entity, view) AS (
    SELECT m.entity, m.view_oid::pg_catalog.oid FROM tviews.pg_tview_meta m
  UNION
    SELECT r.entity, r.relid
    FROM tviews.reads r JOIN pg_catalog.pg_class v ON v.oid = r.relid AND v.relkind = 'v'
)
SELECT DISTINCT m.table_oid::pg_catalog.regclass AS tview,
       pg_catalog.pg_get_userbyid(v.relowner) AS role,
       d.refobjid::pg_catalog.regclass AS "table", a.attname AS "column"
FROM viewing w
JOIN tviews.pg_tview_meta m ON m.entity = w.entity
JOIN pg_catalog.pg_class v ON v.oid = w.view
JOIN pg_catalog.pg_rewrite rw ON rw.ev_class = w.view
JOIN pg_catalog.pg_depend d
  ON d.classid = 'pg_catalog.pg_rewrite'::pg_catalog.regclass AND d.objid = rw.oid
 AND d.refclassid = 'pg_catalog.pg_class'::pg_catalog.regclass
 AND d.refobjsubid > 0 AND d.refobjid <> w.view
JOIN pg_catalog.pg_attribute a ON a.attrelid = d.refobjid AND a.attnum = d.refobjsubid
WHERE NOT pg_catalog.has_column_privilege(v.relowner, d.refobjid, d.refobjsubid::pg_catalog.int2, 'SELECT')
ORDER BY 1, 2, 3, 4;
```
<!-- upgrade-precheck:end -->

Every row of these queries is a TVIEW the update will leave to re-register; fix it
before the update, or after it as above. With the new package installed (writers
paused), the update itself is the exact check: `BEGIN; ALTER EXTENSION pg_tviews
UPDATE; ROLLBACK;` prints its WARNING and changes nothing. Run it in a session of its
own.

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

`test/upgrade/upgrade_check.sh` does both procedures on fixture TVIEWs, runs the
pre-check, and updates a catalog with TVIEWs the update cannot re-derive (CI runs them
from the previous release and from 0.1.0-beta.25, on PostgreSQL 18): the window
refuses row, statement, partition and `TRUNCATE` writes, every old function reaches
pg_tviews' code, and the updated catalog equals a fresh install's.
