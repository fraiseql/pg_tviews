# Concurrency

How TVIEW refreshes behave when several transactions write at once: when the refresh
runs, what it locks, and what each isolation level gives you.

## When a TVIEW is refreshed

A write to a base table is refreshed **inside the writer's transaction**:

- at the end of each statement (a statement-level flush trigger), so an autocommit
  statement is refreshed before it returns;
- before `COMMIT` and before `PREPARE TRANSACTION` (the `ProcessUtility` hook), so the
  refresh writes belong to the transaction.

The refresh writes are ordinary row writes to `tv_<entity>`: other sessions see them
when the writer commits, exactly like its base-table writes, and a rollback discards
both. The refresh runs as the TVIEW's owner with `search_path = pg_catalog, pg_temp`,
whoever the writer is.

Nothing is queued across transactions: work left queued when a transaction commits is
dropped with a WARNING naming the TVIEWs (it means a flush trigger is missing; see
`pg_tviews_health_check()`).

## Locks

| What | Lock | Held until |
|---|---|---|
| A refresh of some rows (every write) | row locks on the `tv_<entity>` rows it recomputes (taken before it reads the view, under `READ COMMITTED`), inserts, updates or deletes | end of the writer's transaction |
| A full refresh caused by a write (`TRUNCATE` of a base table, a table under the `full_refresh` policy) | row locks on the rows that differ (it reconciles the TVIEW with its view) | end of transaction |
| `pg_tviews_refresh(entity)` | `ACCESS EXCLUSIVE` on each TVIEW it rebuilds (`TRUNCATE` + `INSERT`) | end of transaction |
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
of the user's posts, and `tv_post` embeds the author).

| Isolation level | What happens to the second writer | The TVIEW row |
|---|---|---|
| `READ COMMITTED` (PostgreSQL's default) | waits for the first, then recomputes the row with a snapshot that sees the first one's commit | carries both changes |
| `REPEATABLE READ`, `SERIALIZABLE` | fails with `could not serialize access due to concurrent update` (SQLSTATE `40001`) | correct once the application retries the transaction, as for any read-modify-write |

Under `READ COMMITTED`, the refresh locks the existing TVIEW rows it is about to
recompute (`SELECT … FOR UPDATE`, in key order) before it reads the view: a second
writer waits there, and the recompute that follows sees the first writer's change.

One case remains: two transactions that **create** the same TVIEW row at once (the row
does not exist yet, so there is nothing to lock). The second waits on the unique
index, then writes the row it computed. It needs both writers to insert rows that
feed one key that no transaction has materialized yet; the next write to that row,
or `SELECT tviews.pg_tviews_refresh('…')`, repairs it.

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
