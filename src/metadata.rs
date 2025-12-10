use pgrx::prelude::*;
use crate::error::{TViewError, TViewResult};

// Generate SQL to create metadata tables during extension installation
extension_sql!(
    r"
    CREATE TABLE IF NOT EXISTS public.pg_tview_meta (
        entity TEXT NOT NULL PRIMARY KEY,
        view_oid OID NOT NULL,
        table_oid OID NOT NULL,
        definition TEXT NOT NULL,
        dependencies OID[] NOT NULL DEFAULT '{}',
        fk_columns TEXT[] NOT NULL DEFAULT '{}',
        uuid_fk_columns TEXT[] NOT NULL DEFAULT '{}',
        dependency_types TEXT[] NOT NULL DEFAULT '{}',
        dependency_paths TEXT[][] NOT NULL DEFAULT '{}',
        array_match_keys TEXT[] NOT NULL DEFAULT '{}',
        created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
    );

    CREATE TABLE IF NOT EXISTS public.pg_tview_helpers (
        helper_name TEXT NOT NULL PRIMARY KEY,
        is_helper BOOLEAN NOT NULL DEFAULT TRUE,
        used_by TEXT[] NOT NULL DEFAULT '{}',
        depends_on TEXT[] NOT NULL DEFAULT '{}',
        created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
    );

    COMMENT ON TABLE public.pg_tview_meta IS 'Metadata for TVIEW materialized tables';
    COMMENT ON TABLE public.pg_tview_helpers IS 'Tracks helper views used by TVIEWs';
    ",
    name = "create_metadata_tables",
);

/// Create the metadata tables required for pg_tviews extension
pub fn create_metadata_tables() -> TViewResult<()> {
    Spi::run(
        r"
        CREATE TABLE IF NOT EXISTS public.pg_tview_meta (
            entity TEXT NOT NULL PRIMARY KEY,
            view_oid OID NOT NULL,
            table_oid OID NOT NULL,
            definition TEXT NOT NULL,
            dependencies OID[] NOT NULL DEFAULT '{}',
            fk_columns TEXT[] NOT NULL DEFAULT '{}',
            uuid_fk_columns TEXT[] NOT NULL DEFAULT '{}',
            dependency_types TEXT[] NOT NULL DEFAULT '{}',
            dependency_paths TEXT[][] NOT NULL DEFAULT '{}',
            array_match_keys TEXT[] NOT NULL DEFAULT '{}',
            created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
        );

        CREATE TABLE IF NOT EXISTS public.pg_tview_helpers (
            helper_name TEXT NOT NULL PRIMARY KEY,
            is_helper BOOLEAN NOT NULL DEFAULT TRUE,
            used_by TEXT[] NOT NULL DEFAULT '{}',
            depends_on TEXT[] NOT NULL DEFAULT '{}',
            created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
        );

        COMMENT ON TABLE public.pg_tview_meta IS
            'Metadata for TVIEW materialized tables';
        COMMENT ON TABLE public.pg_tview_helpers IS
            'Tracks helper views used by TVIEWs';
        ",
    ).map_err(|e| TViewError::CatalogError {
        operation: "create_metadata_tables".to_string(),
        pg_error: e.to_string(),
    })?;

    Ok(())
}

/// Drop all metadata tables (for testing/cleanup)
pub fn drop_metadata_tables() -> TViewResult<()> {
    Spi::run(
        r"
        DROP TABLE IF EXISTS public.pg_tview_helpers;
        DROP TABLE IF EXISTS public.pg_tview_meta;
        ",
    ).map_err(|e| TViewError::CatalogError {
        operation: "drop_metadata_tables".to_string(),
        pg_error: e.to_string(),
    })?;

    Ok(())
}

/// Check if metadata tables exist
pub fn metadata_tables_exist() -> TViewResult<bool> {
    let meta_exists = Spi::get_one::<bool>(
        "SELECT COUNT(*) = 1 FROM information_schema.tables
         WHERE table_schema = 'public' AND table_name = 'pg_tview_meta'"
    ).map_err(|e| TViewError::SpiError {
        query: "check pg_tview_meta exists".to_string(),
        error: e.to_string(),
    })?;

    let helpers_exists = Spi::get_one::<bool>(
        "SELECT COUNT(*) = 1 FROM information_schema.tables
         WHERE table_schema = 'public' AND table_name = 'pg_tview_helpers'"
    ).map_err(|e| TViewError::SpiError {
        query: "check pg_tview_helpers exists".to_string(),
        error: e.to_string(),
    })?;

    Ok(meta_exists.unwrap_or(false) && helpers_exists.unwrap_or(false))
}

#[cfg(any(test, feature = "pg_test"))]
#[pg_schema]
mod tests {
    use pgrx::prelude::*;
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
             WHERE table_name = 'pg_tview_meta'"
        );
        assert_eq!(result, Ok(Some(true)), "pg_tview_meta table should exist");

        // Verify pg_tview_helpers exists
        let result = Spi::get_one::<bool>(
            "SELECT COUNT(*) = 1 FROM information_schema.tables
             WHERE table_name = 'pg_tview_helpers'"
        );
        assert_eq!(result, Ok(Some(true)), "pg_tview_helpers table should exist");

        // Verify tables are in public schema
        let result = Spi::get_one::<i64>(
            "SELECT COUNT(*) FROM information_schema.columns
             WHERE table_schema = 'public' AND table_name = 'pg_tview_meta'"
        );
        assert!(result.unwrap_or(Some(0)).unwrap_or(0) > 0, "pg_tview_meta should have columns");
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
                WHERE table_name = 'pg_tview_meta' AND table_schema = 'public'
                ORDER BY ordinal_position
            ";

            for row in client.select(query, None, None)? {
                let name: String = row.get(1)?.unwrap_or_default();
                let data_type: String = row.get(2)?.unwrap_or_default();
                let nullable: String = row.get(3)?.unwrap_or_default();
                columns.push((name, data_type, nullable));
            }

            Ok::<_, pgrx::spi::SpiError>(columns)
        }).expect("Failed to query column info");

        // Verify expected columns exist
        let expected_columns = vec![
            ("entity", "text", "NO"),
            ("view_oid", "oid", "NO"),
            ("table_oid", "oid", "NO"),
            ("definition", "text", "NO"),
            ("dependencies", "ARRAY", "NO"),
            ("fk_columns", "ARRAY", "NO"),
            ("uuid_fk_columns", "ARRAY", "NO"),
            ("dependency_types", "ARRAY", "NO"),
            ("dependency_paths", "ARRAY", "NO"),
            ("array_match_keys", "ARRAY", "NO"),
            ("created_at", "timestamp with time zone", "NO"),
        ];

        for (expected_name, expected_type, expected_nullable) in expected_columns {
            let found = columns.iter().any(|(name, data_type, nullable)| {
                name == expected_name &&
                (data_type == expected_type || data_type.starts_with(expected_type)) &&
                nullable == expected_nullable
            });
            assert!(found, "Column {} with type {} nullable {} not found", expected_name, expected_type, expected_nullable);
        }
    }

    #[pg_test]
    fn test_metadata_tables_exist_function() {
        // Clean up first
        let _ = drop_metadata_tables();
        assert_eq!(metadata_tables_exist(), Ok(false), "Tables should not exist initially");

        // Create tables
        create_metadata_tables().expect("Failed to create metadata tables");
        assert_eq!(metadata_tables_exist(), Ok(true), "Tables should exist after creation");
    }
}