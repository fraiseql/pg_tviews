use pgrx::prelude::*;
use crate::schema::{TViewSchema, inference::infer_schema};
use crate::error::{TViewError, TViewResult};

/// Create a TVIEW with atomic rollback on error
///
/// This is the main entry point for CREATE TVIEW. PostgreSQL's transaction
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
pub fn create_tview(
    tview_name: &str,
    select_sql: &str,
) -> TViewResult<()> {
    // Step 1: Check if TVIEW already exists
    let exists = tview_exists(tview_name)?;
    if exists {
        return Err(TViewError::TViewAlreadyExists {
            name: tview_name.to_string(),
        });
    }

    // Step 2: Infer schema from SELECT
    let schema = infer_schema(select_sql)?;

    let entity_name = schema.entity_name.as_ref()
        .ok_or_else(|| TViewError::InvalidSelectStatement {
            sql: select_sql.to_string(),
            reason: "Could not infer entity name from SELECT (missing pk_<entity> column?)".to_string(),
        })?;

    // Step 3: Create backing view v_<entity>
    let view_name = format!("v_{}", entity_name);
    create_backing_view(&view_name, select_sql)?;

    // Step 4: Create materialized table tv_<entity>
    create_materialized_table(tview_name, &schema)?;

    // Step 5: Populate initial data
    populate_initial_data(tview_name, &view_name)?;

    // Step 6: Register metadata
    register_metadata(
        entity_name,
        &view_name,
        tview_name,
        select_sql,
        &schema,
    )?;

    // Step 7: Find base table dependencies
    let dep_graph = crate::dependency::find_base_tables(&view_name)?;

    info!("Found {} base table dependencies for {}", dep_graph.base_tables.len(), tview_name);

    // Step 8: Install triggers on base tables
    if !dep_graph.base_tables.is_empty() {
        crate::dependency::install_triggers(&dep_graph.base_tables, entity_name)?;
        info!("Installed triggers on {} base tables", dep_graph.base_tables.len());
    } else {
        warning!("No base table dependencies found for {}", tview_name);
    }

    info!("TVIEW {} created successfully", tview_name);

    Ok(())
}

/// Check if a TVIEW already exists
fn tview_exists(tview_name: &str) -> TViewResult<bool> {
    let entity_name = tview_name.trim_start_matches("tv_");

    Spi::get_one::<bool>(&format!(
        "SELECT COUNT(*) > 0 FROM pg_tview_meta WHERE entity = '{}'",
        entity_name.replace("'", "''")
    ))
    .map_err(|e| TViewError::CatalogError {
        operation: format!("Check TVIEW exists: {}", tview_name),
        pg_error: format!("{:?}", e),
    })
    .map(|opt| opt.unwrap_or(false))
}

/// Create the backing view that contains the user's SELECT definition
fn create_backing_view(view_name: &str, select_sql: &str) -> TViewResult<()> {
    let create_view_sql = format!(
        "CREATE VIEW public.{} AS {}",
        view_name, select_sql
    );

    Spi::run(&create_view_sql).map_err(|e| TViewError::SpiError {
        query: create_view_sql,
        error: e.to_string(),
    })?;

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
        columns.push(format!("{} BIGINT PRIMARY KEY", pk));
    }

    // ID column (Trinity identifier)
    if let Some(id) = &schema.id_column {
        columns.push(format!("{} UUID NOT NULL", id));
    }

    // Identifier column (optional Trinity identifier)
    if let Some(identifier) = &schema.identifier_column {
        columns.push(format!("{} TEXT", identifier));
    }

    // Data column (JSONB read model)
    if let Some(data) = &schema.data_column {
        columns.push(format!("{} JSONB", data));
    }

    // Foreign key columns (for lineage tracking)
    for fk in &schema.fk_columns {
        columns.push(format!("{} BIGINT", fk));
    }

    // UUID foreign key columns (for filtering)
    for uuid_fk in &schema.uuid_fk_columns {
        columns.push(format!("{} UUID", uuid_fk));
    }

    // Additional columns (infer as TEXT for now, could be enhanced)
    for col in &schema.additional_columns {
        columns.push(format!("{} TEXT", col));
    }

    // Add timestamps for tracking
    columns.push("created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()".to_string());
    columns.push("updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()".to_string());

    let columns_sql = columns.join(",\n    ");

    let create_table_sql = format!(
        "CREATE TABLE public.{} (\n    {}\n)",
        tview_name, columns_sql
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
        let idx_name = format!("idx_{}_{}", tview_name, id);
        let create_idx = format!(
            "CREATE INDEX {} ON public.{} ({})",
            idx_name, tview_name, id
        );
        Spi::run(&create_idx).map_err(|e| TViewError::SpiError {
            query: create_idx.clone(),
            error: e.to_string(),
        })?;
    }

    // Index on UUID foreign key columns
    for uuid_fk in &schema.uuid_fk_columns {
        let idx_name = format!("idx_{}_{}", tview_name, uuid_fk);
        let create_idx = format!(
            "CREATE INDEX {} ON public.{} ({})",
            idx_name, tview_name, uuid_fk
        );
        Spi::run(&create_idx).map_err(|e| TViewError::SpiError {
            query: create_idx.clone(),
            error: e.to_string(),
        })?;
    }

    // Index on data column if it exists (for JSONB queries)
    if let Some(data) = &schema.data_column {
        let idx_name = format!("idx_{}_{}_gin", tview_name, data);
        let create_idx = format!(
            "CREATE INDEX {} ON public.{} USING GIN ({})",
            idx_name, tview_name, data
        );
        Spi::run(&create_idx).map_err(|e| TViewError::SpiError {
            query: create_idx.clone(),
            error: e.to_string(),
        })?;
    }

    Ok(())
}

/// Populate the materialized table with initial data from the backing view
fn populate_initial_data(tview_name: &str, view_name: &str) -> TViewResult<()> {
    let insert_sql = format!(
        "INSERT INTO public.{} SELECT * FROM public.{}",
        tview_name, view_name
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
) -> TViewResult<()> {
    // Serialize schema information
    let fk_columns = schema.fk_columns.join(",");
    let uuid_fk_columns = schema.uuid_fk_columns.join(",");

    // Get OIDs for the created objects
    let view_oid_result = Spi::get_one::<pg_sys::Oid>(&format!(
        "SELECT oid FROM pg_class WHERE relname = '{}' AND relkind = 'v'",
        view_name
    )).map_err(|e| TViewError::CatalogError {
        operation: format!("Get OID for view {}", view_name),
        pg_error: e.to_string(),
    })?;

    let table_oid_result = Spi::get_one::<pg_sys::Oid>(&format!(
        "SELECT oid FROM pg_class WHERE relname = '{}' AND relkind = 'r'",
        tview_name
    )).map_err(|e| TViewError::CatalogError {
        operation: format!("Get OID for table {}", tview_name),
        pg_error: e.to_string(),
    })?;

    let view_oid = view_oid_result.ok_or_else(|| TViewError::CatalogError {
        operation: format!("Find view {}", view_name),
        pg_error: "View OID not found".to_string(),
    })?;

    let table_oid = table_oid_result.ok_or_else(|| TViewError::CatalogError {
        operation: format!("Find table {}", tview_name),
        pg_error: "Table OID not found".to_string(),
    })?;

    // Insert metadata record
    let insert_meta_sql = format!(
        "INSERT INTO public.pg_tview_meta (
            entity,
            view_oid,
            table_oid,
            definition,
            fk_columns,
            uuid_fk_columns
        ) VALUES ('{}', {}, {}, '{}', '{{{}}}', '{{{}}}')
        ON CONFLICT (entity) DO NOTHING",
        entity_name.replace("'", "''"),
        view_oid.as_u32(),
        table_oid.as_u32(),
        definition_sql.replace("'", "''"),
        fk_columns,
        uuid_fk_columns
    );

    Spi::run(&insert_meta_sql).map_err(|e| TViewError::SpiError {
        query: insert_meta_sql,
        error: e.to_string(),
    })?;

    info!("Registered TVIEW metadata for {}", entity_name);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tview_exists_non_existent() {
        // This test would require a database context
        // For now, we just verify the function signature compiles
    }
}
