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

-- tviews.registry gains uncascaded_tables and uncascaded_policy (appended).
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
    m.uncascaded_policy
FROM @extschema@.pg_tview_meta m
LEFT JOIN pg_catalog.pg_class c ON c.oid = m.table_oid
LEFT JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
LEFT JOIN pg_catalog.pg_class v ON v.oid = m.view_oid;

CREATE OR REPLACE FUNCTION @extschema@.pg_tviews_catalog_revision()
RETURNS integer
LANGUAGE sql IMMUTABLE PARALLEL SAFE
AS 'SELECT 2';

-- Registration derives more than before: re-derive every TVIEW with
-- pg_tviews_reregister_all() after the update.
UPDATE @extschema@.pg_tview_meta SET needs_reregister = true;
