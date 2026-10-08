-- pg_tviews 0.1.0-beta.26 → 0.1.0-beta.27
--
-- Pending upgrade script: a pull request that changes the extension SQL adds its
-- statements here (docs/development/extension-versioning.md). Released scripts are
-- never edited.

CREATE OR REPLACE FUNCTION @extschema@.pg_tviews_catalog_revision()
RETURNS integer
LANGUAGE sql IMMUTABLE PARALLEL SAFE
AS 'SELECT 5';

-- Text-pattern schema analysis is gone: every TVIEW is analysed from its query
-- tree when it is registered.
DROP FUNCTION @extschema@.pg_tviews_analyze_select(text);
DROP FUNCTION @extschema@.pg_tviews_infer_types(text, text[]);
