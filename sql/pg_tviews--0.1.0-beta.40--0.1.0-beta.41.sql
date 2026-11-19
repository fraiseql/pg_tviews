-- pg_tviews 0.1.0-beta.40 → 0.1.0-beta.41
--
-- Pending upgrade script: a pull request that changes the extension SQL adds its
-- statements above the re-derivation (docs/development/extension-versioning.md).
-- Released scripts are never edited.

-- pg_tviews is the only writer of a TVIEW's table (D14): the guard trigger's
-- function. The re-derivation below gives every TVIEW's table its guard.
CREATE FUNCTION @extschema@.pg_tview_guard_trigger()
RETURNS TRIGGER
LANGUAGE c
AS 'MODULE_PATHNAME', 'pg_tview_guard_trigger_wrapper';

CREATE OR REPLACE FUNCTION @extschema@.pg_tviews_catalog_revision()
RETURNS integer
LANGUAGE sql IMMUTABLE PARALLEL SAFE
AS 'SELECT 14';

-- Every TVIEW is re-derived inside the update, as its owner. The update does not
-- depend on what a TVIEW's owner chose: a TVIEW that cannot be re-derived keeps
-- needs_reregister (its refreshes skip and patch nothing) and is named in a
-- WARNING; the health check lists it (docs/operations/upgrade.md). A TVIEW whose
-- backing view stays locked past the session's lock_timeout (10 s when it set
-- none) is left so too: a lock never holds the update.
UPDATE @extschema@.pg_tview_meta SET needs_reregister = true;
DO $reregister$
DECLARE
    left_behind TEXT[];
    session_lock_timeout TEXT := pg_catalog.current_setting('lock_timeout');
BEGIN
    IF session_lock_timeout = '0' THEN
        PERFORM pg_catalog.set_config('lock_timeout', '10s', true);
    END IF;
    SELECT pg_catalog.array_agg(pg_catalog.format('%s: %s', m.table_oid::pg_catalog.regclass,
                                                  pg_catalog.split_part(r.status, E'\n', 1))
                                ORDER BY r.entity)
      INTO left_behind
      FROM @extschema@.pg_tviews_reregister_all(strict => false) r
      JOIN @extschema@.pg_tview_meta m ON m.entity = r.entity
     WHERE r.status <> 'reregistered';
    PERFORM pg_catalog.set_config('lock_timeout', session_lock_timeout, true);
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
