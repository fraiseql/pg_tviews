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
| `pg_tviews_cascade(oid, bigint)`, `pg_tviews_insert(oid, bigint)`, `pg_tviews_delete(oid, bigint)` | 0.1.0-beta.27 | None needed: a write to a base table refreshes every TVIEW that reads it. `pg_tviews_refresh(entity)` rebuilds one TVIEW after changes the triggers did not see. |
| `pg_tviews_convert_existing_table(text)`, `pg_tviews_convert_table(text, text)` | 0.1.0-beta.27 | `pg_tviews_create_or_replace(name, select)`, or `CREATE TABLE tv_<entity> AS SELECT …` with the extension preloaded. Both raised an error since 0.1.0-beta.18. |
| `pg_tviews_migrate_triggers()`, `pg_tviews_rebind_cascade_paths(…)` | 0.1.0-beta.27 | None needed: the update to 0.1.0-beta.27 re-derives every TVIEW and its triggers, and the catalog trigger rebinds a restored TVIEW's plan. |
| Migration of row triggers installed before 0.1.0-beta.20 | 0.1.0-beta.27 | None needed: the update drops those triggers and installs the current ones per TVIEW. |
| `pg_tviews.metrics_enabled` | 0.1.0-beta.27 | None: metrics are always collected. Remove the setting from `postgresql.conf`. |
| `pg_tview_meta` columns `cascade_paths`, `fk_columns`, `uuid_fk_columns`, `dependency_types`, `dependency_paths`, `array_match_keys`, `direct_map_columns`, `direct_map_keys`, `distinct_on_keys`, `distinct_on_output_keys`, `is_union`, `aggregate_embeds`, `key_mappings` | 0.1.0-beta.27 | `pg_tview_meta.plan`: one versioned document per TVIEW, derived from its query tree (ADR 0203). `tviews.registry` and `pg_tviews_mapping_query()` stay the stable way to read it. |
| Views `pg_tviews_queue_realtime`, `pg_tviews_cache_stats`, `pg_tviews_performance_summary`; function `pg_tviews_hook_status()` | 0.1.0-beta.21 | `pg_tviews_queue_stats()`, `pg_tviews_health_check()`, `pg_tviews_performance_stats()`. They returned fixed placeholder values. Drop any object of yours that depends on them before updating. |

## Changed (breaking)

Behaviour a release changed in a way that can break a caller. The CHANGELOG has the
full text.

| Change | In | What to do |
|---|---|---|
| `EXECUTE` on `pg_tviews_refresh_all()`, `pg_tviews_refresh_all_entities()`, `pg_tviews_rebuild_all()`, `pg_tviews_reregister_all()`, `pg_tviews_set_logged()`, `pg_tviews_ensure_propagation_indexes()`, `pg_tviews_invalidate_caches()` revoked from `PUBLIC` | 0.1.0-beta.27 | `GRANT EXECUTE` to the operator role before upgrading (`docs/user-guides/operators.md`); others get 42501. |
| Every function acting on one TVIEW requires owning it or the extension | 0.1.0-beta.27 | Call them as the TVIEW's owner, or a member of its role. |
| `pg_tviews_refresh(entity)` and every rebuild run as the TVIEW's owner | 0.1.0-beta.27 | Nothing, unless a definition relied on the caller's privileges. |
| A commit with refresh work still queued fails (55000) instead of a WARNING | 0.1.0-beta.27 | `pg_tviews_health_check()` finds the missing or disabled flush trigger; re-enable it or `pg_tviews_reregister(entity)`. |
| A write fails when its trigger cannot read the TVIEW's plan, identity or policy | 0.1.0-beta.27 | `pg_tviews_reregister(entity)`, as the error's hint says. |
| Errors carry their own SQLSTATE instead of 22000 / XX000 | 0.1.0-beta.27 | Match the codes in `docs/error-reference.md`. |
| A definition making TVIEWs read each other in a cycle is refused (42P17) | 0.1.0-beta.27 | Break the cycle. |
| Refreshes render values under fixed settings (`TimeZone` UTC, `DateStyle` ISO/YMD, …) | 0.1.0-beta.27 | `SELECT tviews.pg_tviews_refresh_all()` once after upgrading if a TVIEW renders dates, times, intervals, floats or `bytea` as text. |
| A read of a materialized view, of the current time, or of a non-immutable function goes through `uncascaded_policy` | 0.1.0-beta.27 | Declare it (`uncascaded_tables`, `time_refresh`, `function_reads`) with `pg_tviews_create_or_replace()`; see the CHANGELOG's upgrade notes. |
| A definition is exactly one SELECT (42601) | 0.1.0-beta.27 | Remove trailing statements. |
