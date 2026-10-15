# Failure Modes and Recovery Procedures

pg_tviews refreshes TVIEW rows inside the transaction that writes the base tables
(at the end of each statement and on COMMIT). Most failures therefore follow ordinary
transaction rules: if anything fails, the base-table writes and the TVIEW writes roll
back together. Examples use the entity `user` (`tb_user`, `v_user`, `tv_user`).

## Database Failures

### PostgreSQL Crash

**Symptoms**:
- The server crashed or was restarted in immediate mode
- UNLOGGED TVIEWs (declared `logged: false`) are empty after crash recovery

**Recovery**:
1. Uncommitted transactions are rolled back; committed base-table writes and LOGGED
   TVIEW writes (the default) are recovered together.
2. When recovery ends, a launcher worker refills the UNLOGGED TVIEWs recovery emptied,
   in every database (`pg_tviews.auto_rebuild_databases`, default `*`; a list
   restricts it, an empty value disables it; server restart required). Otherwise the
   first write to such a TVIEW refills it, or refill them by hand:
   ```sql
   SELECT * FROM tviews.pg_tviews_replication_status() WHERE needs_rebuild;
   SELECT * FROM tviews.pg_tviews_rebuild_all();          -- only empty TVIEWs by default
   ```

**Prevention**: keep TVIEWs that must survive a crash LOGGED (the default). Switch an
UNLOGGED one back with `ALTER TABLE tv_user SET LOGGED` or the `logged` option of
`pg_tviews_create_or_replace()`; both fill a reset TVIEW first.

### Disk Full

**Symptoms**:
- `ERROR: could not extend file ...`
- Writes to base tables fail

**Recovery**:
1. Free disk space.
2. Failed writes rolled back with their TVIEW changes; nothing to repair. To confirm a
   TVIEW matches its view:
   ```sql
   (SELECT pk_user, data FROM tviews.public__tv_user EXCEPT SELECT pk_user, data FROM tv_user)
   UNION ALL
   (SELECT pk_user, data FROM tv_user EXCEPT SELECT pk_user, data FROM tviews.public__tv_user);
   ```
3. If rows differ, rebuild: `SELECT tviews.pg_tviews_refresh('user');`

**Prevention**: alert on disk usage before it fills.

### Out of Memory

**Symptoms**:
- `ERROR: out of memory` during a large write or a full rebuild

**Recovery**:
1. For a rebuild, raise `work_mem` for the session:
   ```sql
   SET work_mem = '256MB';
   SELECT tviews.pg_tviews_refresh('user');
   RESET work_mem;
   ```
2. For a bulk write, split it into smaller transactions, or suspend refresh for the
   load (see "Bulk loads" below).

### Connection Loss

**Symptoms**:
- Client disconnects during a transaction

**Recovery**: the transaction rolls back, TVIEW writes included. Reconnect and retry.

---

## Extension Failures

### Circular Dependency

**Symptoms**:
```
ERROR: Circular dependency detected: a → b → a
```

**Recovery**: the statement that would close the cycle fails. Inspect the chain
and change one view so the dependency graph has no cycle:
```sql
SELECT * FROM tviews.pg_tviews_show_cascade_path('post');
```

### Metadata Not Found

**Symptoms**:
```
ERROR: TVIEW user does not exist
```
- Triggers exist on a base table but no TVIEW is registered for them

**Recovery**:
1. Check what is registered:
   ```sql
   SELECT schema, name, entity FROM tviews.registry ORDER BY schema, name;
   SELECT component, severity, message FROM tviews.pg_tviews_health_check()
   WHERE severity <> 'info';
   ```
2. If the TVIEW is gone, drop the orphaned triggers named by the health check, or
   recreate the TVIEW:
   ```sql
   SELECT tviews.pg_tviews_create_or_replace('tv_user', '<the defining SELECT>');
   ```

**Prevention**: do not edit `tviews.pg_tview_meta` by hand; create, change and drop
TVIEWs with `tviews.pg_tviews_create_or_replace` and `tviews.pg_tviews_drop`.

### Refresh Error During a Write

**Symptoms**:
- An `INSERT`/`UPDATE`/`DELETE` on a base table fails with an error raised while
  refreshing a TVIEW (for example a cast or constraint error inside the view)

**Recovery**: the statement and its transaction rolled back; the TVIEW is unchanged
and consistent. Fix the data or the view definition, then retry the write. To let
writes proceed while the cause is investigated, see "Bulk loads" below.

### Prepared Transactions Left Open

**Symptoms**:
- A TVIEW does not show changes made by a transaction that ran `PREPARE TRANSACTION`
- Locks held on `tv_*` rows or base tables

**Cause**: `PREPARE TRANSACTION` flushes the refresh queue first, so the TVIEW writes are
part of the prepared transaction. They become visible only at `COMMIT PREPARED`, and the
prepared transaction keeps its locks until then.

**Recovery**:
1. List prepared transactions:
   ```sql
   SELECT gid, prepared, owner, database FROM pg_prepared_xacts ORDER BY prepared;
   ```
2. Finish each one:
   ```sql
   COMMIT PREPARED 'gid';     -- or ROLLBACK PREPARED 'gid';
   ```

**Prevention**: always commit or roll back prepared transactions promptly.

### Missing or Disabled Triggers

**Symptoms**:
- Writes to a base table do not reach the TVIEW
- The health check reports trigger problems

**Recovery**:
1. Check the triggers:
   ```sql
   SELECT tgname, tgrelid::regclass, tgfoid::regproc, tgenabled
   FROM pg_trigger
   WHERE tgfoid::regproc::text IN ('tviews.pg_tview_trigger_handler',
                                   'tviews.pg_tview_flush_trigger',
                                   'tviews.pg_tview_truncate_trigger');
   ```
2. Re-install them, then bring the TVIEW up to date:
   ```sql
   SELECT * FROM tviews.pg_tviews_reregister_all();
   SELECT tviews.pg_tviews_refresh('user');
   ```

**Prevention**: do not disable or drop `trg_tview_*` triggers by hand.

---

## Operational Failures

### PostgreSQL Major Version Upgrade

**Procedure**:
1. Before the upgrade:
   ```bash
   pg_dump -Fc mydb > mydb.dump
   psql -d mydb -c "SELECT tviews.pg_tviews_version();"
   ```
2. Install pg_tviews (and jsonb_delta) built for the new PostgreSQL version, then run
   `pg_upgrade`. Supported versions: 16, 17, 18.
3. Verify:
   ```sql
   SELECT tviews.pg_tviews_version();
   SELECT * FROM tviews.pg_tviews_health_check();
   SELECT * FROM tviews.pg_tviews_reregister_all();
   ```

### Backup and Restore

**Backup**:
```bash
# Full logical backup: tv_* tables, v_* views, base-table triggers and the
# pg_tview_meta / pg_tview_helpers catalog rows
pg_dump -Fc mydb > mydb.dump
```

**Restore**:
```bash
pg_restore -d mydb_restored mydb.dump
```

The catalog rows are restored with the rest of the data. `view_oid` and
`table_oid` are `regclass`, and the relation OIDs inside each TVIEW's `plan` are
rebound by name by an insert trigger on `pg_tview_meta`, so the restored TVIEWs
point at the restored relations and keep propagating. A plan naming a table the
restore did not create fails the insert, naming the TVIEW. Do not restore with
`--disable-triggers`: it skips that rebind.

A database whose extension was created before this catalog layout (beta.18 and
earlier) does not mark its catalog for dumping, so its dumps carry no
`pg_tview_meta` rows. Re-register such TVIEWs after the restore.

**Verification**:
```sql
SELECT schema, name, view, needs_reregister FROM tviews.registry ORDER BY schema, name;
SELECT * FROM tviews.pg_tviews_health_check();
-- then write a base-table row and check that the TVIEW row changes
```

### Standbys and Failover

**Symptoms**:
- TVIEWs are empty or unreadable on a hot standby
- TVIEWs are empty after promoting a standby

**Cause**: UNLOGGED TVIEWs (declared `logged: false`) are not replicated.

**Recovery**:
```sql
SELECT * FROM tviews.pg_tviews_replication_status();
-- after promotion, if the launcher worker is disabled:
SELECT * FROM tviews.pg_tviews_rebuild_all();
```

**Prevention**: keep TVIEWs that standbys must read LOGGED (the default;
`ALTER TABLE tv_user SET LOGGED` switches an UNLOGGED one). LOGGED TVIEWs replicate like any
table; they lag the primary exactly as much as the base tables do.

### Concurrent DDL

**Scenario**: a TVIEW is dropped while another session writes its base tables.

**Behavior**: the writing transaction either completes first (the drop waits for its
lock) or fails and rolls back. No partial data is left.

**Prevention**: run TVIEW DDL in maintenance windows.

---

## Emergency Procedures

### Rebuild All TVIEWs

```sql
-- All TVIEWs, dependencies first; returns the order and duration
SELECT tviews.pg_tviews_refresh_all();
```

### Bulk loads, or writes while a refresh keeps failing

Suspend refresh in the writing transaction and catch up at the end:

```sql
BEGIN;
SELECT tviews.pg_tviews_suspend_triggers();
-- writes
SELECT tviews.pg_tviews_suspended_entities();   -- TVIEWs that will need refreshing
SELECT tviews.pg_tviews_resume_triggers();      -- refreshes them
COMMIT;
```

See `docs/operations/runbooks/scripts/emergency-disable.sql`. Disabling `trg_tview_*`
triggers with `ALTER TABLE ... DISABLE TRIGGER` also stops refresh, but nothing records
what was missed: run `SELECT tviews.pg_tviews_refresh_all();` after re-enabling them.

---

## Monitoring and Alerts

1. **Health check**: alert on any row with severity `warning` or `error`:
   ```sql
   SELECT component, severity, message FROM tviews.pg_tviews_health_check()
   WHERE severity <> 'info';
   ```
2. **Old prepared transactions**:
   ```sql
   SELECT count(*) FROM pg_prepared_xacts WHERE prepared < now() - interval '1 hour';
   ```
3. **TVIEWs needing a rebuild** after a crash or failover:
   ```sql
   SELECT entity FROM tviews.pg_tviews_replication_status() WHERE needs_rebuild;
   ```
4. **Writes failing** with pg_tviews errors: watch the server log.

There is no queue to monitor between transactions: the refresh queue exists only
inside a running transaction.

---

## Support

For issues not covered here, see:
- [GitHub Issues](https://github.com/fraiseql/pg_tviews/issues)
- [Troubleshooting Guide](troubleshooting.md)
