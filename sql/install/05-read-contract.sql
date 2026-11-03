-- The read contract for tools (ADR 0136 Decision 4; version 2, ADR 0211). Plain SQL over
-- the internal tables and the system catalogs, calling no function of the library,
-- so it can be read without the library, with a mismatched one, and on a standby.
-- contract_version() covers the view's columns, the `options` keys and the
-- behaviour of pg_tviews_create_or_replace(): additions keep it, anything else
-- bumps it (docs/reference/read-contract.md).
CREATE FUNCTION @extschema@.contract_version()
RETURNS integer
LANGUAGE sql STABLE PARALLEL SAFE
AS 'SELECT 3';

COMMENT ON FUNCTION @extschema@.contract_version() IS
'Version of the read contract: @extschema@.registry and pg_tviews_create_or_replace()';

-- The references of each TVIEW (plan version 2) that name another registered
-- TVIEW: the referenced column is the source of that TVIEW's identity, a column of
-- its root table. Resolved live, so a parent registered after its child is found,
-- and by name through pg_class: a writer without USAGE on that table's schema
-- loads every TVIEW's references (#251).
-- One row per reference and parent. Of two TVIEWs named by one key, `chosen` marks
-- the parent: the one whose identity column has the referenced column's name
-- (`preferred`; a parent without an identity never wins), else the first by entity.
-- The registry and the change feed both publish the chosen one.
-- `index_name` is the managed index (or a constraint's) leading with the column.
CREATE VIEW @extschema@.references AS
SELECT x.*,
       pg_catalog.row_number() OVER (PARTITION BY x.entity, x.column_name
                                     ORDER BY x.preferred DESC NULLS LAST, x.parent) = 1
       AS chosen
FROM (SELECT m.entity,
             r.column_name,
             p.entity AS parent,
             p.identity->'columns'->0->>'name' AS parent_column,
             p.table_oid::pg_catalog.oid AS parent_table,
             r.is_unique,
             r.is_nullable,
             (SELECT pg_catalog.format('%I.%I', inn.nspname, ic.relname)
              FROM pg_catalog.pg_index i
              JOIN pg_catalog.pg_class ic ON ic.oid = i.indexrelid
              JOIN pg_catalog.pg_namespace inn ON inn.oid = ic.relnamespace
              JOIN pg_catalog.pg_attribute a ON a.attrelid = i.indrelid AND a.attnum = i.indkey[0]
              WHERE i.indrelid = m.table_oid::pg_catalog.oid AND a.attname = r.column_name
                AND i.indisvalid AND i.indpred IS NULL
                AND (ic.relname = ANY (m.managed_index_names)
                     OR EXISTS (SELECT 1 FROM pg_catalog.pg_constraint k
                                WHERE k.conindid = i.indexrelid))
              ORDER BY ic.relname = ANY (m.managed_index_names) DESC, ic.relname
              LIMIT 1) AS index_name,
             ARRAY(SELECT c->>'name'
                   FROM pg_catalog.jsonb_array_elements(m.identity->'columns') c) AS order_by,
             p.identity->'columns'->0->>'name' = r.key AS preferred
      FROM @extschema@.pg_tview_meta m
      CROSS JOIN LATERAL @extschema@.pg_tviews_plan_references(m.plan) r
      JOIN @extschema@.pg_tview_meta p
        ON EXISTS (SELECT 1 FROM @extschema@.pg_tviews_plan_roots(p.plan) pr
                   WHERE pr.key = r.key
                     AND EXISTS (SELECT 1 FROM pg_catalog.pg_class sc
                                 JOIN pg_catalog.pg_namespace sn ON sn.oid = sc.relnamespace
                                 WHERE sc.oid = pr.relid
                                   AND pg_catalog.format('%I.%I', sn.nspname, sc.relname)
                                       = r.table_name))) x;

COMMENT ON VIEW @extschema@.references IS
'Internal: the references tviews.registry publishes; may change in any release';
GRANT SELECT ON @extschema@.references TO PUBLIC;

-- A registration whose table is gone stays visible, with NULL for what the table
-- would tell; `view` is NULL once the view is gone. managed_indexes are the
-- recorded indexes still on the table, but not one backing a constraint;
-- data_gin_index is the managed GIN index on data, when valid.
CREATE VIEW @extschema@.registry AS
SELECT
    n.nspname::text AS schema,
    COALESCE(c.relname::text, 'tv_' || m.entity) AS name,
    m.entity,
    m.definition AS query,
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
            JOIN pg_catalog.pg_attribute a
              ON a.attrelid = c.oid AND a.attname = 'data' AND a.attnum = i.indkey[0]
            WHERE i.indrelid = c.oid AND i.indnatts = 1 AND i.indpred IS NULL
              AND i.indisvalid AND ic.relname = ANY (m.managed_index_names)),
        'data_compression', (
            SELECT CASE a.attcompression WHEN 'l' THEN 'lz4' WHEN 'p' THEN 'pglz'
                   ELSE pg_catalog.current_setting('default_toast_compression') END
            FROM pg_catalog.pg_attribute a WHERE a.attrelid = c.oid AND a.attname = 'data'),
        'data_storage', (
            SELECT CASE a.attstorage WHEN 'm' THEN 'main' WHEN 'e' THEN 'external'
                   WHEN 'x' THEN 'extended' ELSE 'plain' END
            FROM pg_catalog.pg_attribute a WHERE a.attrelid = c.oid AND a.attname = 'data'),
        'toast_tuple_target',
            (SELECT o.option_value::integer
             FROM pg_catalog.pg_options_to_table(c.reloptions) o
             WHERE o.option_name = 'toast_tuple_target'),
        'indexes', m.declared_indexes,
        'group_keys', m.group_keys,
        'uncascaded_policy', m.uncascaded_policy,
        'uncascaded_tables', COALESCE(
            (SELECT pg_catalog.jsonb_object_agg(t.relation::pg_catalog.text, t.policy)
             FROM ROWS FROM (pg_catalog.unnest(m.uncascaded_table_oids),
                             pg_catalog.unnest(m.uncascaded_table_policies)) AS t(relation, policy)),
            '{}'),
        'function_reads', COALESCE(
            (SELECT pg_catalog.jsonb_object_agg(f.function, f.tables)
             FROM (SELECT r.function,
                          COALESCE(pg_catalog.jsonb_agg(r.relation::pg_catalog.text ORDER BY r.n)
                                       FILTER (WHERE r.relation IS NOT NULL),
                                   '[]') AS tables
                   FROM ROWS FROM (pg_catalog.unnest(m.function_read_functions),
                                   pg_catalog.unnest(m.function_read_tables))
                        WITH ORDINALITY AS r(function, relation, n)
                   GROUP BY r.function) f),
            '{}'),
        'time_refresh', m.time_refresh,
        'typename', m.graphql_typename) END AS options,
    v.oid::pg_catalog.regclass AS view,
    CASE WHEN m.identity IS NULL THEN ARRAY['pk_' || m.entity]
         ELSE ARRAY(SELECT c->>'name'
                    FROM pg_catalog.jsonb_array_elements(m.identity->'columns') c) END AS identity,
    COALESCE(
        (SELECT pg_catalog.array_agg(b.oid::pg_catalog.regclass ORDER BY bn.nspname, b.relname)
         FROM (SELECT DISTINCT r.relid FROM @extschema@.reads r
               WHERE r.entity = m.entity) x
         JOIN pg_catalog.pg_class b ON b.oid = x.relid AND b.relkind IN ('r', 'p', 'f', 'm')
         JOIN pg_catalog.pg_namespace bn ON bn.oid = b.relnamespace),
        '{}') AS base_tables,
    COALESCE(
        (SELECT pg_catalog.jsonb_object_agg(e.relid::pg_catalog.regclass::pg_catalog.text, e.kind)
         FROM @extschema@.pg_tviews_plan_tables(m.plan) e),
        '{}') AS cascade_kinds,
    m.uncascaded_oids AS uncascaded_tables,
    m.time_dependent,
    CASE WHEN c.oid IS NOT NULL THEN ARRAY(
        SELECT i.indexrelid::pg_catalog.regclass
        FROM pg_catalog.pg_index i
        JOIN pg_catalog.pg_class ic ON ic.oid = i.indexrelid
        WHERE i.indrelid = c.oid AND ic.relname = ANY (m.managed_index_names)
          AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_constraint k
                          WHERE k.conindid = i.indexrelid)
        ORDER BY ic.relname) END AS managed_indexes,
    m.needs_reregister,
    COALESCE(
        (SELECT pg_catalog.jsonb_object_agg(x.column_name, pg_catalog.jsonb_build_object(
                    'entity', x.parent, 'column', x.parent_column, 'unique', x.is_unique,
                    'nullable', x.is_nullable, 'index', x.index_name,
                    'order', pg_catalog.to_jsonb(x.order_by)))
         FROM @extschema@.references x
         WHERE x.entity = m.entity AND x.chosen),
        '{}') AS references,
    -- The type name the report and the change feed use, as crate::catalog::typename
    -- derives it: the typename option, else the PascalCase of the entity, capitalising
    -- ASCII letters only (translate, not upper(): no locale changes the name).
    COALESCE(m.graphql_typename,
             (SELECT pg_catalog.string_agg(
                         pg_catalog.translate(pg_catalog.left(p.part, 1),
                                              'abcdefghijklmnopqrstuvwxyz',
                                              'ABCDEFGHIJKLMNOPQRSTUVWXYZ')
                         || pg_catalog.substr(p.part, 2),
                         '' ORDER BY p.n)
              FROM pg_catalog.unnest(pg_catalog.string_to_array(m.entity, '_'))
                   WITH ORDINALITY AS p(part, n)
              WHERE p.part <> ''),
             '') AS typename
FROM @extschema@.pg_tview_meta m
LEFT JOIN pg_catalog.pg_class c ON c.oid = m.table_oid
LEFT JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
LEFT JOIN pg_catalog.pg_class v ON v.oid = m.view_oid;

COMMENT ON VIEW @extschema@.registry IS
'One row per registered TVIEW; stable under contract_version()';

GRANT SELECT ON @extschema@.registry TO PUBLIC;
