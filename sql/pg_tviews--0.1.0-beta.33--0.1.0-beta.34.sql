-- pg_tviews 0.1.0-beta.33 → 0.1.0-beta.34
--
-- Pending upgrade script: a pull request that changes the extension SQL adds its
-- statements here (docs/development/extension-versioning.md). Released scripts are
-- never edited.

-- Plan version 3 records, per table, the columns each TVIEW's rows are linked by
-- (#263): a transaction that changes none of them takes no shared value lock
-- when it refreshes. A version 2 plan reads as one whose links are unknown, which
-- keeps the old locking: re-derive every TVIEW, dependencies first.
DO $$
DECLARE
    failed TEXT;
BEGIN
    SELECT pg_catalog.string_agg(pg_catalog.format('tv_%s: %s', r.entity, r.status), '; ')
      INTO failed
      FROM @extschema@.pg_tviews_reregister_all() r
     WHERE r.status <> 'reregistered';
    IF failed IS NOT NULL THEN
        RAISE EXCEPTION 'pg_tviews: these TVIEWs could not be re-derived: %', failed
            USING HINT = 'Fix or drop them on the previous release, then update again.';
    END IF;
END
$$;
