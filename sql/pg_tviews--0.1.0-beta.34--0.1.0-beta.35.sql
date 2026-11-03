-- pg_tviews 0.1.0-beta.34 → 0.1.0-beta.35
--
-- Pending upgrade script: a pull request that changes the extension SQL adds its
-- statements here (docs/development/extension-versioning.md). Released scripts are
-- never edited.

-- The public surface (ADR 0211): functions are
-- pg_tviews_<verb>, relations nouns; functions acting on every TVIEW end in _all and
-- are an operator's; overlapping functions are folded. No aliases: the old names go.
-- Renames come first, so the definitions below name the new ones.
ALTER VIEW @extschema@.pg_tview_reads RENAME TO reads;
ALTER VIEW @extschema@.pg_tview_references RENAME TO "references";
ALTER FUNCTION @extschema@.pg_tviews_health_check() RENAME TO pg_tviews_check_health;
ALTER FUNCTION @extschema@.pg_tviews_queue_stats() RENAME TO pg_tviews_show_queue;
ALTER FUNCTION @extschema@.pg_tviews_suspended_entities() RENAME TO pg_tviews_show_suspended;
ALTER FUNCTION @extschema@.pg_tviews_entity_of(TEXT) RENAME TO pg_tviews_resolve;
ALTER FUNCTION @extschema@.pg_tviews_feed_gaps() RENAME TO pg_tviews_count_feed_gaps;
ALTER FUNCTION @extschema@.pg_tviews_replication_status() RENAME TO pg_tviews_show_replication;
ALTER FUNCTION @extschema@.pg_tviews_mapping_query(TEXT, OID)
    RENAME TO pg_tviews_show_mapping_query;
ALTER FUNCTION @extschema@.pg_tviews_read_set_queries(TEXT, OID)
    RENAME TO pg_tviews_show_read_sets;
-- The read contract version 3: these renames (docs/reference/read-contract.md).
CREATE OR REPLACE FUNCTION @extschema@.contract_version()
RETURNS integer
LANGUAGE sql STABLE PARALLEL SAFE
AS 'SELECT 3';
-- Folded into pg_tviews_show_suspended() and pg_tviews_show_queue().
DROP FUNCTION @extschema@.pg_tviews_is_suspended();
DROP FUNCTION @extschema@.pg_tviews_debug_queue();
-- "NULL = every TVIEW" forms split into a single-TVIEW form (owner) and an _all form.
DROP FUNCTION @extschema@.pg_tviews_ensure_propagation_indexes(TEXT, BOOLEAN);
DROP FUNCTION @extschema@.pg_tviews_stats_reset(TEXT);
-- Nothing read it. Out of the extension first, so it leaves the dumped
-- configuration tables (pg_extension.extconfig) too.
ALTER EXTENSION pg_tviews DROP TABLE @extschema@.pg_tview_helpers;
DROP TABLE @extschema@.pg_tview_helpers;

CREATE OR REPLACE FUNCTION @extschema@.pg_tviews_catalog_revision()
RETURNS integer
LANGUAGE sql IMMUTABLE PARALLEL SAFE
AS 'SELECT 11';

-- A restore rebinds the plan's relids and column numbers by name (a table dumped
-- with dropped columns comes back without their holes), in Rust.
CREATE FUNCTION @extschema@.pg_tviews_plan_rebind(tview text, plan jsonb) RETURNS jsonb
STRICT
LANGUAGE c
AS 'MODULE_PATHNAME', 'pg_tviews_plan_rebind_wrapper';

CREATE OR REPLACE FUNCTION @extschema@.pg_tviews_meta_rebind()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, pg_temp
AS $$
BEGIN
    NEW.plan := @extschema@.pg_tviews_plan_rebind(NEW.entity, NEW.plan);
    RETURN NEW;
END;
$$;

-- The plan decoders: the extension's SQL reads stored plans through them.
CREATE FUNCTION @extschema@.pg_tviews_plan_tables(plan jsonb)
RETURNS TABLE (relid oid, kind text, columns text[])
LANGUAGE sql IMMUTABLE PARALLEL SAFE
SET search_path = pg_catalog, pg_temp
AS $$
    SELECT (e->>'relid')::pg_catalog.oid, e->>'kind',
           ARRAY(SELECT pg_catalog.jsonb_array_elements_text(
                     CASE pg_catalog.jsonb_typeof(e->'columns') WHEN 'array' THEN e->'columns'
                                                                ELSE '[]'::pg_catalog.jsonb END))
      FROM pg_catalog.jsonb_array_elements(CASE pg_catalog.jsonb_typeof(plan->'tables')
                                           WHEN 'array' THEN plan->'tables'
                                           ELSE '[]'::pg_catalog.jsonb END) e
     WHERE pg_catalog.jsonb_typeof(e) = 'object'
$$;

CREATE FUNCTION @extschema@.pg_tviews_plan_references(plan jsonb)
RETURNS TABLE (column_name text, table_name text, key text, is_unique boolean,
               is_nullable boolean)
LANGUAGE sql IMMUTABLE PARALLEL SAFE
SET search_path = pg_catalog, pg_temp
AS $$
    SELECT r->>'column', r->>'table', r->>'key', (r->>'unique')::boolean,
           (r->>'nullable')::boolean
      FROM pg_catalog.jsonb_array_elements(CASE pg_catalog.jsonb_typeof(plan->'references')
                                           WHEN 'array' THEN plan->'references'
                                           ELSE '[]'::pg_catalog.jsonb END) r
     WHERE pg_catalog.jsonb_typeof(r) = 'object'
$$;

CREATE FUNCTION @extschema@.pg_tviews_plan_roots(plan jsonb)
RETURNS TABLE (relid oid, key text)
LANGUAGE sql IMMUTABLE PARALLEL SAFE
SET search_path = pg_catalog, pg_temp
AS $$
    SELECT (p->>'source_oid')::pg_catalog.oid, p->>'initial_col'
      FROM pg_catalog.jsonb_array_elements(CASE pg_catalog.jsonb_typeof(plan->'paths')
                                           WHEN 'array' THEN plan->'paths'
                                           ELSE '[]'::pg_catalog.jsonb END) p
     WHERE pg_catalog.jsonb_typeof(p) = 'object'
       AND COALESCE((p->>'root')::boolean, false)
       AND NOT COALESCE((plan->>'set_operation')::boolean, false)
$$;

CREATE FUNCTION @extschema@.pg_tviews_plan_lookups(plan jsonb)
RETURNS SETOF text
LANGUAGE sql IMMUTABLE PARALLEL SAFE
SET search_path = pg_catalog, pg_temp
AS $$
    SELECT pg_catalog.jsonb_array_elements_text(e->'lookups')
      FROM pg_catalog.jsonb_array_elements(CASE pg_catalog.jsonb_typeof(plan->'embeds')
                                           WHEN 'array' THEN plan->'embeds'
                                           ELSE '[]'::pg_catalog.jsonb END) e
     WHERE pg_catalog.jsonb_typeof(e->'lookups') = 'array'
    UNION
    SELECT t->'fanout'->>'lookup_col'
      FROM pg_catalog.jsonb_array_elements(CASE pg_catalog.jsonb_typeof(plan->'tables')
                                           WHEN 'array' THEN plan->'tables'
                                           ELSE '[]'::pg_catalog.jsonb END) t
     WHERE pg_catalog.jsonb_typeof(t->'fanout') = 'object'
    UNION
    SELECT column_name FROM @extschema@.pg_tviews_plan_references(plan)
$$;

-- PL/pgSQL handlers keep the SQLSTATE of an error they cannot handle: a failed
-- cleanup after a dependent drop fails the DROP, the definition check catches
-- class 42 only, reregister_all(strict) raises the first failure's SQLSTATE.
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
        PERFORM @extschema@.pg_tviews_handle_dropped(entity_name);
    END LOOP;
END;
$$;

-- A column rename keeps PostgreSQL's rendering of the backing view: no rewritten
-- text to check.
DROP FUNCTION @extschema@.pg_tviews_defines_view(OID, TEXT);

CREATE OR REPLACE FUNCTION @extschema@.pg_tviews_reregister_all(strict BOOLEAN DEFAULT false)
RETURNS TABLE (entity TEXT, status TEXT)
LANGUAGE plpgsql
SET search_path = pg_catalog, pg_temp
AS $$
#variable_conflict use_column
DECLARE
    next_entity TEXT;
    failures INTEGER := 0;
    first_state TEXT;
    first_message TEXT;
BEGIN
    FOR next_entity IN
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
        SELECT d.entity FROM depth d GROUP BY d.entity ORDER BY max(d.level), d.entity
    LOOP
        entity := next_entity;
        BEGIN
            PERFORM @extschema@.pg_tviews_reregister(next_entity);
            status := 'reregistered';
        -- catches every error: it becomes the entity's status; strict raises the first.
        EXCEPTION WHEN OTHERS THEN
            status := SQLERRM;
            failures := failures + 1;
            first_state := COALESCE(first_state, SQLSTATE);
            first_message := COALESCE(first_message, 'tv_' || next_entity || ': ' || SQLERRM);
        END;
        RETURN NEXT;
    END LOOP;
    -- The first failure's SQLSTATE: a lock timeout or a deadlock stays retryable.
    IF strict AND failures > 0 THEN
        RAISE EXCEPTION 'pg_tviews: % TVIEW(s) could not be re-registered; first: %',
            failures, first_message
            USING ERRCODE = first_state,
                  HINT = 'SELECT * FROM tviews.pg_tviews_reregister_all() lists them';
    END IF;
END;
$$;

-- The read contract and the profile read plans through the decoders.
CREATE OR REPLACE VIEW @extschema@.references AS
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

CREATE OR REPLACE FUNCTION @extschema@.pg_tviews_profile(
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
    doc_bytes    FLOAT8;
    -- The fan-out cost model (ADR 0233), measured 2026-10-13 on the stock
    -- PostgreSQL 18.4 release package: a write costs a fixed part plus a part per
    -- embedding row it patches, and writes WAL per row in proportion to the
    -- document. Documents over toasted_doc bytes are stored out of line and
    -- rewritten whole.
    fixed_ms       CONSTANT FLOAT8 := 0.15;
    inline_row_ms  CONSTANT FLOAT8 := 0.0028;
    toasted_row_ms CONSTANT FLOAT8 := 0.021;
    inline_wal     CONSTANT FLOAT8 := 0.7;
    toasted_wal    CONSTANT FLOAT8 := 1.2;
    toasted_doc    CONSTANT FLOAT8 := 2000;
BEGIN
    -- The TVIEW named as every function names one (ADR 0211).
    IF tview IS NOT NULL THEN
        chosen := @extschema@.pg_tviews_resolve(tview);
    END IF;

    vis_schema := (SELECT n.nspname FROM pg_extension e
                   JOIN pg_namespace n ON n.oid = e.extnamespace
                   WHERE e.extname = 'pg_visibility');

    FOR r IN
        SELECT m.entity AS ent, c.oid AS rel, n.nspname AS nsp, c.relname AS tbl,
               c.relpersistence AS pers, c.reltuples, c.relpages, c.reltoastrelid,
               c.reloptions,
               -- The columns its rows are looked up by: those holding an
               -- embedded TVIEW's key, those a fan-out patch writes through, and
               -- its references.
               ARRAY(SELECT @extschema@.pg_tviews_plan_lookups(m.plan)) AS lookups
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

        -- What one parent change costs at that p99, by the model: its documents
        -- are the table's bytes per row.
        doc_bytes := (heap_bytes + toast_bytes) / greatest(r.reltuples, 1);
        fanout := (
            SELECT jsonb_object_agg(e.key, e.value || jsonb_build_object(
                       'p99_ms_per_write', round((fixed_ms + (e.value->>'p99')::FLOAT8 *
                           CASE WHEN doc_bytes > toasted_doc THEN toasted_row_ms
                                ELSE inline_row_ms END)::NUMERIC, 2),
                       'p99_wal_bytes_per_write', round((e.value->>'p99')::FLOAT8 * doc_bytes *
                           CASE WHEN doc_bytes > toasted_doc THEN toasted_wal
                                ELSE inline_wal END)))
            FROM jsonb_each(fanout) e);

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
                WHERE i.indrelid = r.rel AND a.attname = 'data'
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
        FOR f IN SELECT key, (value->>'p99')::BIGINT AS p99, value->>'p99_ms_per_write' AS ms,
                        (value->>'p99_wal_bytes_per_write')::BIGINT AS wal
                 FROM jsonb_each(fanout) ORDER BY key LOOP
            IF f.p99 > fanout_warn THEN
                warnings := warnings || format(
                    'p99 fan-out through %s is about %s rows per key: one parent change refreshes that many rows, about %s ms and %s of WAL per write (see docs/user-guides/architects.md#fan-out)',
                    f.key, f.p99, f.ms, pg_size_pretty(f.wal));
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



-- jsonb_delta is required (the control file's requires): nothing checks for it.
DROP FUNCTION @extschema@.pg_tviews_check_jsonb_delta();

-- The single-TVIEW and _all forms (owner checked; operator only).
CREATE FUNCTION @extschema@."pg_tviews_ensure_propagation_indexes"(
	"tview" TEXT, /* &str */
	"dry_run" bool DEFAULT false /* bool */
) RETURNS SETOF TEXT /* alloc::string::String */
STRICT
LANGUAGE c /* Rust */
AS 'MODULE_PATHNAME', 'pg_tviews_ensure_propagation_indexes_wrapper';
CREATE FUNCTION @extschema@."pg_tviews_ensure_propagation_indexes_all"(
	"dry_run" bool DEFAULT false /* bool */
) RETURNS SETOF TEXT /* alloc::string::String */
STRICT
LANGUAGE c /* Rust */
AS 'MODULE_PATHNAME', 'pg_tviews_ensure_propagation_indexes_all_wrapper';
CREATE FUNCTION @extschema@."pg_tviews_reset_stats"(
	"tview" TEXT /* &str */
) RETURNS VOID
STRICT
LANGUAGE c /* Rust */
AS 'MODULE_PATHNAME', 'pg_tviews_stats_reset_wrapper';
CREATE FUNCTION @extschema@."pg_tviews_reset_stats_all"() RETURNS VOID
STRICT
LANGUAGE c /* Rust */
AS 'MODULE_PATHNAME', 'pg_tviews_reset_stats_all_wrapper';
REVOKE EXECUTE ON FUNCTION
    @extschema@.pg_tviews_ensure_propagation_indexes_all(BOOLEAN),
    @extschema@.pg_tviews_reset_stats_all()
FROM PUBLIC;
COMMENT ON VIEW @extschema@.stats IS
'Refresh statistics of each TVIEW since the server started or pg_tviews_reset_stats()';

-- The plan registration would store for a view, without creating a TVIEW (the
-- plan-snapshot tests).
CREATE FUNCTION @extschema@."pg_tviews_plan_of"(
	"view" oid, /* pgrx_pg_sys::submodules::oids::Oid */
	"tview" TEXT /* &str */
) RETURNS jsonb
STRICT
LANGUAGE c /* Rust */
AS 'MODULE_PATHNAME', 'pg_tviews_plan_of_wrapper';

-- A backing view is its owner's alone: it reads the base tables with the owner's
-- privileges, so a grant copied from its table handed back the rows row-level
-- security hid there. Every other role's privileges on it are revoked.
DO $$
DECLARE
    revoke_all text;
BEGIN
    FOR revoke_all IN
        SELECT pg_catalog.format('REVOKE ALL ON %I.%I FROM %s CASCADE', n.nspname, v.relname,
                   pg_catalog.string_agg(DISTINCT CASE WHEN a.grantee = 0 THEN 'PUBLIC'
                       ELSE pg_catalog.quote_ident(pg_catalog.pg_get_userbyid(a.grantee)) END, ', '))
        FROM @extschema@.pg_tview_meta m
        JOIN pg_catalog.pg_class v ON v.oid = m.view_oid::pg_catalog.oid
        JOIN pg_catalog.pg_namespace n ON n.oid = v.relnamespace,
             pg_catalog.aclexplode(v.relacl) a
        WHERE a.grantee <> v.relowner
        GROUP BY v.oid, n.nspname, v.relname
    LOOP
        EXECUTE revoke_all;
    END LOOP;
END $$;

-- What registration derives changed (an aggregate's paths come from its query
-- tree, the plan records the row key, the plan readers of older versions are
-- gone): every TVIEW is re-derived here, inside the update, as its owner. A TVIEW
-- that no longer analyses fails the update, named with its error; nothing is left
-- half-migrated.
UPDATE @extschema@.pg_tview_meta SET needs_reregister = true;
SELECT pg_catalog.count(*) FROM @extschema@.pg_tviews_reregister_all(strict => true);
