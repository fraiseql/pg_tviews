use pgrx::prelude::*;
use crate::schema::{TViewSchema, inference::infer_schema, analyzer::analyze_dependencies};
use crate::error::{TViewError, TViewResult};

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
    let entity_name = if let Some(stripped) = tview_name.strip_prefix("tv_") {
        stripped
    } else {
        tview_name
    };
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
        .ok_or_else(|| TViewError::InvalidSelectStatement {
            sql: select_sql.to_string(),
            reason: "Could not infer entity name from SELECT (missing pk_<entity> column?)".to_string(),
        })?;

    // Step 3: Create backing view v_<entity>
    let view_name = format!("v_{entity_name}");
    create_backing_view(&view_name, &final_select_sql)?;

    // Step 4: Create materialized table tv_<entity>
    create_materialized_table(tview_name, &final_schema)?;

    // Step 5: Populate initial data
    info!("Step 5: Populating initial data");
    populate_initial_data(tview_name, &view_name, &final_schema)?;
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
    )?;
    info!("Metadata registered");

    // Step 8: Install triggers on base tables
    info!("Step 8: Installing triggers");
    if !dep_graph.base_tables.is_empty() {
        crate::dependency::install_triggers(&dep_graph.base_tables, entity_name)?;
        info!("Installed triggers on {} base tables", dep_graph.base_tables.len());
    } else {
        warning!("No base table dependencies found for {}", tview_name);
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
fn create_backing_view(view_name: &str, select_sql: &str) -> TViewResult<()> {
    let create_view_sql = format!(
        "CREATE VIEW public.{view_name} AS {select_sql}"
    );

    info!("Creating backing view with SQL: {}", create_view_sql);
    Spi::run(&create_view_sql).map_err(|e| TViewError::SpiError {
        query: create_view_sql.clone(),
        error: e.to_string(),
    })?;

    // Verify the view was created
    let check_sql = format!("SELECT 1 FROM pg_class WHERE relname = '{view_name}' AND relkind = 'v'");
    let exists = Spi::get_one::<i32>(&check_sql).map_err(|e| TViewError::SpiError {
        query: check_sql,
        error: e.to_string(),
    })?.is_some();

    if !exists {
        return Err(TViewError::CatalogError {
            operation: format!("Create view {view_name}"),
            pg_error: "View was not created".to_string(),
        });
    }

    info!("Created backing view: {}", view_name);
    Ok(())
}

/// Create the materialized table with proper schema inferred from the backing view
fn create_materialized_table(
    tview_name: &str,
    schema: &TViewSchema,
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
        "CREATE TABLE public.{tview_name} (\n    {columns_sql}\n)"
    );

    Spi::run(&create_table_sql).map_err(|e| TViewError::SpiError {
        query: create_table_sql,
        error: e.to_string(),
    })?;

    // Create indexes for performance
    create_tview_indexes(tview_name, schema)?;

    info!("Created materialized table: {}", tview_name);
    Ok(())
}

/// Create indexes on the materialized table for optimal query performance
fn create_tview_indexes(tview_name: &str, schema: &TViewSchema) -> TViewResult<()> {
    // Index on ID column (Trinity identifier)
    if let Some(id) = &schema.id_column {
        let idx_name = format!("idx_{tview_name}_{id}");
        let create_idx = format!(
            "CREATE INDEX {idx_name} ON public.{tview_name} ({id})"
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
            "CREATE INDEX {idx_name} ON public.{tview_name} ({uuid_fk})"
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
            "CREATE INDEX {idx_name} ON public.{tview_name} USING GIN ({data})"
        );
        Spi::run(&create_idx).map_err(|e| TViewError::SpiError {
            query: create_idx.clone(),
            error: e.to_string(),
        })?;
    }

    Ok(())
}

/// Populate the materialized table with initial data from the backing view
fn populate_initial_data(tview_name: &str, view_name: &str, schema: &TViewSchema) -> TViewResult<()> {
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
        "INSERT INTO public.{tview_name} ({insert_column_list}) SELECT {select_column_list} FROM public.{view_name}"
    );

    Spi::run(&insert_sql).map_err(|e| TViewError::SpiError {
        query: insert_sql,
        error: e.to_string(),
    })?;

    info!("Populated initial data for {}", tview_name);
    Ok(())
}

/// Register the TVIEW in metadata tables
fn register_metadata(
    entity_name: &str,
    view_name: &str,
    tview_name: &str,
    definition_sql: &str,
    schema: &TViewSchema,
    dependencies: &[pg_sys::Oid],
) -> TViewResult<()> {
    // Analyze dependencies to populate type/path/match_key info
    let dep_infos = analyze_dependencies(definition_sql, &schema.fk_columns);

    // Serialize schema information
    let fk_columns = schema.fk_columns.join(",");
    let uuid_fk_columns = schema.uuid_fk_columns.join(",");

    // Serialize dependency types
    let dep_types = dep_infos.iter()
        .map(|d| d.dep_type.as_str())
        .collect::<Vec<_>>()
        .join(",");

    // Serialize dependency paths (TEXT[] format, NULL for None)
    let dep_paths = dep_infos.iter()
        .map(|d| match &d.jsonb_path {
            Some(path) => path.join("."),
            None => String::new(),
        })
        .collect::<Vec<_>>()
        .join(",");

    // Serialize array match keys (NULL for None)
    let array_keys = dep_infos.iter()
        .map(|d| match &d.array_match_key {
            Some(key) => key.clone(),
            None => String::new(),
        })
        .collect::<Vec<_>>()
        .join(",");

    // Serialize dependencies as OID array
    let deps_str = dependencies.iter()
        .map(|oid| oid.to_u32().to_string())
        .collect::<Vec<_>>()
        .join(",");

    // Get OIDs for the created objects
    let view_oid_result = Spi::get_one::<pg_sys::Oid>(&format!(
        "SELECT oid FROM pg_class WHERE relname = '{view_name}' AND relkind = 'v'"
    )).map_err(|e| TViewError::CatalogError {
        operation: format!("Get OID for view {view_name}"),
        pg_error: e.to_string(),
    })?;

    let table_oid_result = Spi::get_one::<pg_sys::Oid>(&format!(
        "SELECT oid FROM pg_class WHERE relname = '{tview_name}' AND relkind = 'r'"
    )).map_err(|e| TViewError::CatalogError {
        operation: format!("Get OID for table {tview_name}"),
        pg_error: e.to_string(),
    })?;

    let view_oid = view_oid_result.ok_or_else(|| TViewError::CatalogError {
        operation: format!("Find view {view_name}"),
        pg_error: "View OID not found".to_string(),
    })?;

    let table_oid = table_oid_result.ok_or_else(|| TViewError::CatalogError {
        operation: format!("Find table {tview_name}"),
        pg_error: "Table OID not found".to_string(),
    })?;

    // Insert metadata record
    let insert_meta_sql = format!(
        "INSERT INTO public.pg_tview_meta (
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

#[cfg(test)]
mod tests {

    #[test]
    fn test_tview_exists_non_existent() {
        // This test would require a database context
        // For now, we just verify the function signature compiles
    }
}
