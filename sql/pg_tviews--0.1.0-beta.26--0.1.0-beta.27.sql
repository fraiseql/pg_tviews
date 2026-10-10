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

-- The UNLOGGED TVIEW tables whose rows can be trusted. UNLOGGED itself: a crash
-- restart or a promotion empties it together with them, and a table missing here
-- is filled from its view by the next write. Not dumped: a restored TVIEW is
-- filled once.
CREATE UNLOGGED TABLE @extschema@.pg_tview_valid (
    table_oid OID NOT NULL PRIMARY KEY
);
COMMENT ON TABLE @extschema@.pg_tview_valid IS
    'Internal: UNLOGGED TVIEW tables whose rows can be trusted; may change in any release';
GRANT SELECT ON @extschema@.pg_tview_valid TO PUBLIC;
-- An UNLOGGED TVIEW holding rows is trusted; an empty one is filled once by its
-- next write (cheap when its view is empty too). Only the tables are read.
DO $$
DECLARE
    t pg_catalog.regclass;
    filled boolean;
BEGIN
    FOR t IN SELECT m.table_oid FROM @extschema@.pg_tview_meta m
             JOIN pg_catalog.pg_class c ON c.oid = m.table_oid::pg_catalog.oid
             WHERE c.relpersistence = 'u'
    LOOP
        EXECUTE pg_catalog.format('SELECT EXISTS (SELECT 1 FROM %s)', t) INTO filled;
        IF filled THEN
            INSERT INTO @extschema@.pg_tview_valid VALUES (t::pg_catalog.oid);
        END IF;
    END LOOP;
END
$$;

-- Re-derive every TVIEW, dependencies first. Until then a row's plan is empty.
-- A row registered before the row identity (ADR 0169) names its rows by
-- pk_<entity>, which the library no longer assumes: record it, so each TVIEW
-- re-derived below reads the rows of those not re-derived yet.
UPDATE @extschema@.pg_tview_meta m
   SET identity = pg_catalog.jsonb_build_object('kind', 'pk', 'columns',
           pg_catalog.jsonb_build_array(pg_catalog.jsonb_build_object(
               'name', a.attname::pg_catalog.text,
               'type', pg_catalog.format_type(a.atttypid, NULL))))
  FROM pg_catalog.pg_attribute a
 WHERE m.identity IS NULL
   AND a.attrelid = m.table_oid::pg_catalog.oid
   AND a.attname = 'pk_' || m.entity
   AND NOT a.attisdropped;
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

-- Comments that named issue numbers name what they stand for.
COMMENT ON EVENT TRIGGER pg_tviews_sql_drop IS
'Deregisters a TVIEW whose backing view or table was dropped as a dependent';

CREATE OR REPLACE FUNCTION @extschema@.pg_tviews_profile(
    p_entity    TEXT   DEFAULT NULL,
    fanout_warn BIGINT DEFAULT 1000)
RETURNS TABLE (
    entity                      TEXT,
    tview                       TEXT,
    persistence                 TEXT,
    replica_readable            BOOLEAN,
    rows_estimate               BIGINT,
    heap_bytes                  BIGINT,
    index_bytes                 BIGINT,
    toast_bytes                 BIGINT,
    avg_row_width               INTEGER,
    data_avg_width              INTEGER,
    fillfactor                  INTEGER,
    n_tup_upd                   BIGINT,
    n_tup_hot_upd               BIGINT,
    hot_ratio                   DOUBLE PRECISION,
    n_dead_tup                  BIGINT,
    last_vacuum                 TIMESTAMPTZ,
    last_autovacuum             TIMESTAMPTZ,
    all_visible_fraction        DOUBLE PRECISION,
    unused_indexes              TEXT[],
    missing_propagation_indexes TEXT[],
    fanout                      JSONB,
    warnings                    TEXT[])
LANGUAGE plpgsql
STABLE
SET search_path = pg_catalog, pg_temp
AS $$
#variable_conflict use_column
DECLARE
    r            RECORD;
    f            RECORD;
    col          TEXT;
    idx          TEXT;
    vis_schema   NAME;
    all_visible  BIGINT;
    stats_reset  TIMESTAMPTZ := (SELECT d.stats_reset FROM pg_stat_database d
                                 WHERE d.datname = current_database());
BEGIN
    vis_schema := (SELECT n.nspname FROM pg_extension e
                   JOIN pg_namespace n ON n.oid = e.extnamespace
                   WHERE e.extname = 'pg_visibility');

    FOR r IN
        SELECT m.entity AS ent, c.oid AS rel, n.nspname AS nsp, c.relname AS tbl,
               c.relpersistence AS pers, c.reltuples, c.relpages, c.reltoastrelid,
               c.reloptions,
               -- The columns its rows are looked up by: those holding an
               -- embedded TVIEW's key, and those a fan-out patch writes through.
               ARRAY(SELECT jsonb_array_elements_text(e->'lookups')
                     FROM jsonb_array_elements(m.plan->'embeds') e
                     UNION
                     SELECT t->'fanout'->>'lookup_col'
                     FROM jsonb_array_elements(m.plan->'tables') t
                     WHERE t ? 'fanout') AS lookups
        FROM @extschema@.pg_tview_meta m
        JOIN pg_class c ON c.oid = m.table_oid
        JOIN pg_namespace n ON n.oid = c.relnamespace
        WHERE p_entity IS NULL OR m.entity = p_entity
        ORDER BY m.entity
    LOOP
        entity           := r.ent;
        tview            := quote_ident(r.nsp) || '.' || quote_ident(r.tbl);
        persistence      := CASE r.pers WHEN 'u' THEN 'unlogged' ELSE 'logged' END;
        replica_readable := r.pers = 'p';
        rows_estimate    := CASE WHEN r.reltuples < 0 THEN NULL ELSE r.reltuples::BIGINT END;
        heap_bytes       := pg_relation_size(r.rel);
        index_bytes      := pg_indexes_size(r.rel);
        toast_bytes      := CASE WHEN r.reltoastrelid = 0 THEN 0
                                 ELSE pg_relation_size(r.reltoastrelid) END;

        SELECT sum(s.avg_width)::INTEGER, max(s.avg_width) FILTER (WHERE s.attname = 'data')
          INTO avg_row_width, data_avg_width
          FROM pg_stats s WHERE s.schemaname = r.nsp AND s.tablename = r.tbl;

        fillfactor := coalesce((SELECT split_part(o, '=', 2)::INTEGER
                                FROM unnest(r.reloptions) o WHERE o LIKE 'fillfactor=%'), 100);

        SELECT s.n_tup_upd, s.n_tup_hot_upd, s.n_dead_tup, s.last_vacuum, s.last_autovacuum
          INTO n_tup_upd, n_tup_hot_upd, n_dead_tup, last_vacuum, last_autovacuum
          FROM pg_stat_all_tables s WHERE s.relid = r.rel;
        hot_ratio := CASE WHEN n_tup_upd > 0 THEN n_tup_hot_upd::FLOAT8 / n_tup_upd END;

        -- The visibility map of an UNLOGGED table cannot be read during recovery.
        all_visible_fraction := NULL;
        IF vis_schema IS NOT NULL AND r.relpages > 0
           AND NOT (pg_is_in_recovery() AND r.pers = 'u') THEN
            EXECUTE format('SELECT all_visible FROM %I.pg_visibility_map_summary($1)', vis_schema)
               INTO all_visible USING r.rel::REGCLASS;
            all_visible_fraction := all_visible::FLOAT8 / r.relpages;
        END IF;

        -- Never-scanned indexes, except the primary key, unique indexes and the
        -- propagation indexes cascades need (leading with a lookup column).
        unused_indexes := ARRAY(
            SELECT quote_ident(si.indexrelname) FROM pg_index i
            JOIN pg_stat_all_indexes si ON si.indexrelid = i.indexrelid
            JOIN pg_attribute a ON a.attrelid = i.indrelid AND a.attnum = i.indkey[0]
            WHERE i.indrelid = r.rel AND NOT i.indisprimary AND NOT i.indisunique
              AND si.idx_scan = 0 AND a.attname <> ALL (r.lookups)
            ORDER BY 1);

        -- Lookup columns that no index leads with.
        missing_propagation_indexes := ARRAY(
            SELECT a.attname::TEXT FROM pg_attribute a
            WHERE a.attrelid = r.rel AND a.attnum > 0 AND NOT a.attisdropped
              AND a.attname = ANY (r.lookups)
              AND NOT EXISTS (SELECT 1 FROM pg_index i
                              WHERE i.indrelid = r.rel AND i.indkey[0] = a.attnum)
            ORDER BY 1);

        -- Estimated rows per key of each of those columns, from the planner statistics:
        -- p50 = rows / distinct keys, max = top MCV, p99 = the MCV at the 1% rank
        -- (or the average of the non-MCV keys when the MCV list is shorter).
        fanout := (
            SELECT jsonb_object_agg(x.attname, jsonb_build_object(
                       'p50', round(r.reltuples / x.nd),
                       'p99', round(CASE
                           WHEN x.k <= coalesce(array_length(x.mcf, 1), 0)
                               THEN x.mcf[x.k] * r.reltuples
                           WHEN x.nd > coalesce(array_length(x.mcf, 1), 0)
                               THEN (1 - coalesce((SELECT sum(v) FROM unnest(x.mcf) v), 0))
                                    * r.reltuples / (x.nd - coalesce(array_length(x.mcf, 1), 0))
                           ELSE r.reltuples / x.nd END),
                       'max', round(coalesce(x.mcf[1], 1 / x.nd) * r.reltuples)))
            FROM (SELECT s.attname,
                         s.most_common_freqs AS mcf,
                         greatest(CASE WHEN s.n_distinct < 0 THEN -s.n_distinct * r.reltuples
                                       ELSE s.n_distinct END, 1) AS nd,
                         greatest(floor(greatest(CASE WHEN s.n_distinct < 0
                                                      THEN -s.n_distinct * r.reltuples
                                                      ELSE s.n_distinct END, 1) * 0.01)::INTEGER, 1) AS k
                  FROM pg_stats s
                  WHERE s.schemaname = r.nsp AND s.tablename = r.tbl
                    AND s.attname = ANY (r.lookups) AND r.reltuples > 0) x);

        warnings := ARRAY[]::TEXT[];
        FOREACH col IN ARRAY missing_propagation_indexes LOOP
            warnings := warnings || format(
                '%s has no index: a cascade into %s scans the whole table. Run pg_tviews_ensure_propagation_indexes(%L)',
                col, tview, r.ent);
        END LOOP;
        IF n_tup_upd > 1000 AND hot_ratio < 0.5 THEN
            FOR idx IN
                SELECT DISTINCT quote_ident(ic.relname) FROM pg_index i
                JOIN pg_class ic ON ic.oid = i.indexrelid
                JOIN pg_attribute a ON a.attrelid = i.indrelid AND a.attnum = ANY (i.indkey)
                WHERE i.indrelid = r.rel AND a.attname IN ('data', 'updated_at')
                ORDER BY 1
            LOOP
                warnings := warnings || format(
                    'HOT ratio %s%%: index %s on a column every refresh changes prevents HOT updates',
                    round(hot_ratio * 100), idx);
            END LOOP;
        END IF;
        FOR idx IN
            SELECT quote_ident(si.indexrelname) FROM pg_index i
            JOIN pg_class ic ON ic.oid = i.indexrelid
            JOIN pg_am am ON am.oid = ic.relam
            JOIN pg_stat_all_indexes si ON si.indexrelid = i.indexrelid
            JOIN pg_attribute a ON a.attrelid = i.indrelid AND a.attnum = i.indkey[0]
            WHERE i.indrelid = r.rel AND am.amname = 'gin' AND a.attname = 'data'
              AND si.idx_scan = 0
            ORDER BY 1
        LOOP
            warnings := warnings || format(
                'GIN index %s on data never scanned since statistics reset (%s)',
                idx, coalesce(stats_reset::TEXT, 'never'));
        END LOOP;
        IF fillfactor = 100 AND n_tup_upd > 0 AND n_tup_upd > coalesce(rows_estimate, 0) THEN
            warnings := warnings ||
                'fillfactor 100 on a frequently updated TVIEW: refreshed rows cannot stay on their page (see pg_tviews.fillfactor)'::TEXT;
        END IF;
        IF toast_bytes > 0 AND toast_bytes > 0.3 * (heap_bytes + toast_bytes) THEN
            warnings := warnings || format(
                '%s%% of the table is TOAST: each refresh rewrites whole data documents (see docs/adr/0094-large-document-refresh.md)',
                round(100.0 * toast_bytes / (heap_bytes + toast_bytes)));
        END IF;
        FOR f IN SELECT key, (value->>'p99')::BIGINT AS p99 FROM jsonb_each(fanout) ORDER BY key LOOP
            IF f.p99 > fanout_warn THEN
                warnings := warnings || format(
                    'p99 fan-out through %s is about %s rows per key: one parent change refreshes that many rows',
                    f.key, f.p99);
            END IF;
        END LOOP;
        IF r.pers = 'u' THEN
            warnings := warnings ||
                'UNLOGGED: not readable on hot standbys, empty after promotion or a crash restart (pg_tviews_set_logged)'::TEXT;
        END IF;
        IF rows_estimate > 0 AND n_dead_tup > 0.2 * rows_estimate THEN
            warnings := warnings || format(
                '%s%% dead tuples: autovacuum is behind (last autovacuum %s)',
                round(100.0 * n_dead_tup / rows_estimate),
                coalesce(last_autovacuum::TEXT, 'never'));
        END IF;

        RETURN NEXT;
    END LOOP;
END;
$$;

-- The column default matches pg_tviews.uncascaded_policy's default; every
-- registration writes the column, so no row changes.
ALTER TABLE @extschema@.pg_tview_meta ALTER COLUMN uncascaded_policy SET DEFAULT 'error';

-- Maintenance acting on every TVIEW is not for PUBLIC: an operator role is
-- granted it (docs/user-guides/operators.md). Each function acting on one TVIEW
-- checks that the caller owns it.
REVOKE EXECUTE ON FUNCTION
    @extschema@.pg_tviews_refresh_all(),
    @extschema@.pg_tviews_refresh_all_entities(),
    @extschema@.pg_tviews_rebuild_all(BOOLEAN),
    @extschema@.pg_tviews_reregister_all(BOOLEAN),
    @extschema@.pg_tviews_set_logged(TEXT, BOOLEAN),
    @extschema@.pg_tviews_ensure_propagation_indexes(TEXT, BOOLEAN),
    @extschema@.pg_tviews_invalidate_caches(OID)
FROM PUBLIC;
