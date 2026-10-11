# TVIEW Creation Syntax Guide

A TVIEW is created with DDL or with a function call. Both register the same TVIEW:
same table, backing view, triggers and stored plan.

## 1. `CREATE TABLE tv_<entity> AS`

```sql
CREATE TABLE tv_post AS
SELECT
  tb_post.pk_post,
  tb_post.id,
  tb_post.fk_user,
  jsonb_build_object(
    'id', tb_post.id,
    'title', tb_post.title,
    'user_id', tb_user.id
  ) as data
FROM tb_post
INNER JOIN tb_user ON tb_post.fk_user = tb_user.pk_user;
```

**Use when**: working in psql, migrations, hand-written DDL.

pg_tviews intercepts `CREATE TABLE [schema.]tv_<entity> AS SELECT …` and turns it into a
TVIEW. This works:

- as a top-level statement, including in a multi-statement batch (`psql -c "…; …"`) and in the
  same batch as `CREATE EXTENSION pg_tviews`;
- inside `DO` blocks, functions and procedures;
- with `IF NOT EXISTS`: over an existing TVIEW or table the statement is a no-op.

`CREATE TABLE tv_<entity> (col type, …)` (a column list, no `AS SELECT`) is an ordinary table
and is never converted. `SELECT … INTO tv_<entity>`, `WITH NO DATA`, a temporary table and
the other forms pg_tviews cannot make a TVIEW of fail with SQLSTATE `0A000`.

Interception needs the extension's library loaded in the session, so add it to
`shared_preload_libraries` and restart PostgreSQL. If a `tv_*` CTAS reaches the server without
being intercepted, the statement **fails** with an error that names the table and this fix,
instead of leaving a plain table behind.

The statement takes no options: the TVIEW gets the defaults (LOGGED, fillfactor 85,
`uncascaded_policy` `error`, …). `CREATE UNLOGGED TABLE tv_<entity> AS` makes an
UNLOGGED one. No setting changes what a TVIEW is.

## 2. Functions

```sql
SELECT pg_tviews_create('tv_post', $$
  SELECT
    tb_post.pk_post,
    tb_post.id,
    tb_post.fk_user,
    jsonb_build_object(
      'id', tb_post.id,
      'title', tb_post.title,
      'user_id', tb_user.id
    ) as data
  FROM tb_post
  INNER JOIN tb_user ON tb_post.fk_user = tb_user.pk_user
$$);
```

`pg_tviews_create(tview, query, options)` and
`pg_tviews_create_or_replace(tview, query, options)` take options (`logged`,
`fillfactor`, `uncascaded_policy`, …). `pg_tviews_create_or_replace` creates the TVIEW
or makes the smallest change to an existing one (`created`, `unchanged`, `altered`,
`replaced`, `rebuilt`); the options passed are the whole declaration, and one left out
is at its default:

```sql
SELECT pg_tviews_create_or_replace('tv_post', $$
  SELECT tb_post.pk_post, tb_post.id, tb_post.fk_user,
         jsonb_build_object('id', tb_post.id, 'title', tb_post.title) AS data
  FROM tb_post
$$, options => '{"fillfactor": 90}');
```

**Use when**: application code, migration tools, scripts; anything that runs the same
DDL more than once. See [Contract for tools](../reference/read-contract.md).

## Naming

The definition must output a `pk_<entity>` column named after the TVIEW (`pk_post` for
`tv_post`). Nothing else is matched by name: base tables, link columns and the column
holding an embedded TVIEW's key may be called anything. The `tb_*`, `fk_*` and `id`
names above are FraiseQL's conventions. See the [DDL Reference](../reference/ddl.md#naming).
