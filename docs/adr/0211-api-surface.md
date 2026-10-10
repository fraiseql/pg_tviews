# ADR 0211: One name for a TVIEW, one function per action, read contract v2

- Status: Accepted
- Fixes: #211 (one TVIEW name resolver)
- Amends: [ADR 0136](0136-tool-facing-surface.md) (the tool-facing surface)
- Related: [ADR 0220](0220-settings.md) (options), #212 Q46 (identifier helpers)

## Context

Functions acting on one TVIEW accept different spellings of it. Observed on 0.1.0-beta.27:

- `pg_tviews_refresh('tv_user')` fails, though `pg_tviews_reregister('tv_user')` works;
- `pg_tviews_refresh_time_dependent('user')` fails with a raw `42P01`;
- `pg_tviews_drop('user')` reports "TVIEW 'user' dropped".

Their parameters are named `tview_name`, `tview`, `entity`, `entity_name` or `p_entity`. A
caller, or an agent generating calls, has to remember the form per function.

Several functions do what another one, or an option, already does:

| Function | Already done by |
|---|---|
| `create_aggregate` | `create_or_replace` with `group_keys` |
| `refresh_all_entities` | `refresh_all` |
| `recover_after_crash` | `rebuild_all` |
| `set_logged` | the `logged` option, or `ALTER TABLE` |
| `is_replica_readable` | `registry.logged` |
| `performance_stats` | `profile` |
| `set_typename` | an option ([ADR 0220](0220-settings.md)) |

`tviews.registry` (read contract v1) publishes some options twice:

- `logged` both as a column and in `options`;
- the uncascaded policy, the per-table policies, the function reads and `time_refresh` only
  as columns, under names that differ from the option keys.

So reading back what to pass to `pg_tviews_create_or_replace` meant mapping columns to keys.

## Decision

### 1. One resolver, one parameter name

- Every function acting on one existing TVIEW takes it as its first parameter, `tview text`.
- That parameter is resolved by one function, `catalog::resolve`. It accepts:

  | Form | Example |
  |---|---|
  | the entity | `post` |
  | the table name | `tv_post` |
  | a schema-qualified or quoted relation | `app.tv_post`, `"App".tv_post` |

- Resolution order:
  1. A qualified name, or a name starting with `tv_`, is looked up as a relation (with the
     caller's `search_path` for an unqualified one).
  2. Otherwise, or when that relation is not a TVIEW's table, the name is looked up as an
     entity.
  3. When both forms find different TVIEWs, the call fails as ambiguous.
- `tb_` and `v_` names are never accepted: they name other objects by the convention.
- A name that resolves to nothing fails with `42704 undefined_object` and one message listing
  the forms tried. Messages name a TVIEW by its relation (`public.tv_post`).
- Functions that create a TVIEW (`pg_tviews_create`, `pg_tviews_create_or_replace`) parse the
  same forms (`tview` is the TVIEW to create).
- Output columns that name a TVIEW are `entity` (and `schema`, `name` where a relation is
  meant), never `entity_name`.

### 2. One exported layer

- Every SQL-callable function lives in `src/api/`. Each one:
  1. resolves its `tview`;
  2. checks ownership;
  3. checks the catalog revision;
  4. calls a service in the module that implements it.
- So the resolver and the owner check are guaranteed by structure, not by review.
- Internal SQL entry points (event triggers, `pg_tviews_meta_rebind`, `pg_tviews_audit_write`,
  `pg_tviews_handle_dropped`, `pg_tviews_invalidate_caches`) also live in `src/api/`, marked
  internal.
- Naming rule: relations in `tviews` are nouns (`registry`, `stats`, `pg_tview_meta`);
  functions are `pg_tviews_<verb>`.

### 3. Removed functions

`pg_tviews_create_aggregate`, `pg_tviews_refresh_all_entities`,
`pg_tviews_recover_after_crash`, `pg_tviews_set_logged`, `pg_tviews_is_replica_readable`,
`pg_tviews_performance_stats`, `pg_tviews_set_typename`. Their replacements are in the table
above. pg_tviews has no users yet besides its own ecosystem, so there is no deprecation cycle.

### 4. Read contract v2

`contract_version()` returns `2`. `tviews.registry`:

| Column | Type | |
|---|---|---|
| `schema`, `name`, `entity` | `text` | as v1 |
| `query` | `text` | as v1 |
| `options` | `jsonb` | **every option key** ([ADR 0220](0220-settings.md)), defaults included; NULL when the table is gone |
| `view` | `regclass` | as v1 |
| `identity` | `text[]` | as v1 |
| `base_tables` | `regclass[]` | as v1 |
| `cascade_kinds` | `jsonb` | as v1 |
| `uncascaded_tables` | `regclass[]` | as v1: tables no cascade maps (derived; the option of the same name declares per-table policies) |
| `time_dependent` | `boolean` | as v1 |
| `managed_indexes` | `regclass[]` | as v1 |
| `needs_reregister` | `boolean` | as v1 |

Removed columns, now `options` keys:

| v1 column | v2 key |
|---|---|
| `logged` | `options.logged` |
| `uncascaded_policy` | `options.uncascaded_policy` |
| `uncascaded_table_policies` | `options.uncascaded_tables` |
| `function_reads` | `options.function_reads` |
| `time_refresh` | `options.time_refresh` |

Round-trip property, part of the contract:
`pg_tviews_create_or_replace(format('%I.%I', schema, name), query, options)` returns
`unchanged` for every registered TVIEW.

## Consequences

- One form to learn. The API reference has one table of accepted spellings.
- A tool reads `options` and passes it back.
- Breaking for callers:
  - named arguments `tview_name =>`, `entity =>`, `p_entity =>`;
  - the removed functions;
  - the removed registry columns.
- confiture, fraisier and fraiseql are told in one issue each, with the list.
