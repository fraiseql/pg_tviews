# Supported shapes

Which definitions pg_tviews maintains, how, and which it refuses. Each row names the
tests that prove it: regression tests under `test/sql/regress/`, and the TVIEW of the
same shape in the differential suite (`test/sql/differential/schema.sql`), whose random
writes check after every statement that each TVIEW equals its view
([its README](../../test/sql/differential/README.md)).

## How a shape is maintained

- **Row by row**: a write refreshes the TVIEW rows it reaches, at the end of the
  statement. The key is read off the changed row (`local`), found by a query over the
  statement's changed rows (`mapped`), or reached through an embedded TVIEW's refresh
  (`propagated`); `tviews.registry.cascade_kinds` shows which, per table
  ([How a write finds the TVIEW rows to
  refresh](ddl.md#how-a-write-finds-the-tview-rows-to-refresh)).
- **Through the policy**: a table nothing links to the key is `all_keys`, and the
  TVIEW's `uncascaded_policy` (or `uncascaded_tables` for that table) decides, at create
  time: `error`, the default, refuses the definition (`22023`, the hint says what to
  declare); `full_refresh` refreshes the whole TVIEW at the flush of a write to that
  table; `warn` leaves the rows stale until something else refreshes them ([Tables no
  cascade reaches](ddl.md#tables-no-cascade-reaches)). The definition's other tables
  stay row by row.
- **Refused**: nothing is created, with the SQLSTATE given.

## Shapes

| Shape | Example | Maintained | Evidence |
|---|---|---|---|
| Plain projection of one table | `SELECT pk_post, id, jsonb_build_object(…) AS data FROM tb_post` | row by row (`local`) | `refresh/regress_incremental_refresh.sql`; differential `tv_customer` |
| Joins (inner, outer, chains, non-equality conditions) | `FROM tb_line l JOIN tb_order o ON o.pk_order = l.fk_order JOIN tb_customer c …` | row by row (`mapped`): one mapping query per statement; a row moved to another parent refreshes both | `lineage/regress_key_mapping_queries.sql`, `lineage/regress_outer_join_links.sql`, `cascade/regress_parent_change.sql`; differential `tv_purchase`, `tv_line` |
| Copied parent columns | `'author', jsonb_build_object('name', u.name)` over a join on `fk_user` | row by row: the changed value is written into every row holding the parent's key, in one statement | `cascade/regress_fanout_patch.sql`, `cascade/regress_nested_fanout.sql` |
| Embedded TVIEW (cascade) | `JOIN tv_user u ON u.pk_user = p.fk_user`, `'author', u.data` | row by row (`propagated`): a refresh of `tv_user` refreshes the posts embedding it, in the same flush | `cascade/regress_refresh_cascades_to_dependents.sql`, `cascade/regress_embeds_by_oid.sql`; differential `tv_deal`, `tv_desk`, `tv_note` |
| Another TVIEW's table read otherwise (an aggregating view over `tv_line`) | `LEFT JOIN v_line_totals t ON t.fk_order = o.pk_order` | row by row (`mapped`), like a base table; a read nothing links goes through the policy | `lineage/regress_tview_table_reads.sql`; differential `tv_ordsum` |
| Aggregate (`GROUP BY`) | `SELECT o.fk_user AS pk_user_summary, … GROUP BY o.fk_user, …` | group by group: each touched group is recomputed. The `group_keys` option declares the column of each table that is the group key, checked against the query tree; with it, a window function, a set operation or a key that is not a plain `GROUP BY` column is refused (`22023`) | `cascade/regress_aggregate_tviews.sql`, `cascade/regress_aggregate_query_tree.sql`, `cascade/regress_aggregate_embed.sql`; differential `tv_custstat`, `tv_nsrf` |
| `DISTINCT ON` | `SELECT DISTINCT ON (c.id_contract) c.id_contract AS pk_contract, …` | row by row, keyed on the `DISTINCT ON` key ([ADR 0169](../adr/0169-tview-row-identity.md)); a row changing its key refreshes the old and the new group. A composite key, an expression or a key not projected is refused (`0A000`) | `distinct_on/regress_distinct_on_key_change.sql`, `distinct_on/regress_distinct_on_key_types.sql`, `distinct_on/regress_distinct_on_composite.sql`; differential `tv_order`, `tv_contract`, `tv_doc`, `tv_lastline` |
| `UNION` / `UNION ALL` | `SELECT pk_task, … FROM tb_task UNION ALL SELECT -pk_task, … FROM tb_task_archive` | row by row, branch by branch, each branch keyed by a column or immutable expression of its own table. Two rows for one key fail with `21000` (at the create, the write, the refresh) | `lineage/regress_union_cascade.sql`, `lineage/regress_union_keys.sql`, `lineage/regress_union_duplicate_keys.sql`; differential `tv_task`, `tv_anytask` |
| `INTERSECT` / `EXCEPT` | `(SELECT … FROM tb_a) INTERSECT (SELECT … FROM tb_b)` | row by row, branch by branch | `lineage/regress_top_level_opaque_shapes.sql` |
| CTE (`WITH`) | `WITH ids AS (SELECT … FROM tb_node n) SELECT … FROM ids x JOIN …` | row by row through the CTE: chains, multi-table bodies, set operations | `lineage/regress_cte_cascade.sql`, `lineage/regress_complex_cte.sql`, `lineage/regress_cte_levels.sql`; differential `tv_ncast` |
| Recursive CTE (`WITH RECURSIVE`), in the definition or a view | a category path read by an item view | the tables read inside it go through the policy (`full_refresh` for a small tree); the tables outside stay row by row | `lineage/regress_recursive_cte.sql`; differential `tv_item` |
| Correlated subqueries, `LATERAL` | `(SELECT jsonb_agg(…) FROM tb_comment c WHERE c.fk_post = p.pk_post)` | row by row when a condition links the subquery to the key | `lineage/regress_key_mapping_queries.sql`, `lineage/regress_statement_mapping.sql`, `arrays/regress_array_path_joins.sql`; differential `tv_nlat` |
| Uncorrelated subquery | `(SELECT count(*) FROM tb_flag)` | through the policy | `policy/regress_uncascaded_policy_default.sql`, `policy/regress_uncascaded_policy_option.sql` |
| Arrays of keys | `a.pk_node = ANY (<array>)`, `unnest(<array>)`, `LATERAL unnest(…)` | row by row, mapped through `<array> @> ARRAY[<key>]` | `arrays/regress_array_path_joins.sql`, `arrays/regress_unnest_cast.sql`; differential `tv_nsrf`, `tv_nlat`, `tv_nany`, `tv_ncast` |
| Join on a computed column | `x.code = s.upper_name`, `upper(n.name) AS upper_name` in a subquery | row by row when the expression is immutable; a volatile or stable one goes through the policy | differential `tv_nbadge` |
| Window function partitioned by a linked column ("first row per key") | `row_number() OVER (PARTITION BY o.fk_customer …) … WHERE rn = 1` | row by row: a write refreshes the partitions it leaves and enters | `lineage/regress_window_partition.sql`, `lineage/regress_first_row_join.sql`; differential `tv_custfirst`, `tv_custline` |
| Other windows, `LIMIT`/`OFFSET`, `GROUPING SETS`, a set-returning function in the top-level `SELECT` | `count(*) OVER ()`, `ORDER BY … LIMIT 10` | through the policy | `lineage/regress_top_level_opaque_shapes.sql`, `lineage/regress_window_partition.sql` |
| Time-dependent (`now()`, `CURRENT_DATE`, …) | `end_date >= CURRENT_DATE AS is_current` | row by row on writes; refreshed whole by `pg_tviews_refresh_time_dependent()`, called at the boundary, when declared `"time_refresh": "external"`. Undeclared, refused (`22023`) under `error` and `full_refresh` | `policy/regress_time_dependent.sql` |
| Function that reads tables (not `IMMUTABLE`, outside `pg_catalog`) | `name \|\| label_suffix()` | the tables declared in `function_reads` go through the policy. Undeclared, refused (`22023`) under `error` and `full_refresh` | `policy/regress_function_reads.sql`; differential `tv_custbadge` |
| Materialized view | `LEFT JOIN mv_line_count m ON m.fk_order = o.pk_order` | through the policy; under `full_refresh`, `REFRESH MATERIALIZED VIEW` refreshes the whole TVIEW in the same transaction | `policy/regress_materialized_view.sql`; differential `tv_ordcount` |
| Foreign table, sequence | `JOIN tb_remote r …`, `SELECT last_value FROM s_counter` | through the policy, except that `full_refresh` is refused (`22023`): nothing tells pg_tviews when they change. Declare `warn` and call `pg_tviews_refresh()` | `ddl/regress_untrackable_sources.sql` |
| Table read with its inheritance children (no `ONLY`) | `FROM tb_animal` with a child table | refused (`0A000`); `FROM ONLY` is row by row, and the table gets no new child | `ddl/regress_untrackable_sources.sql` |
| Partitioned base table | `tb_event` partitioned by range | row by row through a row trigger on the root; every partition gets the flush and `TRUNCATE` triggers, so writes naming a partition refresh too. `DROP`, `DETACH` and `ATTACH` of a partition refresh the rows it carries | `refresh/regress_partition_direct_write.sql`, `refresh/regress_partitioned_entity_table.sql`, `ddl/regress_partition_truncate_attach.sql`, `ddl/regress_issue_261_partition_drop.sql` |
| Generated columns | `code text GENERATED ALWAYS AS (upper(name))` | `STORED`: an ordinary column. Virtual (PostgreSQL 18's default): followed through its inputs, also as a key or join column | `lineage/regress_virtual_generated_columns.sql`; differential `tv_vnote` |
| Table a logical replication subscription writes | a TVIEW over a subscribed table | refused (`0A000`): the apply worker fires no statement trigger. Create the TVIEW on the publisher and publish its table | `test/replication/logical_subscriber.sh` |
| `pk_<entity>` not an integer | `SELECT pk_x, …` with `pk_x uuid` | refused (`42804`): keep the uuid in `id` ([Keys](ddl.md#keys)) | `identity/regress_key_type.sql` |
| No `pk_*` column | `SELECT * FROM tb_thing` | rewritten to `pk_<entity>, id, data` from a `pk` column, else the first integer column, else `id`; with none, refused (`42601`) | [Columns](ddl.md#columns) (not covered by a regression test) |
| TVIEWs reading each other in a cycle | `tv_a` reads `tv_b`, which reads `tv_a` | refused (`42P17`) | `operations/regress_error_codes.sql` |
| Deep nesting | views, subqueries and CTEs more than 32 query levels deep; views more than 64 deep | refused (`0A000`; `54001`) | `lineage/regress_recursive_cte.sql` (33 levels) |
| Not exactly one `SELECT` | two statements, a `DO` block | refused (`42601`) | `operations/regress_error_codes.sql` |
| Temporary table | `FROM pg_temp.tb_scratch` | refused by PostgreSQL (`cannot create temporary relation in non-temporary schema`) | not covered by a regression test |

## Writes the rows above do not cover

- An `UPDATE` that changes no column the TVIEW reads refreshes nothing; one that
  changes only columns copied into `data` as they are patches the documents in place
  instead of recomputing them (`direct_patch/` tests).
- `TRUNCATE` of a base table refreshes the whole TVIEW
  (`lineage/regress_statement_mapping.sql`,
  `transactions/regress_truncate_in_writing_statement.sql`).
- `COPY FROM` refreshes like an `INSERT` (`transactions/regress_copy_from.sql`).
- A trigger of yours that writes base tables, on a base table or on a `tv_*` table:
  [Firing order](ddl.md#firing-order).

## See also

- [DDL Reference](ddl.md): each shape in detail
- [Aggregate TVIEWs](../user-guides/aggregate-tviews.md)
- [Embed or compose](../user-guides/embed-or-compose.md)
