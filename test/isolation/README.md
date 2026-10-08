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

| Spec | Defect |
|---|---|
| `new-parent-vs-write` | A parent row inserted while the child it embeds is renamed, both under READ COMMITTED: the new parent keeps the old name. The inserter computes its row from a snapshot without the rename, and the rename's propagation cannot see the uncommitted parent. |
