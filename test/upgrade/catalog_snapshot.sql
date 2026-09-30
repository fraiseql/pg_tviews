-- A text snapshot of the pg_tviews catalog in the current database, one line per
-- fact, sorted and free of OIDs, so that an upgraded database and a fresh install
-- can be compared with diff (test/upgrade/upgrade_check.sh). Column order in the
-- extension's tables is not compared: an upgrade appends columns.
--
--   psql -X -At -d <database> -f test/upgrade/catalog_snapshot.sql

\set ON_ERROR_STOP on
SET search_path TO pg_catalog;

WITH ext AS (
    SELECT oid, extversion, extconfig, extcondition, extnamespace
    FROM pg_extension WHERE extname = 'pg_tviews'
),
members AS (
    SELECT d.classid, d.objid
    FROM pg_depend d, ext
    WHERE d.refclassid = 'pg_extension'::regclass AND d.refobjid = ext.oid AND d.deptype = 'e'
),
member_relations AS (
    SELECT c.* FROM pg_class c
    JOIN members m ON m.classid = 'pg_class'::regclass AND m.objid = c.oid
),
member_functions AS (
    SELECT p.* FROM pg_proc p
    JOIN members m ON m.classid = 'pg_proc'::regclass AND m.objid = p.oid
),
facts(fact) AS (
    SELECT 'extension version ' || extversion FROM ext
  UNION ALL
    SELECT 'extension config ' || coalesce(
        (SELECT string_agg(c::regclass::text || ' where ' || coalesce(w, ''), ', '
                           ORDER BY c::regclass::text)
         FROM unnest(extconfig, extcondition) AS u(c, w)), '')
    FROM ext
  UNION ALL
    SELECT 'schema ' || n.nspname || ' owner ' || pg_get_userbyid(n.nspowner)
           || ' acl ' || coalesce(n.nspacl::text, '')
    FROM pg_namespace n, ext WHERE n.oid = ext.extnamespace
  UNION ALL
    SELECT 'member ' || pg_describe_object(classid, objid, 0) FROM members
  UNION ALL
    SELECT 'function ' || p.oid::regprocedure::text
           || ' volatility ' || p.provolatile::text || ' strict ' || p.proisstrict
           || ' security_definer ' || p.prosecdef
           || ' config ' || coalesce(p.proconfig::text, '')
           || ' acl ' || coalesce(p.proacl::text, '')
           || E'\n' || pg_get_functiondef(p.oid)
    FROM member_functions p WHERE p.prokind IN ('f', 'p')
  UNION ALL
    SELECT 'view ' || c.oid::regclass::text || ' acl ' || coalesce(c.relacl::text, '')
           || E'\n' || pg_get_viewdef(c.oid)
    FROM member_relations c WHERE c.relkind = 'v'
  UNION ALL
    SELECT 'table ' || c.oid::regclass::text || ' persistence ' || c.relpersistence::text
           || ' acl ' || coalesce(c.relacl::text, '')
    FROM member_relations c WHERE c.relkind = 'r'
  UNION ALL
    SELECT 'column ' || c.oid::regclass::text || '.' || a.attname
           || ' ' || format_type(a.atttypid, a.atttypmod)
           || CASE WHEN a.attnotnull THEN ' not null' ELSE '' END
           || coalesce(' default ' || pg_get_expr(d.adbin, d.adrelid), '')
    FROM member_relations c
    JOIN pg_attribute a ON a.attrelid = c.oid AND a.attnum > 0 AND NOT a.attisdropped
    LEFT JOIN pg_attrdef d ON d.adrelid = c.oid AND d.adnum = a.attnum
    WHERE c.relkind = 'r'
  UNION ALL
    SELECT 'constraint ' || c.oid::regclass::text || ' ' || k.conname || ' '
           || pg_get_constraintdef(k.oid)
    FROM member_relations c JOIN pg_constraint k ON k.conrelid = c.oid
  UNION ALL
    SELECT 'index ' || pg_get_indexdef(i.indexrelid)
    FROM member_relations c JOIN pg_index i ON i.indrelid = c.oid
  UNION ALL
    SELECT 'trigger ' || pg_get_triggerdef(t.oid)
    FROM member_relations c JOIN pg_trigger t ON t.tgrelid = c.oid AND NOT t.tgisinternal
  UNION ALL
    SELECT 'sequence ' || c.oid::regclass::text || ' ' || format_type(s.seqtypid, NULL)
           || ' start ' || s.seqstart || ' increment ' || s.seqincrement
           || ' min ' || s.seqmin || ' max ' || s.seqmax || ' cycle ' || s.seqcycle
           || ' acl ' || coalesce(c.relacl::text, '')
    FROM member_relations c JOIN pg_sequence s ON s.seqrelid = c.oid
  UNION ALL
    SELECT 'type ' || format_type(t.oid, NULL) || ' ' || t.typtype::text || ' input '
           || t.typinput::text || ' output ' || t.typoutput::text
           || ' acl ' || coalesce(t.typacl::text, '')
    FROM pg_type t JOIN members m ON m.classid = 'pg_type'::regclass AND m.objid = t.oid
  UNION ALL
    SELECT 'event trigger ' || e.evtname || ' on ' || e.evtevent || ' calls '
           || e.evtfoid::regprocedure::text || ' enabled ' || e.evtenabled::text
           || ' tags ' || coalesce(e.evttags::text, '')
    FROM pg_event_trigger e
    JOIN members m ON m.classid = 'pg_event_trigger'::regclass AND m.objid = e.oid
  UNION ALL
    SELECT 'comment ' || pg_describe_object(d.classoid, d.objoid, d.objsubid) || ': '
           || d.description
    FROM pg_description d
    WHERE (d.classoid, d.objoid) IN (SELECT classid, objid FROM members)
)
SELECT fact FROM facts ORDER BY fact;
