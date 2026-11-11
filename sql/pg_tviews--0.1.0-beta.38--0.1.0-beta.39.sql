-- pg_tviews 0.1.0-beta.38 → 0.1.0-beta.39
--
-- Pending upgrade script: a pull request that changes the extension SQL adds its
-- statements above the re-derivation (docs/development/extension-versioning.md).
-- Released scripts are never edited.

-- Every TVIEW is re-derived inside the update, as its owner: an update leaves none
-- on an older plan.
UPDATE @extschema@.pg_tview_meta SET needs_reregister = true;
SELECT pg_catalog.count(*) FROM @extschema@.pg_tviews_reregister_all(strict => true);
