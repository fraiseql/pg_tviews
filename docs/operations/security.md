# Security

Who may do what with pg_tviews, how refreshes run, and what an operator should
check. pg_tviews follows the rules of `ALTER TABLE` and `REFRESH MATERIALIZED VIEW`:
a TVIEW belongs to the role that owns its `tv_*` table, and every refresh runs as
that role.

The SQL blocks on this page run in order in a fresh database, as a superuser, on a
server that preloads pg_tviews. They create an `app` schema and four roles. Roles
are cluster-wide: the blocks skip the ones that already exist.

```sql
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;   -- superuser only: the extension is marked superuser = true

DO $$
DECLARE r text;
BEGIN
    FOREACH r IN ARRAY ARRAY['docs_sec_owner', 'docs_sec_writer',
                             'docs_sec_reader', 'docs_sec_operator'] LOOP
        IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = r) THEN
            EXECUTE format('CREATE ROLE %I', r);
        END IF;
    END LOOP;
END $$;

CREATE SCHEMA app;
GRANT USAGE ON SCHEMA app TO docs_sec_writer, docs_sec_reader;
GRANT USAGE, CREATE ON SCHEMA app TO docs_sec_owner;

CREATE TABLE app.tb_user (
    pk_user bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id      uuid NOT NULL DEFAULT gen_random_uuid(),
    name    text NOT NULL,
    email   text NOT NULL
);
ALTER TABLE app.tb_user OWNER TO docs_sec_owner;
```

## Who may do what

| Role | May |
|---|---|
| Superuser | `CREATE EXTENSION pg_tviews` (its script creates event triggers and C functions); set the `pg_tviews.*` settings |
| Extension owner | everything a TVIEW owner may, on every TVIEW; run the `_all` functions; read and prune the audit log |
| A TVIEW's owner (or a member of the owning role) | `pg_tviews_create_or_replace`, `pg_tviews_drop`, `pg_tviews_refresh`, `pg_tviews_refresh_time_dependent`, `pg_tviews_reregister`, `pg_tviews_ensure_propagation_indexes`, `pg_tviews_reset_stats`, `pg_tviews_show_cascade_path`, `pg_tviews_show_mapping_query`, `pg_tviews_show_read_sets` on that TVIEW |
| A role with `CREATE` on a schema | create a TVIEW there (`pg_tviews_create`, `CREATE TABLE tv_x AS …`); it becomes the owner |
| An operator role | the `_all` functions it was granted |
| Everyone | the read-only functions (`pg_tviews_check_health`, `pg_tviews_profile`, `pg_tviews_resolve`, `pg_tviews_show_replication`, …), its own session's functions (`pg_tviews_suspend_triggers`, `pg_tviews_resume_triggers`, `pg_tviews_show_suspended`, `pg_tviews_show_queue`, `pg_tviews_flush_and_report`), and the views `tviews.registry` and `tviews.stats`; the read-only functions show row data (`is_empty`, queued keys, reported rows) only from the `tv_*` tables the caller may read |

Every function acting on or showing one TVIEW checks ownership first, before taking
any lock: the caller must have the privileges of the `tv_*` table's owner or of the
extension's owner (superusers pass). Anyone else gets `42501`
(`must be owner of TVIEW app.tv_user`).

The functions acting on every TVIEW end in `_all`, and `EXECUTE` on them is revoked
from `PUBLIC`: `pg_tviews_refresh_all()`, `pg_tviews_rebuild_all(boolean)`,
`pg_tviews_reregister_all(boolean)`,
`pg_tviews_ensure_propagation_indexes_all(boolean)` and `pg_tviews_reset_stats_all()`
(and the internal `pg_tviews_invalidate_caches` and `pg_tviews_audit_write`). A
non-superuser deploy or restore role gets them with `GRANT EXECUTE`; see
[Operator role](../user-guides/operators.md#operator-role).
`pg_tviews_reregister_all()` still checks ownership of each TVIEW it touches.

The TVIEW's owner creates it and decides who reads it, with an ordinary `GRANT`:

```sql
SET ROLE docs_sec_owner;
SELECT tviews.pg_tviews_create('app.tv_user', $q$
    SELECT pk_user, id, jsonb_build_object('id', id, 'name', name) AS data
    FROM app.tb_user
$q$);
GRANT SELECT ON app.tv_user TO docs_sec_reader;
GRANT INSERT, UPDATE, DELETE ON app.tb_user TO docs_sec_writer;
RESET ROLE;
```

A role that does not own the TVIEW is refused:

```sql
SET ROLE docs_sec_writer;
DO $$
BEGIN
    PERFORM tviews.pg_tviews_refresh('app.tv_user');
EXCEPTION WHEN insufficient_privilege THEN
    RAISE NOTICE '%', SQLERRM;   -- must be owner of TVIEW app.tv_user
END $$;
RESET ROLE;
```

An operator granted `pg_tviews_refresh_all()` refreshes every TVIEW, each as its
owner, without any privilege on the tables:

```sql
GRANT EXECUTE ON FUNCTION tviews.pg_tviews_refresh_all() TO docs_sec_operator;
SET ROLE docs_sec_operator;
SELECT tviews.pg_tviews_refresh_all() ->> 'refreshed_count' AS refreshed;
RESET ROLE;
```

### Readers

Reading a TVIEW needs `SELECT` on its `tv_*` table and `USAGE` on its schema, as for
any table.

The definition is stored as a backing view in pg_tviews' schema,
`tviews.<schema>__tv_<entity>` (`tviews.registry.view`). It is the TVIEW owner's
alone: it reads the base tables as the owner and recomputes every row, so no grant on
the table is copied to it, and a privilege granted on it directly is revoked by the
next `GRANT`, `REVOKE`, `ALTER TABLE … OWNER TO`, `REASSIGN OWNED`, create or rebuild.
`ALTER TABLE tv_* OWNER TO` gives the view the new owner. Readers read `tv_*`, where
row-level security applies as on any table: a policy on `tv_user` hides the same rows
from every reader, since no other relation hands them back.

```sql
SELECT r.view,
       has_table_privilege('docs_sec_reader', r.view, 'SELECT') AS reader_sees_view,
       has_table_privilege('docs_sec_writer', r.view, 'SELECT') AS writer_sees_view
FROM tviews.registry r WHERE r.entity = 'user';   -- false, false
```

A TVIEW holds whatever its definition selects: leave secrets (`email` above,
password hashes, tokens) out of `data`, since anyone granted the TVIEW reads all of it.

## How a refresh runs

A write to a base table refreshes the TVIEWs that read it, at the end of the
statement or at `COMMIT`. Each refresh, single or bulk, reads the backing view and
writes the `tv_*` table as the **owner of that `tv_*` table**, the way `REFRESH
MATERIALIZED VIEW` does (`src/owner.rs`):

- in a security-restricted operation: code it runs cannot `SET ROLE`, create
  temporary tables or otherwise change the session;
- with `search_path = pg_catalog, pg_temp`, so no name resolves to an object the
  writer planted in a schema on its own path;
- with the rendering settings pinned (`TimeZone` `UTC`, `DateStyle` `ISO, YMD`,
  `IntervalStyle` `postgres`, `extra_float_digits` `1`, `bytea_output` `hex`), so a
  TVIEW's rows do not depend on the session that wrote last;
- with `plan_cache_mode = auto` and `jit = off`: a refresh reads a few rows by key or
  rebuilds a TVIEW once, which JIT compilation slows down (#283).

The writer therefore needs only what the write needs without pg_tviews: the DML
privilege on the base table and `USAGE` on its schema (and `USAGE` on `tviews`,
granted to `PUBLIC`). It needs no privilege on the TVIEW, its backing view or the
other tables the view reads, and it cannot make a refresh read other objects or
render values differently.

```sql
SET ROLE docs_sec_writer;
INSERT INTO app.tb_user (name, email) VALUES ('Alice', 'alice@example.com');
SELECT has_table_privilege('app.tv_user', 'SELECT') AS writer_reads_tview;  -- false
RESET ROLE;

SET ROLE docs_sec_reader;
SELECT data ->> 'name' AS name FROM app.tv_user;   -- Alice, refreshed as docs_sec_owner
RESET ROLE;
```

pg_tviews' own catalog SQL runs the same way, under `search_path = pg_catalog,
pg_temp`. Names the caller wrote (a definition's text, an unqualified TVIEW) are
resolved under the caller's own `search_path`, when the TVIEW is created or
replaced: the backing view stores the objects it found, not their names.

### Functions a definition calls

A function body is resolved when it runs, during the refresh, under the pinned path.
A function a definition calls must therefore schema-qualify the tables and functions
it reads, or set its own `search_path`:

```sql fragment
CREATE FUNCTION app.label_suffix() RETURNS text
LANGUAGE sql STABLE
SET search_path = pg_catalog, pg_temp
AS $f$ SELECT value FROM app.tb_setting WHERE key = 'suffix' $f$;
```

Such a function runs as the TVIEW's owner, never as the writer. A non-`IMMUTABLE`
function outside `pg_catalog` must also declare the tables it reads in the
`function_reads` option, or the TVIEW is refused under the default `error` policy:
see [Functions that read tables](../reference/ddl.md#functions-that-read-tables).

## Settings

What a TVIEW is comes only from its definition and its options. The settings that
remain are fixed for every session unless a superuser changes them
([ADR 0220](../adr/0220-settings.md) §3): a setting that decides whether a write or a
creation succeeds, or what other sessions observe, must be the same for every
session, and an audit must not be one the audited can turn off.

| Setting | Who sets it | Why |
|---|---|---|
| `pg_tviews.max_refresh_rows` | superuser | decides whether a write succeeds |
| `pg_tviews.lock_escalation_threshold` | superuser | decides whether writes lock values or whole relations |
| `pg_tviews.publish_changes` | superuser | what listeners of `tviews_changes` observe |
| `pg_tviews.audit_enabled` | superuser | an audit the audited cannot turn off |
| `pg_tviews.auto_rebuild_databases` | `postgresql.conf`, restart | which databases a background worker refills after recovery |

The diagnostics `graph_cache_enabled`, `table_cache_enabled`, `direct_patch_enabled`
and `max_queue_size` are superuser-only too, and hidden from `SHOW ALL`. A superuser
may set any of them per role or database (`ALTER ROLE … SET`). The `pg_tviews.`
prefix is reserved: a misspelled or removed name fails instead of doing nothing.

```sql
SET ROLE docs_sec_writer;
DO $$
BEGIN
    SET pg_tviews.max_refresh_rows = 0;
EXCEPTION WHEN insufficient_privilege THEN
    RAISE NOTICE '%', SQLERRM;   -- permission denied to set parameter
END $$;
RESET ROLE;
```

Suspending refresh is not a way around them: `pg_tviews_suspend_triggers()` lasts one
transaction and records every change, refreshed at `pg_tviews_resume_triggers()` or
at commit.

## Writers

- **READ COMMITTED only.** A transaction at `REPEATABLE READ` or `SERIALIZABLE` that
  writes to a table a TVIEW reads, or refreshes or rebuilds TVIEWs, fails with
  `0A000` before writing: its snapshot could not see what it waits for. Reads of
  TVIEWs work at any level.
- **No subscribed base tables.** A TVIEW over a table a logical replication
  subscription writes is refused (`0A000`): the apply worker fires no statement
  trigger, so the TVIEW would go stale. Create the TVIEW on the publisher and
  publish its table instead ([Replication](replication.md)).
- **Triggers off means stale.** `session_replication_role = replica` (superuser) or
  `ALTER TABLE … DISABLE TRIGGER` (table owner) skips the refresh;
  `pg_tviews_check_health()` reports disabled triggers.

```sql
BEGIN ISOLATION LEVEL REPEATABLE READ;
DO $$
BEGIN
    INSERT INTO app.tb_user (name, email) VALUES ('Bob', 'bob@example.com');
EXCEPTION WHEN feature_not_supported THEN
    RAISE NOTICE '%', SQLERRM;   -- a REPEATABLE READ transaction cannot write …
END $$;
COMMIT;
```

## Audit log

With `pg_tviews.audit_enabled = on` (off by default), TVIEW creations, drops and the
refreshes of each flush are recorded in `tviews.pg_tview_audit_log`, in the same
transaction: a rollback takes its entries with it.

- `performed_by` is `session_user`: the role that logged in, whatever role it
  `SET ROLE` to.
- Rows are written by the extension's owner, through `pg_tviews_audit_write`, which
  `PUBLIC` may not execute. Other roles can neither write nor read the log unless
  granted.
- The log is not part of `pg_dump` output: it records this cluster's operations.
- Nothing prunes it: the extension's owner deletes old rows.

```sql
SET pg_tviews.audit_enabled = on;   -- superuser; normally in postgresql.conf
SET ROLE docs_sec_writer;
INSERT INTO app.tb_user (name, email) VALUES ('Carol', 'carol@example.com');
RESET ROLE;
SELECT operation, entity, performed_by = session_user AS by_session_user
FROM tviews.pg_tview_audit_log ORDER BY log_id DESC LIMIT 5;

DELETE FROM tviews.pg_tview_audit_log WHERE performed_at < now() - interval '90 days';
```

## Checklist

- [ ] pg_tviews is in `shared_preload_libraries`; `CREATE EXTENSION` was run by a
      superuser.
- [ ] Each TVIEW is owned by an application role, not a superuser: its refreshes run
      with that role's privileges.
- [ ] Readers have `SELECT` on the `tv_*` tables they need, and nothing else.
- [ ] TVIEWs leave secrets out of `data`.
- [ ] Functions called by definitions schema-qualify what they read or set their
      own `search_path`, and are declared in `function_reads`.
- [ ] Only operator roles hold `EXECUTE` on the `_all` functions.
- [ ] Writers run at `READ COMMITTED`; no TVIEW reads a subscribed table.
- [ ] `pg_tviews.*` settings are set in `postgresql.conf` or by a superuser.
- [ ] `pg_tviews_check_health()` reports no disabled triggers.
- [ ] If auditing is required, `pg_tviews.audit_enabled = on` and old rows are
      pruned.

## See also

- [API reference: privileges](../reference/api.md#privileges)
- [Operator guide](../user-guides/operators.md)
- [DDL reference](../reference/ddl.md)
- [Replication](replication.md)
