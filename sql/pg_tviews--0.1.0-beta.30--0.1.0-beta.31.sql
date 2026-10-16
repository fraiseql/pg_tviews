-- pg_tviews 0.1.0-beta.30 → 0.1.0-beta.31
--
-- Pending upgrade script: a pull request that changes the extension SQL adds its
-- statements here (docs/development/extension-versioning.md). Released scripts are
-- never edited.

-- #247: the transactions committed without publishing their changes, readable by
-- every session.
CREATE  FUNCTION @extschema@."pg_tviews_feed_gaps"() RETURNS bigint /* core::result::Result<i64, pgrx_pg_sys::submodules::panic::ErrorReport> */
STRICT
LANGUAGE c /* Rust */
AS 'MODULE_PATHNAME', 'pg_tviews_feed_gaps_wrapper';
