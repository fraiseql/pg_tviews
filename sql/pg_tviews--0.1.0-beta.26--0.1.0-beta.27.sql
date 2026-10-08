-- pg_tviews 0.1.0-beta.26 → 0.1.0-beta.27
--
-- Pending upgrade script: a pull request that changes the extension SQL adds its
-- statements here (docs/development/extension-versioning.md). Released scripts are
-- never edited.

CREATE OR REPLACE FUNCTION @extschema@.pg_tviews_catalog_revision()
RETURNS integer
LANGUAGE sql IMMUTABLE PARALLEL SAFE
AS 'SELECT 5';

-- Text-pattern schema analysis is gone: every TVIEW is analysed from its query
-- tree when it is registered.
DROP FUNCTION @extschema@.pg_tviews_analyze_select(text);
DROP FUNCTION @extschema@.pg_tviews_infer_types(text, text[]);

-- pg_tviews_cascade(), _insert() and _delete() guessed a TVIEW's rows from the
-- table's name: a write to the base table refreshes them.
DROP FUNCTION @extschema@.pg_tviews_cascade(oid, bigint);
DROP FUNCTION @extschema@.pg_tviews_insert(oid, bigint);
DROP FUNCTION @extschema@.pg_tviews_delete(oid, bigint);

-- The text-pattern analysis's result type: every definition is read from its
-- query tree.
DROP TYPE @extschema@.tviewschema CASCADE;

-- One propagation plan per TVIEW (ADR 0203). This release re-derives every
-- TVIEW instead of carrying its old metadata over, and drops the columns the
-- plan replaces (docs/development/extension-versioning.md records the
-- exception). A TVIEW that no longer analyses fails the update and is named.
ALTER TABLE @extschema@.pg_tview_meta ADD COLUMN plan JSONB;

-- The rebind trigger reads the plan; the old one called a removed function.
DROP TRIGGER pg_tview_meta_rebind ON @extschema@.pg_tview_meta;
CREATE OR REPLACE FUNCTION @extschema@.pg_tviews_meta_rebind()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, pg_temp
AS $$
DECLARE
    olds TEXT[];
    news TEXT[];
    missing TEXT;
    e JSONB;
    q TEXT;
    i INT;
    tables JSONB := '[]';
    paths JSONB := '[]';
BEGIN
    -- Each table's relid in the source database, and here (found by name).
    SELECT pg_catalog.array_agg(x.e->>'relid' ORDER BY x.n),
           pg_catalog.array_agg(
               pg_catalog.to_regclass(x.e->>'table')::pg_catalog.oid::pg_catalog.text
               ORDER BY x.n),
           pg_catalog.string_agg(x.e->>'table', ', ' ORDER BY x.n)
               FILTER (WHERE pg_catalog.to_regclass(x.e->>'table') IS NULL)
      INTO olds, news, missing
      FROM pg_catalog.jsonb_array_elements(NEW.plan->'tables') WITH ORDINALITY AS x(e, n);
    IF missing IS NOT NULL THEN
        RAISE EXCEPTION 'the plan of TVIEW tv_% names tables that do not exist: %',
            NEW.entity, missing
            USING ERRCODE = 'undefined_table',
                  HINT = 'Restore the tables the TVIEW reads before its catalog row.';
    END IF;
    FOR e IN SELECT value FROM pg_catalog.jsonb_array_elements(NEW.plan->'tables') LOOP
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
        tables := tables || pg_catalog.jsonb_build_array(e);
    END LOOP;
    FOR e IN SELECT value FROM pg_catalog.jsonb_array_elements(NEW.plan->'paths') LOOP
        i := pg_catalog.array_position(olds, e->>'source_oid');
        IF i IS NULL THEN
            RAISE EXCEPTION 'the plan of TVIEW tv_% has a path from %, which it maps no '
                            'write of', NEW.entity, e->>'source_table'
                USING ERRCODE = 'data_corrupted',
                      HINT = 'SELECT tviews.pg_tviews_reregister(''' || NEW.entity
                             || ''') re-derives it.';
        END IF;
        e := pg_catalog.jsonb_set(e, '{source_oid}',
                                  pg_catalog.to_jsonb(news[i]::pg_catalog.int8));
        paths := paths || pg_catalog.jsonb_build_array(e);
    END LOOP;
    NEW.plan := pg_catalog.jsonb_set(pg_catalog.jsonb_set(NEW.plan,
                    '{tables}', tables), '{paths}', paths);
    RETURN NEW;
END;
$$;

CREATE TRIGGER pg_tview_meta_rebind
    BEFORE INSERT ON @extschema@.pg_tview_meta
    FOR EACH ROW
    EXECUTE FUNCTION @extschema@.pg_tviews_meta_rebind();

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
         FROM pg_catalog.jsonb_array_elements(m.plan->'tables') e),
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

-- Triggers an older release installed without their entity argument (or with
-- the PL/pgSQL handler) serve every TVIEW of their table: re-registration
-- installs one per TVIEW instead.
DO $$
DECLARE
    legacy RECORD;
BEGIN
    FOR legacy IN
        SELECT tg.tgname, tg.tgrelid::pg_catalog.regclass AS rel
        FROM pg_catalog.pg_trigger tg
        JOIN pg_catalog.pg_proc p ON p.oid = tg.tgfoid
        WHERE NOT tg.tgisinternal AND tg.tgparentid = 0
          AND (p.proname = 'tview_trigger_handler'
               OR (p.pronamespace = '@extschema@'::pg_catalog.regnamespace
                   AND p.proname IN ('pg_tview_trigger_handler', 'pg_tview_flush_trigger')
                   AND tg.tgnargs = 0))
    LOOP
        EXECUTE pg_catalog.format('DROP TRIGGER %I ON %s', legacy.tgname, legacy.rel);
    END LOOP;
END
$$;

-- Re-derive every TVIEW, dependencies first. Until then a row's plan is empty.
UPDATE @extschema@.pg_tview_meta SET plan = '{"version": 1}';
DO $$
DECLARE
    failed TEXT;
BEGIN
    SELECT pg_catalog.string_agg(pg_catalog.format('tv_%s: %s', r.entity, r.status), '; ')
      INTO failed
      FROM @extschema@.pg_tviews_reregister_all() r
     WHERE r.status <> 'reregistered';
    IF failed IS NOT NULL THEN
        RAISE EXCEPTION 'pg_tviews: these TVIEWs could not be re-derived: %', failed
            USING HINT = 'Fix or drop them on the previous release, then update again.';
    END IF;
END
$$;
ALTER TABLE @extschema@.pg_tview_meta ALTER COLUMN plan SET NOT NULL;

ALTER TABLE @extschema@.pg_tview_meta
    DROP COLUMN cascade_paths,
    DROP COLUMN fk_columns,
    DROP COLUMN uuid_fk_columns,
    DROP COLUMN dependency_types,
    DROP COLUMN dependency_paths,
    DROP COLUMN array_match_keys,
    DROP COLUMN distinct_on_keys,
    DROP COLUMN distinct_on_output_keys,
    DROP COLUMN direct_map_columns,
    DROP COLUMN direct_map_keys,
    DROP COLUMN is_union,
    DROP COLUMN aggregate_embeds,
    DROP COLUMN key_mappings;

-- Removed (docs/DEPRECATION_WARNINGS.md): the plan is rebound by the trigger
-- above; no trigger is older than this release's; a table is never converted.
DROP FUNCTION @extschema@.pg_tviews_rebind_cascade_paths(oid, text[]);
DROP FUNCTION @extschema@.pg_tviews_migrate_triggers();
DROP FUNCTION @extschema@.pg_tviews_convert_existing_table(text);
CREATE OR REPLACE FUNCTION @extschema@.pg_tviews_handle_ddl_event()
RETURNS event_trigger
LANGUAGE plpgsql
AS $$
DECLARE
    obj record;
BEGIN
    FOR obj IN SELECT * FROM pg_catalog.pg_event_trigger_ddl_commands()
    LOOP
        -- Only CTAS-style creation: a plain `CREATE TABLE tv_x (cols…)` has no query to
        -- make a TVIEW of, so it stays a plain table.
        IF obj.command_tag IN ('CREATE TABLE AS', 'SELECT INTO') THEN
            -- Only intercept tv_* tables
            IF obj.object_identity LIKE '%.tv_%' OR obj.object_identity LIKE 'tv_%' THEN
                RAISE EXCEPTION 'pg_tviews: cannot convert ''%'' to a TVIEW: the statement '
                                'was not intercepted', obj.object_identity
                    USING ERRCODE = 'object_not_in_prerequisite_state',
                          DETAIL = 'pg_tviews is not active in this session''s ProcessUtility '
                                   'hook',
                          HINT = 'Add pg_tviews to shared_preload_libraries in '
                                 'postgresql.conf and restart PostgreSQL, or create the TVIEW '
                                 'with tviews.pg_tviews_create_or_replace().';
            END IF;
        END IF;
    END LOOP;
END;
$$;
DROP FUNCTION @extschema@.pg_tviews_convert_table(text, text);
