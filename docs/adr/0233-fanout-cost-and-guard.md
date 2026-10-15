# ADR 0233: Fan-out cost model and the refresh guard

- Status: Accepted
- Related: [ADR 0220](0220-settings.md) (who may set what), [ADR 0207](0207-concurrent-maintenance.md)
  (locks), #233, #235, #238, [write path](../benchmarks/write-path.md)

## Context

A TVIEW that embeds another entity's columns copies them into every row that embeds that
entity. A write to such a column refreshes all those rows, in the writer's statement:
renaming a category embedded in a million products rewrites a million documents before the
`UPDATE` returns. Nothing warned about this before the write. Nothing stopped it either: the
statement held its locks, wrote its WAL and took as long as it took.

The cost was measured on the stock PostgreSQL 18.4 release package: one category embedded in
N items, `UPDATE tb_cat SET name = …`, one client, fresh database per cell.

| Documents | Path | fixed per write | per embedding row | WAL per row |
|---|---|---:|---:|---:|
| ~1 KB, inline | patch (`jsonb_set` in place) | ~0.13 ms | **2.8 µs** | ~0.7 KB |
| ~1 KB, inline | recompute (backing view) | ~0.20 ms | 8.8 µs | ~0.8 KB |
| ~4 KB, TOASTed | patch | ~0.24 ms | ~21 µs | ~4.9 KB |
| ~4 KB, TOASTed | recompute | ~0.23 ms | ~20-25 µs | ~5.0 KB |

- Inline documents: an update logs about 0.7 × the document when old and new share a page
  (HOT). A TOASTed document is rewritten whole: new chunks and index entries, about 1.2 × its
  size.
- TOASTed documents gain nothing from the patch: detoasting and rewriting the value dominate.
  Building the patched document once instead of twice changed nothing measurable (#238,
  0 % at 4 KB, −5 % at 1 KB inline).
- The rows are found through the lookup index, planned for the number of keys: one key by
  `= $1` from a generic plan, several from an array planned per execution (#235). Before
  that, a fresh TVIEW with few lookup values seq-scanned on every write.

| fan-out N | per write (1 KB) | WAL per write | at 100 writes/s |
|---:|---:|---:|---|
| 10 | 0.18 ms | 7 KB | 2 % of a core, 0.7 MB/s |
| 100 | 0.43 ms | 69 KB | 4 % of a core, 6.9 MB/s |
| 1,000 | 3.1 ms | 0.69 MB | 31 % of a core, 69 MB/s |
| 10,000 | 28 ms | 6.9 MB | 2.8 cores, 690 MB/s |
| 1,000,000 | ~2.8 s | ~690 MB | — |

The refreshed rows stay locked until the statement's transaction ends.

## Decision

### 1. The model is documented where embeds are designed

The architects guide carries the table, the rule and the remedies. `pg_tviews_profile()`
projects each fan-out's cost per write from its measured p99, using the constants above and
the TVIEW's average document size. Rule: above ~1,000 rows per write at sustained rates, or
~100,000 at all, don't embed. Reference the id and resolve it at read time, or batch the
change offline.

### 2. `pg_tviews.max_refresh_rows`

An integer, `0` (off, the default) up to 2,147,483,647, **superuser** context like
`max_queue_size`: it decides whether a write succeeds (ADR 0220 §3).

- **Per statement**, which is one flush: the flush runs once per statement, and nested flushes
  fold into the outer one. A transaction of many small statements is never charged as one.
- **Charged before writing**:
  - a recompute: by its keys;
  - a direct patch: by its rows;
  - a fan-out: by its children, counted through the lookup index (`count(*)` with the
    #235 shapes), and only while the guard is on;
  - a full refresh: by the rows the TVIEW holds (counted, a scan the full refresh
    reads past anyway), an upper bound of what it rewrites while the TVIEW is filled.
  When the rows charged so far plus the next batch exceed the limit, the statement fails
  before that batch is written, and the base write rolls back with it.
- Explicit rebuilds are not charged: `pg_tviews_refresh()`, `pg_tviews_rebuild_all()`, the
  refill of an UNLOGGED TVIEW a crash or a promotion reset (recovery, not a write's work), and
  the catch-up that rebuilds the TVIEWs a suspended transaction changed, at
  `pg_tviews_resume_triggers()` or at commit. They are the remedy for a bulk change:
  `pg_tviews_suspend_triggers()`, the change, then the resume or the commit (or
  `pg_tviews_refresh('tv_…')`).
- Off costs one integer comparison per flush, with no count queries.

### 3. One error for refresh limits

`54000`, `RefreshLimit { kind }`, with `kind` either the queue (`max_queue_size`) or the rows
(`max_refresh_rows`): one SQLSTATE per variant, and both mean "this statement's refresh is
bigger than allowed". The rows message names the TVIEW, the lookup column (or "full
refresh"), the count and the limit:

```
pg_tviews: refreshing tv_item via fk_cat would write 10000 rows in one statement (max_refresh_rows = 5000)
HINT: Don't embed this entity in tv_item, batch the change offline, or make it after pg_tviews_suspend_triggers(): the TVIEWs it changed are then rebuilt once, at resume or commit.
```

### 4. No deferred refresh before 1.0

An asynchronous refresh would trade the statement's cost for staleness that readers cannot
see. pg_tviews' contract is that a committed write is visible in every TVIEW. The guard and
the documented remedy cover the large cases. Reopen only with a real workload.

## Consequences

- An architect sees the cost of an embed before building it, and `pg_tviews_profile()` shows
  it for embeds that exist.
- A superuser can cap the work one statement does, and get a loud error instead of a
  long-held lock.
- `QueueFull` is gone as a variant: its SQLSTATE, message and hint are unchanged under
  `RefreshLimit`.
