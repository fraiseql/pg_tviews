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

-- Backing views move to the extension's schema, named after their TVIEW's table:
-- <schema>.v_<entity> becomes @extschema@.<schema>__tv_<entity> (#181), fitted to
-- 63 bytes as pg_tviews fits generated names (cut, then an FNV-1a hash of the full
-- name). pg_tview_meta keeps their OIDs. Run the update as a superuser, or as the
-- extension's owner when it owns every backing view.
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
        FROM @extschema@.pg_tview_meta m
        JOIN pg_catalog.pg_class v ON v.oid = m.view_oid::pg_catalog.oid
        JOIN pg_catalog.pg_class t ON t.oid = m.table_oid::pg_catalog.oid
        JOIN pg_catalog.pg_namespace tn ON tn.oid = t.relnamespace
        WHERE v.relnamespace <> '@extschema@'::pg_catalog.regnamespace
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
        EXECUTE pg_catalog.format('ALTER VIEW %s SET SCHEMA @extschema@', r.view::pg_catalog.regclass);
        EXECUTE pg_catalog.format('ALTER VIEW @extschema@.%I RENAME TO %I', r.view_name, target);
    END LOOP;
END $$;
