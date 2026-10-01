# Contract for tools

Tools that generate migrations for TVIEWs, or read back what a database registers,
use objects whose behaviour is versioned: a view to read, and a function to create or
replace. The read side:

```sql
SELECT tviews.contract_version();   -- integer, currently 1
SELECT * FROM tviews.registry;      -- one row per registered TVIEW
```

Both are plain SQL over the system catalogs and pg_tviews' internal tables: they call
no function of the pg_tviews library, so they can be read on a hot standby, and while
the library and the installed extension disagree (see
[extension versioning](../development/extension-versioning.md)). Every role can read
them.

## `tviews.registry`

| column | type | meaning |
|---|---|---|
| `schema` | `text` | schema of the TVIEW table |
| `name` | `text` | TVIEW table name, unquoted (`tv_post`) |
| `entity` | `text` | entity (`post`); unique across the database |
| `query` | `text` | the normalized definition (below) |
| `base_tables` | `regclass[]` | the tables the backing view reads (below) |
| `logged` | `boolean` | the table is LOGGED |
| `options` | `jsonb` | the effective options, every key present (below) |
| `needs_reregister` | `boolean` | a release changed what registration derives since this TVIEW was last registered; `SELECT * FROM tviews.pg_tviews_reregister_all()` clears it |
| `view` | `regclass` | the backing view (`v_post`); NULL when the view is gone |

**`query`** is the definition as pg_tviews stores it: the author's text after the
creation pipeline, with `SELECT *` expanded, a raw SELECT rewritten to the
`pk_<entity>, id, data` shape, and column renames applied. It is not the author's
original text.

**`view`** is a `regclass`, like `base_tables`: it follows renames, prints
schema-qualified and quoted as needed for the reader's `search_path`
(`::text`), and casts to `oid` to join the system catalogs.

**`base_tables`** is every relation reached from the backing view `v_<entity>` through
its rewrite rule's dependencies whose `relkind` is `r`, `p`, `f` or `m`:

- views are followed; the walk stops at the four relkinds above;
- another TVIEW's `tv_*` table is a table: it is listed, and its own sources are not;
- functions, sequences and types the view uses are not listed;
- the list is sorted by schema name, then relation name.

**`options`**:

| key | type | from |
|---|---|---|
| `logged` | `boolean` | `pg_class.relpersistence` |
| `fillfactor` | `integer` | the table's `fillfactor` reloption, 100 when unset |
| `data_gin_index` | `boolean` | whether a valid GIN index on `data` with the default `jsonb_ops` operator class exists |
| `group_keys` | `object` or `null` | the source table → group key column map of an aggregate TVIEW; `null` for a plain one |

Values come from the system catalogs where they can, so the view reports the truth
after a manual `ALTER TABLE`. A registration whose table is gone (dropped without pg_tviews
seeing it) stays visible, with `schema`, `logged` and `options` NULL; one whose
view is gone has `view` NULL.

## `tviews.pg_tviews_create_or_replace()`

```sql
SELECT tviews.pg_tviews_create_or_replace(
    'app.tv_post', $$SELECT …$$,
    options => '{"logged": true, "fillfactor": 85}');
-- 'created' | 'unchanged' | 'altered' | 'replaced' | 'rebuilt'
SELECT tviews.pg_tviews_drop('app.tv_post', if_exists => true);
```

**Name.** `tv_post`, `post` and `app.tv_post` name the same TVIEW; an unqualified name
resolves to `current_schema()`. Each part is taken as written, case included;
double-quote a part that contains a dot: `"app.v2".tv_post`. The entity is unique across the database: naming
`app.tv_post` while `post` is registered in another schema is an error. The name must
match the definition's key (`pk_post`), including for `DISTINCT ON` and aggregate TVIEWs.

**Options**, a JSON object; an unknown key or a wrongly typed value is an error. An
omitted key takes its default when the TVIEW is created and **keeps its current value**
when it exists.

| key | type | default on create |
|---|---|---|
| `logged` | boolean | `NOT pg_tviews.unlogged_by_default` |
| `fillfactor` | integer 10–100 | `pg_tviews.fillfactor` |
| `data_gin_index` | boolean | `pg_tviews.data_gin_index` |
| `group_keys` | object or `null` | `null`: a plain TVIEW; an object makes an aggregate TVIEW |

**What counts as the same.** The definition goes through the creation pipeline, is
created as a temporary view, and is the same when `pg_get_viewdef` renders it like the
TVIEW's backing view: layout, comments and keyword case do not matter, name resolution
under the current `search_path` does. An invalid definition raises its error. Passing
`registry.query` back with the same options returns `unchanged`.

**Results**, by the smallest change that applies:

- `created`: the TVIEW did not exist.
- `unchanged`: definition and options are the same; nothing is touched. The comparison
  creates a temporary view, so the call cannot run on a standby or in a read-only
  transaction.
- `altered`: only `logged`, `fillfactor` or `data_gin_index` differ; changed in place
  (`ALTER TABLE … SET LOGGED/UNLOGGED`, `SET (fillfactor = n)`, the GIN index created or
  dropped), rows kept.
- `replaced`: the definition differs but produces the same columns (names and types,
  in order), and `group_keys` is the same. The backing view is replaced, the TVIEW
  re-registered (triggers added and removed), and the rows reconciled in place with
  three statements that touch only rows that change, deletions first. Every TVIEW
  whose view reads the backing view, directly or through views, is re-registered and
  reconciled the same way, as its owner, each after those it reads. The call holds
  `SHARE` on the tables these TVIEWs read, before and after (writers wait; taken as
  each table's owner), then `EXCLUSIVE` on their tables (readers go on). The tables,
  their indexes, privileges, comments and dependents are untouched. `replaced` also
  requires the table's key to stay: the first `DISTINCT ON` key, or `pk_<entity>`.
- `rebuilt`: the columns or `group_keys` differ. The backing view, table and
  registration are dropped and created again, and the rows computed. The table's and
  view's owner, privileges and comment, the GraphQL type name and the indexes a user
  added to the table are carried over; an added index that no longer applies fails the
  call, naming it. A rebuild is refused, naming the reason, when an object depends on the
  table or view, or the table has row level security or policies, triggers, rules,
  publication membership, a non-default replica identity, constraints other than the
  primary key, per-column statistics targets, privileges or comments, extended
  statistics, or security labels.

**Who may call it.** The DDL runs as the caller: the new objects belong to it, creating
them needs `CREATE` on the schema, the view needs `SELECT` on what it reads, and the
base-table triggers need `TRIGGER` on each base table. Replacing or dropping an existing
TVIEW requires owning it (or being a member of the owning role). No superuser is needed.
A registration whose table is gone can be dropped by the owner of its view, or by anyone
once the view is gone too.

The comparison creates a temporary view, so the caller needs the `TEMPORARY` privilege
on the database (granted to `PUBLIC` by default), and a transaction that called it on
an existing TVIEW cannot be prepared with `PREPARE TRANSACTION`.

**Serialization.** Every call that registers, changes or drops a TVIEW holds
`pg_advisory_xact_lock(<class>, hashtext(entity))` until the transaction ends, so two
calls for one entity run one after the other.

It works in any transaction, `DO` block or multi-statement batch (including the one that
runs `CREATE EXTENSION`), and in a session where the library is not preloaded.

## Stability rules

`contract_version()` covers the view above, the `options` keys and their meaning, and
the signature, "same" rules and return values of `pg_tviews_create_or_replace()`.

- **Additive changes do not bump it:** a new column (always appended), a new `options`
  key, a new function. Select columns by name and ignore keys you do not know.
- **Everything else bumps it:** removing or renaming a column, changing a type or a
  meaning, removing an `options` key, changing what counts as "same" or what the
  functions return. A bump is called out in the CHANGELOG's upgrade notes. While
  pg_tviews is in beta, the previous contract is not kept alongside the new one.

## Internal tables

`tviews.pg_tview_meta`, `tviews.pg_tview_helpers`, `tviews.pg_tview_audit_log` and the
other `pg_tview_*` objects are internal. They may change in any release; tools must not
read them.
