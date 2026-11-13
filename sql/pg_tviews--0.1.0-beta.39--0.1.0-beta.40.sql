-- pg_tviews 0.1.0-beta.39 → 0.1.0-beta.40
--
-- Pending upgrade script: a pull request that changes the extension SQL adds its
-- statements above the re-derivation (docs/development/extension-versioning.md).
-- Released scripts are never edited.

CREATE OR REPLACE FUNCTION @extschema@.pg_tviews_catalog_revision()
RETURNS integer
LANGUAGE sql IMMUTABLE PARALLEL SAFE
AS 'SELECT 13';

-- Suspending refresh is an operator's, as the maintenance acting on every TVIEW
-- (docs/user-guides/operators.md).
REVOKE EXECUTE ON FUNCTION
    @extschema@.pg_tviews_suspend_triggers(),
    @extschema@.pg_tviews_resume_triggers()
FROM PUBLIC;

-- Every TVIEW is re-derived inside the update, as its owner. The update does not
-- depend on what a TVIEW's owner chose: a TVIEW that cannot be re-derived keeps
-- needs_reregister (its refreshes skip and patch nothing) and is named in a
-- WARNING; the health check lists it (docs/operations/upgrade.md).
UPDATE @extschema@.pg_tview_meta SET needs_reregister = true;
DO $reregister$
DECLARE
    left_behind TEXT[];
BEGIN
    SELECT pg_catalog.array_agg(pg_catalog.format('%s: %s', m.table_oid::pg_catalog.regclass,
                                                  pg_catalog.split_part(r.status, E'\n', 1))
                                ORDER BY r.entity)
      INTO left_behind
      FROM @extschema@.pg_tviews_reregister_all(strict => false) r
      JOIN @extschema@.pg_tview_meta m ON m.entity = r.entity
     WHERE r.status <> 'reregistered';
    IF left_behind IS NOT NULL THEN
        RAISE WARNING 'pg_tviews: % TVIEW(s) could not be re-derived and need re-registration: %',
            pg_catalog.cardinality(left_behind), pg_catalog.array_to_string(left_behind, '; ')
            USING HINT = 'Fix each and run SELECT @extschema@.pg_tviews_reregister(''<TVIEW>''); '
                         'until then its refreshes skip and patch nothing, and fail where its '
                         'definition no longer holds. @extschema@.pg_tviews_check_health() '
                         'lists them (component reregister).';
    END IF;
END
$reregister$;
