-- pg_tviews 0.1.0-beta.35 → 0.1.0-beta.36
--
-- Pending upgrade script: a pull request that changes the extension SQL adds its
-- statements above the re-derivation (docs/development/extension-versioning.md).
-- Released scripts are never edited.

CREATE OR REPLACE FUNCTION @extschema@.pg_tviews_catalog_revision()
RETURNS integer
LANGUAGE sql IMMUTABLE PARALLEL SAFE
AS 'SELECT 12';

-- Recompute what given rows of a base table reach, as an UPDATE of them would (#280).
CREATE FUNCTION @extschema@.pg_tviews_resync(source regclass, keys bigint[])
RETURNS TABLE (tview text, rows_changed bigint)
STRICT
LANGUAGE c
AS 'MODULE_PATHNAME', 'pg_tviews_resync_wrapper';

-- The tag of a value lock, keyed with the server's secret: superusers only.
CREATE FUNCTION @extschema@.pg_tviews_value_lock_tag(relation regclass, column_name text, value text)
RETURNS bigint
LANGUAGE c
AS 'MODULE_PATHNAME', 'pg_tviews_value_lock_tag_wrapper';
REVOKE EXECUTE ON FUNCTION @extschema@.pg_tviews_value_lock_tag(regclass, text, text) FROM PUBLIC;

-- The GraphQL type name of each TVIEW, stored: one TVIEW per name.
ALTER TABLE @extschema@.pg_tview_meta ADD COLUMN typename TEXT;
UPDATE @extschema@.pg_tview_meta m
SET typename = COALESCE(m.graphql_typename,
                        (SELECT pg_catalog.string_agg(
                                    pg_catalog.translate(pg_catalog.left(p.part, 1),
                                                         'abcdefghijklmnopqrstuvwxyz',
                                                         'ABCDEFGHIJKLMNOPQRSTUVWXYZ')
                                    || pg_catalog.substr(p.part, 2),
                                    '' ORDER BY p.n)
                         FROM pg_catalog.unnest(pg_catalog.string_to_array(m.entity, '_'))
                              WITH ORDINALITY AS p(part, n)
                         WHERE p.part <> ''),
                        '');
DO $$
DECLARE
    clashes text;
BEGIN
    SELECT pg_catalog.string_agg(pg_catalog.format('%s (%s)', c.typename, c.tviews), '; '
                                 ORDER BY c.typename)
      INTO clashes
    FROM (SELECT m.typename, pg_catalog.string_agg('tv_' || m.entity, ', ' ORDER BY m.entity) AS tviews
          FROM @extschema@.pg_tview_meta m
          GROUP BY m.typename HAVING pg_catalog.count(*) > 1) c;
    IF clashes IS NOT NULL THEN
        RAISE EXCEPTION 'pg_tviews: a GraphQL type name names one TVIEW; these are shared: %', clashes
            USING ERRCODE = '42710',
                  HINT = 'Give all but one TVIEW of each name a typename option of its own '
                         '(pg_tviews_create_or_replace(tview, definition, options => '
                         '''{"typename": "..."}'')) or drop it, then update again.';
    END IF;
END $$;
ALTER TABLE @extschema@.pg_tview_meta ALTER COLUMN typename SET NOT NULL,
    ADD CONSTRAINT pg_tview_meta_typename_key UNIQUE (typename);

-- registry.typename reads the stored name.
CREATE OR REPLACE VIEW @extschema@.registry AS
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
    m.typename
FROM @extschema@.pg_tview_meta m
LEFT JOIN pg_catalog.pg_class c ON c.oid = m.table_oid
LEFT JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
LEFT JOIN pg_catalog.pg_class v ON v.oid = m.view_oid;

-- Each TVIEW is re-derived below as its owner, and each view it reads through as
-- that view's owner: every relation a view reads must still be readable by the
-- view's owner, and every table a declared function reads by the TVIEW's owner.
-- All that are not are named in one error, before anything is re-derived.
DO $$
DECLARE
    missing text;
BEGIN
    WITH RECURSIVE chain(entity, reader, relid, depth) AS (
        SELECT m.entity, v.relowner, v.oid, 0
        FROM @extschema@.pg_tview_meta m
        JOIN pg_catalog.pg_class v ON v.oid = m.view_oid::pg_catalog.oid
      UNION
        SELECT c.entity, v.relowner, d.refobjid, c.depth + 1
        FROM chain c
        JOIN pg_catalog.pg_class v ON v.oid = c.relid AND v.relkind = 'v'
        JOIN pg_catalog.pg_rewrite w ON w.ev_class = v.oid
        JOIN pg_catalog.pg_depend d
          ON d.classid = 'pg_catalog.pg_rewrite'::pg_catalog.regclass AND d.objid = w.oid
         AND d.refclassid = 'pg_catalog.pg_class'::pg_catalog.regclass AND d.refobjid <> v.oid
        WHERE c.depth < 100
    ),
    needed(entity, reader, relid) AS (
        SELECT DISTINCT c.entity, c.reader, c.relid FROM chain c WHERE c.depth > 0
      UNION
        SELECT m.entity, t.relowner, f.relid::pg_catalog.oid
        FROM @extschema@.pg_tview_meta m
        JOIN pg_catalog.pg_class t ON t.oid = m.table_oid::pg_catalog.oid,
             pg_catalog.unnest(m.function_read_tables) AS f(relid)
        WHERE f.relid IS NOT NULL
    ),
    lacking(entity, reader, privilege) AS (
        SELECT n.entity, n.reader, pg_catalog.format('USAGE on schema %I', s.nspname)
        FROM needed n
        JOIN pg_catalog.pg_class r ON r.oid = n.relid AND r.relkind IN ('r', 'p', 'v', 'm', 'f')
        JOIN pg_catalog.pg_namespace s ON s.oid = r.relnamespace
        WHERE NOT pg_catalog.has_schema_privilege(n.reader, s.oid, 'USAGE')
      UNION
        SELECT n.entity, n.reader, pg_catalog.format('SELECT on %s', r.oid::pg_catalog.regclass)
        FROM needed n
        JOIN pg_catalog.pg_class r ON r.oid = n.relid AND r.relkind IN ('r', 'p', 'v', 'm', 'f')
        WHERE NOT pg_catalog.has_any_column_privilege(n.reader, r.oid, 'SELECT')
    )
    SELECT pg_catalog.string_agg(pg_catalog.format('tv_%s (%s lacks %s)', l.entity,
                                     pg_catalog.pg_get_userbyid(l.reader), l.privilege),
                                 '; ' ORDER BY l.entity, l.privilege)
      INTO missing
    FROM lacking l;
    IF missing IS NOT NULL THEN
        RAISE EXCEPTION 'pg_tviews: TVIEWs whose owners may no longer read what they read: %', missing
            USING ERRCODE = '42501',
                  HINT = 'Grant each privilege named, or drop the TVIEW, then update again.';
    END IF;
END $$;

-- Every TVIEW is re-derived inside the update, as its owner: an update leaves none
-- on an older plan. Re-deriving renders its plan by the current rules, takes every
-- privilege on its backing view (its columns included) from the other roles, and
-- warns about each function its refresh runs whose search_path does not end in
-- pg_temp (the health check's function_paths component lists them too).
UPDATE @extschema@.pg_tview_meta SET needs_reregister = true;
SELECT pg_catalog.count(*) FROM @extschema@.pg_tviews_reregister_all(strict => true);
