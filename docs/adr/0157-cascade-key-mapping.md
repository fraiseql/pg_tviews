# ADR 0157: Map base-table writes to TVIEW keys from PostgreSQL's query tree

- Status: Accepted; superseded in part by [ADR 0169](0169-tview-row-identity.md) (DISTINCT ON keys);
  amended for #182 and #183 (see [Amendment](#amendment-182-183-arrays-computed-columns-recursion))
  and for the default policy (see [Amendment](#amendment-untraceable-reads-fail-at-create)),
  and for #187, #188 and #189 (see [Amendment](#amendment-187-188-189-window-partitions-union-branch-keys-materialized-views)),
  and for #191 (see [Amendment](#amendment-191-reads-of-another-tviews-table))
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
   - `Local(col)`: the TVIEW key is a column of the changed row: the root table, or a table linked
     to the key by one equality `T.col = <key>`, in a join, a correlated subquery or a view's
     `GROUP BY` key (#157, #158). Mapped inside the row-level trigger without SQL. The
     direct-patch (#56) fast path keeps working unchanged.
   - `Mapped(sql)`: a generated key-mapping query over the changed rows (chains of joins,
     non-equality correlations). A table linked by one equality onto a projected root column
     (`u.pk_user = p.fk_user`) keeps the fan-out patch (#120).
   - `Propagated(entity)`: read through the `v_<entity>` of a TVIEW this one embeds
     (`fk_<entity>` or an aggregate embed) that maps the table itself. Entity propagation at flush
     already refreshes the embedding rows, with its patch and prune optimisations; nothing more is
     installed.
   - `AllKeys`: no selective predicate (an uncorrelated subquery, a window function, `LIMIT`, a
     join on a column computed by a volatile expression, a recursive CTE). Handled by `pg_tviews.uncascaded_policy` (`warn` | `error` |
     `full_refresh`), stored per TVIEW at create time. This includes a window function,
     `LIMIT`/`OFFSET`, a set-returning function or `GROUPING SETS` in the backing view's own
     SELECT (or a set-operation branch): such a level gets no key root, since a write changes rows
     other than its own (`count(*) OVER ()`, the rows a `LIMIT` keeps).
   A table is `AllKeys` when any read of it is, but its traceable reads keep their mapping query,
   which runs under every policy but `full_refresh` (#162). An outer join's equality maps a
   preserved row toward the nullable side; the path then ends at the key or goes on by an equality
   (#165). In a `GROUP BY` or `DISTINCT ON` level, a column equal to a key column through such an
   equality passes through like the key (#162).
   Registration fails when the analyzer and `pg_depend` disagree on the tables the view reads;
   tables read only by an unused CTE or behind view columns nothing reads are accepted and not
   tracked (#163, #166).
   A predicate the analyzer cannot write in SQL is left out, which only widens a mapping.
2. `Mapped` and `AllKeys` tables get **statement-level triggers** with transition tables, one per
   event (`INSERT`: `NEW TABLE`; `UPDATE`: both; `DELETE`: `OLD TABLE`), which run the cached
   mapping query and bulk-enqueue the keys (`AllKeys` under `full_refresh`: the whole TVIEW).
   Row-level triggers stay only on `Local` tables. Every table but a `Propagated` one also gets an
   `AFTER TRUNCATE` trigger that refreshes the whole TVIEW.
3. The text-based extraction, the per-hop `spi_batch_lookup` loop and the dead statement handler
   are removed. `sqlparser` remains for create-time text handling that has no query tree: DISTINCT
   ON keys, embed lookups, fan-out field maps and column-rename rewriting.

### Constraints this relies on

- **PostgreSQL 16+ only.** The `Query`, `RangeTblEntry`, `SubLink` and `Var` fields the analyzer
  reads are the same in 16, 17 and 18 (the `RTEPermissionInfo` split landed in 16). CI runs the
  regression suite on all three.
- **Transition-table limits:**
  - `TRUNCATE` has no transition table. It maps to `AllKeys` for every dependent TVIEW.
  - PostgreSQL copies only row triggers onto partitions (a trigger with transition tables is
    not copied), and a statement trigger fires only on the table the statement names: the root's
    transition tables miss a statement that names a partition. A partitioned `Mapped` table
    therefore keeps a row trigger on the root (copied to its partitions) that runs the mapping
    query per row, and every partition gets the flush and `TRUNCATE` triggers of its own,
    including partitions created or attached later (the `ProcessUtility` hook).
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
- `pg_tview_meta.key_mappings` is the successor representation; `cascade_paths` keeps only the
  zero-hop paths of `Local` tables, derived from it. The flush-time propagation prune
  (`queue/graph.rs`) and fan-out read the one-hop `Mapped` entries. Mapping queries are stored as
  templates naming relations and columns by OID and attribute number, so renames don't break them,
  and restore rebinds them. The upgrade marks every TVIEW for re-registration
  (`pg_tviews_reregister_all`); until then a TVIEW's stored multi-hop paths refresh it in full.
- Tables read only inside functions the view calls stay invisible to both `pg_depend` and the
  analyzer. The analyzer warns about non-immutable function calls. Closing that gap is separate work.

## Amendment (#182, #183): arrays, computed columns, recursion

Equivalent spellings of one condition must classify alike: a hierarchy stored as a path of
ids joined to its ancestors with `= ANY (<array>)`, a `LATERAL unnest(<array>)` or an
`unnest` in a subquery's select list classified three different ways, and none of them mapped
the ancestor read (#182). A view reading a recursive view was refused with a depth error
(#183).

- **Opacity per output.** A set-returning function in a subquery's select list multiplies rows
  but leaves the other columns as they are: only its own output is opaque. A window function,
  `LIMIT`/`OFFSET` and `GROUPING SETS` stay level-wide, and in the backing view's own SELECT a
  set-returning function still leaves the level without a root.
- **Computed outputs.** An output computed by an immutable expression of base columns resolves
  to that expression written over them (`Resolved::Expr`). A predicate on it embeds the
  expression; it is never a key, a group key or a root. A volatile expression, an aggregate or
  anything the deparser does not write stays opaque, so the `AllKeys` case "join on a computed
  column" narrows to those.
- **Array membership.** `x = ANY (<array>)` is a conjunct. An output `unnest(<array>)` (select
  list, or alone in `FROM` without ordinality) stands for an element of the array: `=` with it
  is written `x = ANY (<array>)`; any other use links nothing. With the element type's own
  equality the conjunct is also written `<array> @> ARRAY[x]`, which the membership implies and
  a GIN index serves; the create-time notice for a mapping that scans a table by an expression
  prints the `CREATE INDEX` (GIN for an array, btree for a scalar expression).
- **Strictness.** Predicates were kept only when strict, so that a NULL-extended row could not
  satisfy them. `= ANY` over a non-constant array and functions such as `string_to_array` are
  not strict. A non-strict predicate is now kept when none of the occurrences it reads is on the
  nullable side of an outer join below it; computed outputs carry their own strictness.
- **Recursive CTEs.** The recursive term's reference to its own CTE is opaque, and a recursive
  CTE is walked once with opaque outputs: the tables read inside it are `AllKeys` ("read in a
  recursive CTE (<view>)"), and the tables read outside keep their mapping. The text check that
  refused `WITH RECURSIVE` in a definition is removed.

## Amendment: untraceable reads fail at create

`AllKeys` tables were handled under `pg_tviews.uncascaded_policy`, `warn` by default: a
TVIEW whose writes to some table refresh nothing was created with a WARNING. An agent
building a schema fixes ERRORs in its loop and does not read WARNINGs, and a session
setting is state a definition file must set first.

- The default is now **`error`**: such a TVIEW is refused at create. The message names
  each table with its reason; the HINT gives the exact option to declare and, for
  `CREATE TABLE … AS` and `pg_tviews_create()`, the setting to use.
- The policy is **declared with the TVIEW**: `pg_tviews_create_or_replace(…, options =>
  '{"uncascaded_policy": "full_refresh"}')` (ADR 0136, amendment); the setting remains
  for the entry points that take no options. Existing TVIEWs keep their stored policy.
- An opaque top level still has no root (a top-level window function or `LIMIT` is not
  an entity projection): such a TVIEW is refused unless it declares a policy.
- Reads through another TVIEW's backing view are recognised by the view's OID, whatever
  its name (`Propagated`); a definition now usually reads the other TVIEW's `tv_<entity>`
  table, which is not a base table.

## Amendment (#187, #188, #189): window partitions, UNION branch keys, materialized views

Three shapes of real read models were refused or left stale: "the first row per key"
written with `ROW_NUMBER() OVER (PARTITION BY <linked key>)` was `all_keys` where the
`DISTINCT ON` spelling was traced (#187); a UNION of two entities with their own key
spaces had no root for a branch keyed by an expression, nor for any branch when the
UNION sat in a view (#188); a materialized view the definition read had no kind at
all, so no policy saw it and `REFRESH MATERIALIZED VIEW` left the TVIEW stale (#189).

- **Window partitions.** A level whose window functions all have a `PARTITION BY` is
  not opaque: a row's window values come from the rows of its partition. An output
  column in every window's partition, or equal to one through an equality (#162),
  passes through like a `DISTINCT ON` key; every other output, window results
  included, is opaque. A window without `PARTITION BY` keeps the level opaque, and the
  top level still has no root: its window values come from other TVIEW rows.
- **Branch keys.** A root is a column of one occurrence, or an immutable expression
  of one occurrence's row (`-l.pk_order_line`, `pk + 1000000000`); the root
  occurrence is then `mapped`, its mapping query computing the key, and an equality
  with the key column stands for the root in other mapping queries when the
  expression reads nothing else of the root. A key computed from two occurrences is
  no root.
- **UNION scopes.** Occurrences carry the UNION leaves they sit in (`(union, leaf)`,
  outermost first), for a UNION in the definition or in a view or subquery. A UNION
  subquery's output stands for each branch's column or computed output (`Alt`), and
  remembers the branches where it is opaque. When the key comes from such an output,
  each branch gets its own root: a read inside a branch maps to that branch's root,
  a read outside every UNION maps to the roots of all branches (one mapping query
  each, combined), and a read that can meet a branch without a root is `all_keys`.
  Such a TVIEW is a union like a set-operation definition: rows are recomputed, and
  a key returned twice is refused by `union_duplicate_policy`, on the bulk refresh
  path too, as an ERROR that fails the write.
- **Materialized views.** A matview read is an occurrence (and a base table) that is
  always `all_keys`: no trigger can see its rows. The policy decides at create;
  under `full_refresh` the `ProcessUtility` hook refreshes the TVIEWs listing the
  matview among their uncascaded tables after `REFRESH MATERIALIZED VIEW` (plain or
  `CONCURRENTLY`, not `WITH NO DATA`) and flushes the queue. Mapping the refreshed
  rows' keys (a diff of the matview before and after) is not done.

## Amendment (#191): reads of another TVIEW's table

A TVIEW's table read by another TVIEW was opaque: only an equality on its
`pk_<entity>` was noticed, as an embed. Any other read, a view aggregating it by
another column for one, had no kind and no policy, and the reader went stale when
the inner TVIEW was refreshed.

- Such a read is an occurrence like a base table's, its columns resolved, so the
  walk links it through any predicate. Joined on the inner TVIEW's key by a TVIEW
  that embeds it (`fk_<entity>`, an aggregate embed), it is `Propagated`: entity
  propagation already refreshes the embedding rows. Otherwise it is `Mapped`, never
  `Local`, or `AllKeys` under the policy.
- The inner TVIEW's refreshes are its writes: the occurrence's table gets the
  three delta triggers, and no flush or `TRUNCATE` trigger, which would flush from
  inside the flush. They fire on the flush's own upserts and deletes and queue the
  outer TVIEW's keys, which the flush drains. `key_mappings` names the inner TVIEW,
  and the flush's dependency order refreshes it first, so the outer rows are
  recomputed from fresh inner rows, once.
