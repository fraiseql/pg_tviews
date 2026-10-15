-- pg_tviews 0.1.0-beta.27 → 0.1.0-beta.28
--
-- Pending upgrade script: a pull request that changes the extension SQL adds its
-- statements here (docs/development/extension-versioning.md). Released scripts are
-- never edited.

CREATE OR REPLACE FUNCTION @extschema@.pg_tviews_catalog_revision()
RETURNS integer
LANGUAGE sql IMMUTABLE PARALLEL SAFE
AS 'SELECT 6';

-- Functions another one or an option replaces (ADR 0211): create_aggregate
-- (group_keys option), refresh_all_entities (refresh_all), recover_after_crash
-- (rebuild_all), set_logged (logged option, ALTER TABLE … SET [UN]LOGGED),
-- is_replica_readable (replication_status, registry.options->'logged'),
-- performance_stats (profile), set_typename (typename option).
DROP FUNCTION @extschema@.pg_tviews_create_aggregate(text, text, jsonb);
DROP FUNCTION @extschema@.pg_tviews_refresh_all_entities();
DROP FUNCTION @extschema@.pg_tviews_recover_after_crash(text);
DROP FUNCTION @extschema@.pg_tviews_set_logged(text, boolean);
DROP FUNCTION @extschema@.pg_tviews_is_replica_readable(text);
DROP FUNCTION @extschema@.pg_tviews_performance_stats();
DROP FUNCTION @extschema@.pg_tviews_set_typename(text, text);

-- One parameter name for a TVIEW, `tview`, resolved one way (ADR 0211): a
-- parameter cannot be renamed in place. pg_tviews_create also takes options
-- (ADR 0220), with a C symbol of its own.
DROP FUNCTION @extschema@.pg_tviews_create(text, text);
DROP FUNCTION @extschema@.pg_tviews_create_or_replace(text, text, jsonb);
DROP FUNCTION @extschema@.pg_tviews_drop(text, boolean, boolean);
DROP FUNCTION @extschema@.pg_tviews_ensure_propagation_indexes(text, boolean);
DROP FUNCTION @extschema@.pg_tviews_refresh(text);
DROP FUNCTION @extschema@.pg_tviews_reregister(text);
DROP FUNCTION @extschema@.pg_tviews_show_cascade_path(text);
DROP FUNCTION @extschema@.pg_tviews_profile(text, bigint);
CREATE  FUNCTION @extschema@."pg_tviews_create"(
	"tview" TEXT, /* &str */
	"query" TEXT, /* &str */
	"options" jsonb DEFAULT '{}' /* pgrx::datum::json::JsonB */
) RETURNS TEXT /* core::result::Result<alloc::string::String, pgrx_pg_sys::submodules::panic::ErrorReport> */
STRICT 
LANGUAGE c /* Rust */
AS 'MODULE_PATHNAME', 'pg_tviews_create_with_options_wrapper';
CREATE  FUNCTION @extschema@."pg_tviews_create_or_replace"(
	"tview" TEXT, /* &str */
	"query" TEXT, /* &str */
	"options" jsonb DEFAULT '{}' /* pgrx::datum::json::JsonB */
) RETURNS TEXT /* core::result::Result<alloc::string::String, pgrx_pg_sys::submodules::panic::ErrorReport> */
STRICT
LANGUAGE c /* Rust */
AS 'MODULE_PATHNAME', 'pg_tviews_create_or_replace_wrapper';
CREATE  FUNCTION @extschema@."pg_tviews_drop"(
	"tview" TEXT, /* &str */
	"if_exists" bool DEFAULT false, /* bool */
	"cascade" bool DEFAULT false /* bool */
) RETURNS TEXT /* core::result::Result<alloc::string::String, pgrx_pg_sys::submodules::panic::ErrorReport> */
STRICT
LANGUAGE c /* Rust */
AS 'MODULE_PATHNAME', 'pg_tviews_drop_wrapper';
CREATE  FUNCTION @extschema@."pg_tviews_ensure_propagation_indexes"(
	"tview" TEXT DEFAULT NULL, /* core::option::Option<&str> */
	"dry_run" bool DEFAULT false /* bool */
) RETURNS SETOF TEXT /* alloc::string::String */
LANGUAGE c /* Rust */
AS 'MODULE_PATHNAME', 'pg_tviews_ensure_propagation_indexes_wrapper';
CREATE  FUNCTION @extschema@."pg_tviews_refresh"(
	"tview" TEXT /* &str */
) RETURNS VOID /* core::result::Result<(), pgrx_pg_sys::submodules::panic::ErrorReport> */
STRICT
LANGUAGE c /* Rust */
AS 'MODULE_PATHNAME', 'pg_tviews_refresh_wrapper';
CREATE  FUNCTION @extschema@."pg_tviews_reregister"(
	"tview" TEXT /* &str */
) RETURNS TEXT /* core::result::Result<alloc::string::String, pgrx_pg_sys::submodules::panic::ErrorReport> */
STRICT
LANGUAGE c /* Rust */
AS 'MODULE_PATHNAME', 'pg_tviews_reregister_wrapper';
CREATE  FUNCTION @extschema@."pg_tviews_show_cascade_path"(
	"tview" TEXT /* &str */
) RETURNS TABLE (
	"depth" INT,  /* i32 */
	"entity" TEXT,  /* alloc::string::String */
	"depends_on" TEXT  /* alloc::string::String */
)
STRICT
LANGUAGE c /* Rust */
AS 'MODULE_PATHNAME', 'pg_tviews_show_cascade_path_wrapper';
REVOKE EXECUTE ON FUNCTION @extschema@.pg_tviews_ensure_propagation_indexes(TEXT, BOOLEAN) FROM PUBLIC;

-- The resolver, for tools and pg_tviews_profile (ADR 0211).
CREATE  FUNCTION @extschema@."pg_tviews_entity_of"(
	"tview" TEXT /* &str */
) RETURNS TEXT /* core::result::Result<alloc::string::String, pgrx_pg_sys::submodules::panic::ErrorReport> */
STRICT STABLE PARALLEL SAFE
LANGUAGE c /* Rust */
AS 'MODULE_PATHNAME', 'pg_tviews_entity_of_wrapper';

-- pg_tviews_profile takes `tview` and returns schema and name instead of tview.
CREATE FUNCTION @extschema@.pg_tviews_profile(
    tview       TEXT   DEFAULT NULL,
    fanout_warn BIGINT DEFAULT 1000)
RETURNS TABLE (
    entity                      TEXT,
    schema                      TEXT,
    name                        TEXT,
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
    chosen       TEXT;
    qualified    TEXT;
BEGIN
    -- The TVIEW named as every function names one (ADR 0211).
    IF tview IS NOT NULL THEN
        chosen := @extschema@.pg_tviews_entity_of(tview);
    END IF;

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
        WHERE chosen IS NULL OR m.entity = chosen
        ORDER BY m.entity
    LOOP
        entity           := r.ent;
        schema           := r.nsp;
        name             := r.tbl;
        qualified        := quote_ident(r.nsp) || '.' || quote_ident(r.tbl);
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
                col, qualified, r.ent);
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
                'fillfactor 100 on a frequently updated TVIEW: refreshed rows cannot stay on their page (option fillfactor, default 85)'::TEXT;
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
                'UNLOGGED: not readable on hot standbys, empty after promotion or a crash restart (option logged)'::TEXT;
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

COMMENT ON FUNCTION @extschema@.pg_tviews_profile(TEXT, BIGINT) IS
'Physical health of each TVIEW (sizes, HOT ratio, dead tuples, indexes, fan-out) with warnings';

-- Read contract v2 (ADR 0211): every option in `options`, the columns that
-- duplicated them dropped.
CREATE OR REPLACE FUNCTION @extschema@.contract_version()
RETURNS integer
LANGUAGE sql STABLE PARALLEL SAFE
AS 'SELECT 2';

DROP VIEW @extschema@.registry;
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
         FROM (SELECT DISTINCT r.relid FROM @extschema@.pg_tview_reads r
               WHERE r.entity = m.entity) x
         JOIN pg_catalog.pg_class b ON b.oid = x.relid AND b.relkind IN ('r', 'p', 'f', 'm')
         JOIN pg_catalog.pg_namespace bn ON bn.oid = b.relnamespace),
        '{}') AS base_tables,
    COALESCE(
        (SELECT pg_catalog.jsonb_object_agg(
                    (e->>'relid')::pg_catalog.oid::pg_catalog.regclass::pg_catalog.text,
                    e->>'kind')
         FROM pg_catalog.jsonb_array_elements(m.plan->'tables') e),
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
    m.needs_reregister
FROM @extschema@.pg_tview_meta m
LEFT JOIN pg_catalog.pg_class c ON c.oid = m.table_oid
LEFT JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
LEFT JOIN pg_catalog.pg_class v ON v.oid = m.view_oid;

COMMENT ON VIEW @extschema@.registry IS
'One row per registered TVIEW; stable under contract_version()';

GRANT SELECT ON @extschema@.registry TO PUBLIC;

-- Per-TVIEW refresh statistics (ADR 0221).
CREATE  FUNCTION @extschema@."pg_tviews_stats_rows"() RETURNS TABLE (
	"schema" TEXT,  /* core::option::Option<alloc::string::String> */
	"name" TEXT,  /* alloc::string::String */
	"entity" TEXT,  /* alloc::string::String */
	"view_recomputes" bigint,  /* core::option::Option<i64> */
	"noop_skipped" bigint,  /* core::option::Option<i64> */
	"patch_captured" bigint,  /* core::option::Option<i64> */
	"patch_applied" bigint,  /* core::option::Option<i64> */
	"patch_fallbacks" bigint,  /* core::option::Option<i64> */
	"propagation_pruned" bigint,  /* core::option::Option<i64> */
	"rows_written" bigint,  /* core::option::Option<i64> */
	"rows_deleted" bigint,  /* core::option::Option<i64> */
	"full_refreshes" bigint,  /* core::option::Option<i64> */
	"refresh_ms" double precision,  /* core::option::Option<f64> */
	"stats_reset" timestamp with time zone,  /* core::option::Option<pgrx::datetime::time_stamp_with_timezone::TimestampWithTimeZone> */
	"untracked" bool  /* bool */
)
STRICT
LANGUAGE c /* Rust */
AS 'MODULE_PATHNAME', 'pg_tviews_stats_rows_wrapper';
CREATE  FUNCTION @extschema@."pg_tviews_stats_reset"(
	"tview" TEXT DEFAULT NULL /* core::option::Option<&str> */
) RETURNS VOID /* core::result::Result<(), pgrx_pg_sys::submodules::panic::ErrorReport> */
LANGUAGE c /* Rust */
AS 'MODULE_PATHNAME', 'pg_tviews_stats_reset_wrapper';
REVOKE EXECUTE ON FUNCTION @extschema@.pg_tviews_stats_reset(TEXT) FROM PUBLIC;
CREATE VIEW @extschema@.stats AS SELECT * FROM @extschema@.pg_tviews_stats_rows();
COMMENT ON VIEW @extschema@.stats IS
'Refresh statistics of each TVIEW since the server started or pg_tviews_stats_reset()';
GRANT SELECT ON @extschema@.stats TO PUBLIC;
