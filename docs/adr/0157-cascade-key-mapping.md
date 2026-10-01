# ADR 0157: Map base-table writes to TVIEW keys from PostgreSQL's query tree

- Status: Accepted
- Issues: #157 (scalar subquery), #158 (view with an aggregate)
- Supersedes: cascade-path extraction from the view's SQL text (`sql_parser::extract_join_paths`)

## Context

For every base table `T` a TVIEW reads, pg_tviews needs `affected(T, changed rows) → TVIEW keys`.
That function must never miss a key, and it must be cheap on the write path. Two independent
mechanisms approximate it today:

| Concern | Mechanism | Source of truth |
|---|---|---|
| Which tables get triggers | `dependency/graph.rs::find_base_tables` | `pg_depend` on the backing view's rewrite rule (exact) |
| How a changed row maps to keys | `sql_parser::extract_join_paths` → hop chain | A re-parse of the SQL text with `sqlparser` (approximate) |

Any shape the text parser does not model leaves a table with triggers but no mapping, and
writes to it are dropped silently. #157 (a correlated subquery in the select list) and #158
(a view with `GROUP BY`) are two instances. LATERAL, `IN (SELECT …)`, window partitions and
non-equality correlations are the same class. A hop chain can only express equality joins on a
single key.

The mapping runs in a row-level trigger: per changed row, per cascade path, per OLD/NEW image,
**one SPI query per hop** (`queue/ops.rs::spi_batch_lookup`).
`trigger.rs::pg_tview_stmt_trigger_handler` reads transition tables that no trigger declares,
so it is dead code.

### Measurements

PL/pgSQL stand-ins on PostgreSQL 18.1: 200k `tb_line`, 20k orders, 2k SKUs. The host load
average was 80–280, so compare ratios, not absolute times. Two runs agreed within ~5%.

| Write | No trigger | Row-level mapping | Statement-level, transition tables |
|---|---:|---:|---:|
| UPDATE 100k lines, 1 hop, key in the row | 894 ms | 1 260 ms (+41%) | 1 035 ms (+16%) |
| UPDATE 1 000 SKUs → 100k lines, 2 hops | — | 126–998 ms | < 50–175 ms (2.5–6× faster) |
| 3 single-row UPDATEs, 1 hop | — | 0.43 / 0.16 / 0.14 ms | 0.40 / 0.17 / 0.15 ms |

Set-based mapping is never slower for single-row writes and is clearly faster for bulk and
multi-hop writes, because it replaces rows × hops SPI calls with one join per statement.

## Options

- **A. Extend the text parser** for each new shape. This is incremental, but it re-implements
  PostgreSQL's semantic analysis (name resolution, quoting, view expansion, correlation levels),
  each missed shape stays silent, and hop chains can't express non-equality dependencies.
- **B. Analyze PostgreSQL's query tree.** `get_view_query` on the backing view, recursing into
  views (RTEs with `relkind = 'v'`), CTEs and `SubLink`s. Tables are resolved OIDs, correlation
  is explicit (`Var.varlevelsup`), and quals carry resolved operators. The walk sees the same
  tables `pg_depend` reports, so "watched but not understood" is detected by construction.
- **C. Generate a key-mapping query** per (base table, TVIEW), run once per statement over
  `OLD TABLE ∪ NEW TABLE`. Any predicate can be expressed. The worst case is a correct
  query returning every key.
- **D. Lineage table** of `(tview_key, base_table, base_pk)` written at each refresh. It is exact,
  but it adds fan-in writes, index maintenance and vacuum to every refresh. Rejected.
- **E. Full IVM** (delta algebra on values). Out of scope. Recomputing a row by key, then
  smart-patching JSONB, is simpler and already robust.

## Decision

**B + C, with a hybrid trigger layout.**

1. At create and re-registration time, a query-tree analyzer classifies every table the backing
   view reads:
   - `Local(col)`: the TVIEW key is a column of the changed row (the root table, a direct fk).
     Mapped inside the row-level trigger without SQL. The direct-patch (#56) and fan-out (#120)
     fast paths keep working unchanged.
   - `Mapped(sql)`: a generated key-mapping query over the changed rows.
   - `AllKeys`: no selective predicate (global aggregate, window without `PARTITION BY`, opaque
     correlation). Handled by `pg_tviews.uncascaded_policy` (`warn` | `error` | `full_refresh`).
   The analyzer errors loudly on any node it does not understand. It never drops a table silently.
2. `Mapped` tables get **one statement-level trigger** with `REFERENCING OLD TABLE … NEW TABLE …`,
   which runs their cached mapping queries and bulk-enqueues keys. Row-level triggers stay only on
   `Local` tables.
3. The text-based extraction, the per-hop `spi_batch_lookup` loop and the dead statement handler
   are removed. `sqlparser` remains only for create-time text handling that has no query tree
   (CTAS interception), if anything still needs it.

### Constraints this relies on

- **PostgreSQL 16+ only.** The `Query`, `RangeTblEntry`, `SubLink` and `Var` fields the analyzer
  reads are the same in 16, 17 and 18 (the `RTEPermissionInfo` split landed in 16). CI runs the
  regression suite on all three.
- **Transition-table limits:**
  - `TRUNCATE` has no transition table. It maps to `AllKeys` for every dependent TVIEW.
  - Partitions can't declare transition tables, so the trigger goes on the partitioned root.
  - `UPDATE OF col` can't be combined with transition tables. Column-aware filtering moves into
    SQL: join OLD to NEW on the table's key and compare the referenced columns with `*=`
    (search_path-independent, as in #156).
- **Same-statement changes to several tables** (writable CTEs): each changed table maps its own
  `OLD ∪ NEW` against post-statement state. The test suite must pin this, including a
  foreign-key move in an intermediate table.

## Consequences

- Silent staleness from unmodelled shapes is gone. What remains is explicit and governed by policy.
- Bulk and multi-hop writes get cheaper. Single-row writes cost the same.
- New `unsafe` code walks PostgreSQL nodes. It is confined to one module and covered by SQL
  regression tests on PG16, 17 and 18. It can't be unit-tested without a backend.
- `pg_tview_meta.cascade_paths` gains a successor representation. The flush-time propagation
  prune (`queue/graph.rs`), rename rebinding (`rebind_cascade_paths`) and fan-out read it, so
  the upgrade re-registers every TVIEW (`pg_tviews_reregister_all`) once.
- Tables read only inside functions the view calls stay invisible to both `pg_depend` and the
  analyzer. The analyzer warns about non-immutable function calls. Closing that gap is separate work.
