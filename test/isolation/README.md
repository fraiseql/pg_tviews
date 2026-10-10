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
schedule, listed here, and run by name. None is open.

Writers wait on each other's value locks (see `docs/concurrency.md`), so a spec
under `READ COMMITTED` runs only the orders where the session that writes first
also commits first: the other orders would wait forever in the tester.
