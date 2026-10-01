//! Metadata Management: TVIEW Catalog Tables and Schema
//!
//! This module manages the system catalog tables for TVIEW metadata:
//! - **`pg_tview_meta`**: Core TVIEW definitions and relationships
//! - **`pg_tview_monitoring`**: Performance metrics and statistics
//! - **Schema Management**: Automatic table creation and updates
//!
//! ## Catalog Tables
//!
//! ### `pg_tview_meta`
//! Stores complete TVIEW definitions:
//! - Entity name and OIDs
//! - SQL definition and dependencies
//! - Foreign key relationships
//! - Dependency types and paths
//!
//! ## Extension Lifecycle
//!
//! - **CREATE EXTENSION**: Creates catalog tables
//! - **ALTER EXTENSION**: Handles schema migrations
//! - **DROP EXTENSION**: Cleans up metadata

use crate::error::{TViewError, TViewResult};
use pgrx::prelude::*;

// The control file fixes the install schema to `tviews` (issue #136). CREATE
// EXTENSION creates it when missing, owned by the installing role, but adopts an
// existing one as is: refuse one owned by another role, which could replace the
// objects created in it. The install script never uses CREATE OR REPLACE or
// IF NOT EXISTS, so an object planted under one of its names is an error too.
extension_sql!(
    r#"
DO $$
DECLARE
    schema_owner NAME;
BEGIN
    SELECT pg_catalog.pg_get_userbyid(n.nspowner) INTO schema_owner
      FROM pg_catalog.pg_namespace n
     WHERE n.nspname = '@extschema@';
    IF schema_owner IS DISTINCT FROM CURRENT_USER THEN
        RAISE EXCEPTION 'schema "@extschema@" already exists and is owned by role "%"',
            schema_owner
            USING HINT = 'pg_tviews installs into schema @extschema@, which must be owned '
                         'by the role running CREATE EXTENSION. Drop the schema or change '
                         'its owner; to restore a dump, restore it as that owner or with '
                         'pg_restore --no-owner.';
    END IF;
END
$$;

-- Every role reaches the triggers, functions and catalog views (issue #136).
GRANT USAGE ON SCHEMA @extschema@ TO PUBLIC;
    "#,
    name = "check_extension_schema",
    bootstrap
);

// Generate SQL to create metadata tables during extension installation.
// @extschema@ is substituted by PostgreSQL with the extension's install schema.
extension_sql!(
    r"
    -- view_oid / table_oid are regclass, not oid: pg_dump writes them as qualified
    -- names, so a restored row names the restored relations (issue #96).
    CREATE TABLE @extschema@.pg_tview_meta (
        entity TEXT NOT NULL PRIMARY KEY,
        view_oid REGCLASS NOT NULL,
        table_oid REGCLASS NOT NULL,
        definition TEXT NOT NULL,
        cascade_paths TEXT[] NOT NULL DEFAULT '{}',
        fk_columns TEXT[] NOT NULL DEFAULT '{}',
        uuid_fk_columns TEXT[] NOT NULL DEFAULT '{}',
        dependency_types TEXT[] NOT NULL DEFAULT '{}',
        dependency_paths TEXT[]  NOT NULL DEFAULT '{}',
        array_match_keys TEXT[] NOT NULL DEFAULT '{}',
        distinct_on_keys TEXT[] NOT NULL DEFAULT '{}',
        distinct_on_output_keys TEXT[] NOT NULL DEFAULT '{}',
        direct_map_columns TEXT[] NOT NULL DEFAULT '{}',
        direct_map_keys TEXT[] NOT NULL DEFAULT '{}',
        is_union BOOLEAN NOT NULL DEFAULT FALSE,
        created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
        -- GraphQL type reported by pg_tviews_flush_and_report (issue #76); NULL
        -- means PascalCase(entity).
        graphql_typename TEXT,
        -- Aggregate TVIEWs (issue #58): source table name -> group key column.
        group_keys JSONB,
        -- Aggregate TVIEWs this one embeds (issue #126): aggregate entity -> the
        -- output column holding the aggregate's key, used to propagate aggregate
        -- changes.
        aggregate_embeds JSONB NOT NULL DEFAULT '{}',
        -- A release changed what registration derives since this TVIEW was last
        -- registered (issue #137): it keeps refreshing with its old metadata until
        -- pg_tviews_reregister() re-derives it. Upgrade scripts set it.
        needs_reregister BOOLEAN NOT NULL DEFAULT false
    );

    CREATE TABLE @extschema@.pg_tview_helpers (
        helper_name TEXT NOT NULL PRIMARY KEY,
        is_helper BOOLEAN NOT NULL DEFAULT TRUE,
        used_by TEXT[] NOT NULL DEFAULT '{}',
        depends_on TEXT[] NOT NULL DEFAULT '{}',
        created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
    );

    COMMENT ON TABLE @extschema@.pg_tview_meta IS
        'Internal TVIEW registrations; may change in any release. Tools read @extschema@.registry';
    COMMENT ON TABLE @extschema@.pg_tview_helpers IS
        'Internal: helper views used by TVIEWs; may change in any release';

    -- Indexes for catalog lookup performance (entity PK already has a unique index)
    CREATE INDEX idx_pg_tview_meta_table_oid
        ON @extschema@.pg_tview_meta(table_oid);

    -- Extension-owned tables are skipped by pg_dump unless marked: without this a
    -- dump/restore brings back tv_*, v_* and the triggers but no registered TVIEW.
    SELECT pg_catalog.pg_extension_config_dump('@extschema@.pg_tview_meta', '');
    SELECT pg_catalog.pg_extension_config_dump('@extschema@.pg_tview_helpers', '');

    -- The row trigger reads the catalog as the writing role (issue #136). It holds
    -- view definitions, which pg_views already shows to everyone. Only the
    -- extension owner writes it.
    GRANT SELECT ON @extschema@.pg_tview_meta, @extschema@.pg_tview_helpers TO PUBLIC;

    -- Revision of this catalog (issue #137). The library refuses to work against a
    -- catalog of another revision; an upgrade script that changes the extension SQL
    -- redefines this function, and the library's revision::CATALOG_REVISION with it.
    CREATE FUNCTION @extschema@.pg_tviews_catalog_revision()
    RETURNS integer
    LANGUAGE sql IMMUTABLE PARALLEL SAFE
    AS 'SELECT 1';
    ",
    name = "create_metadata_tables",
);

// The relations each TVIEW's backing view reads, followed through views (issue
// #139): the tables its triggers belong on, the dependency order of TVIEWs, and the
// registry's base_tables. Plain SQL over the catalogs.
extension_sql!(
    r"
CREATE VIEW @extschema@.pg_tview_reads AS
WITH RECURSIVE reads(entity, relid) AS (
    SELECT m.entity, m.view_oid::oid FROM @extschema@.pg_tview_meta m
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

COMMENT ON VIEW @extschema@.pg_tview_reads IS
'Internal: relations each TVIEW reads, through views; may change in any release';

GRANT SELECT ON @extschema@.pg_tview_reads TO PUBLIC;
    ",
    name = "tview_reads",
    requires = ["create_metadata_tables"],
);

// The read contract for tools (issue #133, ADR 0136 Decision 4). Plain SQL over
// the internal tables and the system catalogs, calling no function of the library,
// so it can be read without the library, with a mismatched one, and on a standby.
// contract_version() covers the view's columns, the `options` keys and the
// behaviour of pg_tviews_create_or_replace(): additions keep it, anything else
// bumps it (docs/reference/read-contract.md).
extension_sql!(
    r"
CREATE FUNCTION @extschema@.contract_version()
RETURNS integer
LANGUAGE sql STABLE PARALLEL SAFE
AS 'SELECT 1';

COMMENT ON FUNCTION @extschema@.contract_version() IS
'Version of the read contract: @extschema@.registry and pg_tviews_create_or_replace()';

-- A registration whose table is gone stays visible, with NULL for what the table
-- would tell; `view` is NULL once the view is gone. data_gin_index is a valid,
-- default (jsonb_ops) GIN index on data.
CREATE VIEW @extschema@.registry AS
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
    v.oid::pg_catalog.regclass AS view
FROM @extschema@.pg_tview_meta m
LEFT JOIN pg_catalog.pg_class c ON c.oid = m.table_oid
LEFT JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
LEFT JOIN pg_catalog.pg_class v ON v.oid = m.view_oid;

COMMENT ON VIEW @extschema@.registry IS
'One row per registered TVIEW; stable under contract_version()';

GRANT SELECT ON @extschema@.registry TO PUBLIC;
    ",
    name = "read_contract",
    requires = ["create_metadata_tables", "tview_reads"],
);

// Register event triggers for DDL interception
// The PL/pgSQL `pg_tviews_handle_ddl_event()` function is defined in this SQL block.
// It calls `pg_tviews_convert_table()`, which is a #[pg_extern] C function in event_trigger.rs.
// Note: we do NOT use a Rust #[pg_extern] for the event trigger handler itself because pgrx
// generates RETURNS VOID instead of the required RETURNS event_trigger pseudo-type.
extension_sql!(
    r"
-- Event trigger handler: the ProcessUtility hook turns CREATE TABLE tv_* AS into a TVIEW
-- before PostgreSQL creates anything, so a tv_* table created this way means the hook did
-- not see the statement. pg_tviews_convert_table() (src/event_trigger.rs) reports that
-- as an error. PL/pgSQL because pgrx cannot declare RETURNS event_trigger.
CREATE FUNCTION @extschema@.pg_tviews_handle_ddl_event()
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
                DECLARE
                    table_name_only TEXT;
                BEGIN
                    table_name_only := CASE
                        WHEN obj.object_identity LIKE '%.%'
                        THEN split_part(obj.object_identity, '.', 2)
                        ELSE obj.object_identity
                    END;

                    PERFORM @extschema@.pg_tviews_convert_table(table_name_only, obj.command_tag);
                EXCEPTION
                    WHEN OTHERS THEN
                        -- pg_tviews_convert_table raises its own error; re-raise here.
                        RAISE;
                END;
            END IF;
        END IF;
    END LOOP;
END;
$$;

-- Create the event trigger (fires after CREATE TABLE completes — safe SPI context)
CREATE EVENT TRIGGER pg_tviews_ddl_end
    ON ddl_command_end
    WHEN TAG IN ('CREATE TABLE', 'CREATE TABLE AS', 'SELECT INTO')
    EXECUTE FUNCTION @extschema@.pg_tviews_handle_ddl_event();

COMMENT ON EVENT TRIGGER pg_tviews_ddl_end IS
'Fails a CREATE TABLE tv_* AS that the pg_tviews hook did not turn into a TVIEW';

-- Event trigger handler: deregister a TVIEW whose backing view or table was dropped as
-- a dependent of something else (issues #53, #57).  The base-table -> tview link is not
-- a hard PG dependency, so CASCADE from a base table, a helper view or a schema removes
-- the backing view v_* (and the base-table triggers on that table) but never the
-- trigger-populated tv_* table, its pg_tview_meta row or its triggers on other tables.
-- The dropped view is matched by OID, so any TVIEW reading the dropped object is found,
-- whatever its name.
--
-- Only objects dropped as dependents (original = false) count: pg_tviews' own drops of
-- v_* / tv_* (pg_tviews_drop, DROP TABLE tv_* via the ProcessUtility hook) name them
-- directly, so they never re-enter here.
--
-- PL/pgSQL (not #[pg_extern]) because pgrx cannot emit RETURNS event_trigger.  It fires for
-- EVERY dropped object system-wide, so it must be cheap and must never break an unrelated
-- DROP: references are schema-qualified via @extschema@ (search-path independent) and the
-- work is guarded by a defensive EXCEPTION handler.
--
-- Runs as the dropping role (issue #136): PostgreSQL authorized that role's drop, and
-- pg_tviews_handle_dropped() does the rest without giving it more rights (see there).
CREATE FUNCTION @extschema@.pg_tviews_handle_drop_event()
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
        WHERE NOT d.original
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

CREATE EVENT TRIGGER pg_tviews_sql_drop
    ON sql_drop
    EXECUTE FUNCTION @extschema@.pg_tviews_handle_drop_event();

COMMENT ON EVENT TRIGGER pg_tviews_sql_drop IS
'Deregisters a TVIEW whose backing view or table was dropped as a dependent (issues #53, #57)';

-- Whether candidate SQL defines the same view as view_oid (issue #81): a column
-- rename rewrites a TVIEW's stored definition, and the rewrite is kept only if
-- PostgreSQL renders it exactly like the renamed backing view. The EXCEPTION
-- block turns any failure (syntax, unknown column) into false and discards the
-- scratch view.
CREATE FUNCTION @extschema@.pg_tviews_defines_view(view_oid OID, candidate TEXT)
RETURNS BOOLEAN
LANGUAGE plpgsql
AS $$
DECLARE
    same BOOLEAN;
BEGIN
    EXECUTE 'CREATE TEMP VIEW pg_tviews_rename_check AS ' || candidate;
    same := pg_catalog.pg_get_viewdef('pg_temp.pg_tviews_rename_check'::regclass)
            = pg_catalog.pg_get_viewdef(view_oid);
    DROP VIEW pg_temp.pg_tviews_rename_check;
    RETURN same;
EXCEPTION WHEN OTHERS THEN
    RETURN false;
END;
$$;

-- Catalog rows loaded by pg_restore carry the source database's OIDs inside
-- cascade_paths (JSON text; view_oid / table_oid are regclass and re-resolve on
-- their own). Rebind them to the restored relations as each row is inserted.
-- For a row written by pg_tviews itself the rebind is the identity.
CREATE FUNCTION @extschema@.pg_tviews_meta_rebind()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, pg_temp
AS $$
BEGIN
    NEW.cascade_paths := @extschema@.pg_tviews_rebind_cascade_paths(
        NEW.view_oid::oid, NEW.cascade_paths);
    RETURN NEW;
END;
$$;

CREATE TRIGGER pg_tview_meta_rebind
    BEFORE INSERT ON @extschema@.pg_tview_meta
    FOR EACH ROW
    WHEN (pg_catalog.cardinality(NEW.cascade_paths) > 0)
    EXECUTE FUNCTION @extschema@.pg_tviews_meta_rebind();

-- Other backends cache TVIEW metadata (issue #91). Any write to the catalog
-- invalidates its relcache entry at commit, which every backend watches.
CREATE FUNCTION @extschema@.pg_tviews_meta_changed()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    PERFORM @extschema@.pg_tviews_invalidate_caches(TG_RELID);
    RETURN NULL;
END;
$$;

CREATE TRIGGER pg_tview_meta_changed
    AFTER INSERT OR UPDATE OR DELETE OR TRUNCATE ON @extschema@.pg_tview_meta
    FOR EACH STATEMENT
    EXECUTE FUNCTION @extschema@.pg_tviews_meta_changed();
    ",
    name = "event_triggers",
    requires = ["create_metadata_tables"],
    finalize
);

// pg_tviews_convert_table is auto-registered via #[pg_extern] in src/event_trigger.rs

// Audit logging table for DDL operations
extension_sql!(
    r"
CREATE TABLE @extschema@.pg_tview_audit_log (
    log_id BIGSERIAL PRIMARY KEY,
    operation TEXT NOT NULL,  -- CREATE, DROP, REFRESH
    entity TEXT NOT NULL,
    performed_by TEXT NOT NULL DEFAULT current_user,
    performed_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    transaction_id BIGINT DEFAULT pg_current_xact_id()::text::bigint,
    rows_affected BIGINT,
    details JSONB,
    client_addr INET DEFAULT inet_client_addr(),
    client_port INTEGER DEFAULT inet_client_port()
);

CREATE INDEX idx_audit_log_entity_time ON @extschema@.pg_tview_audit_log(entity, performed_at);

COMMENT ON TABLE @extschema@.pg_tview_audit_log IS 'Audit log for TVIEW operations';

-- Writes buffered audit entries (issue #136). Only the extension owner may call it:
-- the library calls it as that owner, for whichever role triggered the entries, and
-- performed_by is the session user, whatever role the caller has set.
CREATE FUNCTION @extschema@.pg_tviews_audit_write(entries JSONB)
RETURNS void
LANGUAGE sql
SET search_path = pg_catalog, @extschema@, pg_temp
AS $$
    INSERT INTO @extschema@.pg_tview_audit_log
        (operation, entity, performed_by, rows_affected, details)
    SELECT e->>'op', e->>'entity', SESSION_USER, (e->>'rows')::bigint,
           CASE WHEN e->'details' = 'null'::jsonb THEN NULL ELSE e->'details' END
    FROM jsonb_array_elements(entries) AS e;
$$;
REVOKE EXECUTE ON FUNCTION @extschema@.pg_tviews_audit_write(JSONB) FROM PUBLIC;
    ",
    name = "audit_table",
);

// Monitoring views for production observability
extension_sql!(
    r"
-- Queue monitoring view
CREATE VIEW @extschema@.pg_tviews_queue_realtime AS
SELECT
    current_setting('application_name') as session,
    pg_backend_pid() as backend_pid,
    txid_current() as transaction_id,
    0 as queue_size,
    ARRAY[]::TEXT[] as entities,
    NOW() as last_enqueued;

-- Cache statistics view
CREATE VIEW @extschema@.pg_tviews_cache_stats AS
SELECT
    'graph_cache' as cache_type,
    0::BIGINT as entries,
    '0 bytes' as estimated_size
UNION ALL
SELECT
    'table_cache' as cache_type,
    0::BIGINT as entries,
    '0 bytes' as estimated_size;

-- Performance summary view
CREATE VIEW @extschema@.pg_tviews_performance_summary AS
SELECT
    entity,
    COUNT(*) as total_refreshes,
    0.0 as avg_refresh_ms,
    NOW() as last_refresh
FROM @extschema@.pg_tview_meta
GROUP BY entity;
    ",
    name = "monitoring_views",
    requires = ["create_metadata_tables"]
);

// Per-TVIEW physical health report (issue #74). Pure SQL over the catalogs and the
// statistics views, so it is read-only and callable on a hot standby.
extension_sql!(
    r"
CREATE FUNCTION @extschema@.pg_tviews_profile(
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
               c.reloptions
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
        -- propagation indexes cascades need (leading fk_* column).
        unused_indexes := ARRAY(
            SELECT quote_ident(si.indexrelname) FROM pg_index i
            JOIN pg_stat_all_indexes si ON si.indexrelid = i.indexrelid
            JOIN pg_attribute a ON a.attrelid = i.indrelid AND a.attnum = i.indkey[0]
            WHERE i.indrelid = r.rel AND NOT i.indisprimary AND NOT i.indisunique
              AND si.idx_scan = 0 AND a.attname NOT LIKE 'fk\_%'
            ORDER BY 1);

        -- Integer fk_* columns that no index leads with (issue #71).
        missing_propagation_indexes := ARRAY(
            SELECT a.attname::TEXT FROM pg_attribute a
            WHERE a.attrelid = r.rel AND a.attnum > 0 AND NOT a.attisdropped
              AND a.attname LIKE 'fk\_%'
              AND a.atttypid IN ('int2'::REGTYPE, 'int4'::REGTYPE, 'int8'::REGTYPE)
              AND NOT EXISTS (SELECT 1 FROM pg_index i
                              WHERE i.indrelid = r.rel AND i.indkey[0] = a.attnum)
            ORDER BY 1);

        -- Estimated rows per key of each fk_* column, from the planner statistics:
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
                    AND s.attname LIKE 'fk\_%' AND r.reltuples > 0) x);

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

COMMENT ON FUNCTION @extschema@.pg_tviews_profile(TEXT, BIGINT) IS
'Physical health of each TVIEW (sizes, HOT ratio, dead tuples, indexes, fan-out) with warnings';
    ",
    name = "profile_function",
    requires = ["create_metadata_tables"]
);

/// Create the metadata tables required for `pg_tviews` extension
///
/// # Errors
/// Returns error if table creation fails due to insufficient permissions or SQL errors
pub fn create_metadata_tables() -> TViewResult<()> {
    Spi::run(
        r"
        CREATE TABLE IF NOT EXISTS pg_tview_meta (
            entity TEXT NOT NULL PRIMARY KEY,
            view_oid REGCLASS NOT NULL,
            table_oid REGCLASS NOT NULL,
            definition TEXT NOT NULL,
            cascade_paths TEXT[] NOT NULL DEFAULT '{}',
            fk_columns TEXT[] NOT NULL DEFAULT '{}',
            uuid_fk_columns TEXT[] NOT NULL DEFAULT '{}',
            dependency_types TEXT[] NOT NULL DEFAULT '{}',
            dependency_paths TEXT[]  NOT NULL DEFAULT '{}',
            array_match_keys TEXT[] NOT NULL DEFAULT '{}',
            distinct_on_keys TEXT[] NOT NULL DEFAULT '{}',
            direct_map_columns TEXT[] NOT NULL DEFAULT '{}',
            direct_map_keys TEXT[] NOT NULL DEFAULT '{}',
            is_union BOOLEAN NOT NULL DEFAULT FALSE,
            created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
        );

        CREATE TABLE IF NOT EXISTS pg_tview_helpers (
            helper_name TEXT NOT NULL PRIMARY KEY,
            is_helper BOOLEAN NOT NULL DEFAULT TRUE,
            used_by TEXT[] NOT NULL DEFAULT '{}',
            depends_on TEXT[] NOT NULL DEFAULT '{}',
            created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
        );

        COMMENT ON TABLE pg_tview_meta IS
            'Metadata for TVIEW materialized tables';
        COMMENT ON TABLE pg_tview_helpers IS
            'Tracks helper views used by TVIEWs';
        ",
    )
    .map_err(|e| TViewError::CatalogError {
        operation: "create_metadata_tables".to_string(),
        pg_error: e.to_string(),
    })?;

    Ok(())
}

/// Drop all metadata tables (for testing/cleanup)
///
/// # Errors
/// Returns error if table drop fails due to insufficient permissions or SQL errors
pub fn drop_metadata_tables() -> TViewResult<()> {
    Spi::run(
        r"
        DROP TABLE IF EXISTS pg_tview_helpers;
        DROP TABLE IF EXISTS pg_tview_meta;
        ",
    )
    .map_err(|e| TViewError::CatalogError {
        operation: "drop_metadata_tables".to_string(),
        pg_error: e.to_string(),
    })?;

    Ok(())
}

/// Check if metadata tables exist
///
/// # Errors
/// Returns error if `information_schema` query fails
pub fn metadata_tables_exist() -> TViewResult<bool> {
    let meta_exists = Spi::get_one::<bool>(
        "SELECT COUNT(*) = 1 FROM information_schema.tables
         WHERE table_name = 'pg_tview_meta'",
    )
    .map_err(|e| TViewError::SpiError {
        query: "check pg_tview_meta exists".to_string(),
        error: e.to_string(),
    })?;

    let helpers_exists = Spi::get_one::<bool>(
        "SELECT COUNT(*) = 1 FROM information_schema.tables
         WHERE table_name = 'pg_tview_helpers'",
    )
    .map_err(|e| TViewError::SpiError {
        query: "check pg_tview_helpers exists".to_string(),
        error: e.to_string(),
    })?;

    Ok(meta_exists.unwrap_or(false) && helpers_exists.unwrap_or(false))
}

#[cfg(any(test, feature = "pg_test"))]
#[pg_schema]
mod tests {
    use super::*;

    #[pg_test]
    fn test_metadata_tables_creation() {
        // Clean up first
        let _ = drop_metadata_tables();

        // Create tables
        create_metadata_tables().expect("Failed to create metadata tables");

        // Verify pg_tview_meta exists
        let result = Spi::get_one::<bool>(
            "SELECT COUNT(*) = 1 FROM information_schema.tables
             WHERE table_name = 'pg_tview_meta'",
        );
        assert_eq!(result, Ok(Some(true)), "pg_tview_meta table should exist");

        // Verify pg_tview_helpers exists
        let result = Spi::get_one::<bool>(
            "SELECT COUNT(*) = 1 FROM information_schema.tables
             WHERE table_name = 'pg_tview_helpers'",
        );
        assert_eq!(
            result,
            Ok(Some(true)),
            "pg_tview_helpers table should exist"
        );

        // Verify pg_tview_meta has expected columns
        let result = Spi::get_one::<i64>(
            "SELECT COUNT(*) FROM information_schema.columns
             WHERE table_name = 'pg_tview_meta'",
        );
        assert!(
            result.unwrap_or(Some(0)).unwrap_or(0) > 0,
            "pg_tview_meta should have columns"
        );
    }

    #[pg_test]
    fn test_metadata_tables_schema() {
        // Ensure tables exist
        create_metadata_tables().expect("Failed to create metadata tables");

        // Check pg_tview_meta columns
        let columns = Spi::connect(|client| {
            let mut columns = Vec::new();
            let query = "
                SELECT column_name, data_type, is_nullable::text
                FROM information_schema.columns
                WHERE table_name = 'pg_tview_meta'
                ORDER BY ordinal_position
            ";

            for row in client.select(query, None, &[])? {
                let name: String = row.get(1)?.unwrap_or_default();
                let data_type: String = row.get(2)?.unwrap_or_default();
                let nullable: String = row.get(3)?.unwrap_or_default();
                columns.push((name, data_type, nullable));
            }

            Ok::<_, pgrx::spi::SpiError>(columns)
        })
        .expect("Failed to query column info");

        // Verify expected columns exist
        let expected_columns = vec![
            ("entity", "text", "NO"),
            ("view_oid", "oid", "NO"),
            ("table_oid", "oid", "NO"),
            ("definition", "text", "NO"),
            ("cascade_paths", "ARRAY", "NO"),
            ("fk_columns", "ARRAY", "NO"),
            ("uuid_fk_columns", "ARRAY", "NO"),
            ("dependency_types", "ARRAY", "NO"),
            ("dependency_paths", "ARRAY", "NO"),
            ("array_match_keys", "ARRAY", "NO"),
            ("created_at", "timestamp with time zone", "NO"),
        ];

        for (expected_name, expected_type, expected_nullable) in expected_columns {
            let found = columns.iter().any(|(name, data_type, nullable)| {
                name == expected_name
                    && (data_type == expected_type || data_type.starts_with(expected_type))
                    && nullable == expected_nullable
            });
            assert!(
                found,
                "Column {expected_name} with type {expected_type} nullable {expected_nullable} not found"
            );
        }
    }

    #[pg_test]
    fn test_metadata_tables_exist_function() {
        // Clean up first
        let _ = drop_metadata_tables();
        assert_eq!(
            metadata_tables_exist(),
            Ok(false),
            "Tables should not exist initially"
        );

        // Create tables
        create_metadata_tables().expect("Failed to create metadata tables");
        assert_eq!(
            metadata_tables_exist(),
            Ok(true),
            "Tables should exist after creation"
        );
    }
}
