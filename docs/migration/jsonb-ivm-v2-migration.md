# jsonb_delta "v2" integration (historical)

This page described a planned "v2" integration with jsonb_delta (helper functions,
array batch operations, path fallbacks) and an upgrade to it. That plan is not what
shipped, and the steps it gave (`ALTER EXTENSION pg_tviews UPDATE TO '0.1.0'` as a
rollback, "no breaking changes") are wrong for the current releases. Do not follow it.

## What is true today

- jsonb_delta is optional. Without it every TVIEW row is recomputed from its backing
  view; with it, eligible changes are patched in place.
- pg_tviews calls two jsonb_delta functions, always qualified with the schema
  jsonb_delta is installed in: `jsonb_smart_patch_scalar` (direct patch of a TVIEW's
  own row, fan-out patch) and `jsonb_smart_patch_nested` (a child's patch under the path
  a parent embeds it at). See [architecture.md](../../architecture.md#patching-with-jsonb_delta).
- `SELECT tviews.pg_tviews_check_jsonb_delta();` says whether it is found.
- Installing or dropping jsonb_delta needs no change to the TVIEWs: the session that
  runs `CREATE`/`DROP EXTENSION jsonb_delta` looks it up again at its next refresh.
- Upgrading pg_tviews itself: `ALTER EXTENSION pg_tviews UPDATE` from 0.1.0-beta.20 on
  ([../DEPRECATION_WARNINGS.md](../DEPRECATION_WARNINGS.md),
  [../development/extension-versioning.md](../development/extension-versioning.md)).
  There is no downgrade script.
