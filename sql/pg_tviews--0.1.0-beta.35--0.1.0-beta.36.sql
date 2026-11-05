-- pg_tviews 0.1.0-beta.35 → 0.1.0-beta.36
--
-- Pending upgrade script: a pull request that changes the extension SQL adds its
-- statements above the re-derivation (docs/development/extension-versioning.md).
-- Released scripts are never edited.

CREATE OR REPLACE FUNCTION @extschema@.pg_tviews_catalog_revision()
RETURNS integer
LANGUAGE sql IMMUTABLE PARALLEL SAFE
AS 'SELECT 12';

-- Recompute what given rows of a base table reach, as an UPDATE of them would (#280).
CREATE FUNCTION @extschema@.pg_tviews_resync(source regclass, keys bigint[])
RETURNS TABLE (tview text, rows_changed bigint)
STRICT
LANGUAGE c
AS 'MODULE_PATHNAME', 'pg_tviews_resync_wrapper';

-- The tag of a value lock, keyed with the server's secret: superusers only.
CREATE FUNCTION @extschema@.pg_tviews_value_lock_tag(relation regclass, column_name text, value text)
RETURNS bigint
LANGUAGE c
AS 'MODULE_PATHNAME', 'pg_tviews_value_lock_tag_wrapper';
REVOKE EXECUTE ON FUNCTION @extschema@.pg_tviews_value_lock_tag(regclass, text, text) FROM PUBLIC;

-- Every TVIEW is re-derived inside the update, as its owner: an update leaves none
-- on an older plan.
UPDATE @extschema@.pg_tview_meta SET needs_reregister = true;
SELECT pg_catalog.count(*) FROM @extschema@.pg_tviews_reregister_all(strict => true);
