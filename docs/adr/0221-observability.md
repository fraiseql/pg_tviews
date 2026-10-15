# ADR 0221: Per-TVIEW refresh statistics readable from any session

- Status: Accepted
- Fixes: #220 (refresh counters are per session and not per TVIEW)
- Related: [ADR 0211](0211-api-surface.md) (naming, resolver), [ADR 0220](0220-settings.md)

## Context

The refresh-path counters live in backend-local memory:

- `view_recomputes`, `noop_skipped`, the direct-patch counters, `propagation_pruned`;
- read with `pg_tviews_queue_stats()` in the session that did the work;
- not attributed to a TVIEW.

A monitoring connection, a pooled application server or a metrics exporter cannot read them.
Even the writing session cannot tell which TVIEW recomputed. fraiseql's `/metrics` leaves them
out for that reason.

## Decision

### Three tiers

| Tier | Surface | Contract |
|---|---|---|
| What is declared | `tviews.registry` | yes; plain SQL, readable without the library and on a standby |
| What happens | `tviews.stats` (this ADR) | yes; reads the library's shared memory |
| Physical health | `pg_tviews_profile()` | no |

`pg_tviews_queue_stats()` and `pg_tviews_debug_queue()` stay session debugging tools, outside
the contract.

### `tviews.stats`

One row per registered TVIEW of the current database:

| Column | Meaning |
|---|---|
| `schema`, `name`, `entity` | as `registry` (join on `entity`) |
| `view_recomputes` | rows recomputed from the backing view |
| `noop_skipped` | refreshes skipped because the row already held the result |
| `patch_captured`, `patch_applied`, `patch_fallbacks` | the direct-patch path |
| `propagation_pruned` | parent lookups skipped because the child row did not change |
| `rows_written`, `rows_deleted` | TVIEW rows inserted or updated, deleted, by a refresh of some rows or a reconcile (a whole rebuild counts in `full_refreshes`) |
| `full_refreshes` | whole-TVIEW refreshes (uncascaded `full_refresh`, `pg_tviews_refresh`, the refill of a reset TVIEW) |
| `refresh_ms` | time spent refreshing this TVIEW (`double precision`) |
| `stats_reset` | when its counters started |
| `untracked` | the shared table was full when it first refreshed: its counters are NULL |

`tviews.pg_tviews_stats_reset(tview text DEFAULT NULL)` zeroes one TVIEW (resolved as in ADR
0211) or every TVIEW of the database. Its `EXECUTE` is revoked from `PUBLIC`, like the other
maintenance functions. The view is readable by `PUBLIC`: it holds counts, no data.

### Semantics

- **Cumulative** since the server started or the last reset. Not persisted across restarts.
- **Counted at transaction end**, committed or aborted, like `pg_stat_*`: work an aborted
  transaction did still cost.
- **Keyed by TVIEW table OID.** A `rebuilt` replace creates a new table, and its counters start
  over (like `pg_stat_user_tables`). Dropping a TVIEW removes its entry.

### Storage

- Shared memory, allocated at preload: a fixed table of 4096 TVIEWs (about 400 kB) for the
  cluster, keyed by `(database oid, table oid)`, under one lightweight lock. All-zero memory
  is an empty table, so the startup hook zeroes it instead of building a value on the stack.
- Each backend adds to a local map during the transaction. At commit, abort or prepare, the
  map is merged into shared memory: one lock acquisition per transaction, no SPI.
- Dropping a TVIEW, or rebuilding it (a new table), frees its entry; a reset of the database
  frees all of its entries. A TVIEW that finds the table full is untracked: `tviews.stats`
  shows it with NULL counters, and its `untracked` column is true. Nothing is silently missing.
- Without `shared_preload_libraries = 'pg_tviews'`, reading `tviews.stats` fails with a hint;
  it does not return zeros.
- The store sits behind one module (`stats`). When PostgreSQL 18 is the oldest supported
  release, it can move to custom cumulative statistics (`pgstat_register_kind`, persisted)
  with no SQL change.

## Consequences

- fraiseql can export per-TVIEW refresh metrics from a pooled connection.
- Every increment site names its TVIEW. Counting moves out of `metrics` into the leaf module
  `stats`.
- On a standby, the view shows the TVIEWs with zero counters: nothing refreshes there.
