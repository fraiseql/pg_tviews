# Deprecations and removals

pg_tviews is in beta: a function, setting or catalog column can be removed in the next
release. This page lists each one, the release that removes it, and what replaces it.

## Upgrading

`ALTER EXTENSION pg_tviews UPDATE` is supported from **0.1.0-beta.20** on. An older
install has no upgrade path: drop the TVIEWs, drop and re-create the extension, and
re-create the TVIEWs.

## Removed

| Removed | In | Replacement |
|---|---|---|
| `pg_tviews_analyze_select(text)` | 0.1.0-beta.27 | None. `pg_tviews_create` analyses the definition and reports what it refuses; `tviews.pg_tview_reads` and `pg_tviews_mapping_query()` show what a TVIEW reads and how writes map to its rows. |
| `pg_tviews_infer_types(text, text[])` | 0.1.0-beta.27 | `format_type(atttypid, atttypmod)` from `pg_attribute`. |

## Scheduled for removal in 0.1.0-beta.27

These remain until the release that changes the catalog, which re-derives every TVIEW
at `ALTER EXTENSION pg_tviews UPDATE`.

| Item | Replacement |
|---|---|
| `pg_tviews_cascade(oid, bigint)`, `pg_tviews_insert(oid, bigint)`, `pg_tviews_delete(oid, bigint)` | None needed: a write to a base table refreshes every TVIEW that reads it. `pg_tviews_refresh(entity)` rebuilds one TVIEW. |
| `pg_tviews_convert_existing_table(text)`, `pg_tviews_convert_table(text, text)` | `pg_tviews_create_or_replace(name, select)`, or `CREATE TABLE tv_<entity> AS SELECT …` with the extension preloaded. Both already raise an error. |
| `pg_tviews_migrate_triggers()`, `pg_tviews_rebind_cascade_paths(…)` | The upgrade re-derives every TVIEW and its triggers. |
| `pg_tviews.metrics_enabled` | None: metrics are always collected. The setting has no effect. |
| Catalog columns of the text-pattern analysis (`fk_columns`, `uuid_fk_columns`, `dependency_types`, `dependency_paths`, `array_match_keys`, `direct_map_*`, `cascade_paths`, `distinct_on_*`) | One stored propagation plan per TVIEW, derived from its query tree. |
