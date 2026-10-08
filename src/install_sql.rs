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
//! - SQL definition and its propagation plan (ADR 0203)
//!
//! ## Extension Lifecycle
//!
//! - **CREATE EXTENSION**: Creates catalog tables
//! - **ALTER EXTENSION**: Handles schema migrations
//! - **DROP EXTENSION**: Cleans up metadata

use pgrx::prelude::*;

// The control file fixes the install schema to `tviews`. CREATE
// EXTENSION creates it when missing, owned by the installing role, but adopts an
// existing one as is: refuse one owned by another role, which could replace the
// objects created in it. The install script never uses CREATE OR REPLACE or
// IF NOT EXISTS, so an object planted under one of its names is an error too.
extension_sql!(
    r#"
-- Supported PostgreSQL versions: 16, 17, 18. Refuse older servers here, before
-- anything loads the library, with a message instead of a load error.
DO $$
BEGIN
    IF pg_catalog.current_setting('server_version_num')::int < 160000 THEN
        RAISE EXCEPTION 'pg_tviews requires PostgreSQL 16 or later (this server is %)',
            pg_catalog.current_setting('server_version');
    END IF;
END
$$;

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

-- Every role reaches the triggers, functions and catalog views.
GRANT USAGE ON SCHEMA @extschema@ TO PUBLIC;

-- Say once, here, that refreshes run without jsonb_delta; the
-- refresh path itself only writes it to the server log. A WARNING, because
-- CREATE EXTENSION hides an install script's NOTICEs.
DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_catalog.pg_extension WHERE extname = 'jsonb_delta') THEN
        RAISE WARNING 'smart JSONB patching is disabled: jsonb_delta is not installed'
            USING HINT = 'CREATE EXTENSION jsonb_delta enables it; cascades then patch '
                         'documents instead of replacing them.';
    END IF;
END
$$;
    "#,
    name = "check_extension_schema",
    bootstrap
);

// Generate SQL to create metadata tables during extension installation.
// @extschema@ is substituted by PostgreSQL with the extension's install schema.
extension_sql!(
    r"
    -- view_oid / table_oid are regclass, not oid: pg_dump writes them as qualified
    -- names, so a restored row names the restored relations.
    CREATE TABLE @extschema@.pg_tview_meta (
        entity TEXT NOT NULL PRIMARY KEY,
        view_oid REGCLASS NOT NULL,
        table_oid REGCLASS NOT NULL,
        definition TEXT NOT NULL,
        -- What registration derived from the backing view's query tree (ADR 0203),
        -- one versioned document: how a write to each base table maps to keys
        -- (tables), the tables whose rows carry a key (paths), the TVIEWs it
        -- embeds (embeds) and the direct-patch map (direct).
        plan JSONB NOT NULL,
        created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
        -- GraphQL type reported by pg_tviews_flush_and_report; NULL
        -- means PascalCase(entity).
        graphql_typename TEXT,
        -- Aggregate TVIEWs: source table name -> group key column.
        group_keys JSONB,
        -- A release changed what registration derives since this TVIEW was last
        -- registered: it keeps refreshing with its old metadata until
        -- pg_tviews_reregister() re-derives it. Upgrade scripts set it.
        needs_reregister BOOLEAN NOT NULL DEFAULT false,
        -- Base tables the backing view reads whose writes no cascade maps to this
        -- TVIEW's keys. regclass, like view_oid: a dump names
        -- them, so a restored row names the restored tables.
        uncascaded_oids REGCLASS[] NOT NULL DEFAULT '{}',
        -- pg_tviews.uncascaded_policy when the TVIEW was created: what a write to
        -- one of uncascaded_oids does. The row trigger reads this, never the
        -- writing session's setting.
        uncascaded_policy TEXT NOT NULL DEFAULT 'warn'
            CHECK (uncascaded_policy IN ('warn', 'error', 'full_refresh')),
        -- The output column that names this TVIEW's rows (ADR 0169), read from the
        -- backing view's query tree: an object with its kind (pk, distinct_on) and
        -- its columns (name, type). Every registration writes it.
        identity JSONB,
        -- Tables declared with a policy of their own, in the
        -- uncascaded_tables option: uncascaded_table_policies[i] applies to writes
        -- to uncascaded_table_oids[i] instead of uncascaded_policy.
        uncascaded_table_oids REGCLASS[] NOT NULL DEFAULT '{}',
        uncascaded_table_policies TEXT[] NOT NULL DEFAULT '{}',
        -- The functions the definition calls that may read tables,
        -- declared in the function_reads option with the tables each reads: one
        -- (function, table) pair per table, NULL for a function reading none.
        -- Functions as text, schema.name(argument types): a regprocedure column
        -- would block pg_upgrade.
        function_read_functions TEXT[] NOT NULL DEFAULT '{}',
        function_read_tables REGCLASS[] NOT NULL DEFAULT '{}',
        -- How a TVIEW that reads the current time is brought up to date:
        -- 'external', pg_tviews_refresh_time_dependent() called at the
        -- boundary; NULL for a TVIEW that reads no time or declared nothing.
        time_refresh TEXT CHECK (time_refresh IN ('external')),
        -- The definition reads the current time (CURRENT_DATE, now()…), so its
        -- rows change with no write.
        time_dependent BOOLEAN NOT NULL DEFAULT false
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

    -- The row trigger reads the catalog as the writing role. It holds
    -- view definitions, which pg_views already shows to everyone. Only the
    -- extension owner writes it.
    GRANT SELECT ON @extschema@.pg_tview_meta, @extschema@.pg_tview_helpers TO PUBLIC;

    -- Revision of this catalog. The library refuses to work against a
    -- catalog of another revision; an upgrade script that changes the extension SQL
    -- redefines this function, and the library's revision::CATALOG_REVISION with it.
    CREATE FUNCTION @extschema@.pg_tviews_catalog_revision()
    RETURNS integer
    LANGUAGE sql IMMUTABLE PARALLEL SAFE
    AS 'SELECT 5';
    ",
    name = "create_metadata_tables",
);

// The relations each TVIEW's backing view reads, followed through views: the
// tables its triggers belong on, the dependency order of TVIEWs, and the
// registry's base_tables. Plain SQL over the catalogs.
extension_sql!(
    r"
CREATE VIEW @extschema@.pg_tview_reads AS
WITH RECURSIVE reads(entity, relid) AS (
    SELECT m.entity, m.view_oid::oid FROM @extschema@.pg_tview_meta m
  UNION
    -- Tables read inside the functions it calls, as declared.
    SELECT m.entity, t.relid::oid
    FROM @extschema@.pg_tview_meta m,
         pg_catalog.unnest(m.function_read_tables) AS t(relid)
    WHERE t.relid IS NOT NULL
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

// The read contract for tools (ADR 0136 Decision 4). Plain SQL over
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

COMMENT ON VIEW @extschema@.registry IS
'One row per registered TVIEW; stable under contract_version()';

GRANT SELECT ON @extschema@.registry TO PUBLIC;
    ",
    name = "read_contract",
    requires = ["create_metadata_tables", "tview_reads"],
);

// The event trigger that reports a `CREATE TABLE tv_* AS` the ProcessUtility hook did
// not intercept. PL/pgSQL because pgrx generates RETURNS VOID instead of the required
// RETURNS event_trigger pseudo-type.
extension_sql!(
    r"
-- Event trigger handler: the ProcessUtility hook turns CREATE TABLE tv_* AS into a TVIEW
-- before PostgreSQL creates anything, so a tv_* table created this way means the hook did
-- not see the statement (pg_tviews is not in shared_preload_libraries): fail loudly
-- rather than leave a table deploy tools would take for a TVIEW.
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

-- Create the event trigger (fires after CREATE TABLE completes — safe SPI context)
CREATE EVENT TRIGGER pg_tviews_ddl_end
    ON ddl_command_end
    WHEN TAG IN ('CREATE TABLE', 'CREATE TABLE AS', 'SELECT INTO')
    EXECUTE FUNCTION @extschema@.pg_tviews_handle_ddl_event();

COMMENT ON EVENT TRIGGER pg_tviews_ddl_end IS
'Fails a CREATE TABLE tv_* AS that the pg_tviews hook did not turn into a TVIEW';

-- Event trigger handler: deregister a TVIEW whose backing view or table was dropped as
-- a dependent of something else.  The base-table -> tview link is not
-- a hard PG dependency, so CASCADE from a base table, a helper view or a schema removes
-- the backing view v_* (and the base-table triggers on that table) but never the
-- trigger-populated tv_* table, its pg_tview_meta row or its triggers on other tables.
-- The dropped view is matched by OID, so any TVIEW reading the dropped object is found,
-- whatever its name.
--
-- Only objects dropped as dependents (original = false) count: pg_tviews' own drops of
-- v_* / tv_* (pg_tviews_drop, DROP TABLE tv_* via the ProcessUtility hook) name them
-- directly, so they never re-enter here. DROP OWNED names a role's objects directly
-- too, and pg_tviews never runs it: its drops count whatever their flag.
--
-- PL/pgSQL (not #[pg_extern]) because pgrx cannot emit RETURNS event_trigger.  It fires for
-- EVERY dropped object system-wide, so it must be cheap and must never break an unrelated
-- DROP: references are schema-qualified via @extschema@ (search-path independent) and the
-- work is guarded by a defensive EXCEPTION handler.
--
-- Runs as the dropping role: PostgreSQL authorized that role's drop, and
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
        WHERE (NOT d.original OR TG_TAG = 'DROP OWNED')
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
'Deregisters a TVIEW whose backing view or table was dropped as a dependent';

-- Whether candidate SQL defines the same view as view_oid: a column
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
-- plan (view_oid / table_oid are regclass and re-resolve on their own). Rebind
-- them to the restored relations, found by their qualified names, as each row is
-- inserted; a table that no longer resolves fails the insert. For a row written
-- by pg_tviews itself the rebind is the identity.
CREATE FUNCTION @extschema@.pg_tviews_meta_rebind()
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

-- Other backends cache TVIEW metadata. Any write to the catalog
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

-- Writes buffered audit entries. Only the extension owner may call it:
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

// Per-TVIEW physical health report. Pure SQL over the catalogs and the
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

        -- Integer fk_* columns that no index leads with: parents are looked up by them.
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
