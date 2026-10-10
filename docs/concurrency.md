# Concurrency

How TVIEW refreshes behave when several transactions write at once: when the refresh
runs, what it locks, and what each isolation level gives you.

## When a TVIEW is refreshed

A write to a base table is refreshed **inside the writer's transaction**:

- at the end of each statement (a statement-level flush trigger), so an autocommit
  statement is refreshed before it returns. A statement run from a trigger of a write
  to a TVIEW's base table (a trigger that writes its own table, level by level) leaves
  its work to that write, which refreshes once at its end: each TVIEW row is computed
  once, from the final state. A query that only reads defers nothing: in `SELECT f()`,
  each statement of `f` that writes is refreshed before the next one reads;
- before `COMMIT` and before `PREPARE TRANSACTION` (the `ProcessUtility` hook), so the
  refresh writes belong to the transaction.

The refresh writes are ordinary row writes to `tv_<entity>`: other sessions see them
when the writer commits, exactly like its base-table writes, and a rollback discards
both. The refresh runs as the TVIEW's owner with `search_path = pg_catalog, pg_temp`,
whoever the writer is, and renders values under fixed settings (see
[Rendering](reference/ddl.md#rendering)).

Nothing is queued across transactions. A transaction that would commit with refresh
work still queued (a flush trigger dropped or disabled) fails instead, with SQLSTATE
`55000`, naming the TVIEWs; `pg_tviews_health_check()` reports the missing trigger.

## Locks

| What | Lock | Held until |
|---|---|---|
| A refresh of some rows (every write) | row locks on the `tv_<entity>` rows it recomputes (taken before it reads the view, under `READ COMMITTED`), inserts, updates or deletes | end of the writer's transaction |
| A full refresh caused by a write (`TRUNCATE` of a base table, a table under the `full_refresh` policy) | row locks on the rows that differ (it reconciles the TVIEW with its view) | end of transaction |
| `pg_tviews_refresh(entity)` | `ACCESS EXCLUSIVE` on each TVIEW it rebuilds (`TRUNCATE` + `INSERT`) | end of transaction |
| Filling an UNLOGGED TVIEW a crash or a promotion reset (the first write, `pg_tviews_rebuild_all()`, the startup worker) | the TVIEW's row in `tviews.pg_tview_valid`, inserted before the fill: other writers of that TVIEW wait for it (under `REPEATABLE READ` they fail with `40001`); readers are not blocked | end of transaction |
| Creating, replacing or dropping a TVIEW | advisory lock `pg_advisory_xact_lock(1953917285, hashtext(entity))`, so DDL on one entity runs one call at a time | end of transaction |

So two transactions whose writes refresh the same TVIEW row run one after the other
on that row: the second waits for the first to commit or roll back. Readers are never
blocked, except by `pg_tviews_refresh()`.

Like any workload that locks rows, two transactions refreshing overlapping TVIEW rows
in different orders can deadlock. PostgreSQL detects it and aborts one of them with
`deadlock detected` (SQLSTATE `40P01`); retry it.

## Isolation levels

Every isolation level works. They differ when **two concurrent transactions write
rows that feed the same TVIEW row** (a writer renames a user while another edits one
of the user's posts, and `tv_post` embeds the author). The specs in
`test/isolation/` check each case below.

| Isolation level | What happens to the second writer | The TVIEW row |
|---|---|---|
| `READ COMMITTED` (PostgreSQL's default) | waits for the first, then recomputes the row with a snapshot that sees the first one's commit | carries both changes |
| `REPEATABLE READ`, `SERIALIZABLE` | fails with `could not serialize access due to concurrent update` (SQLSTATE `40001`) | correct once the application retries the transaction, as for any read-modify-write |

Under `READ COMMITTED`, the refresh locks the existing TVIEW rows it is about to
recompute (`SELECT … FOR UPDATE`, in key order) before it reads the view: a second
writer waits there, and the recompute that follows sees the first writer's change.
Under `REPEATABLE READ` and `SERIALIZABLE` the rows are not locked first: the write of
a row another transaction changed since the snapshot fails with `40001` rather than
store a document computed from that snapshot.

### Known defects

A row that does not exist yet has nothing to lock, so a TVIEW row being **created**
or **re-linked** while a row it reads is changed can be written stale, under
`READ COMMITTED` and `REPEATABLE READ` (#207):

- **A new parent while its embedded child changes.** One transaction inserts a parent
  row (a post) while another renames the child it embeds (its author), and both
  commit: the new `tv_post` row keeps the author's old name. The inserter computes the
  row from a snapshot without the rename, and the rename's propagation cannot see the
  parent row that is not committed yet.
- The same happens with a direct join of the child's base table, a fan-out patch, a
  parent re-pointed to the child (`UPDATE tb_post SET fk_user`), a child two hops away,
  an outer join to a child inserted concurrently, and a table under the `full_refresh`
  policy.
- **Two transactions creating the same TVIEW row.** Both insert rows that feed one key
  no transaction has materialized yet; the second waits on the unique index, then
  writes the row it computed.

Each case is reproduced by a spec in `test/isolation/specs/`, out of the default
schedule (see `test/isolation/README.md`). The fix is designed in
[ADR 0207](adr/0207-concurrent-maintenance.md). `SERIALIZABLE` is not affected: one of
the two transactions fails with `40001`.

In both cases the next write to that row, or `SELECT tviews.pg_tviews_refresh('…')`,
repairs it.

## Suspended refresh

`pg_tviews_suspend_triggers()` defers refreshes until `pg_tviews_resume_triggers()`
or the end of the transaction, whichever comes first: suspension never outlives the
transaction that started it. Resuming rebuilds the TVIEWs that changed and those that
embed them (each with `ACCESS EXCLUSIVE`, as `pg_tviews_refresh` does). The
`pg_tviews.suspend_triggers` setting suspends for the session, records nothing, and
leaves the TVIEWs stale until they are refreshed.

## Settings

The settings are listed in the [README's Configuration table](../README.md#configuration).
The ones that bear on concurrent writers:

- `pg_tviews.max_queue_size`: refreshes one transaction may queue before it fails;
- `pg_tviews.direct_patch_enabled`: the direct-patch fast path (on by default);
- `pg_tviews.suspend_triggers`: see above.

There is no lock timeout of pg_tviews' own: PostgreSQL's `lock_timeout` and
`deadlock_timeout` apply to the row locks above. A `pg_tviews.*` name that pg_tviews
does not define is refused.
