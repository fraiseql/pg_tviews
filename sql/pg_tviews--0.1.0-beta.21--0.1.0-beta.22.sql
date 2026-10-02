-- pg_tviews 0.1.0-beta.21 → 0.1.0-beta.22
--
-- Pending upgrade script: a pull request that changes the extension SQL adds its
-- statements here (docs/development/extension-versioning.md). Released scripts are
-- never edited.

-- Registration derives more than before (#162, #163, #164, #165, #166): re-derive
-- every TVIEW with pg_tviews_reregister_all() after the update.
UPDATE @extschema@.pg_tview_meta SET needs_reregister = true;
