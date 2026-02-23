use pgrx::prelude::*;
use crate::schema::{TViewSchema, inference::infer_schema, analyzer::analyze_dependencies};
use crate::error::{TViewError, TViewResult};

/// Resolve the target schema for creating TVIEW objects.
///
/// Uses `current_schema()` to respect the active `search_path`, matching
/// standard PostgreSQL convention for unqualified DDL statements.
fn current_schema() -> TViewResult<String> {
    crate::utils::spi_get_string("SELECT current_schema()")
        .map_err(|e| TViewError::CatalogError {
            operation: "Get current schema".to_string(),
            pg_error: e.to_string(),
        })?
        .ok_or_else(|| TViewError::CatalogError {
            operation: "Get current schema".to_string(),
            pg_error: "current_schema() returned NULL (no schema in search_path?)".to_string(),
        })
}

/// Create a TVIEW with atomic rollback on error
///
/// This is the main entry point for CREATE TABLE tv_ AS SELECT .... `PostgreSQL`'s transaction
/// system automatically provides atomicity - if any step fails, all changes
/// are rolled back.
///
/// Steps:
/// 1. Check if TVIEW already exists
/// 2. Infer schema from SELECT statement
/// 3. Create backing view v_<entity>
/// 4. Create materialized table tv_<entity>
/// 5. Populate initial data
/// 6. Register metadata
/// 7. Find base table dependencies and install triggers
///
/// # Errors
/// Returns error if TVIEW already exists, SQL is invalid, or trigger installation fails
pub fn create_tview(
    tview_name: &str,
    select_sql: &str,
) -> TViewResult<()> {
    info!("create_tview called: tview_name={}, select_sql={}", tview_name, select_sql);

    // Step 1: Check if TVIEW already exists
    info!("Step 1: Checking if TVIEW already exists");
    let exists = tview_exists(tview_name)?;
    if exists {
        return Err(TViewError::TViewAlreadyExists {
            name: tview_name.to_string(),
        });
    }
    info!("TVIEW does not exist, proceeding");

    // Step 1.5: Extract entity name from tview_name
    // Support both "tv_entity" and just "entity" formats
    let entity_name = tview_name.strip_prefix("tv_").map_or(tview_name, |stripped| stripped);
    info!("Entity name: {}", entity_name);

    // Step 2: Infer schema from SELECT
    // If SELECT doesn't have TVIEW format (pk_<entity>, id, data), create a prepared view first
    info!("Step 2: Inferring schema from SELECT");
    let schema = infer_schema(select_sql)?;
    info!("Schema inferred successfully");

    // Check if we need to transform the SELECT to TVIEW format
    let (final_select_sql, final_schema) = if schema.entity_name.is_none() {
        // Raw SELECT - needs transformation to TVIEW format
        info!("Transforming raw SELECT to TVIEW format for entity '{}'", entity_name);
        transform_raw_select_to_tview(entity_name, select_sql)?
    } else {
        // Already in TVIEW format
        (select_sql.to_string(), schema)
    };

    let entity_name = final_schema.entity_name.as_ref()
        .ok_or_else(|| TViewError::RequiredColumnMissing {
            column_name: format!("pk_{}", tview_name.strip_prefix("tv_").unwrap_or(tview_name)),
            context: "pg_tviews requires a Trinity Pattern primary key column named \
                      \"pk_<entity>\" (e.g., pk_user, pk_post)".to_string(),
        })?;

    // Resolve the target schema once, respecting the active search_path.
    let schema_name = current_schema()?;
    info!("Creating TVIEW objects in schema: {}", schema_name);

    // Step 3: Create backing view v_<entity>
    let view_name = format!("v_{entity_name}");
    create_backing_view(&view_name, &final_select_sql, &schema_name)?;

    // Step 4: Create materialized table tv_<entity>
    create_materialized_table(tview_name, &final_schema, &schema_name)?;

    // Step 5: Populate initial data
    info!("Step 5: Populating initial data");
    populate_initial_data(tview_name, &view_name, &final_schema, &schema_name)?;
    info!("Initial data populated");

    // Step 6: Find base table dependencies
    info!("Step 6: Finding base table dependencies");
    let dep_graph = crate::dependency::find_base_tables(&view_name)?;
    info!("Found {} base table dependencies for {}", dep_graph.base_tables.len(), tview_name);

    // Step 7: Register metadata (with dependencies)
    info!("Step 7: Registering metadata");
    register_metadata(
        entity_name,
        &view_name,
        tview_name,
        &final_select_sql,
        &final_schema,
        &dep_graph.base_tables,
        &schema_name,
    )?;
    info!("Metadata registered");

    // Step 8: Install triggers on base tables
    info!("Step 8: Installing triggers");
    if dep_graph.base_tables.is_empty() {
        warning!("No base table dependencies found for {}", tview_name);
    } else {
        crate::dependency::install_triggers(&dep_graph.base_tables, entity_name)?;
        info!("Installed triggers on {} base tables", dep_graph.base_tables.len());
    }

    // Invalidate caches since new TVIEW was created
    info!("Step 9: Invalidating caches");
    crate::queue::cache::invalidate_all_caches();

    // Log the creation for audit trail
    if let Err(e) = crate::audit::log_create(entity_name, select_sql) {
        warning!("Failed to log TVIEW creation: {}", e);
    }

    info!("TVIEW {} created successfully", tview_name);

    Ok(())
}

/// Check if a TVIEW already exists
fn tview_exists(tview_name: &str) -> TViewResult<bool> {
    let entity_name = tview_name.trim_start_matches("tv_");

    Spi::get_one::<bool>(&format!(
        "SELECT COUNT(*) > 0 FROM pg_tview_meta WHERE entity = '{}'",
        entity_name.replace('\'', "''")
    ))
    .map_err(|e| TViewError::CatalogError {
        operation: format!("Check TVIEW exists: {tview_name}"),
        pg_error: format!("{e:?}"),
    })
    .map(|opt| opt.unwrap_or(false))
}

/// Create the backing view that contains the user's SELECT definition
fn create_backing_view(view_name: &str, select_sql: &str, schema_name: &str) -> TViewResult<()> {
    let create_view_sql = format!(
        "CREATE VIEW {schema_name}.{view_name} AS {select_sql}"
    );

    info!("Creating backing view with SQL: {}", create_view_sql);
    Spi::run(&create_view_sql).map_err(|e| TViewError::SpiError {
        query: create_view_sql.clone(),
        error: e.to_string(),
    })?;

    // Verify the view was created (schema-qualified to avoid false positives across schemas)
    let check_sql = format!(
        "SELECT 1 FROM pg_class c \
         JOIN pg_namespace n ON c.relnamespace = n.oid \
         WHERE c.relname = '{view_name}' AND n.nspname = '{schema_name}' AND c.relkind = 'v'"
    );
    let exists = Spi::get_one::<i32>(&check_sql).map_err(|e| TViewError::SpiError {
        query: check_sql,
        error: e.to_string(),
    })?.is_some();

    if !exists {
        return Err(TViewError::CatalogError {
            operation: format!("Create view {schema_name}.{view_name}"),
            pg_error: "View was not created".to_string(),
        });
    }

    info!("Created backing view: {}.{}", schema_name, view_name);
    Ok(())
}

/// Create the materialized table with proper schema inferred from the backing view
fn create_materialized_table(
    tview_name: &str,
    schema: &TViewSchema,
    schema_name: &str,
) -> TViewResult<()> {
    // Build column definitions based on inferred schema
    let mut columns = Vec::new();

    // Primary key column (if exists)
    if let Some(pk) = &schema.pk_column {
        columns.push(format!("{pk} BIGINT PRIMARY KEY"));
    }

    // ID column (Trinity identifier)
    if let Some(id) = &schema.id_column {
        columns.push(format!("{id} UUID NOT NULL"));
    }

    // Identifier column (optional Trinity identifier)
    if let Some(identifier) = &schema.identifier_column {
        columns.push(format!("{identifier} TEXT"));
    }

    // Data column (JSONB read model)
    if let Some(data) = &schema.data_column {
        columns.push(format!("{data} JSONB"));
    }

    // Foreign key columns (for lineage tracking)
    for fk in &schema.fk_columns {
        columns.push(format!("{fk} BIGINT"));
    }

    // UUID foreign key columns (for filtering)
    for uuid_fk in &schema.uuid_fk_columns {
        columns.push(format!("{uuid_fk} UUID"));
    }

    // Additional columns with inferred types
    for (col_name, col_type) in &schema.additional_columns_with_types {
        columns.push(format!("{col_name} {col_type}"));
    }

    // Add timestamps for tracking
    columns.push("created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()".to_string());
    columns.push("updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()".to_string());

    let columns_sql = columns.join(",\n    ");

    let create_table_sql = format!(
        "CREATE TABLE {schema_name}.{tview_name} (\n    {columns_sql}\n)"
    );

    Spi::run(&create_table_sql).map_err(|e| TViewError::SpiError {
        query: create_table_sql,
        error: e.to_string(),
    })?;

    // Create indexes for performance
    create_tview_indexes(tview_name, schema, schema_name)?;

    info!("Created materialized table: {}.{}", schema_name, tview_name);
    Ok(())
}

/// Create indexes on the materialized table for optimal query performance
fn create_tview_indexes(tview_name: &str, schema: &TViewSchema, schema_name: &str) -> TViewResult<()> {
    // Index on ID column (Trinity identifier)
    if let Some(id) = &schema.id_column {
        let idx_name = format!("idx_{tview_name}_{id}");
        let create_idx = format!(
            "CREATE INDEX {idx_name} ON {schema_name}.{tview_name} ({id})"
        );
        Spi::run(&create_idx).map_err(|e| TViewError::SpiError {
            query: create_idx.clone(),
            error: e.to_string(),
        })?;
    }

    // Index on UUID foreign key columns
    for uuid_fk in &schema.uuid_fk_columns {
        let idx_name = format!("idx_{tview_name}_{uuid_fk}");
        let create_idx = format!(
            "CREATE INDEX {idx_name} ON {schema_name}.{tview_name} ({uuid_fk})"
        );
        Spi::run(&create_idx).map_err(|e| TViewError::SpiError {
            query: create_idx.clone(),
            error: e.to_string(),
        })?;
    }

    // Index on data column if it exists (for JSONB queries)
    if let Some(data) = &schema.data_column {
        let idx_name = format!("idx_{tview_name}_{data}_gin");
        let create_idx = format!(
            "CREATE INDEX {idx_name} ON {schema_name}.{tview_name} USING GIN ({data})"
        );
        Spi::run(&create_idx).map_err(|e| TViewError::SpiError {
            query: create_idx.clone(),
            error: e.to_string(),
        })?;
    }

    Ok(())
}

/// Populate the materialized table with initial data from the backing view
fn populate_initial_data(tview_name: &str, view_name: &str, schema: &TViewSchema, schema_name: &str) -> TViewResult<()> {
    // Build column list from schema (excluding created_at/updated_at which have defaults)
    let mut select_columns = Vec::new();
    let mut insert_columns = Vec::new();

    if let Some(pk) = &schema.pk_column {
        insert_columns.push(pk.clone());
        select_columns.push(pk.clone());
    }
    if let Some(id) = &schema.id_column {
        insert_columns.push(id.clone());
        // Cast id to UUID to ensure compatibility
        select_columns.push(format!("{id}::uuid"));
    }
    if let Some(identifier) = &schema.identifier_column {
        insert_columns.push(identifier.clone());
        select_columns.push(identifier.clone());
    }
    if let Some(data) = &schema.data_column {
        insert_columns.push(data.clone());
        select_columns.push(data.clone());
    }
    for fk in &schema.fk_columns {
        insert_columns.push(fk.clone());
        select_columns.push(fk.clone());
    }
    for uuid_fk in &schema.uuid_fk_columns {
        insert_columns.push(uuid_fk.clone());
        select_columns.push(uuid_fk.clone());
    }
    for col in &schema.additional_columns {
        insert_columns.push(col.clone());
        select_columns.push(col.clone());
    }

    let insert_column_list = insert_columns.join(", ");
    let select_column_list = select_columns.join(", ");

    let insert_sql = format!(
        "INSERT INTO {schema_name}.{tview_name} ({insert_column_list}) \
         SELECT {select_column_list} FROM {schema_name}.{view_name}"
    );

    Spi::run(&insert_sql).map_err(|e| TViewError::SpiError {
        query: insert_sql,
        error: e.to_string(),
    })?;

    info!("Populated initial data for {}", tview_name);
    Ok(())
}

/// Quote a string for use in a PostgreSQL array literal.
///
/// Empty strings and strings containing special characters must be double-quoted
/// to avoid producing invalid array literals like `'{,}'`.
fn pg_array_elem(s: &str) -> String {
    if s.is_empty() || s.contains([',', '"', '\\', '{', '}', ' ']) {
        format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        s.to_string()
    }
}

/// Register the TVIEW in metadata tables
fn register_metadata(
    entity_name: &str,
    view_name: &str,
    tview_name: &str,
    definition_sql: &str,
    schema: &TViewSchema,
    dependencies: &[pg_sys::Oid],
    schema_name: &str,
) -> TViewResult<()> {
    // Analyze dependencies to populate type/path/match_key info
    let dep_infos = analyze_dependencies(definition_sql, &schema.fk_columns);

    // Serialize schema information (quoted for safe PostgreSQL array literals)
    let fk_columns = schema.fk_columns.iter()
        .map(|s| pg_array_elem(s))
        .collect::<Vec<_>>()
        .join(",");
    let uuid_fk_columns = schema.uuid_fk_columns.iter()
        .map(|s| pg_array_elem(s))
        .collect::<Vec<_>>()
        .join(",");

    // Serialize dependency types
    let dep_types = dep_infos.iter()
        .map(|d| pg_array_elem(d.dep_type.as_str()))
        .collect::<Vec<_>>()
        .join(",");

    // Serialize dependency paths (TEXT[] format, empty string for None)
    let dep_paths = dep_infos.iter()
        .map(|d| pg_array_elem(&d.jsonb_path.as_ref().map_or_else(String::new, |path| path.join("."))))
        .collect::<Vec<_>>()
        .join(",");

    // Serialize array match keys (empty string for None)
    let array_keys = dep_infos.iter()
        .map(|d| pg_array_elem(&d.array_match_key.clone().unwrap_or_default()))
        .collect::<Vec<_>>()
        .join(",");

    // Serialize dependencies as OID array
    let deps_str = dependencies.iter()
        .map(|oid| oid.to_u32().to_string())
        .collect::<Vec<_>>()
        .join(",");

    // Get OIDs for the created objects (schema-qualified to avoid false matches
    // when identical names exist in multiple schemas)
    let view_oid_result = Spi::get_one::<pg_sys::Oid>(&format!(
        "SELECT c.oid FROM pg_class c \
         JOIN pg_namespace n ON c.relnamespace = n.oid \
         WHERE c.relname = '{view_name}' AND n.nspname = '{schema_name}' AND c.relkind = 'v'"
    )).map_err(|e| TViewError::CatalogError {
        operation: format!("Get OID for view {schema_name}.{view_name}"),
        pg_error: e.to_string(),
    })?;

    let table_oid_result = Spi::get_one::<pg_sys::Oid>(&format!(
        "SELECT c.oid FROM pg_class c \
         JOIN pg_namespace n ON c.relnamespace = n.oid \
         WHERE c.relname = '{tview_name}' AND n.nspname = '{schema_name}' AND c.relkind = 'r'"
    )).map_err(|e| TViewError::CatalogError {
        operation: format!("Get OID for table {schema_name}.{tview_name}"),
        pg_error: e.to_string(),
    })?;

    let view_oid = view_oid_result.ok_or_else(|| TViewError::CatalogError {
        operation: format!("Find view {schema_name}.{view_name}"),
        pg_error: "View OID not found".to_string(),
    })?;

    let table_oid = table_oid_result.ok_or_else(|| TViewError::CatalogError {
        operation: format!("Find table {schema_name}.{tview_name}"),
        pg_error: "Table OID not found".to_string(),
    })?;

    // Insert metadata record
    let insert_meta_sql = format!(
        "INSERT INTO pg_tview_meta (
            entity,
            view_oid,
            table_oid,
            definition,
            dependencies,
            fk_columns,
            uuid_fk_columns,
            dependency_types,
            dependency_paths,
            array_match_keys
        ) VALUES ('{}', {}, {}, '{}', '{{{}}}', '{{{}}}', '{{{}}}', '{{{}}}', '{{{}}}', '{{{}}}')
        ON CONFLICT (entity) DO NOTHING",
        entity_name.replace('\'', "''"),
        view_oid.to_u32(),
        table_oid.to_u32(),
        definition_sql.replace('\'', "''"),
        deps_str,
        fk_columns,
        uuid_fk_columns,
        dep_types,
        dep_paths,
        array_keys
    );

    Spi::run(&insert_meta_sql).map_err(|e| TViewError::SpiError {
        query: insert_meta_sql,
        error: e.to_string(),
    })?;

    info!("Registered TVIEW metadata for {}", entity_name);
    Ok(())
}

/// Transform a raw SELECT statement into TVIEW format
///
/// Takes a simple SELECT like "SELECT id, name, price FROM `tb_product`"
/// and transforms it into a proper TVIEW format with:
/// - pk_<entity> column (generated from the source table's primary key or id column)
/// - id column (UUID, generated from the source table's primary key)
/// - data column (JSONB with all fields)
///
/// This creates a "prepared view" that wraps the raw SELECT with TVIEW conventions.
fn transform_raw_select_to_tview(
    entity_name: &str,
    select_sql: &str,
) -> TViewResult<(String, TViewSchema)> {
    // Create a temporary view to analyze the raw SELECT
    let temp_view_name = format!("_temp_raw_{entity_name}");

    // First, create temp view to analyze columns
    let create_temp = format!(
        "CREATE TEMP VIEW {temp_view_name} AS {select_sql}"
    );

    Spi::run(&create_temp).map_err(|e| TViewError::SpiError {
        query: create_temp.clone(),
        error: e.to_string(),
    })?;

    // Get columns from temp view
    // Cast to text to avoid sql_identifier domain type issues
    let get_columns_sql = format!(
        "SELECT column_name::text, data_type::text
         FROM information_schema.columns
         WHERE table_name = '{temp_view_name}'
         ORDER BY ordinal_position"
    );

    let columns: Vec<(String, String)> = Spi::connect(|client| {
        let rows = client.select(&get_columns_sql, None, &[])?;
        let mut result = Vec::new();
        for row in rows {
            let col_name: String = row[1].value()?
                .ok_or_else(|| spi::Error::from(crate::TViewError::SpiError {
                    query: get_columns_sql.clone(),
                    error: "column name is NULL".to_string(),
                }))?;
            let data_type: String = row[2].value()?
                .ok_or_else(|| spi::Error::from(crate::TViewError::SpiError {
                    query: get_columns_sql.clone(),
                    error: "data type is NULL".to_string(),
                }))?;
            result.push((col_name, data_type));
        }
        Ok(result)
    }).map_err(|e: spi::Error| TViewError::CatalogError {
        operation: "Get columns from temp view".to_string(),
        pg_error: format!("{e:?}"),
    })?;

    // Drop temp view
    Spi::run(&format!("DROP VIEW {temp_view_name}")).ok();

    // Find primary key column (look for 'id' or first integer/bigint column)
    let pk_source_col = columns.iter()
        .find(|(name, _)| name == "id")
        .or_else(|| columns.iter().find(|(_, typ)| {
            typ.contains("int") || typ.contains("serial")
        }))
        .map(|(name, _)| name.clone())
        .ok_or_else(|| TViewError::InvalidSelectStatement {
            sql: select_sql.to_string(),
            reason: "No suitable primary key column found (need 'id' or an integer column)".to_string(),
        })?;

    // Build explicit column lists for clarity and control

    // 1. Build the source column list (from the subquery)
    let _source_columns: Vec<String> = columns.iter()
        .map(|(name, _)| format!("source.{name}"))
        .collect();

    // 2. Build JSONB data column pairs explicitly
    let data_columns: Vec<String> = columns.iter()
        .map(|(name, _)| {
            format!("'{name}', source.{name}")
        })
        .collect();

    // 3. Generate transformed SELECT with explicit column references
    // This makes it clear exactly what's being selected and how it's transformed
    let transformed_select = format!(
        "SELECT
            source.{} AS pk_{},
            gen_random_uuid() AS id,
            jsonb_build_object({}) AS data
        FROM ({}) AS source",
        pk_source_col,
        entity_name,
        data_columns.join(", "),
        select_sql
    );

    info!("Transformed SELECT to TVIEW format with pk column from '{}'", pk_source_col);

    // Infer schema from transformed SELECT
    let schema = infer_schema(&transformed_select)?;

    Ok((transformed_select, schema))
}

#[cfg(any(test, feature = "pg_test"))]
#[pgrx::pg_schema]
mod tests {
    #[cfg(feature = "pg_test")]
    use pgrx::prelude::*;
    #[cfg(feature = "pg_test")]
    use pgrx_tests::pg_test;

    #[test]
    fn test_tview_exists_non_existent() {
        // Compile-time check only — live DB tests use #[pg_test] below
    }

    /// TVIEW objects are created in the schema that is first in search_path,
    /// not hardcoded to public.
    #[cfg(feature = "pg_test")]
    #[pg_test]
    fn test_create_tview_respects_search_path() {
        Spi::run("CREATE SCHEMA tview_test_ns").unwrap();
        Spi::run("SET search_path TO tview_test_ns, public").unwrap();
        Spi::run("CREATE TABLE tb_item (pk_item BIGSERIAL PRIMARY KEY, name TEXT)").unwrap();
        Spi::run("INSERT INTO tb_item VALUES (1, 'Widget')").unwrap();

        Spi::run("SELECT pg_tviews_create('item', $$
            SELECT pk_item, jsonb_build_object('name', name) AS data
            FROM tb_item
        $$)").unwrap();

        // tv_item must be in the target schema
        let in_target = Spi::get_one::<bool>(
            "SELECT COUNT(*) > 0 FROM pg_class c \
             JOIN pg_namespace n ON c.relnamespace = n.oid \
             WHERE c.relname = 'tv_item' AND n.nspname = 'tview_test_ns'"
        ).unwrap().unwrap_or(false);
        assert!(in_target, "tv_item should be in tview_test_ns, not public");

        // tv_item must NOT leak into public
        let in_public = Spi::get_one::<bool>(
            "SELECT COUNT(*) > 0 FROM pg_class c \
             JOIN pg_namespace n ON c.relnamespace = n.oid \
             WHERE c.relname = 'tv_item' AND n.nspname = 'public'"
        ).unwrap().unwrap_or(false);
        assert!(!in_public, "tv_item must not be created in public schema");

        // The backing view v_item must be in the same schema
        let view_in_target = Spi::get_one::<bool>(
            "SELECT COUNT(*) > 0 FROM pg_class c \
             JOIN pg_namespace n ON c.relnamespace = n.oid \
             WHERE c.relname = 'v_item' AND n.nspname = 'tview_test_ns'"
        ).unwrap().unwrap_or(false);
        assert!(view_in_target, "v_item should be in tview_test_ns");
    }

    /// With the default search_path, objects still land in public (regression guard).
    #[cfg(feature = "pg_test")]
    #[pg_test]
    fn test_create_tview_defaults_to_public() {
        Spi::run("SET search_path TO public").unwrap();
        Spi::run("CREATE TABLE tb_gadget (pk_gadget BIGSERIAL PRIMARY KEY, label TEXT)").unwrap();
        Spi::run("INSERT INTO tb_gadget VALUES (1, 'Gizmo')").unwrap();

        Spi::run("SELECT pg_tviews_create('gadget', $$
            SELECT pk_gadget, jsonb_build_object('label', label) AS data
            FROM tb_gadget
        $$)").unwrap();

        let in_public = Spi::get_one::<bool>(
            "SELECT COUNT(*) > 0 FROM pg_class c \
             JOIN pg_namespace n ON c.relnamespace = n.oid \
             WHERE c.relname = 'tv_gadget' AND n.nspname = 'public'"
        ).unwrap().unwrap_or(false);
        assert!(in_public, "tv_gadget should be in public with default search_path");
    }
}
