# DDL Reference

Creating, changing and dropping TVIEWs, and what a definition may contain.

## Overview

A TVIEW is a table `tv_<entity>` kept equal to its definition, a single `SELECT`, by
triggers on the tables the definition reads. pg_tviews reads the definition from
PostgreSQL's query tree ([ADR 0157](../adr/0157-cascade-key-mapping.md), [ADR
0203](../adr/0203-propagation-plan.md)): what each base table's writes refresh is
derived from the joins and conditions as PostgreSQL analyzed them, not from table or
column names.

## Creating TVIEWs

### `CREATE TABLE tv_<entity> AS SELECT`

```sql
CREATE TABLE tv_<entity> AS
SELECT
    <key>       AS pk_<entity>,  -- required: names the TVIEW's rows
    <columns>,                   -- optional: any other columns
    <jsonb>     AS data          -- optional: the JSONB read model
FROM <tables> …;
```

The `ProcessUtility` hook turns the statement into a TVIEW, also inside a `DO` block,
a function or a multi-statement batch. It needs the library loaded in the session
(`shared_preload_libraries`); a `tv_*` CTAS that reaches the server without being
intercepted fails, naming the table, instead of leaving a plain table behind. Forms
pg_tviews cannot turn into a TVIEW (`WITH NO DATA`, a temporary table, a column list,
`TABLESPACE`, …) fail with SQLSTATE `0A000` ([error reference](../error-reference.md)).

### `pg_tviews_create()` and `pg_tviews_create_or_replace()`

```sql
SELECT tviews.pg_tviews_create('tv_<entity>', $$ SELECT … $$);            -- returns text
SELECT tviews.pg_tviews_create_or_replace('tv_<entity>', $$ SELECT … $$,
                                          options => '{}');               -- created | unchanged | altered | replaced | rebuilt
```

`pg_tviews_create_or_replace()` is the one tools should call: it creates the TVIEW, or
makes the smallest change to an existing one, and takes the options below
(`logged`, `fillfactor`, `data_gin_index`, `group_keys`, `uncascaded_policy`,
`uncascaded_tables`, `function_reads`, `time_refresh`). Its contract is in
[Contract for tools](read-contract.md#tviewspg_tviews_create_or_replace).
`CREATE TABLE … AS` and `pg_tviews_create()` take no options: they read the settings
`pg_tviews.uncascaded_policy`, `pg_tviews.time_refresh`,
`pg_tviews.unlogged_by_default`, `pg_tviews.fillfactor` and
`pg_tviews.data_gin_index`.

A name that already exists fails with `42P07`; a definition that is not exactly one
`SELECT` with `42601`.

### Naming

The only naming rule is the key column. The definition outputs a column
`pk_<entity>`; the first `pk_*` column names the entity, and the TVIEW's table is
`tv_<entity>`. `pg_tviews_create('tv_post', …)` and `pg_tviews_create('post', …)` name
the same TVIEW; a name that does not match the key fails (`TVIEW tv_foo does not match
its definition, which is keyed on pk_post`, SQLSTATE `22023`).

Nothing else is matched by name:

- base tables may have any name (`orders`, not only `tb_order`);
- the columns that link tables may have any name: a write is mapped through the join
  conditions themselves;
- a TVIEW whose rows are another table's is accepted (`pk_order_summary` over
  `tb_order`);
- the column holding an embedded TVIEW's key may have any name (`author_pk`, below).

FraiseQL's trinity identifiers (`id` UUID, `pk_<entity>`, `identifier`, and
`<parent>_id`) are a convention that fits pg_tviews, not a requirement.

A TVIEW's rows are named by `pk_<entity>`, which is its table's primary key. A
`DISTINCT ON` TVIEW's rows are named by its `DISTINCT ON` key instead ([ADR
0169](../adr/0169-tview-row-identity.md)); `tviews.registry.identity` reports the
column.

**Backing view.** The definition is stored as a view `tviews.<schema>__tv_<entity>`
(in pg_tviews' own schema, named after the TVIEW's table and fitted to 63 bytes;
`tviews.registry.view` reports it). The application's own views are left alone: a
TVIEW can materialize one (`pg_tviews_create('tv_order', 'SELECT * FROM v_order')`) or
read views that read it. Whoever can `SELECT` from `tv_<entity>` can `SELECT` from the
backing view (see [Privileges](#privileges)).

**Embedding another TVIEW.** Read its table and join it on its key; the column that
holds the key may have any name:

```sql
CREATE TABLE tv_post AS
SELECT p.pk_post, p.id, p.author AS author_pk,
       jsonb_build_object('title', p.title, 'author', u.data) AS data
FROM tb_post p
JOIN tv_user u ON u.pk_user = p.author;
```

`tv_user` is then `propagated` in `tviews.registry.cascade_kinds`: a refresh of
`tv_user` refreshes the posts that embed it, in the same flush. Any other read of
another TVIEW's table, directly or through views (a view aggregating `tv_line` by
`order_id`, a correlated subquery on a column other than its key), is traced like a
read of a base table; a read nothing links to the key goes through the
`uncascaded_policy`.

### Columns

Every column of `tv_<entity>` has the type of the backing view's column, typmod
included (an enum, a domain, a composite, an array, `varchar(5)`, `numeric(6,2)`),
except columns whose names give them a fixed type:

| Column | Type | Notes |
|---|---|---|
| `pk_<entity>` (the first `pk_*`) | `bigint` | the primary key, unless the TVIEW is `DISTINCT ON`; the definition gives it as `smallint`, `integer`, `bigint` or a domain over one, else it is refused (`42804`) |
| `id` | `uuid` | `NOT NULL`, indexed |
| `data` | `jsonb` | optional |
| `fk_*` | `bigint` | indexed with `pk_<entity>` |
| `*_id` | the view's type | indexed |

The table's columns come in this order: `pk_<entity>`, `id`, `identifier`, `data`,
`fk_*`, `*_id`, then the others in definition order; pg_tviews adds `created_at` and
`updated_at` (`timestamptz`). Name columns in queries rather than relying on
`SELECT *` order.

A definition with no `pk_*` column is rewritten to the shape `pk_<entity>, id, data`:
its column `pk`, else its first integer column, else `id`, becomes `pk_<entity>`, `id`
is generated, and every column goes into `data` under its own name. With none of
these, it fails (`no column to key the rows on`, `42601`). `tviews.registry.query`
shows the definition as stored.

The table is `UNLOGGED` unless `pg_tviews.unlogged_by_default` is off or the
`logged` option is set; its fillfactor is `pg_tviews.fillfactor` (85) unless the
`fillfactor` option is set.

Because the backing view's columns depend on their types, `DROP TYPE … CASCADE` of a
type the view returns drops the view, and pg_tviews then drops the whole TVIEW (its
table, triggers and registration), as when a base table is dropped with `CASCADE`.

### Example

```sql
CREATE TABLE tv_post AS
SELECT
    p.pk_post,
    p.id,
    p.fk_user,
    u.id AS user_id,
    jsonb_build_object(
        'id', p.id,
        'title', p.title,
        'author', jsonb_build_object('id', u.id, 'name', u.name),
        'comments', COALESCE(
            (SELECT jsonb_agg(jsonb_build_object('id', c.id, 'text', c.text)
                              ORDER BY c.pk_comment)
             FROM tb_comment c WHERE c.fk_post = p.pk_post),
            '[]'::jsonb)
    ) AS data
FROM tb_post p
JOIN tb_user u ON u.pk_user = p.fk_user;
```

### What happens during creation

1. PostgreSQL parses and analyzes the definition (exactly one `SELECT`), under the
   caller's `search_path`; `SELECT *` is written out with its columns.
2. The backing view `tviews.<schema>__tv_<entity>` is created, owned by the caller.
3. pg_tviews reads the view's query tree: the column that names the rows, and how a
   write to each table it reads maps to those rows. Definitions it cannot maintain
   are refused here, before anything else is created.
4. The table `tv_<entity>` is created and filled from the view.
5. The TVIEW is registered in `tviews.pg_tview_meta`, with what step 3 derived stored
   as one propagation plan (`plan`); tools read it through `tviews.registry`
   ([Contract for tools](read-contract.md)).
6. Triggers are installed on the tables the definition reads (see
   [Triggers](#triggers)).

All of it happens in the caller's transaction: a rollback leaves nothing behind.

### Supported SQL features

- **JOINs**: INNER, LEFT, RIGHT, FULL OUTER
- **Aggregations**: GROUP BY, HAVING, jsonb_agg(), array_agg()
- **Expressions**: CASE, COALESCE, NULLIF, FILTER
- **Subqueries** in the SELECT list or WHERE (`(SELECT …)`, `ARRAY(SELECT …)`,
  `EXISTS`, `IN (SELECT …)`), `LATERAL`, and plain views (with `GROUP BY` too):
  writes to the tables they read cascade when a condition links them to the TVIEW
  key (`l.fk_order = o.pk_order`, `l.pos > o.min_pos`). An uncorrelated subquery
  links nothing: see [Tables no cascade reaches](#tables-no-cascade-reaches)
- **Outer joins**: a table on the preserved side of a `LEFT`/`RIGHT JOIN` is linked
  through the nullable side when the key is on that side or the path goes on from it
  by an equality (a view
  `tb_line l LEFT JOIN tb_order o ON l.fk_order = o.pk_order` exposing `o.id AS
  order_id`, read by the TVIEW with `v.order_id = t.id`): a row with no match yields
  NULLs there and matches no key
- **`GROUP BY` / `DISTINCT ON` views**: a view column passes through when it is a
  grouping or `DISTINCT ON` key, or equal to one through a join condition
  (`DISTINCT ON (l.fk_order) o.pk_order` with `l.fk_order = o.pk_order`): its value
  is the key's on every row that can match
- **Window functions partitioned by a linked column**, in a view or subquery: a
  row's window values come from the rows of its partition, so a column in every
  window's `PARTITION BY` passes through like a `DISTINCT ON` key, and the other
  columns don't. "The first row per key", `ROW_NUMBER() OVER (PARTITION BY
  o.fk_customer ORDER BY …) … WHERE rn = 1` joined on `fk_customer`, refreshes the
  partitions a write leaves and enters, as `DISTINCT ON (o.fk_customer)` does; so do
  `RANK`, `DENSE_RANK`, `FIRST_VALUE` and other window functions, whatever filters
  their output. A table joined to **another** column of that first row (the product
  of each customer's first order, `LEFT JOIN tb_product p ON p.pk_product =
  f.fk_product`) is mapped in two hops: a write to it reaches every row of the
  subquery's table carrying it (a superset of the first rows), then their partition
  or `DISTINCT ON` key. The subquery's own table maps only through that key: a write
  that changes which row is first changes the partition it is in, while the other
  column can belong to a row that was not written. So a table linked to the TVIEW
  key *only* through such a column of its own first-row level stays `all_keys`
- **View columns the TVIEW doesn't read**: a view, subquery or CTE is followed only
  for the columns read from it (in the select list, `WHERE`, joins, or through a
  whole-row reference). The tables behind the other columns get no trigger, so
  writes to them cost nothing. A column used for sorting, grouping or `DISTINCT`,
  or returning a set, always counts
- **Functions**: jsonb_build_object(), jsonb_array_elements(), etc.
- **Operators**: Standard PostgreSQL operators
- **UNION / UNION ALL**: incremental refresh cascades to every branch's base
  table. Each branch derives `pk_<entity>` from its own table: a column, or an
  immutable expression of that table's row when two entities have their own key
  spaces (`p.pk_product` in one branch, `-l.pk_order_line` or
  `l.pk_order_line + 1000000000` in the other). The UNION may be the definition
  itself or a view it reads (`SELECT v.pk_attachment, … FROM v_attachment v`): a
  write to a branch's table refreshes that branch's keys, and a table joined to
  the union's output refreshes the keys of every branch. Branch keys must be
  disjoint: overlapping ones fail the create (duplicate key) and, later, the
  write that makes two rows share a key (`pg_tviews.union_duplicate_policy`,
  `error` by default). A key computed from two tables (`COALESCE(p.pk_product,
  -l.pk_order_line)` over two outer joins) is no branch table's: put it in the
  branches instead. A key taken from one branch's table through an inner join
  holds only that branch's rows, as the definition says. A branch keyed by an
  expression is refreshed by filtering on it: an index on the expression
  (`CREATE INDEX ON tb_order_line ((-pk_order_line))`) keeps that cheap
- **INTERSECT / EXCEPT**: maintained branch by branch like UNION. A refresh
  recomputes the view's row for each changed key, and both operators compare whole
  rows, key included, so a row enters or leaves the TVIEW as the set operation says
- **CTEs (`WITH`)**: cascade paths resolve through a CTE whose body reads one or
  several joined base tables, reads earlier CTEs (a chain of any length) or
  subqueries in its `FROM`, or is a set operation. A CTE the view defines but never
  uses is accepted; the tables it reads get no trigger
- **Computed columns**: a view, subquery or CTE column computed by an immutable
  expression of base columns (`upper(n.name) AS code`) links like the expression
  itself: a join on it (`x.code = s.code`) maps writes through `x.code =
  upper(n.name)`. A column computed by a volatile or stable expression (`now()`,
  `random()`) links nothing
- **Arrays of keys**: `a.pk_node = ANY (<array>)`, a join on `unnest(<array>)` in a
  subquery's or CTE's select list, and `LATERAL unnest(<array>)` are the same
  condition: the array's element equals the key. A write to the joined table maps
  through `<array> @> ARRAY[<key>]`, which a GIN index on the array expression
  serves; the create-time notice names it (below). A cast of the element,
  `unnest(string_to_array(n.path, '.'))::bigint` or `u.x::bigint` for `LATERAL
  unnest(…) AS u(x)`, is an element of the cast array,
  `(string_to_array(n.path, '.'))::bigint[]`, and the index goes on that
- **Window functions, `LIMIT`/`OFFSET`, `GROUPING SETS`**: accepted, but a write to a
  table read under one of them can change rows other than its own (a window without
  `PARTITION BY`, or partitioned by a column nothing links to the key; any window in
  the backing view's own SELECT), so the table is
  `all_keys` and the TVIEW's `uncascaded_policy` decides (see [Tables no cascade
  reaches](#tables-no-cascade-reaches)). The same holds for a set-returning function
  in the backing view's own select list. In a subquery's select list a set-returning
  function only multiplies rows: the other columns pass through, and an `unnest`
  output is an array element (above)
- **Materialized views**: a materialized view the definition reads (directly or
  through a view) is `all_keys`: `REFRESH MATERIALIZED VIEW` replaces its rows and
  no trigger sees them. Under the default `error` policy the TVIEW is refused;
  under `full_refresh`, `REFRESH MATERIALIZED VIEW` (plain or `CONCURRENTLY`)
  refreshes the TVIEW in full, in the same transaction; under `warn` the TVIEW
  keeps the rows it had. `REFRESH … WITH NO DATA` makes the matview unreadable and
  refreshes nothing. No trigger is installed on a matview
- **Recursive CTEs (`WITH RECURSIVE`)**, in the definition or in a view it reads: a
  row of a recursive CTE comes from rows of the step before, so the tables read
  inside it are `all_keys` (`read in a recursive CTE (public.v_category_path)`) and
  the tables read outside it keep their mapping. A small lookup tree read by a large
  entity view is the usual case: create the TVIEW with `uncascaded_policy =
  'full_refresh'`, and the entity's own writes stay incremental
- **DISTINCT ON**: deduplicated read models, keyed on their `DISTINCT ON` key
  ([ADR 0169](../adr/0169-tview-row-identity.md)): its value names the TVIEW's
  rows, it is the table's primary key, and `tviews.registry.identity` reports it.
  - The key is a column, projected (`DISTINCT ON (o.id) o.pk_order, o.id …`; it may
    be aliased, `DISTINCT ON (c.id_contract) c.id_contract AS pk_contract`) or equal
    through a join condition to a projected column (`DISTINCT ON (l.fk_order)
    o.pk_order` with `l.fk_order = o.pk_order`). Its type can be anything (`bigint`,
    `uuid`, `text`, `numeric`, `date`, a quoted mixed-case column…).
  - Tables read through joins are followed like any TVIEW's, whatever the key.
  - Writes are followed from the old and the new row: a row that changes its key
    leaves its old group and joins the new one, and a statement writing several
    groups refreshes each of them. The refresh filters on the key with its type, so
    PostgreSQL reaches the base table's index through the `DISTINCT ON`.
  - `pk_<entity>` is still required: it names the entity. It is an ordinary column
    here and may repeat; parents that embed the TVIEW join its table on it and follow
    the winning row when it changes.
  - Refused at create: a composite key (`DISTINCT ON (a, b)`: a TVIEW row is one
    entity with one key; model "one row per (a, b)" as an entity of its own), and a
    key that is an expression, or a column not projected that no projected column
    equals. The message names the key.

- **Generated columns**: `STORED` columns are ordinary columns. A **virtual**
  generated column (PostgreSQL 18's default) has no value in the rows a trigger
  sees, so pg_tviews follows its inputs instead: a TVIEW reading `code = upper(name)`
  refreshes when `name` changes. A key or join on a virtual column is mapped by
  computing the column from its expression over the changed rows, and the direct
  and fan-out patches never copy a virtual column or one of its inputs.

### Refused definitions

A definition is refused, nothing created, when:

- it is not exactly one `SELECT` (`42601`);
- it has no column to key the rows on, or its key does not match the TVIEW's name
  (above);
- its `DISTINCT ON` key is composite, an expression, or not projected (`0A000`);
- it would make TVIEWs read each other in a cycle (`42P17`), or nests views deeper
  than `pg_tviews.max_dependency_depth` (10; `54001`);
- it reads a table no cascade reaches, calls a function that may read tables, or reads
  the current time, without declaring what to do about it (`22023`; see the next
  sections).

The [error reference](../error-reference.md) lists every SQLSTATE.

### How a write finds the TVIEW rows to refresh

When a TVIEW is created, pg_tviews reads PostgreSQL's query tree of its backing view
(views, CTEs, subqueries and `UNION` branches included) and records, per base table,
how a changed row maps to TVIEW keys (`tviews.registry.cascade_kinds`): the key is a
column of the row (`local`), a generated query over the changed rows finds it
(`mapped`, for chains of joins and non-equality conditions), a TVIEW it embeds
refreshes it (`propagated`), or nothing selective links them (`all_keys`). A `mapped`
query that would scan a large table sequentially is reported at create time with
the index that avoids it:

```
NOTICE:  writes to public.tb_sku map to tv_order keys with a sequential scan of tb_line
         (about 20000 rows); an index on tb_line (fk_sku) would make them cheaper
```

A lookup by an expression (an array of keys, a computed column) names the index to
create:

```
NOTICE:  writes to public.tb_node map to tv_node keys with a sequential scan of tb_node
         (about 3000 rows); CREATE INDEX ON public.tb_node USING gin
         (((pg_catalog.string_to_array(path, '.'::pg_catalog.text))::bigint[])) would
         make them cheaper
```

`tviews.pg_tviews_mapping_query('tv_order', 'tb_sku'::regclass)` returns the query.

The triggers follow the kind of each table:

| Kind | Triggers on the base table (per TVIEW) | What a write does |
|---|---|---|
| `local` | row trigger | the key is read off each changed row (and its old image) |
| `mapped`, `all_keys` | three statement triggers with transition tables (`INSERT`, `UPDATE`, `DELETE`) | one mapping query over the statement's changed rows; under `full_refresh` an `all_keys` write refreshes the whole TVIEW |
| `propagated` | none | refreshing the embedded TVIEW refreshes this one |

Another TVIEW's table read as `mapped` or `all_keys` gets the three statement
triggers alone: they fire on the refreshes of that TVIEW, inside the flush, which
then refreshes the rows of this one they map to.

Every base table but a `propagated` one also gets an `AFTER TRUNCATE` trigger, which
refreshes the whole TVIEW. An `UPDATE` that changes none of the columns the TVIEW
reads from a `mapped` table maps nothing (rows are matched to their old image by
primary key).

**Partitioned tables.** A partitioned table of any kind but `propagated` gets the
row trigger, which PostgreSQL copies onto every partition, and maps each changed row
from it: the transition tables of a statement trigger on the partitioned table would
miss the rows of a statement that names a partition. Every partition, leaf or middle
level, also gets the flush and `AFTER TRUNCATE` triggers, because a statement
trigger fires only on the table the statement names. So a write or a `TRUNCATE` that
targets a partition directly refreshes the TVIEW like one through the partitioned
table, and a `TRUNCATE` of the partitioned table refreshes it once.

Partitions created (`CREATE TABLE … PARTITION OF`, also from a function such as a
partition manager's) or attached after the TVIEW get the same triggers, and a
detached one loses them. `ATTACH` and `DETACH` move rows in or out with no row
trigger firing, so each refreshes the TVIEWs over that table in full.
`pg_tviews_health_check()` reports a partition whose triggers are missing, and
`pg_tviews_reregister_all()` puts them back.

### Tables no cascade reaches

A table classified `all_keys` has no condition linking its rows to the TVIEW key,
so a write to it could change any row. It is reported when the TVIEW is created and
listed in `tviews.registry.uncascaded_tables`. Common shapes:

- an uncorrelated subquery (`(SELECT count(*) FROM tb_flag)` in every row);
- a window function, `LIMIT`/`OFFSET`, a set-returning function in the select list,
  or `GROUPING SETS`, in the backing view's own SELECT (`count(*) OVER ()` changes
  every row when one is inserted; `ORDER BY … LIMIT 10` changes which rows are in):
  the reason reads `read under a window function in the top-level SELECT`;
- a subquery or view whose rows are not passed through to the key: under a window
  function that is not partitioned by a linked column, `LIMIT`/`OFFSET` or
  `GROUPING SETS`, or joined on a column computed by a volatile or stable
  expression;
- a materialized view (`a materialized view: REFRESH MATERIALIZED VIEW replaces its
  rows without firing triggers`);
- the rows of a UNION branch whose key is not a column or expression of one of
  its tables (`the TVIEW key is not a column of a base table in every UNION
  branch`);
- a table read inside a recursive CTE (`read in a recursive CTE (…)`);
- a table read inside a function the definition calls, declared in
  `function_reads` (`read inside public.label_suffix()`, see [Functions that read
  tables](#functions-that-read-tables)).

A table read in several places is `all_keys` when one of them can't be traced, but
its other reads keep refreshing the rows they reach: a TVIEW's own table, read
where its key comes from, refreshes the row of each entity it writes. The reason
then ends with `the rows its other reads reach are still refreshed`, and the policy
decides only about the rest.

What happens is fixed per TVIEW by its `uncascaded_policy`, declared with the TVIEW
and stored with it:

| Policy | At create time | On a write to such a table |
|---|---|---|
| `error` (default) | `ERROR`: nothing is created; the HINT says what to declare | — |
| `full_refresh` | `NOTICE` | the whole TVIEW is brought up to date at flush, once per transaction or statement; unchanged rows are not rewritten |
| `warn` | `WARNING:  writes to public.tb_flag will not refresh public.tv_report (read in a subquery, with no condition linking it to the TVIEW key)` | nothing: the rows stay stale until a mapped table changes |

A TVIEW with such a table and no declared policy is refused:

```
ERROR:  writes to public.tb_flag would not refresh public.tv_report (read in a subquery, with no
        condition linking it to the TVIEW key): declare what such a write does with the TVIEW's
        uncascaded_policy
HINT:  To refresh public.tv_report in full on such writes: pg_tviews_create_or_replace(
       'public.tv_report', <definition>, options => '{"uncascaded_tables":
       {"public.tb_flag": "full_refresh"}}'), or for the whole TVIEW '{"uncascaded_policy":
       "full_refresh"}'; before CREATE TABLE … AS or pg_tviews_create(): SET
       pg_tviews.uncascaded_policy = 'full_refresh'. "warn" accepts stale rows instead. Or
       join the tables on a column pg_tviews can trace.
```

Declare it with the TVIEW:

```sql
SELECT pg_tviews_create_or_replace('tv_report', $$ … $$,
    options => '{"uncascaded_policy": "full_refresh"}');
```

`CREATE TABLE … AS` and `pg_tviews_create()` take no options: they read the setting
`pg_tviews.uncascaded_policy` (default `error`) instead.

```sql
SET pg_tviews.uncascaded_policy = 'full_refresh';
CREATE TABLE tv_report AS SELECT …;
RESET pg_tviews.uncascaded_policy;          -- the TVIEW keeps full_refresh
```

Changing the option of an existing TVIEW with `pg_tviews_create_or_replace()` is an
`altered` change: the policy is stored and the TVIEW re-registered, with no rebuild.

`full_refresh` recomputes every row of the TVIEW on each flush that wrote to such a
table, so its cost grows with the TVIEW. Use it for small TVIEWs or small reference
tables ([a policy per table](#a-policy-per-table)), or rewrite the definition so that
the table is joined on a column pg_tviews can trace.

#### A policy per table

A TVIEW that reads a locale table or a small reference list nothing links to the key
can give that table its own policy, and keep refusing every other untraced read:

```sql
SELECT pg_tviews_create_or_replace('public.tv_x', $q$ … $q$, '{
  "uncascaded_policy": "error",
  "uncascaded_tables": {"public.tb_locale": "full_refresh", "catalog.tb_currency": "full_refresh"}
}');
```

A write to a named table follows its policy; any other table no cascade reaches
follows `uncascaded_policy`, so a later edit of a view that adds an untraced read is
still refused. A named table the definition does not read, or whose writes it traces
(`local`, `mapped`, `propagated`), is refused, so the list cannot rot; so is one that
does not exist. `tviews.registry.uncascaded_table_policies` reports the map. Leaving
`uncascaded_tables` out of a later `pg_tviews_create_or_replace()` keeps the map;
passing another one is an `altered` change.

### Functions that read tables

A function the definition calls (directly, or in a view, subquery or CTE it reads)
that is not `IMMUTABLE` and lives outside `pg_catalog` may read tables pg_tviews
cannot see: a `STABLE` lookup of a setting, a translation. Writes to those tables
would leave the TVIEW stale, so under the `error` and `full_refresh` policies such a
call is refused unless it is declared, and under `warn` it is reported:

```
ERROR:  public.tv_contract calls public.label_suffix(), not immutable: the tables it reads are
        invisible to pg_tviews, and writes to them would not refresh public.tv_contract: declare
        them in function_reads
```

Declare the tables each function reads, naming the function with its argument types
(`[]` for a function that reads none, such as one reading a setting):

```sql
SELECT pg_tviews_create_or_replace('public.tv_contract', $q$
    SELECT pk_contract, id, name || label_suffix() AS label FROM tb_contract $q$, '{
  "function_reads": {"public.label_suffix()": ["public.tb_setting"], "public.app_tag()": []},
  "uncascaded_tables": {"public.tb_setting": "full_refresh"}
}');
```

The declared tables become tables the TVIEW reads that no cascade reaches
(`all_keys`, `read inside public.label_suffix()`): they get the statement triggers,
appear in `base_tables`, `uncascaded_tables` and `cascade_kinds`, and their policy
decides what a write does. Above, `tb_setting` refreshes the TVIEW in full and any
other untraced read is still refused. A table the view also reads directly keeps
mapping the reads of it that can be traced. A declared function the definition does
not call, or that does not exist, is refused; `tviews.registry.function_reads`
reports the declarations.

Refreshes run as the TVIEW's owner with `search_path = pg_catalog, pg_temp`: a
function a definition calls must qualify the tables it reads (`public.tb_setting`) or
`SET search_path` itself. pg_tviews sees calls, not function bodies: the time a
function reads (`now()` inside it) is not detected.

### Time-dependent TVIEWs

A definition that reads the current time (`CURRENT_DATE`, `CURRENT_TIME`,
`CURRENT_TIMESTAMP`, `LOCALTIME`, `LOCALTIMESTAMP`, `now()`, `clock_timestamp()`,
`statement_timestamp()`, `transaction_timestamp()`, `timeofday()`, one-argument
`age()`), directly or in a view, subquery or CTE it reads, has rows that change
with no write: `end_date >= CURRENT_DATE AS is_current` flips at midnight. Under the
`error` and `full_refresh` policies it is refused unless it declares who brings it
up to date; under `warn` it is created with a WARNING.

```sql
SELECT pg_tviews_create_or_replace('public.tv_contract', $q$
    SELECT pk_contract, id, name, end_date >= CURRENT_DATE AS is_current FROM tb_contract $q$,
    '{"time_refresh": "external"}');
-- CREATE TABLE … AS, pg_tviews_create():
SET pg_tviews.time_refresh = 'external';
```

`tviews.registry.time_dependent` reports such TVIEWs (also one created under `warn`),
and `time_refresh` the declaration. Writes refresh it as usual; at the boundary,
something outside calls

```sql
SELECT * FROM tviews.pg_tviews_refresh_time_dependent();               -- every one you own
SELECT * FROM tviews.pg_tviews_refresh_time_dependent('public.tv_contract');
```

which refreshes it in full, as a write to a `full_refresh` table does (the TVIEWs
reading it follow), and returns the TVIEWs refreshed. With pg_cron, just after
midnight:

```sql
SELECT cron.schedule('tviews-day', '1 0 * * *',
                     'SELECT tviews.pg_tviews_refresh_time_dependent()');
```

`time_refresh` on a definition that reads no time is refused; the setting is ignored
for one. A literal evaluated at run time (`'now'::timestamptz`) is not detected: pass
the date as data, or write `now()`.

### Rendering

A value's text depends on session settings: `to_jsonb(timestamptz)` on `TimeZone`,
`::text` of a date or time on `DateStyle` too, of an interval on `IntervalStyle`, of a
float on `extra_float_digits`, of a `bytea` on `bytea_output`. Every computation of a
TVIEW's rows (creation, refreshes on writes, `pg_tviews_refresh()`, the time refresh,
`create_or_replace`) runs under fixed values, whoever writes and from whatever session:

| Setting | Value |
|---|---|
| `TimeZone` | `UTC` |
| `DateStyle` | `ISO, YMD` |
| `IntervalStyle` | `postgres` |
| `extra_float_digits` | `1` |
| `bytea_output` | `hex` |

so a TVIEW's rows don't depend on who wrote last. The session's own settings are left
as they were. The definition itself is parsed under the caller's settings, as `CREATE
VIEW` parses it; a text-to-date conversion inside it runs at refresh time and reads
`ISO, YMD`.

`CURRENT_DATE` and `now()::date` in a refresh are the **UTC** day. For a local day,
write it in the definition, `(now() AT TIME ZONE 'Europe/Paris')::date`, and call
`pg_tviews_refresh_time_dependent()` just after that zone's midnight.

To compare a TVIEW with its definition, run the definition under the same settings:

```sql
BEGIN;
SET LOCAL TimeZone = 'UTC'; SET LOCAL DateStyle = 'ISO, YMD'; SET LOCAL IntervalStyle = 'postgres';
SET LOCAL extra_float_digits = 1; SET LOCAL bytea_output = 'hex';
SELECT count(*) FROM (TABLE tv_event EXCEPT SELECT … ) d;
COMMIT;
```

## Privileges

A TVIEW's table is an ordinary table: grant on it as on any other. Its backing view,
in `tviews`, follows it:

- whoever can `SELECT` from `tv_<entity>` (a role, or `PUBLIC`) can `SELECT` from its
  backing view, so a role granted `SELECT ON ALL TABLES IN SCHEMA app`, or reading
  `app` through default privileges, reads `tviews.app__tv_<entity>` too;
- the view's grants are made its table's when the TVIEW is created or rebuilt, and
  after every `GRANT` or `REVOKE` on tables, including `ON ALL TABLES IN SCHEMA`;
- `ALTER TABLE tv_<entity> OWNER TO` (and `REASSIGN OWNED`) gives the view the new
  owner, who reads the base tables through it;
- only `SELECT` is copied, without grant option; `INSERT`, `UPDATE` and the others
  granted on the table are not. A grant made on the backing view alone is taken back
  by the next of these.

`USAGE` on `tviews` is granted to `PUBLIC` by the extension.

Creating a TVIEW needs `CREATE` on its schema, `SELECT` on what the definition reads
and `TRIGGER` on each base table. Replacing, dropping, refreshing or re-registering a
TVIEW requires owning it (or being a member of its owner, or of the extension's
owner), otherwise `42501`. The maintenance functions that act on every TVIEW are not
executable by `PUBLIC` ([Operator role](../user-guides/operators.md#operator-role)).

## DROP TABLE tv_*

```sql
DROP TABLE [IF EXISTS] tv_<entity> [CASCADE];
-- or
SELECT tviews.pg_tviews_drop('tv_<entity>', if_exists => true, cascade => false);
```

Dropping a TVIEW's table removes its triggers, its backing view and its registration
with it.

A TVIEW that another TVIEW reads cannot be dropped alone: the other TVIEW's backing
view depends on its table, and PostgreSQL refuses (`cannot drop table tv_user because
other objects depend on it`). Drop the readers first, or use `CASCADE`, which drops
every TVIEW that reads it too. To list them:

```sql
SELECT schema, name FROM tviews.registry
WHERE 'public.tv_user'::regclass = ANY (base_tables);
```

**Dropped with something else.** A TVIEW whose table goes as a dependent of another
object, its schema (`DROP SCHEMA app CASCADE`), a base table or view its definition
reads (`DROP TABLE tb_post CASCADE`), or its owner's objects (`DROP OWNED BY`), is
deregistered with it: its triggers are removed and its backing view in `tviews` is
dropped along with what depends on it, so the TVIEW can be created again under the
same name.

**`DROP EXTENSION pg_tviews CASCADE`** drops the triggers and every backing view; the
`tv_*` tables stay as plain tables with their rows. Recreating a TVIEW under the same
name needs the table out of the way first (drop it, or rename it and copy what you
need). A backing view left by a drop in a session that never loaded the library (no
`shared_preload_libraries`) is dropped by the next `pg_tviews_create()` of that
TVIEW, with a NOTICE, when no TVIEW is registered with it and nothing depends on it.

## Changing a TVIEW

Change a TVIEW's definition or storage with `pg_tviews_create_or_replace()`, which makes
the smallest change (`unchanged`, `altered`, `replaced` in place, or `rebuilt`):

```sql
SELECT tviews.pg_tviews_create_or_replace('tv_post', $$ SELECT … $$);
```

A column rename on a table or view the definition reads (`ALTER TABLE tb_post RENAME
COLUMN title TO headline`) is followed: the stored definition and plan are rewritten
with the new name.

The columns of `tv_<entity>` itself are its definition's: refreshes write each one
by name, with the backing view's type. `ALTER TABLE tv_<entity>` refuses (`42809`)
`RENAME COLUMN`, `DROP COLUMN`, and `ALTER COLUMN … TYPE` on `pk_<entity>`, `id` or
`data`, or to a type the view's column does not convert to on assignment. Change the
column in the definition with `pg_tviews_create_or_replace()` instead. Other `ALTER
TABLE` forms (storage, compression, fillfactor, a type the view's converts to) run.

## Triggers

Creating a TVIEW installs triggers on each table its definition reads, by the table's
kind in `tviews.registry.cascade_kinds` (see [How a write finds the TVIEW rows to
refresh](#how-a-write-finds-the-tview-rows-to-refresh)):

| Kind | Triggers |
|---|---|
| `local` | a row trigger (`tviews.pg_tview_trigger_handler`) that queues the keys of each changed row |
| `mapped`, `all_keys` | three statement triggers with transition tables (`tviews.pg_tview_delta_trigger`, one per `INSERT`, `UPDATE`, `DELETE`) |
| `propagated` | none |

Every table but a `propagated` one also gets a statement-level flush trigger
(`tviews.pg_tview_flush_trigger`), which refreshes the queued keys at the end of each
statement, and an `AFTER TRUNCATE` trigger (`tviews.pg_tview_truncate_trigger`). They
are named `trg_tview_<role>_<entity>_on_<schema>_<table>`, one set per TVIEW. Nothing
needs installing by hand: `tviews.pg_tviews_health_check()` reports missing or orphaned
triggers, and `SELECT * FROM tviews.pg_tviews_reregister_all()` re-installs them.

A commit with refresh work still queued (a flush trigger dropped or disabled) fails
with `55000`; a write whose row trigger cannot tell what to refresh (a plan that does
not decode) fails naming the TVIEW, with the `pg_tviews_reregister` hint.

## Troubleshooting

| Error | SQLSTATE | Fix |
|---|---|---|
| `TVIEW tv_foo does not match its definition, which is keyed on pk_post` | `22023` | name the TVIEW after the key, or alias the key `pk_foo` |
| `Invalid SELECT statement: no column to key the rows on …` | `42601` | output a `pk_<entity>` column |
| `Invalid SELECT statement: a TVIEW is defined by exactly one SELECT` | `42601` | remove the other statements |
| `TVIEW tv_post already exists` | `42P07` | use `pg_tviews_create_or_replace()` |
| `writes to … would not refresh …` | `22023` | declare an `uncascaded_policy` or `uncascaded_tables` ([Tables no cascade reaches](#tables-no-cascade-reaches)), or join on a traceable column |
| `relations would read each other in a cycle: …` | `42P17` | restructure the definitions so no TVIEW reads itself through others |
| `pk_x is uuid: a TVIEW's pk_<entity> must be an integer key …` | `42804` | key on an integer column, and keep the uuid in `id` |
| `RENAME COLUMN on TVIEW public.tv_x is refused …` | `42809` | change the definition with `pg_tviews_create_or_replace()` |
| `cannot drop table tv_user because other objects depend on it` | `2BP01` | drop the TVIEWs that read it first, or `DROP TABLE … CASCADE` |

See [Troubleshooting](../operations/troubleshooting.md) for refresh problems and the
[error reference](../error-reference.md) for every SQLSTATE.

## See also

- [Contract for tools](read-contract.md): `tviews.registry` and
  `pg_tviews_create_or_replace()`
- [API Reference](api.md)
- [FraiseQL Integration Guide](../getting-started/fraiseql-integration.md)
