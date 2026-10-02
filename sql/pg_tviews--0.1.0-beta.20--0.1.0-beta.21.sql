-- pg_tviews 0.1.0-beta.20 → 0.1.0-beta.21
--
-- Pending upgrade script: a pull request that changes the extension SQL adds its
-- statements here (docs/development/extension-versioning.md). Released scripts are
-- never edited.

-- Base tables no cascade reaches, and the policy each TVIEW was created with
-- (issues #157, #158).
ALTER TABLE @extschema@.pg_tview_meta
    ADD COLUMN uncascaded_oids REGCLASS[] NOT NULL DEFAULT '{}',
    ADD COLUMN uncascaded_policy TEXT NOT NULL DEFAULT 'warn'
        CHECK (uncascaded_policy IN ('warn', 'error', 'full_refresh'));

-- How each base table's writes map to keys, read from the view's query tree
-- (ADR 0157).
ALTER TABLE @extschema@.pg_tview_meta
    ADD COLUMN key_mappings JSONB NOT NULL DEFAULT '[]';

-- tviews.registry gains uncascaded_tables, uncascaded_policy and cascade_kinds
-- (appended).
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
        '{}') AS cascade_kinds
FROM @extschema@.pg_tview_meta m
LEFT JOIN pg_catalog.pg_class c ON c.oid = m.table_oid
LEFT JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
LEFT JOIN pg_catalog.pg_class v ON v.oid = m.view_oid;

-- Restore rebinds the relids in key_mappings as it does cascade_paths.
CREATE OR REPLACE FUNCTION @extschema@.pg_tviews_meta_rebind()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, pg_temp
AS $$
DECLARE
    olds TEXT[];
    news TEXT[];
    e JSONB;
    q TEXT;
    i INT;
    rebound JSONB := '[]';
BEGIN
    IF pg_catalog.cardinality(NEW.cascade_paths) > 0 THEN
        NEW.cascade_paths := @extschema@.pg_tviews_rebind_cascade_paths(
            NEW.view_oid::oid, NEW.cascade_paths);
    END IF;
    IF pg_catalog.jsonb_array_length(NEW.key_mappings) > 0 THEN
        -- Each table's relid in the source database, and here (found by name).
        SELECT pg_catalog.array_agg(x.e->>'relid' ORDER BY x.n),
               pg_catalog.array_agg(COALESCE(
                   pg_catalog.to_regclass(x.e->>'table')::pg_catalog.oid::pg_catalog.text,
                   x.e->>'relid') ORDER BY x.n)
          INTO olds, news
          FROM pg_catalog.jsonb_array_elements(NEW.key_mappings) WITH ORDINALITY AS x(e, n);
        FOR e IN SELECT value FROM pg_catalog.jsonb_array_elements(NEW.key_mappings) LOOP
            -- A mapping query names relations and columns by relid: {r:<relid>},
            -- {c:<relid>:<attnum>}. Marked first, so a new relid equal to another
            -- table's old one is not rebound twice.
            IF e ? 'sql' THEN
                q := e->>'sql';
                FOR i IN 1 .. pg_catalog.array_length(olds, 1) LOOP
                    q := pg_catalog.replace(pg_catalog.replace(q,
                             '{r:' || olds[i] || '}', '{r:#' || news[i] || '}'),
                             '{c:' || olds[i] || ':', '{c:#' || news[i] || ':');
                END LOOP;
                q := pg_catalog.replace(pg_catalog.replace(q, '{r:#', '{r:'), '{c:#', '{c:');
                e := pg_catalog.jsonb_set(e, '{sql}', pg_catalog.to_jsonb(q));
            END IF;
            i := pg_catalog.array_position(olds, e->>'relid');
            e := pg_catalog.jsonb_set(e, '{relid}', pg_catalog.to_jsonb(news[i]::pg_catalog.int8));
            rebound := rebound || pg_catalog.jsonb_build_array(e);
        END LOOP;
        NEW.key_mappings := rebound;
    END IF;
    RETURN NEW;
END;
$$;

DROP TRIGGER pg_tview_meta_rebind ON @extschema@.pg_tview_meta;
CREATE TRIGGER pg_tview_meta_rebind
    BEFORE INSERT ON @extschema@.pg_tview_meta
    FOR EACH ROW
    WHEN (pg_catalog.cardinality(NEW.cascade_paths) > 0
          OR pg_catalog.jsonb_array_length(NEW.key_mappings) > 0)
    EXECUTE FUNCTION @extschema@.pg_tviews_meta_rebind();

-- Writes to a mapped base table are mapped once per statement over its transition
-- tables, and TRUNCATE refreshes the whole TVIEW (ADR 0157). The statement handler
-- no trigger used goes. pg_tviews_reregister_all() installs the new triggers.
CREATE FUNCTION @extschema@."pg_tview_delta_trigger"()
	RETURNS TRIGGER
	LANGUAGE c
	AS 'MODULE_PATHNAME', 'pg_tview_delta_trigger_wrapper';
CREATE FUNCTION @extschema@."pg_tview_truncate_trigger"()
	RETURNS TRIGGER
	LANGUAGE c
	AS 'MODULE_PATHNAME', 'pg_tview_truncate_trigger_wrapper';
DROP FUNCTION @extschema@.pg_tview_stmt_trigger_handler();
CREATE  FUNCTION @extschema@."pg_tviews_mapping_query"(
	"tview" TEXT, /* &str */
	"base_table" oid /* pgrx_pg_sys::submodules::oids::Oid */
) RETURNS TEXT /* core::option::Option<alloc::string::String> */
STRICT
LANGUAGE c /* Rust */
AS 'MODULE_PATHNAME', 'pg_tviews_mapping_query_wrapper';

CREATE OR REPLACE FUNCTION @extschema@.pg_tviews_catalog_revision()
RETURNS integer
LANGUAGE sql IMMUTABLE PARALLEL SAFE
AS 'SELECT 2';

-- Registration derives more than before: re-derive every TVIEW with
-- pg_tviews_reregister_all() after the update.
UPDATE @extschema@.pg_tview_meta SET needs_reregister = true;
