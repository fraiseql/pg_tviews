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
| `pg_tviews.unlogged_by_default` | 0.1.0-beta.28 | Option `logged` (default `true`), or `CREATE UNLOGGED TABLE tv_<entity> AS …`. Setting it fails: remove it from `postgresql.conf` and `ALTER ROLE … SET`. |
| `pg_tviews.fillfactor` | 0.1.0-beta.28 | Option `fillfactor` (default 85). |
| `pg_tviews.data_gin_index` | 0.1.0-beta.28 | Option `data_gin_index` (default `false`). |
| `pg_tviews.uncascaded_policy` | 0.1.0-beta.28 | Option `uncascaded_policy` (default `error`); `pg_tviews_create()` takes options too. |
| `pg_tviews.time_refresh` | 0.1.0-beta.28 | Option `time_refresh`. |
| `pg_tviews.union_duplicate_policy` (and its value `first`) | 0.1.0-beta.28 | None: a key returned by two UNION branches is always an error (`21000`). Keep one row per key in the definition with `DISTINCT ON` over the UNION, ordered by preference (ADR 0216). |
| `pg_tviews.suspend_triggers` | 0.1.0-beta.28 | `pg_tviews_suspend_triggers()` / `pg_tviews_resume_triggers()`, which record the changed TVIEWs and refresh them at resume or commit. |
| `pg_tviews.log_level` | 0.1.0-beta.28 | `client_min_messages = debug1` / `log_min_messages`: diagnostics are `DEBUG1` messages. |
| `pg_tviews_create_aggregate(name, query, keys)` | 0.1.0-beta.28 | `pg_tviews_create(name, query, '{"group_keys": {…}}')`, or `pg_tviews_create_or_replace` with the same option. |
| `pg_tviews_refresh_all_entities()` | 0.1.0-beta.28 | `pg_tviews_refresh_all()`. |
| `pg_tviews_recover_after_crash(entity)` | 0.1.0-beta.28 | `pg_tviews_rebuild_all()`; the background worker (`pg_tviews.auto_rebuild_databases`, default `*`) refills reset UNLOGGED TVIEWs after a crash restart or a promotion. |
| `pg_tviews_set_logged(entity, logged)` | 0.1.0-beta.28 | Option `logged` with `pg_tviews_create_or_replace()`, or `ALTER TABLE tv_<entity> SET [UN]LOGGED`; both fill a reset TVIEW first. |
| `pg_tviews_is_replica_readable(entity)` | 0.1.0-beta.28 | `pg_tviews_replication_status()` (`replica_readable`), or `tviews.registry.options->'logged'`. |
| `pg_tviews_performance_stats()` | 0.1.0-beta.28 | `pg_tviews_profile()` (sizes, rows estimate, indexes). |
| `pg_tviews_set_typename(entity, name)` | 0.1.0-beta.28 | Option `typename`. |
| `tviews.registry` columns `logged`, `uncascaded_policy`, `uncascaded_table_policies`, `function_reads`, `time_refresh` (read contract v2) | 0.1.0-beta.28 | `options->'logged'`, `options->>'uncascaded_policy'`, `options->'uncascaded_tables'`, `options->'function_reads'`, `options->>'time_refresh'`. `tviews.contract_version()` returns 2. |
| Named arguments `tview_name =>`, `entity =>`, `entity_name =>`, `p_entity =>`; output columns `entity_name` (`pg_tviews_show_cascade_path`) and `tview` (`pg_tviews_profile`) | 0.1.0-beta.28 | `tview =>`; output columns `entity`, and `schema`, `name`. |
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
| TVIEWs are LOGGED by default; every option has one fixed default | 0.1.0-beta.28 | Declare `logged: false` for a TVIEW that should stay UNLOGGED when it is next created. Existing TVIEWs keep their tables. |
| `pg_tviews_create_or_replace()` options are the whole declaration: an option not passed goes back to its default on an existing TVIEW | 0.1.0-beta.28 | Pass every option the TVIEW should have; `tviews.registry.options` lists them. |
| Every function acting on one TVIEW takes `tview` as entity, `tv_<entity>` or `schema.tv_<entity>`; an unknown name fails with 42704 | 0.1.0-beta.28 | Rename named arguments to `tview =>`; match messages naming the relation (`TVIEW public.tv_post created`). |
| Limits (`max_propagation_depth`, `max_dependency_depth`, `max_queue_size`, `lock_escalation_threshold`) and `audit_enabled` are superuser settings | 0.1.0-beta.28 | Set them in `postgresql.conf` or as a superuser. |
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
