# Read contract for tools

Tools that generate migrations for TVIEWs, or read back what a database registers,
use two objects whose behaviour is versioned:

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

**`query`** is the definition as pg_tviews stores it: the author's text after the
creation pipeline, with `SELECT *` expanded, a raw SELECT rewritten to the
`pk_<entity>, id, data` shape, and column renames applied. It is not the author's
original text.

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
| `data_gin_index` | `boolean` | whether a GIN index on `data` exists |
| `group_keys` | `object` or `null` | the source table → group key column map of an aggregate TVIEW; `null` for a plain one |

Values come from the system catalogs where they can, so the view reports the truth
after a manual `ALTER TABLE`.

## Stability rules

`contract_version()` covers the view above, the `options` keys and their meaning, and
the signatures, "same" rules and return values of `pg_tviews_create_or_replace()`.

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
