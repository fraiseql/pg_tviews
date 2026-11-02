# jsonb_delta "v2" integration (historical)

This page described a planned "v2" integration with jsonb_delta (helper functions,
array batch operations, path fallbacks) and an upgrade to it. That plan is not what
shipped, and the steps it gave (`ALTER EXTENSION pg_tviews UPDATE TO '0.1.0'` as a
rollback, "no breaking changes") are wrong for the current releases. Do not follow it.

## What is true today

- jsonb_delta is required (since 0.1.0-beta.35): eligible changes are patched in place
  with it.
- pg_tviews calls two jsonb_delta functions, always qualified with the schema
  jsonb_delta is installed in: `jsonb_smart_patch_scalar` (direct patch of a TVIEW's
  own row, fan-out patch) and `jsonb_smart_patch_nested` (a child's patch under the path
  a parent embeds it at). See [architecture.md](../../architecture.md#patching-with-jsonb_delta).
- It cannot be dropped while pg_tviews is installed.
- Upgrading pg_tviews itself: `ALTER EXTENSION pg_tviews UPDATE` from 0.1.0-beta.20 on
  ([../DEPRECATION_WARNINGS.md](../DEPRECATION_WARNINGS.md),
  [../development/extension-versioning.md](../development/extension-versioning.md)).
  There is no downgrade script.
