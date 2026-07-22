//! Metadata Management: TVIEW Catalog Tables and Schema
//!
//! This module manages the system catalog tables for TVIEW metadata:
//! - **`pg_tview_meta`**: Core TVIEW definitions and relationships
//! - **`pg_tview_pending_refreshes`**: 2PC transaction queue persistence
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
//! ### `pg_tview_pending_refreshes`
//! Persists refresh queues for 2PC transactions:
//! - Transaction GID linkage
//! - Serialized refresh operations
//! - Expiration handling
//!
//! ## Extension Lifecycle
//!
//! - **CREATE EXTENSION**: Creates catalog tables
//! - **ALTER EXTENSION**: Handles schema migrations
//! - **DROP EXTENSION**: Cleans up metadata

use crate::error::{TViewError, TViewResult};
use pgrx::prelude::*;

// Generate SQL to create metadata tables during extension installation.
// @extschema@ is substituted by PostgreSQL with the extension's install schema.
extension_sql!(
    r"
    CREATE TABLE IF NOT EXISTS @extschema@.pg_tview_meta (
        entity TEXT NOT NULL PRIMARY KEY,
        view_oid OID NOT NULL,
        table_oid OID NOT NULL,
        definition TEXT NOT NULL,
        cascade_paths TEXT[] NOT NULL DEFAULT '{}',
        fk_columns TEXT[] NOT NULL DEFAULT '{}',
        uuid_fk_columns TEXT[] NOT NULL DEFAULT '{}',
        dependency_types TEXT[] NOT NULL DEFAULT '{}',
        dependency_paths TEXT[]  NOT NULL DEFAULT '{}',
        array_match_keys TEXT[] NOT NULL DEFAULT '{}',
        distinct_on_keys TEXT[] NOT NULL DEFAULT '{}',
        distinct_on_output_keys TEXT[] NOT NULL DEFAULT '{}',
        is_union BOOLEAN NOT NULL DEFAULT FALSE,
        created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
    );

    CREATE TABLE IF NOT EXISTS @extschema@.pg_tview_helpers (
        helper_name TEXT NOT NULL PRIMARY KEY,
        is_helper BOOLEAN NOT NULL DEFAULT TRUE,
        used_by TEXT[] NOT NULL DEFAULT '{}',
        depends_on TEXT[] NOT NULL DEFAULT '{}',
        created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
    );

    COMMENT ON TABLE @extschema@.pg_tview_meta IS 'Metadata for TVIEW materialized tables';
    COMMENT ON TABLE @extschema@.pg_tview_helpers IS 'Tracks helper views used by TVIEWs';

    -- Indexes for catalog lookup performance (entity PK already has a unique index)
    CREATE INDEX IF NOT EXISTS idx_pg_tview_meta_table_oid
        ON @extschema@.pg_tview_meta(table_oid);
    ",
    name = "create_metadata_tables",
);

// Register event triggers for DDL interception
// The PL/pgSQL `pg_tviews_handle_ddl_event()` function is defined in this SQL block.
// It calls `pg_tviews_convert_table()`, which is a #[pg_extern] C function in event_trigger.rs.
// Note: we do NOT use a Rust #[pg_extern] for the event trigger handler itself because pgrx
// generates RETURNS VOID instead of the required RETURNS event_trigger pseudo-type.
extension_sql!(
    r"
-- Event trigger handler: PL/pgSQL wrapper that calls the Rust C function pg_tviews_convert_table().
-- Using PL/pgSQL (not a direct C function) because pgrx cannot generate RETURNS event_trigger
-- for #[pg_extern] functions — it always emits RETURNS VOID, which PostgreSQL rejects for
-- event trigger handlers.  The Rust logic lives in src/event_trigger.rs::handle_ddl_event_internal.
CREATE OR REPLACE FUNCTION pg_tviews_handle_ddl_event()
RETURNS event_trigger
LANGUAGE plpgsql
AS $$
DECLARE
    obj record;
BEGIN
    FOR obj IN SELECT * FROM pg_event_trigger_ddl_commands()
    LOOP
        -- Only process table-creation commands
        IF obj.command_tag IN ('CREATE TABLE', 'CREATE TABLE AS', 'SELECT INTO') THEN
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

                    PERFORM pg_tviews_convert_table(table_name_only);
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
DROP EVENT TRIGGER IF EXISTS pg_tviews_ddl_end;
CREATE EVENT TRIGGER pg_tviews_ddl_end
    ON ddl_command_end
    WHEN TAG IN ('CREATE TABLE', 'CREATE TABLE AS', 'SELECT INTO')
    EXECUTE FUNCTION pg_tviews_handle_ddl_event();

COMMENT ON EVENT TRIGGER pg_tviews_ddl_end IS
'Intercepts CREATE TABLE tv_* commands and converts them to TVIEWs';

-- Event trigger handler: deregister a TVIEW when its base table tb_<entity> is dropped
-- (issue #53).  The base-table -> tview link is not a hard PG dependency, so CASCADE
-- removes the backing view v_* and the base-table triggers but never the trigger-populated
-- tv_* table or its pg_tview_meta row.  This sql_drop handler cleans both.
--
-- PL/pgSQL (not #[pg_extern]) because pgrx cannot emit RETURNS event_trigger.  It fires for
-- EVERY dropped object system-wide, so it must be cheap and must never break an unrelated
-- DROP: references are schema-qualified via @extschema@ (search-path independent) and the
-- work is guarded by an existence check plus a defensive EXCEPTION handler.
CREATE OR REPLACE FUNCTION pg_tviews_handle_drop_event()
RETURNS event_trigger
LANGUAGE plpgsql
AS $$
DECLARE
    obj record;
    entity_name TEXT;
BEGIN
    FOR obj IN SELECT * FROM pg_event_trigger_dropped_objects()
    LOOP
        -- Only react to a dropped base table tb_*.  A directly-dropped tv_* table is
        -- handled by the ProcessUtility hook and never matches this tb_ filter, so there
        -- is no double-handling (and the nested tv_*/v_* drops this handler issues below
        -- likewise never match tb_, so there is no re-entrant loop).
        IF obj.object_type = 'table' AND left(obj.object_name, 3) = 'tb_' THEN
            entity_name := substring(obj.object_name FROM 4);
            IF EXISTS (SELECT 1 FROM @extschema@.pg_tview_meta WHERE entity = entity_name) THEN
                BEGIN
                    PERFORM @extschema@.pg_tviews_drop(entity_name, true, true);
                    RAISE NOTICE 'pg_tviews: base table % dropped; deregistered TVIEW tv_%',
                        obj.object_name, entity_name;
                EXCEPTION WHEN OTHERS THEN
                    -- Never abort the user's DROP; at minimum clear the stale metadata row.
                    DELETE FROM @extschema@.pg_tview_meta WHERE entity = entity_name;
                    RAISE WARNING 'pg_tviews: cleanup after DROP TABLE % failed (%); removed stale metadata for tv_%',
                        obj.object_name, SQLERRM, entity_name;
                END;
            END IF;
        END IF;
    END LOOP;
END;
$$;

DROP EVENT TRIGGER IF EXISTS pg_tviews_sql_drop;
CREATE EVENT TRIGGER pg_tviews_sql_drop
    ON sql_drop
    EXECUTE FUNCTION pg_tviews_handle_drop_event();

COMMENT ON EVENT TRIGGER pg_tviews_sql_drop IS
'Deregisters and drops a TVIEW when its base table tb_<entity> is dropped (issue #53)';
    ",
    name = "event_triggers",
    requires = ["create_metadata_tables"],
    finalize
);

// pg_tviews_convert_table is auto-registered via #[pg_extern] in src/event_trigger.rs

// Audit logging table for DDL operations
extension_sql!(
    r"
CREATE TABLE IF NOT EXISTS @extschema@.pg_tview_audit_log (
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

CREATE INDEX IF NOT EXISTS idx_audit_log_entity_time ON public.pg_tview_audit_log(entity, performed_at);

COMMENT ON TABLE public.pg_tview_audit_log IS 'Audit log for TVIEW operations';
    ",
    name = "audit_table",
);

// Monitoring views for production observability
extension_sql!(
    r"
-- Queue monitoring view
CREATE OR REPLACE VIEW @extschema@.pg_tviews_queue_realtime AS
SELECT
    current_setting('application_name') as session,
    pg_backend_pid() as backend_pid,
    txid_current() as transaction_id,
    0 as queue_size,
    ARRAY[]::TEXT[] as entities,
    NOW() as last_enqueued;

-- Cache statistics view
CREATE OR REPLACE VIEW @extschema@.pg_tviews_cache_stats AS
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
-- Note: public.pg_tview_meta is hardcoded (not @extschema@) because pgrx strips
-- @extschema@. from generated SQL, causing the reference to be unqualified and
-- fail during extension installation (search_path does not include public at
-- install time). The rest of the extension uses public.* explicitly.
CREATE OR REPLACE VIEW public.pg_tviews_performance_summary AS
SELECT
    entity,
    COUNT(*) as total_refreshes,
    0.0 as avg_refresh_ms,
    NOW() as last_refresh
FROM public.pg_tview_meta
GROUP BY entity;
    ",
    name = "monitoring_views",
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
            view_oid OID NOT NULL,
            table_oid OID NOT NULL,
            definition TEXT NOT NULL,
            cascade_paths TEXT[] NOT NULL DEFAULT '{}',
            fk_columns TEXT[] NOT NULL DEFAULT '{}',
            uuid_fk_columns TEXT[] NOT NULL DEFAULT '{}',
            dependency_types TEXT[] NOT NULL DEFAULT '{}',
            dependency_paths TEXT[]  NOT NULL DEFAULT '{}',
            array_match_keys TEXT[] NOT NULL DEFAULT '{}',
            distinct_on_keys TEXT[] NOT NULL DEFAULT '{}',
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
