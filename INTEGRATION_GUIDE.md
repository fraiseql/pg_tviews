# Integration Guide: TVIEWs in a Schema Build

This guide shows how to create TVIEWs from a schema build script, and how to check
them afterwards. The functions are documented in
[docs/reference/api.md](docs/reference/api.md); what a deploy tool may read back is in
[docs/reference/read-contract.md](docs/reference/read-contract.md).

## Prerequisites

- `pg_tviews` in `shared_preload_libraries` (restart required), so that
  `CREATE TABLE tv_* AS` is intercepted and the settings exist
  ([installation](docs/getting-started/installation.md)).
- `tviews` on the `search_path`, or every call qualified with `tviews.`.
- `jsonb_delta` is optional: without it TVIEW rows are always recomputed, never patched.

## 1. Base tables

Base tables may have any names. What a TVIEW needs is in its definition: an output
column `pk_<entity>` naming its rows, usually `id`, and `data`.

```sql
-- schema/01_base_tables.sql
CREATE TABLE tb_user (
    pk_user BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id      UUID NOT NULL DEFAULT gen_random_uuid() UNIQUE,
    email   TEXT NOT NULL,
    name    TEXT NOT NULL
);

CREATE TABLE tb_post (
    pk_post BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id      UUID NOT NULL DEFAULT gen_random_uuid() UNIQUE,
    fk_user BIGINT NOT NULL REFERENCES tb_user(pk_user),
    title   TEXT NOT NULL,
    content TEXT NOT NULL
);
```

## 2. TVIEWs

Do not create `tv_*` tables yourself: creating the TVIEW creates its table, fills it,
and installs the triggers. Create dependencies first (`tv_user` before `tv_post`,
which embeds it).

```sql
-- schema/02_tviews.sql
CREATE TABLE tv_user AS
SELECT pk_user, id,
       jsonb_build_object('id', id, 'email', email, 'name', name) AS data
FROM tb_user;

CREATE TABLE tv_post AS
SELECT p.pk_post, p.id, p.fk_user,
       jsonb_build_object('id', p.id, 'title', p.title, 'author', u.data) AS data
FROM tb_post p
JOIN tv_user u ON u.pk_user = p.fk_user;
```

For a build that runs more than once, use `pg_tviews_create_or_replace()`: it creates
the TVIEW, or brings an existing one to the new definition with the smallest change,
and returns what it did (`created`, `unchanged`, `altered`, `replaced`, `rebuilt`).

```sql
SELECT tviews.pg_tviews_create_or_replace('public.tv_user', $$
    SELECT pk_user, id,
           jsonb_build_object('id', id, 'email', email, 'name', name) AS data
    FROM tb_user $$);
```

## Relationships between TVIEWs

There is no metadata to write by hand: `pg_tviews` reads every relationship from the
definition's query tree when the TVIEW is created. A TVIEW that joins another TVIEW
(`JOIN tv_user u ON u.pk_user = p.fk_user`) and projects the joined column is refreshed
when that TVIEW's rows change, whatever the column is called. A definition read that
nothing links to the TVIEW's key is refused under the default `uncascaded_policy`
(`error`); see [docs/reference/ddl.md](docs/reference/ddl.md). `tviews.registry` shows
how each base table's writes reach a TVIEW.

## Build script

```bash
#!/bin/bash
set -euo pipefail
db=${1:?database}

psql -v ON_ERROR_STOP=1 -d "$db" -c "CREATE EXTENSION IF NOT EXISTS jsonb_delta;"
psql -v ON_ERROR_STOP=1 -d "$db" -c "CREATE EXTENSION IF NOT EXISTS pg_tviews;"
psql -v ON_ERROR_STOP=1 -d "$db" -f schema/01_base_tables.sql
psql -v ON_ERROR_STOP=1 -d "$db" -f schema/02_tviews.sql
psql -v ON_ERROR_STOP=1 -d "$db" -f schema/03_seed_data.sql   # TVIEWs follow the writes
```

## Verify

```sql
-- Registered TVIEWs, what they read, and how writes reach them
SELECT schema, name, base_tables, cascade_kinds FROM tviews.registry;

-- Health
SELECT * FROM tviews.pg_tviews_health_check();

-- Automatic refresh
INSERT INTO tb_user (email, name) VALUES ('alice@example.com', 'Alice');
SELECT data FROM tv_user;
```

Read `tviews.registry`, not the internal `tviews.pg_tview_meta`, whose columns change
between releases.

## Troubleshooting

- **`cannot convert 'tv_…' to a TVIEW: the statement was not intercepted` (55000).**
  The library is not loaded in the session: add it to `shared_preload_libraries` and
  restart, or call `pg_tviews_create()`.
- **`42P07` on create.** The TVIEW exists: use `pg_tviews_create_or_replace()`.
- **`pk_<entity>` missing.** The definition must project a column named `pk_<entity>`
  for `tv_<entity>`.
- **Refused definition (`0A000`).** The message names the read that no write can be
  traced through. Declare it (`uncascaded_tables`, `function_reads`, `time_refresh`)
  or change the definition; see [docs/error-reference.md](docs/error-reference.md).

## Next steps

1. **Monitor refresh**: run `docs/operations/runbooks/scripts/health-check.sql` and
   `tviews.pg_tviews_health_check()`.
2. **Grant operators** the maintenance functions
   ([docs/user-guides/operators.md](docs/user-guides/operators.md)).
3. **Index propagation lookups** on large TVIEWs:
   `tviews.pg_tviews_ensure_propagation_indexes()`.
