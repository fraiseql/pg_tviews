-- The relations each TVIEW's backing view reads, followed through views: the
-- tables its triggers belong on, the dependency order of TVIEWs, and the
-- registry's base_tables. Plain SQL over the catalogs.
CREATE VIEW @extschema@.pg_tview_reads AS
WITH RECURSIVE reads(entity, relid) AS (
    SELECT m.entity, m.view_oid::oid FROM @extschema@.pg_tview_meta m
  UNION
    -- Tables read inside the functions it calls, as declared.
    SELECT m.entity, t.relid::oid
    FROM @extschema@.pg_tview_meta m,
         pg_catalog.unnest(m.function_read_tables) AS t(relid)
    WHERE t.relid IS NOT NULL
  UNION
    SELECT r.entity, d.refobjid
    FROM reads r
    JOIN pg_catalog.pg_class v ON v.oid = r.relid AND v.relkind = 'v'
    JOIN pg_catalog.pg_rewrite w ON w.ev_class = v.oid
    JOIN pg_catalog.pg_depend d
      ON d.classid = 'pg_catalog.pg_rewrite'::pg_catalog.regclass
     AND d.objid = w.oid
     AND d.refclassid = 'pg_catalog.pg_class'::pg_catalog.regclass
     AND d.refobjid <> v.oid
)
SELECT entity, relid FROM reads;

COMMENT ON VIEW @extschema@.pg_tview_reads IS
'Internal: relations each TVIEW reads, through views; may change in any release';

GRANT SELECT ON @extschema@.pg_tview_reads TO PUBLIC;
