-- Migrate a pg_tviews 0.1.0 install to this release, keeping the TVIEWs and their rows.
--
-- Every release up to 0.1.0-beta.19 installed its SQL as version 0.1.0, into
-- whatever schema was first on search_path. Those installs cannot be updated with
-- ALTER EXTENSION: 0.1.0 names many different catalogs, and the extension now lives
-- in the fixed schema tviews. This script re-creates the extension and re-registers
-- every TVIEW from its stored definition. The tv_* tables, their rows and anything
-- built on them are kept; each backing view v_* is kept too, moved to tviews under
-- the name its table derives (<schema>__tv_<entity>).
--
-- The audit log (pg_tview_audit_log) is NOT carried over: copy it out first if you
-- need its history.
--
-- Run it after installing the new package and restarting PostgreSQL, as the
-- extension's owner, in every database that has pg_tviews:
--
--   psql -v ON_ERROR_STOP=1 -d <database> -f scripts/migrate-from-0.1.0.sql
--
-- It runs in one transaction; on any error nothing changes. Writes to the TVIEWs'
-- base tables wait for it to finish.

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
BEGIN;

-- 0. Only a 0.1.0 install is migrated.
DO $$
DECLARE
    version TEXT := (SELECT extversion FROM pg_catalog.pg_extension
                     WHERE extname = 'pg_tviews');
BEGIN
    IF version IS NULL THEN
        RAISE EXCEPTION 'pg_tviews is not installed in database %', current_database();
    END IF;
    IF version <> '0.1.0' THEN
        RAISE EXCEPTION 'pg_tviews % is installed, not 0.1.0', version
            USING HINT = 'Update it with ALTER EXTENSION pg_tviews UPDATE.';
    END IF;
END $$;

-- 1. DROP EXTENSION ... CASCADE below would silently drop anything outside the
--    extension that depends on it (a view over pg_tviews_queue_realtime, a function
--    calling a pg_tviews_* function, a column of type tviewschema...). Refuse
--    instead, listing them. The objects the extension's own tables and views carry,
--    and the triggers pg_tviews installed on base tables (re-created in step 4), are
--    not counted.
DO $$
DECLARE
    dependents TEXT;
BEGIN
    WITH ext AS (
        SELECT oid FROM pg_catalog.pg_extension WHERE extname = 'pg_tviews'
    ),
    members AS (
        SELECT d.classid, d.objid
        FROM pg_catalog.pg_depend d, ext
        WHERE d.refclassid = 'pg_catalog.pg_extension'::pg_catalog.regclass
          AND d.refobjid = ext.oid AND d.deptype = 'e'
    ),
    member_relations AS (
        SELECT objid FROM members WHERE classid = 'pg_catalog.pg_class'::pg_catalog.regclass
    ),
    member_functions AS (
        SELECT objid FROM members WHERE classid = 'pg_catalog.pg_proc'::pg_catalog.regclass
    )
    SELECT pg_catalog.string_agg(DISTINCT
               pg_catalog.pg_describe_object(d.classid, d.objid, d.objsubid), E'\n  ')
      INTO dependents
      FROM pg_catalog.pg_depend d
      JOIN members m ON m.classid = d.refclassid AND m.objid = d.refobjid
     WHERE d.deptype = 'n'
       AND (d.classid, d.objid) NOT IN (SELECT classid, objid FROM members)
       AND NOT (d.classid = 'pg_catalog.pg_rewrite'::pg_catalog.regclass
                AND (SELECT ev_class FROM pg_catalog.pg_rewrite WHERE oid = d.objid)
                    IN (SELECT objid FROM member_relations))
       AND NOT (d.classid = 'pg_catalog.pg_trigger'::pg_catalog.regclass
                AND EXISTS (SELECT 1 FROM pg_catalog.pg_trigger t
                            WHERE t.oid = d.objid
                              AND (t.tgfoid IN (SELECT objid FROM member_functions)
                                   OR t.tgrelid IN (SELECT objid FROM member_relations))))
       AND NOT (d.classid = 'pg_catalog.pg_attrdef'::pg_catalog.regclass
                AND (SELECT adrelid FROM pg_catalog.pg_attrdef WHERE oid = d.objid)
                    IN (SELECT objid FROM member_relations))
       AND NOT (d.classid = 'pg_catalog.pg_constraint'::pg_catalog.regclass
                AND (SELECT conrelid FROM pg_catalog.pg_constraint WHERE oid = d.objid)
                    IN (SELECT objid FROM member_relations));
    -- Extensions that require pg_tviews would be dropped with it too.
    SELECT pg_catalog.concat_ws(E'\n  ', dependents,
               pg_catalog.string_agg('extension ' || e2.extname, E'\n  '))
      INTO dependents
      FROM pg_catalog.pg_depend d
      JOIN pg_catalog.pg_extension e2 ON e2.oid = d.objid
     WHERE d.classid = 'pg_catalog.pg_extension'::pg_catalog.regclass
       AND d.refclassid = 'pg_catalog.pg_extension'::pg_catalog.regclass
       AND d.refobjid = (SELECT oid FROM pg_catalog.pg_extension WHERE extname = 'pg_tviews');
    dependents := NULLIF(dependents, '');
    IF dependents IS NOT NULL THEN
        RAISE EXCEPTION E'objects outside pg_tviews depend on it and would be dropped:\n  %',
            dependents
            USING HINT = 'Drop them (and re-create them after the migration) or remove '
                         'their dependency on pg_tviews, then run this script again.';
    END IF;
END $$;

-- 2. Save the registrations.
DO $$
DECLARE
    old_schema TEXT := (SELECT n.nspname FROM pg_catalog.pg_extension e
                        JOIN pg_catalog.pg_namespace n ON n.oid = e.extnamespace
                        WHERE e.extname = 'pg_tviews');
BEGIN
    EXECUTE pg_catalog.format(
        'CREATE TEMP TABLE pg_tviews_saved_meta ON COMMIT DROP AS SELECT * FROM %I.pg_tview_meta',
        old_schema);
    EXECUTE pg_catalog.format(
        'CREATE TEMP TABLE pg_tviews_saved_helpers ON COMMIT DROP AS SELECT * FROM %I.pg_tview_helpers',
        old_schema);
END $$;
SELECT pg_catalog.count(*) AS tview_count FROM pg_temp.pg_tviews_saved_meta \gset
\echo Migrating :tview_count TVIEW(s)

-- 3. Re-create the extension. CASCADE drops the extension's objects and the
--    triggers it installed on base tables, not the tv_* tables or v_* views.
DROP EXTENSION pg_tviews CASCADE;
CREATE EXTENSION pg_tviews;

-- 4. Restore the registrations: the columns both catalogs have, cast to the new
--    types. Cascade paths hold OIDs of the old derivation, so they are emptied, and
--    every TVIEW is re-registered, which re-derives its metadata and re-installs its
--    triggers.
CREATE FUNCTION pg_temp.pg_tviews_restore(target REGCLASS, source REGCLASS, overrides JSONB)
RETURNS void LANGUAGE plpgsql AS $$
DECLARE
    columns TEXT;
    vals TEXT;
BEGIN
    SELECT pg_catalog.string_agg(pg_catalog.quote_ident(a.attname), ', ' ORDER BY a.attnum),
           pg_catalog.string_agg(COALESCE(overrides->>a.attname::text,
               pg_catalog.format('s.%I::%s', a.attname,
                                 pg_catalog.format_type(a.atttypid, a.atttypmod))),
               ', ' ORDER BY a.attnum)
      INTO columns, vals
      FROM pg_catalog.pg_attribute a
     WHERE a.attrelid = target AND a.attnum > 0 AND NOT a.attisdropped
       AND (overrides ? a.attname::text
            OR EXISTS (SELECT 1 FROM pg_catalog.pg_attribute o
                       WHERE o.attrelid = source AND o.attname = a.attname
                         AND o.attnum > 0 AND NOT o.attisdropped));
    EXECUTE pg_catalog.format('INSERT INTO %s (%s) SELECT %s FROM %s s',
                              target, columns, vals, source);
END $$;
SELECT pg_temp.pg_tviews_restore('tviews.pg_tview_meta', 'pg_temp.pg_tviews_saved_meta',
    '{"cascade_paths": "''{}''::text[]", "needs_reregister": "true"}');
SELECT pg_temp.pg_tviews_restore('tviews.pg_tview_helpers', 'pg_temp.pg_tviews_saved_helpers',
    '{}');

-- 5. Move each backing view to tviews, named after its TVIEW's table and fitted to
--    63 bytes as pg_tviews fits generated names (as the 0.1.0-beta.25 update does).
DO $$
DECLARE
    r record;
    full_name text;
    target text;
    h bigint;
    b int;
BEGIN
    FOR r IN
        SELECT v.oid AS view, v.relname::text AS view_name, tn.nspname::text AS table_schema,
               t.relname::text AS table_name
        FROM tviews.pg_tview_meta m
        JOIN pg_catalog.pg_class v ON v.oid = m.view_oid::pg_catalog.oid
        JOIN pg_catalog.pg_class t ON t.oid = m.table_oid::pg_catalog.oid
        JOIN pg_catalog.pg_namespace tn ON tn.oid = t.relnamespace
        WHERE v.relnamespace <> 'tviews'::pg_catalog.regnamespace
        ORDER BY m.entity
    LOOP
        full_name := r.table_schema || '__' || r.table_name;
        IF pg_catalog.octet_length(full_name) <= 63 THEN
            target := full_name;
        ELSE
            h := 2166136261;
            FOR i IN 0 .. pg_catalog.octet_length(full_name) - 1 LOOP
                b := pg_catalog.get_byte(pg_catalog.convert_to(full_name, 'UTF8'), i);
                h := ((h # b) * 16777619) % 4294967296;
            END LOOP;
            target := full_name;
            WHILE pg_catalog.octet_length(target) > 54 LOOP
                target := pg_catalog.left(target, -1);
            END LOOP;
            target := target || '_' || pg_catalog.lpad(pg_catalog.to_hex(h), 8, '0');
        END IF;
        EXECUTE pg_catalog.format('ALTER VIEW %s SET SCHEMA tviews', r.view::pg_catalog.regclass);
        EXECUTE pg_catalog.format('ALTER VIEW tviews.%I RENAME TO %I', r.view_name, target);
    END LOOP;
END $$;

SELECT entity, status FROM tviews.pg_tviews_reregister_all(strict => true);

COMMIT;

\echo pg_tviews migrated. Add tviews to search_path to call its functions unqualified,
\echo for example: ALTER DATABASE <database> SET search_path = "$user", public, tviews;
