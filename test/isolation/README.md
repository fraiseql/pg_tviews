# Isolation specs

Concurrent sessions against pg_tviews, run by PostgreSQL's `pg_isolation_regress`:
two writers refreshing the same TVIEW row, a TVIEW replaced while it is written,
and writers at REPEATABLE READ and SERIALIZABLE. Each spec checks that every
TVIEW equals its backing view once the sessions are done.

```sh
PGHOST=localhost PGPORT=28818 PGUSER=postgres PG_CONFIG=/path/to/pg_config ./test/isolation/run.sh
```

`isolation_schedule` lists the specs that run by default. A spec reproducing an
open defect is kept with the output the fixed behaviour gives and left out of the
schedule; run it by name:

Writers wait on each other's value locks (see `docs/concurrency.md`), so a spec
under `READ COMMITTED` runs only the orders where the session that writes first
also commits first: the other orders would wait forever in the tester.

| Spec | Defect |
|---|---|
| `new-parent-vs-write` | (#207) A parent row inserted while the child it embeds is renamed: the new parent keeps the old name. The inserter computes its row from a snapshot without the rename, and the rename's propagation can't see the uncommitted parent. |
| `join-parent-vs-write` | The same, when the parent joins the child's base table instead of embedding its TVIEW. |
| `fanout-vs-new-parent` | The same, when the rename is written by a fan-out patch (`UPDATE tv_post … WHERE fk_user = …`). |
| `repoint-vs-write` | An existing parent re-pointed to a child (`UPDATE tb_post SET fk_user`) while that child is renamed: the parent keeps the old name, embedded or joined. |
| `multihop-vs-write` | A post inserted while its author's organisation, two hops away, is renamed: the post keeps the old name, through embedded TVIEWs or a two-hop join. |
| `phantom-child-vs-parent` | A post naming a user that doesn't exist yet (outer join) inserted while that user is inserted: the post keeps no author. |
| `full-refresh-vs-new-parent` | A post inserted while a table under the `full_refresh` policy changes: the full refresh can't see the post, and the post is computed without the change. |
| `empty-tview-fill-vs-write` | Two first writes into a TVIEW a crash emptied (#214): both fill it from the view, and the second fails on a duplicate key. |
| `rr-new-parent-vs-write` | Under `REPEATABLE READ` the new parent is stale in every order, even when the rename committed before the insert ran: a snapshot can't see what committed after it, however long it waits. |
