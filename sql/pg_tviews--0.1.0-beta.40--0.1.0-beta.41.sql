-- pg_tviews 0.1.0-beta.40 → 0.1.0-beta.41
--
-- Pending upgrade script: a pull request that changes the extension SQL adds its
-- statements above the re-derivation (docs/development/extension-versioning.md).
-- Released scripts are never edited.

-- pg_tviews is the only writer of a TVIEW's table (D14): the guard trigger's
-- function. The re-derivation below gives every TVIEW's table its guard.
CREATE FUNCTION @extschema@.pg_tview_guard_trigger()
RETURNS TRIGGER
LANGUAGE c
AS 'MODULE_PATHNAME', 'pg_tview_guard_trigger_wrapper';

-- The id of a row of a TVIEW created from a SELECT without one (E1-14): derived
-- from the entity and the key, the same on every evaluation. A TVIEW created
-- that way before keeps gen_random_uuid() in its backing view, which the
-- re-derivation below refuses (volatile, 0A000): it is named in the WARNING and
-- created again with pg_tviews_create_or_replace.
CREATE FUNCTION @extschema@.pg_tviews_row_id(name TEXT, key TEXT)
RETURNS uuid
IMMUTABLE STRICT PARALLEL SAFE
LANGUAGE c
AS 'MODULE_PATHNAME', 'pg_tviews_row_id_wrapper';

-- The settings option (E1-01): custom settings pinned in a TVIEW's owner frame.
ALTER TABLE @extschema@.pg_tview_meta ADD COLUMN frame_settings JSONB NOT NULL DEFAULT '{}';

-- The audit log and its sequence are dumped as extension configuration (E1-12).
SELECT pg_catalog.pg_extension_config_dump('@extschema@.pg_tview_audit_log', '');
SELECT pg_catalog.pg_extension_config_dump('@extschema@.pg_tview_audit_log_log_id_seq', '');

-- The instant each time-dependent TVIEW's rows were last brought to by the
-- time refresh: its transaction's now(). Its own table, not a column of
-- pg_tview_meta: a write there invalidates every backend's cached metadata.
CREATE TABLE @extschema@.pg_tview_time_boundary (
    entity TEXT NOT NULL PRIMARY KEY
        REFERENCES @extschema@.pg_tview_meta (entity) ON DELETE CASCADE,
    boundary TIMESTAMPTZ NOT NULL
);
COMMENT ON TABLE @extschema@.pg_tview_time_boundary IS
    'Internal: the instant the time refresh last brought each TVIEW to; may change in any release';
GRANT SELECT ON @extschema@.pg_tview_time_boundary TO PUBLIC;
SELECT pg_catalog.pg_extension_config_dump('@extschema@.pg_tview_time_boundary', '');

-- Keys of time-dependent TVIEWs written, after a time boundary was announced,
-- by a transaction that began before it: its rows were computed at an earlier
-- now(). The drain refreshes them again once the writer commits. pk NULL: the
-- whole TVIEW. Not granted: keys are a TVIEW's rows.
CREATE TABLE @extschema@.pg_tview_time_handoff (
    entity TEXT NOT NULL
        REFERENCES @extschema@.pg_tview_meta (entity) ON DELETE CASCADE,
    pk BIGINT,
    boundary TIMESTAMPTZ NOT NULL,
    handed_at TIMESTAMPTZ NOT NULL DEFAULT pg_catalog.clock_timestamp()
);
CREATE INDEX pg_tview_time_handoff_entity ON @extschema@.pg_tview_time_handoff (entity);
COMMENT ON TABLE @extschema@.pg_tview_time_handoff IS
    'Internal: keys written across a time boundary, refreshed again by the drain; may change in any release';

-- registry.time_boundary reads it.
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
    m.typename,
    (SELECT b.boundary FROM @extschema@.pg_tview_time_boundary b
     WHERE b.entity = m.entity) AS time_boundary
FROM @extschema@.pg_tview_meta m
LEFT JOIN pg_catalog.pg_class c ON c.oid = m.table_oid
LEFT JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
LEFT JOIN pg_catalog.pg_class v ON v.oid = m.view_oid;

-- The time refresh at the boundary, one transaction per TVIEW, dependencies first:
-- a writer waits at most for one TVIEW's rows, and each TVIEW's boundary is recorded
-- as it commits. Procedures, so they must be CALLed outside a transaction block.
-- They have no SET search_path, as a procedure with a SET clause cannot COMMIT: they
-- run as their caller and name every object with its schema.

-- The operator's time refresh (E1-10): every time-dependent TVIEW (or one),
-- whoever owns it, each refreshed as its owner. Revoked from PUBLIC below.
CREATE FUNCTION @extschema@.pg_tviews_refresh_time_dependent_all(tview TEXT DEFAULT NULL)
RETURNS SETOF TEXT
LANGUAGE c
AS 'MODULE_PATHNAME', 'pg_tviews_refresh_time_dependent_all_wrapper';

-- Every time-dependent TVIEW, dependencies first (an entity after every TVIEW its
-- backing view reads), and whether the caller may refresh it as its owner.
CREATE FUNCTION @extschema@.pg_tviews_time_dependent_order()
RETURNS TABLE (tview TEXT, owned BOOLEAN)
LANGUAGE sql STABLE
SET search_path = pg_catalog, pg_temp
AS $$
    WITH RECURSIVE edges(entity, dependency) AS (
        SELECT DISTINCT r.entity, m.entity
        FROM @extschema@.reads r
        JOIN @extschema@.pg_tview_meta m
          ON r.relid IN (m.view_oid::oid, m.table_oid::oid)
        WHERE m.entity <> r.entity
    ),
    depth(entity, level) AS (
        SELECT m.entity, 0 FROM @extschema@.pg_tview_meta m
      UNION
        SELECT e.entity, d.level + 1
        FROM depth d JOIN edges e ON e.dependency = d.entity
        WHERE d.level < 100
    )
    SELECT pg_catalog.quote_ident(n.nspname) || '.' || pg_catalog.quote_ident(c.relname),
           pg_catalog.pg_has_role(c.relowner, 'USAGE')
    FROM depth d
    JOIN @extschema@.pg_tview_meta m ON m.entity = d.entity
    JOIN pg_catalog.pg_class c ON c.oid = m.table_oid
    JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
    WHERE m.time_dependent
    GROUP BY d.entity, n.nspname, c.relname, c.relowner
    ORDER BY max(d.level), d.entity
$$;

COMMENT ON FUNCTION @extschema@.pg_tviews_time_dependent_order() IS
'Internal: the time-dependent TVIEWs in dependency order, and whether the caller owns each';

-- Every time-dependent TVIEW the caller owns; the others are named in a WARNING.
CREATE PROCEDURE @extschema@.pg_tviews_time_boundary()
LANGUAGE plpgsql
AS $$
DECLARE
    next_tview TEXT;
    skipped TEXT;
BEGIN
    SELECT pg_catalog.string_agg(o.tview, ', ' ORDER BY o.tview) INTO skipped
    FROM @extschema@.pg_tviews_time_dependent_order() o WHERE NOT o.owned;
    FOR next_tview IN
        SELECT o.tview FROM @extschema@.pg_tviews_time_dependent_order() o WHERE o.owned
    LOOP
        PERFORM @extschema@.pg_tviews_refresh_time_dependent(next_tview);
        COMMIT;
    END LOOP;
    IF skipped IS NOT NULL THEN
        RAISE WARNING 'time-dependent TVIEWs not refreshed, owned by other roles: %', skipped
            USING HINT = 'Run it as their owner, or grant CALL tviews.pg_tviews_time_boundary_all() '
                         'to the role that runs it (docs/user-guides/operators.md).';
    END IF;
END;
$$;

COMMENT ON PROCEDURE @extschema@.pg_tviews_time_boundary() IS
'Refresh every time-dependent TVIEW the caller owns at the time boundary, one transaction each, dependencies first';

-- An operator's: every time-dependent TVIEW, each as its owner.
CREATE PROCEDURE @extschema@.pg_tviews_time_boundary_all()
LANGUAGE plpgsql
AS $$
DECLARE
    next_tview TEXT;
BEGIN
    FOR next_tview IN SELECT o.tview FROM @extschema@.pg_tviews_time_dependent_order() o
    LOOP
        PERFORM @extschema@.pg_tviews_refresh_time_dependent_all(next_tview);
        COMMIT;
    END LOOP;
END;
$$;

COMMENT ON PROCEDURE @extschema@.pg_tviews_time_boundary_all() IS
'Refresh every time-dependent TVIEW at the time boundary, each as its owner, one transaction each, dependencies first';

REVOKE EXECUTE ON PROCEDURE @extschema@.pg_tviews_time_boundary_all() FROM PUBLIC;
REVOKE EXECUTE ON FUNCTION @extschema@.pg_tviews_refresh_time_dependent_all(TEXT) FROM PUBLIC;

CREATE OR REPLACE FUNCTION @extschema@.pg_tviews_catalog_revision()
RETURNS integer
LANGUAGE sql IMMUTABLE PARALLEL SAFE
AS 'SELECT 15';

-- Every TVIEW is re-derived inside the update, as its owner. The update does not
-- depend on what a TVIEW's owner chose: a TVIEW that cannot be re-derived keeps
-- needs_reregister (its refreshes skip and patch nothing) and is named in a
-- WARNING; the health check lists it (docs/operations/upgrade.md). A TVIEW whose
-- backing view stays locked past the session's lock_timeout (10 s when it set
-- none) is left so too: a lock never holds the update.
UPDATE @extschema@.pg_tview_meta SET needs_reregister = true;
DO $reregister$
DECLARE
    left_behind TEXT[];
    session_lock_timeout TEXT := pg_catalog.current_setting('lock_timeout');
BEGIN
    IF session_lock_timeout = '0' THEN
        PERFORM pg_catalog.set_config('lock_timeout', '10s', true);
    END IF;
    SELECT pg_catalog.array_agg(pg_catalog.format('%s: %s', m.table_oid::pg_catalog.regclass,
                                                  pg_catalog.split_part(r.status, E'\n', 1))
                                ORDER BY r.entity)
      INTO left_behind
      FROM @extschema@.pg_tviews_reregister_all(strict => false) r
      JOIN @extschema@.pg_tview_meta m ON m.entity = r.entity
     WHERE r.status <> 'reregistered';
    PERFORM pg_catalog.set_config('lock_timeout', session_lock_timeout, true);
    IF left_behind IS NOT NULL THEN
        RAISE WARNING 'pg_tviews: % TVIEW(s) could not be re-derived and need re-registration: %',
            pg_catalog.cardinality(left_behind), pg_catalog.array_to_string(left_behind, '; ')
            USING HINT = 'Fix each and run SELECT @extschema@.pg_tviews_reregister(''<TVIEW>''); '
                         'until then its refreshes skip and patch nothing, and fail where its '
                         'definition no longer holds. @extschema@.pg_tviews_check_health() '
                         'lists them (component reregister).';
    END IF;
END
$reregister$;
