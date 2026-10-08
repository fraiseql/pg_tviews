# ADR 0203: One propagation plan per TVIEW, derived from the query tree

- Status: Accepted (implemented in 0.1.0-beta.27)
- Completes: [ADR 0157](0157-cascade-key-mapping.md) (query-tree key mapping),
  [ADR 0169](0169-tview-row-identity.md) (row identity)
- Supersedes: the text-pattern analysis (`src/schema/*`), `cascade_paths`, and the
  `fk_<entity>` / `tb_<entity>` naming lookups at run time

## Context

ADR 0157 moved the question "which TVIEW keys does a write to table `T` affect?" onto
PostgreSQL's query tree, and ADR 0169 did the same for row identity. Neither migration was
finished. A TVIEW is still described by about fifteen derived catalog columns, written by
three analysers:

| Analyser | Writes | Drives at run time |
|---|---|---|
| Text patterns (`src/schema/*`: regexes and a hand-written scanner over the SELECT text) | `fk_columns`, `uuid_fk_columns`, `dependency_types`, `dependency_paths`, `array_match_keys`, `direct_map_columns`/`_keys`, the entity name, the list of embeds lineage starts from | parent propagation, refresh order, direct patch and its gates, smart-patch strategy |
| Query tree (`src/lineage/*`) | `key_mappings`, `identity`, `is_union`, `aggregate_embeds`, `uncascaded_oids`, `time_dependent`, most of `cascade_paths` | statement and row triggers, trigger placement |
| `sqlparser` | the fan-out lookup column inside `cascade_paths` and `key_mappings`; aggregate validation; the rename rewrite | fan-out patches |

Five mechanisms turn a write into TVIEW keys, each reading a different subset:

1. the row trigger over `cascade_paths` (local keys, direct patch, fan-out capture);
2. the statement trigger over `key_mappings` (mapping queries over transition tables);
3. flush-time parent propagation over `fk_columns` + `dependency_types`, finding parent rows
   with `WHERE fk_<child> = ANY(…)`;
4. the direct patch and fan-out, gated by `direct_map_*`, `fk_columns`, `uuid_fk_columns`,
   `is_union` and a re-scan of the definition text at cache load;
5. `pg_tviews_cascade(oid, pk)`, which guesses with `tb_`/`fk_`/`pk_` prefixes and falls back to
   refreshing every row.

The two analysers disagree in practice: the text one rejects quoted identifiers containing
`'`, re-emits `SELECT *` columns unquoted, resolves unqualified tables without a schema, and
builds catalog array literals by hand (report Q21, Q22). A TVIEW whose parent column is not
spelled `fk_<child>` gets no parent edge at all. Every derived column is a drift risk between
analysers and between the writer and its readers.

## Decision

### One analysis

`lineage::plan(entity, view_oid) -> TviewPlan` is the only analysis of a definition. It reads
the backing view's query tree and the catalog of the other TVIEWs, and returns:

- **identity**: the output column naming the rows, its type and kind (ADR 0169);
- **set_operation**: the rows come from UNION/INTERSECT/EXCEPT branches, so they are
  recomputed, never patched;
- **outputs**: each output column, its type, and the base column it copies, if any;
- **data**: the shape of the `data` output, read from its expression tree:
  - *fields*: `jsonb_build_object` keys whose value is a plain column of the table holding the
    identity, mapped to that column (JSON path → column);
  - *embeds*: where another TVIEW's `data` lands, at which path, and how: `nested` (the
    child's `data` as a value), `array` (inside `jsonb_agg`, through `COALESCE` or a scalar
    subquery), or `scalar` (the definition reads the child but copies only some of its
    columns);
  - an opaque `data` (computed by a function, a `CASE`, a subquery level the walker cannot see
    into) has no fields and no paths: it is recomputed, never patched;
- **embeds**: each TVIEW this one reads, found as an occurrence of that TVIEW's table or
  backing view, with:
  - *lookup*: the output column of this TVIEW that the definition equates to one of the
    child's key columns (its identity or `pk_<child>`), so a refreshed child's parents are
    `SELECT identity FROM tv_parent WHERE lookup = ANY(child keys)`;
  - *child column*: which of the child's key columns it equals;
  - *source columns*: the child's columns the definition reads (for the column-aware prune);
  - a read of another TVIEW with no such output column is not an embed: its base tables are
    traced like any other (`mapped`, or `all_keys` under the TVIEW's policy);
- **tables**: one `TablePlan` per base table:
  - `kind`: `local` (the key is a column of the changed row), `mapped` (a mapping query over
    the transition table), `propagated` (reached only through an embed, no trigger), `tview`
    (another TVIEW's table, mapped like a base table, ADR 0157 #191), `all_keys` (no trace:
    the policy applies);
  - `trigger`: `row`, `statement` or none, decided here and nowhere else;
  - `key_column` (`local`), `mapping` (template, `mapped`/`all_keys`), the columns read,
    `virtual_reads`, `matview`;
  - `direct_patch`: for the table holding the identity, the field map restricted to columns
    the definition reads **only** as a `data` field (not in a join, filter, grouping, another
    output, or a sublink). This one rule replaces the `fk_columns`, `uuid_fk_columns` and
    "referenced outside `data`" gates;
  - `fanout`: for a `mapped` table one equality away from a column this TVIEW projects, the
    patch an UPDATE of its read columns writes into every row with that value (issue #120).

Every relationship is read from the tree. Nothing at run time builds or matches a
`fk_`/`tb_`/`pk_` name to find one; `pk_<entity>` survives only as the identity convention
ADR 0169 already allows.

### One storage

`pg_tview_meta` gets a `plan jsonb` column, versioned (`{"version": 1, …}`), which replaces
`cascade_paths`, `fk_columns`, `uuid_fk_columns`, `dependency_types`, `dependency_paths`,
`array_match_keys`, `direct_map_columns`, `direct_map_keys`, `key_mappings`, `is_union`,
`aggregate_embeds`, `distinct_on_keys` and `distinct_on_output_keys`. Columns that are
declarations or indexed lookups stay: `entity`, `view_oid`, `table_oid`, `definition`,
`identity`, `group_keys`, the uncascaded policy columns, `function_read_*`, `time_*`,
`graphql_typename`, `needs_reregister`.

A plan refers to a relation by OID **and** qualified name, and to a column by name and attnum.
`pg_restore` reloads catalog rows into new OIDs and attnums, so the `BEFORE INSERT` trigger
that rebinds `key_mappings` today rebinds the plan instead: OIDs from the names, attnums from
the column names. A plan that no longer resolves fails the insert and names the TVIEW.

The runtime decodes a plan once per backend and caches it, keyed by the catalog generation.
A decode error is a `CatalogError` naming the entity, with the `pg_tviews_reregister` hint;
it is never read as "no dependencies".

### One run model

- The **row trigger** serves `local` tables and partitions of `mapped` ones: it reads the key
  off the changed row, captures a direct patch or a fan-out patch when the `TablePlan`
  allows it, and enqueues.
- The **statement trigger** runs the table's mapping query over the transition tables
  (prepared once per backend), or the fan-out patch for an UPDATE.
- **Parent propagation** at flush follows `embeds`: after a child's keys are refreshed, the
  parents are found through each embed's lookup column, pruned per the embed's source
  columns, and patched through the embed's path when the parent can take a derived patch.
- **Refresh order** is the topological order of embeds and `tview` reads; a cycle is refused
  when it is registered (42P17).
- The **direct patch** and **fan-out** are optimisations driven by `TablePlan.direct_patch`
  and `.fanout`; anything they cannot express falls back to a key refresh.
- `pg_tviews_cascade`, `pg_tviews_insert` and `pg_tviews_delete` are removed (DEC-7): a write to
  a base table already does what they guessed at.

### One flush engine

Orchestration (drain, order, propagate, apply patches) moves from `queue/xact.rs` to
`src/flush/`. `queue/` keeps the transaction state only (keys, patches, savepoint undo log)
and imports nothing from `refresh`, `ddl`, `admin`, `hooks` or `executor`. Abort cleanup runs
through one registry of reset functions that those modules register into.

### Upgrade

This is the one catalog break of the series. The upgrade script to 0.1.0-beta.27 adds `plan`,
re-derives every TVIEW (`pg_tviews_reregister_all(strict => true)`, which runs each
registration as the TVIEW's owner), then drops the replaced columns and the removed functions.
A TVIEW that no longer analyses fails `ALTER EXTENSION … UPDATE` and is named in the error;
nothing is left half-migrated. This overrides, for this release only, the versioning rules
that upgrade scripts never re-derive and never drop a column
(`docs/development/extension-versioning.md` records the exception). A lazy re-derivation
would have had to keep both catalog shapes readable for a release: the two-analyser drift
this ADR removes.

### Removed

`src/schema/*` (the text-pattern analysis and the `TViewSchema` SQL type), `cascade_paths`
and `src/cascade_path.rs`, `src/cascade.rs` (`pg_tviews_cascade*`), `sqlparser` in analysis
(kept for the rename rewrite of the definition text), `pg_tviews_rebind_cascade_paths`,
`pg_tviews_migrate_triggers` and the legacy row-trigger migration, the `LegacyRoot` paths for
catalog rows older than ADR 0169, `pg_tviews.metrics_enabled`,
`pg_tviews_convert_existing_table` and `pg_tviews_convert_table`.

### Performance contract

- No extra SPI per changed row on the row-trigger path: everything the trigger needs is in
  the cached plan.
- A mapping query is prepared once per (table, TVIEW) per backend.
- A plan is decoded once per backend per catalog generation.
- `test/sql/real_benchmark` (single-row, batch, two-hop mapping, scalar and document fan-out)
  stays within ±5% of the baseline measured before the change, or gets faster.

### As implemented

The stored document (`pg_tview_meta.plan`, `src/catalog/plan.rs`, version 1) keeps
what the run-time paths read, not the whole analysis above:

- `embeds`: per embedded TVIEW, its `lookups` (every output column equal to the
  child's key), `kind` and `path`;
- `direct`: the direct-patch map, `(column, data key)`;
- `tables`: per base table, how its writes map to keys (`kind`: `local`,
  `mapped`, `propagated`, `all_keys`; the mapping query of a `mapped` table, its
  fan-out patch, the TVIEW it belongs to when it is another TVIEW's table);
- `paths`: the tables whose changed row holds a key (the row trigger's input);
- `set_operation`: the rows come from a UNION/INTERSECT/EXCEPT.

The output columns and the identity are not in the plan: the backing view and
`pg_tview_meta.identity` hold them. A plan that does not decode, or names a mapped
table without its query, fails every write that needs it, naming the TVIEW;
`pg_tviews_reregister` derives it again.

## Consequences

- One place answers every question about a TVIEW's definition, and the answer follows
  PostgreSQL's parser: quoting, schemas, `SELECT *` and comments need no handling of their
  own.
- A parent column may be called anything: the relationship is the equality in the
  definition, not the name.
- A `data` expression the walker cannot read is recomputed instead of patched. That is
  slower than a patch, never wrong. Each shape the text patterns patched and the tree does
  not is listed as a regression test, so a later release can teach the walker that shape.
- Catalog rows carry one versioned document instead of fifteen positional arrays; a future
  change to the plan bumps its version and re-derives, as this one does.

## Alternatives considered

- **Keep the text analysis as a fallback where the tree is opaque.** Rejected: it keeps the
  disagreements, and the fallback would only ever run on the shapes nothing tests.
- **Derive the plan lazily, on first use after the upgrade (DEC-6 b).** Rejected: both
  catalog shapes would stay readable for a release, and a TVIEW that no longer analyses
  would fail on a user's write instead of at the update.
- **Store the plan in separate typed columns.** Rejected: fifteen columns that must change
  together are the problem; one versioned document changes in one statement.
