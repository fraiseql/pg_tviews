-- pg_tviews 0.1.0-beta.24 → 0.1.0-beta.25
--
-- Pending upgrade script: a pull request that changes the extension SQL adds its
-- statements here (docs/development/extension-versioning.md). Released scripts are
-- never edited.

-- Registration derives more (#182, #183): array membership (`= ANY`, `unnest`) and
-- computed subquery outputs link tables to the key, a set-returning function in a
-- subquery hides only its own output, and recursive CTEs are walked once. Re-derive
-- every TVIEW with pg_tviews_reregister_all() after the update.
UPDATE @extschema@.pg_tview_meta SET needs_reregister = true;
