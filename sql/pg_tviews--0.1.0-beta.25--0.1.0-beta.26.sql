-- pg_tviews 0.1.0-beta.25 → 0.1.0-beta.26
--
-- Pending upgrade script: a pull request that changes the extension SQL adds its
-- statements here (docs/development/extension-versioning.md). Released scripts are
-- never edited.

-- Registration derives more (#187, #188, #189, #191): a read under window
-- functions all partitioned by a linked column maps through the partition, UNION
-- branches keyed by their own tables (a column, or an expression of one row) get
-- a root each, a materialized view read is uncascaded (refused under the error
-- policy), and a read of another TVIEW's table is traced like a base table's.
-- Re-derive every TVIEW with pg_tviews_reregister_all() after the update.
UPDATE @extschema@.pg_tview_meta SET needs_reregister = true;

-- A TVIEW's table dropped by DROP OWNED (which names a role's objects directly)
-- is deregistered like one dropped as a dependent (#186).
CREATE OR REPLACE FUNCTION @extschema@.pg_tviews_handle_drop_event()
RETURNS event_trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, @extschema@, pg_temp
AS $$
DECLARE
    entity_name TEXT;
BEGIN
    FOR entity_name IN
        SELECT DISTINCT m.entity
        FROM pg_catalog.pg_event_trigger_dropped_objects() AS d
        JOIN @extschema@.pg_tview_meta AS m
          ON d.objid IN (m.view_oid, m.table_oid)
        WHERE (NOT d.original OR TG_TAG = 'DROP OWNED')
          AND d.classid = 'pg_catalog.pg_class'::pg_catalog.regclass
          AND d.objsubid = 0
    LOOP
        BEGIN
            PERFORM @extschema@.pg_tviews_handle_dropped(entity_name);
        EXCEPTION WHEN OTHERS THEN
            -- Never abort the user's DROP.
            RAISE WARNING 'pg_tviews: cleanup of tv_% after a dependent drop failed: %',
                entity_name, SQLERRM;
        END;
    END LOOP;
END;
$$;

-- Backing views beta.25 left behind when a TVIEW's table went with its schema
-- (DROP SCHEMA … CASCADE) or its owner's objects (#186): views named
-- <schema>__tv_<entity> in @extschema@ that no TVIEW owns block re-creating the
-- TVIEW. Dropped unless something depends on them.
DO $$
DECLARE
    r record;
BEGIN
    FOR r IN
        SELECT v.oid::pg_catalog.regclass AS view
        FROM pg_catalog.pg_class v
        WHERE v.relnamespace = '@extschema@'::pg_catalog.regnamespace
          AND v.relkind = 'v'
          AND v.relname LIKE '%\_\_tv\_%'
          AND NOT EXISTS (SELECT 1 FROM @extschema@.pg_tview_meta m
                          WHERE m.view_oid::pg_catalog.oid = v.oid)
          AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_depend d
                          WHERE d.classid = 'pg_catalog.pg_class'::pg_catalog.regclass
                            AND d.objid = v.oid AND d.deptype = 'e')
    LOOP
        BEGIN
            EXECUTE pg_catalog.format('DROP VIEW %s', r.view);
        EXCEPTION WHEN dependent_objects_still_exist THEN
            RAISE NOTICE 'pg_tviews: % belongs to no TVIEW but other objects depend on it; kept',
                r.view;
        END;
    END LOOP;
END $$;

-- Tables declared with a policy of their own (#195): uncascaded_table_policies[i]
-- applies to writes to uncascaded_table_oids[i].
ALTER TABLE @extschema@.pg_tview_meta
    ADD COLUMN uncascaded_table_oids REGCLASS[] NOT NULL DEFAULT '{}',
    ADD COLUMN uncascaded_table_policies TEXT[] NOT NULL DEFAULT '{}';

-- The functions a TVIEW calls, declared with the tables each reads (#193).
ALTER TABLE @extschema@.pg_tview_meta
    ADD COLUMN function_read_functions TEXT[] NOT NULL DEFAULT '{}',
    ADD COLUMN function_read_tables REGCLASS[] NOT NULL DEFAULT '{}';

-- Whether a TVIEW reads the current time, and who brings it up to date (#193).
ALTER TABLE @extschema@.pg_tview_meta
    ADD COLUMN time_refresh TEXT CHECK (time_refresh IN ('external')),
    ADD COLUMN time_dependent BOOLEAN NOT NULL DEFAULT false;

-- Refreshes the TVIEWs that read the time, at the boundary (#193).
CREATE  FUNCTION @extschema@."pg_tviews_refresh_time_dependent"(
	"tview" TEXT DEFAULT NULL /* core::option::Option<&str> */
) RETURNS SETOF TEXT /* alloc::string::String */
LANGUAGE c /* Rust */
AS 'MODULE_PATHNAME', 'pg_tviews_refresh_time_dependent_wrapper';

-- The tables those functions read are read by the TVIEW (#193).
CREATE OR REPLACE VIEW @extschema@.pg_tview_reads AS
WITH RECURSIVE reads(entity, relid) AS (
    SELECT m.entity, m.view_oid::oid FROM @extschema@.pg_tview_meta m
  UNION
    -- Tables read inside the functions it calls, as declared (issue #193).
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

-- tviews.registry gains uncascaded_table_policies (#195), function_reads,
-- time_dependent and time_refresh (#193), appended.
CREATE OR REPLACE VIEW @extschema@.registry AS
SELECT
    n.nspname::text AS schema,
    COALESCE(c.relname::text, 'tv_' || m.entity) AS name,
    m.entity,
    m.definition AS query,
    COALESCE(
        (SELECT pg_catalog.array_agg(b.oid::pg_catalog.regclass ORDER BY bn.nspname, b.relname)
         FROM (SELECT DISTINCT r.relid FROM @extschema@.pg_tview_reads r
               WHERE r.entity = m.entity) x
         JOIN pg_catalog.pg_class b ON b.oid = x.relid AND b.relkind IN ('r', 'p', 'f', 'm')
         JOIN pg_catalog.pg_namespace bn ON bn.oid = b.relnamespace),
        '{}') AS base_tables,
    c.relpersistence = 'p' AS logged,
    CASE WHEN c.oid IS NOT NULL THEN pg_catalog.jsonb_build_object(
        'logged', c.relpersistence = 'p',
        'fillfactor', COALESCE(
            (SELECT o.option_value::integer
             FROM pg_catalog.pg_options_to_table(c.reloptions) o
             WHERE o.option_name = 'fillfactor'),
            100),
        'data_gin_index', EXISTS (
            SELECT 1
            FROM pg_catalog.pg_index i
            JOIN pg_catalog.pg_class ic ON ic.oid = i.indexrelid
            JOIN pg_catalog.pg_am am ON am.oid = ic.relam AND am.amname = 'gin'
            JOIN pg_catalog.pg_opclass oc ON oc.oid = i.indclass[0]
             AND oc.opcname = 'jsonb_ops'
            JOIN pg_catalog.pg_attribute a
              ON a.attrelid = c.oid AND a.attname = 'data' AND a.attnum = i.indkey[0]
            WHERE i.indrelid = c.oid AND i.indnatts = 1 AND i.indpred IS NULL
              AND i.indisvalid),
        'group_keys', m.group_keys) END AS options,
    m.needs_reregister,
    v.oid::pg_catalog.regclass AS view,
    m.uncascaded_oids AS uncascaded_tables,
    m.uncascaded_policy,
    COALESCE(
        (SELECT pg_catalog.jsonb_object_agg(
                    (e->>'relid')::pg_catalog.oid::pg_catalog.regclass::pg_catalog.text,
                    e->>'kind')
         FROM pg_catalog.jsonb_array_elements(m.key_mappings) e),
        '{}') AS cascade_kinds,
    CASE WHEN m.identity IS NULL THEN ARRAY['pk_' || m.entity]
         ELSE ARRAY(SELECT c->>'name'
                    FROM pg_catalog.jsonb_array_elements(m.identity->'columns') c) END AS identity,
    COALESCE(
        (SELECT pg_catalog.jsonb_object_agg(t.relation::pg_catalog.text, t.policy)
         FROM ROWS FROM (pg_catalog.unnest(m.uncascaded_table_oids),
                         pg_catalog.unnest(m.uncascaded_table_policies)) AS t(relation, policy)),
        '{}') AS uncascaded_table_policies,
    COALESCE(
        (SELECT pg_catalog.jsonb_object_agg(f.function, f.tables)
         FROM (SELECT r.function,
                      COALESCE(pg_catalog.jsonb_agg(r.relation::pg_catalog.text ORDER BY r.n)
                                   FILTER (WHERE r.relation IS NOT NULL),
                               '[]') AS tables
               FROM ROWS FROM (pg_catalog.unnest(m.function_read_functions),
                               pg_catalog.unnest(m.function_read_tables))
                    WITH ORDINALITY AS r(function, relation, n)
               GROUP BY r.function) f),
        '{}') AS function_reads,
    m.time_dependent,
    m.time_refresh
FROM @extschema@.pg_tview_meta m
LEFT JOIN pg_catalog.pg_class c ON c.oid = m.table_oid
LEFT JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
LEFT JOIN pg_catalog.pg_class v ON v.oid = m.view_oid;

CREATE OR REPLACE FUNCTION @extschema@.pg_tviews_catalog_revision()
RETURNS integer
LANGUAGE sql IMMUTABLE PARALLEL SAFE
AS 'SELECT 4';
