mod derive;
mod indexes;
mod register;
mod relations;
mod select;

use derive::{Derivation, derive, uncascaded_tables};
pub(crate) use indexes::{index_ddl, index_name, managed_index_names, propagation_index_ddl};
use register::register_metadata;
use relations::{
    create_backing_view, create_materialized_table, key_table_on_identity, populate_initial_data,
    relation_exists, relation_oid, tview_exists,
};
pub(crate) use relations::{rebind_cascade_paths, view_source_columns};
#[cfg(any(test, feature = "pg_test"))]
mod tests;

pub use select::ViewColumns;

use super::uncascaded::{Declarations, Uncascaded};
use crate::error::{TViewError, TViewResult};
use crate::utils::log_debug;
use pgrx::prelude::*;

/// Resolve the target schema for creating TVIEW objects.
///
/// Uses `current_schema()` to respect the active `search_path`, matching
/// standard `PostgreSQL` convention for unqualified DDL statements.
pub(crate) fn current_schema() -> TViewResult<String> {
    crate::utils::spi_get_string("SELECT current_schema()::text")
        .map_err(|e| TViewError::CatalogError {
            operation: "Get current schema".to_string(),
            pg_error: e.to_string(),
        })?
        .ok_or_else(|| TViewError::CatalogError {
            operation: "Get current schema".to_string(),
            pg_error: "current_schema() returned NULL (no schema in search_path?)".to_string(),
        })
}

/// Storage of a TVIEW's table (issue #134): its persistence, its fillfactor, and
/// whether `data` has a GIN index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Storage {
    pub logged: bool,
    pub fillfactor: i32,
    pub data_gin_index: bool,
}

impl Storage {
    /// What a new TVIEW gets unless told otherwise: `pg_tviews.unlogged_by_default`,
    /// `pg_tviews.fillfactor` and `pg_tviews.data_gin_index`.
    #[must_use]
    pub fn from_settings() -> Self {
        Self {
            logged: !crate::config::unlogged_by_default(),
            fillfactor: crate::config::fillfactor(),
            data_gin_index: crate::config::data_gin_index(),
        }
    }
}

/// Create a TVIEW in `schema_name` with the given storage, as an aggregate TVIEW
/// when `group_keys` is given, and return the number of rows it was populated
/// with (issue #134).
///
/// Steps: normalize and analyze the definition; create the backing view
/// `v_<entity>` and the table `tv_<entity>`; populate it; register it; install
/// triggers on its base tables.
///
/// # Errors
/// Returns an error if the TVIEW exists, the definition is invalid, or creation fails.
///
/// `declarations` are the uncascaded policies to store (a rebuilt TVIEW keeps its
/// own); `None` reads `pg_tviews.uncascaded_policy`.
pub(crate) fn create_tview_in(
    tview_name: &str,
    select_sql: &str,
    schema_name: &str,
    group_keys: Option<&super::aggregate::GroupKeys>,
    storage: Storage,
    declarations: Option<Declarations>,
) -> TViewResult<u64> {
    create_tview_inner(
        tview_name,
        select_sql,
        schema_name,
        group_keys,
        storage,
        declarations.unwrap_or_else(Declarations::from_settings),
    )
}

/// The creation pipeline's normalization of a definition: `SELECT *` expanded to
/// its columns, and a raw SELECT rewritten to the `pk_<entity>, id, data` shape.
/// Its output normalizes to itself.
///
/// # Errors
/// Returns an error if the definition is not one SELECT PostgreSQL can analyze.
pub(crate) fn normalize_definition(
    entity_name: &str,
    select_sql: &str,
) -> TViewResult<(String, ViewColumns)> {
    select::normalize(entity_name, select_sql)
}

fn create_tview_inner(
    tview_name: &str,
    select_sql: &str,
    schema_name: &str,
    group_keys: Option<&super::aggregate::GroupKeys>,
    storage: Storage,
    declarations: Declarations,
) -> TViewResult<u64> {
    crate::revision::check();
    log_debug!(
        "create_tview start for '{}' in schema '{}'",
        tview_name,
        schema_name
    );
    // Calls that register, change or drop one entity run one after the other.
    super::lock_entity(tview_name.strip_prefix("tv_").unwrap_or(tview_name))?;

    // Step 1: Check if TVIEW already exists
    let exists = tview_exists(tview_name)?;
    if exists {
        return Err(TViewError::RelationExists {
            name: tview_name.to_string(),
        });
    }

    // Step 1.5: Extract entity name from tview_name
    // Support both "tv_entity" and just "entity" formats
    let entity_name = tview_name
        .strip_prefix("tv_")
        .map_or(tview_name, |stripped| stripped);

    // Step 2: Read the definition's columns with PostgreSQL's parser, expand
    // `SELECT *` and rewrite a raw SELECT to the `pk_*, id, data` shape.
    let (final_select_sql, final_schema) = normalize_definition(entity_name, select_sql)?;

    let entity_name =
        final_schema
            .entity
            .as_ref()
            .ok_or_else(|| TViewError::RequiredColumnMissing {
                column_name: format!(
                    "pk_{}",
                    tview_name.strip_prefix("tv_").unwrap_or(tview_name)
                ),
                context: "pg_tviews requires a Trinity Pattern primary key column named \
                      \"pk_<entity>\" (e.g., pk_user, pk_post)"
                    .to_string(),
            })?;

    // Validate entity_name inferred from the SELECT to prevent SQL injection
    // (tview_name is validated at the pg_extern boundary, but entity_name comes
    // from infer_schema and could contain metacharacters if the user crafts a
    // malicious column alias like pk_evil'injection).
    crate::validation::validate_sql_identifier(entity_name, "entity_name")?;

    if group_keys.is_some() {
        select::check_aggregate(&final_select_sql, entity_name)?;
    }

    // Derive the canonical materialized-table name: always tv_<entity>.
    // This normalises both calling conventions:
    //   pg_tviews_create('post', ...)   → tv_post
    //   pg_tviews_create('tv_post', ...) → tv_post
    let tv_table_name = format!("tv_{entity_name}");

    let schema_name = schema_name.to_string();

    // Step 3: Create the backing view
    let (view_schema, view_name) = super::backing_view_name(&schema_name, &tv_table_name);
    if relation_exists(&view_schema, &view_name)?
        && !super::drop::reclaim_leftover_view(&view_schema, &view_name)?
    {
        return Err(TViewError::DefinitionRefused {
            reason: format!(
                "the backing view of {schema_name}.{tv_table_name}, {view_schema}.{view_name}, \
                 is already taken by another relation"
            ),
        });
    }
    super::in_extension_schema(|| {
        create_backing_view(&view_name, &final_select_sql, &view_schema)
    })?;
    let view_oid = relation_oid(&view_schema, &view_name)?;

    // Step 4: Find base table dependencies, and how a write to each maps to keys,
    // from the view's query tree (ADR 0157), with the column that names the
    // TVIEW's rows (ADR 0169); and the cascade paths of its local tables (for an
    // aggregate TVIEW, one per declared group key, issue #58).
    // Pass schema_name so the view OID lookup searches in the correct schema even when
    // current_schema() resolves to a different schema due to the database search_path.
    let base_table_oids = crate::dependency::find_base_tables(&view_name, Some(&view_schema))?;
    let Derivation {
        lineage,
        key_mappings,
        cascade_paths,
        base_tables,
        undeclared_functions,
    } = derive(
        entity_name,
        group_keys,
        &base_table_oids,
        view_oid,
        &declarations,
    )?;

    // Step 5: Create materialized table tv_<entity>, keyed on the identity.
    create_materialized_table(
        &tv_table_name,
        &final_schema,
        &schema_name,
        &lineage.identity.name,
        storage,
        view_oid,
    )?;

    // Step 6: Populate initial data
    let rows = populate_initial_data(&tv_table_name, &schema_name, view_oid)?;

    // Step 6.6: Reject a TVIEW no write can ever refresh: its definition reads no
    // table (issue #49). Any table it reads, whatever it is called, maps its writes
    // to the TVIEW's keys or goes through the uncascaded policy below. The objects
    // created above roll back with the ERROR.
    if group_keys.is_none() && lineage.tables.is_empty() {
        return Err(TViewError::DefinitionRefused {
            reason: format!(
                "TVIEW '{tv_table_name}' can never be refreshed: its definition reads no table"
            ),
        });
    }

    // Step 6.7: Base tables whose writes no cascade reaches (issues #157, #158),
    // reported under the policy; `error` aborts here, and the objects created above
    // roll back with it.
    let qualified_tv =
        crate::utils::qualified_relname_from_oid(relation_oid(&schema_name, &tv_table_name)?)?;
    super::uncascaded::report_functions(&qualified_tv, &undeclared_functions, declarations.policy)?;
    super::uncascaded::report_time(&qualified_tv, &lineage.time_reads, &declarations)?;
    super::uncascaded::check_declared(&qualified_tv, &declarations, &lineage)?;
    let uncascaded = Uncascaded {
        tables: uncascaded_tables(&lineage),
        declarations,
        time_dependent: !lineage.time_reads.is_empty(),
    };
    super::uncascaded::report(&qualified_tv, &uncascaded)?;

    // Step 7: Register metadata (with cascade paths)
    register_metadata(
        entity_name,
        view_oid,
        &tv_table_name,
        &final_select_sql,
        &final_schema,
        &cascade_paths,
        &schema_name,
        group_keys,
        &uncascaded,
        &key_mappings,
        &lineage,
        false,
    )?;

    // Step 7.5: Whoever reads the TVIEW's table reads its backing view (#181).
    super::privileges::follow(Some(relation_oid(&schema_name, &tv_table_name)?), false)?;

    // Step 8: Install triggers on base tables, as their lineage needs them.
    if base_tables.is_empty() {
        warning!("No base table dependencies found for {}", tv_table_name);
    } else {
        crate::dependency::install_triggers(
            &crate::dependency::trigger_plan(&base_tables, &lineage)?,
            entity_name,
        )?;
    }

    // Invalidate caches since new TVIEW was created
    crate::cache::invalidate_all();

    // Buffer and flush audit entry immediately (we're in SPI context)
    crate::audit::log_create(entity_name, &final_select_sql);
    if let Err(e) = crate::audit::flush_audit_buffer() {
        warning!("Failed to flush audit after CREATE: {}", e);
    }

    Ok(rows)
}

/// Re-derive and replace the metadata of an existing TVIEW from `definition`,
/// with the same analysis as `create_tview_in`, and return the base tables its
/// backing view reads. Used when a column rename has changed the text that
/// defines the backing view, and by `pg_tviews_reregister()`; the relations
/// themselves are unchanged.
///
/// # Errors
/// Returns an error if the definition cannot be analyzed or the catalog update fails.
pub fn reregister_metadata(
    entity_name: &str,
    schema_name: &str,
    definition: &str,
) -> TViewResult<crate::dependency::TriggerPlan> {
    let meta = crate::catalog::TviewMeta::load_by_entity(entity_name)
        .map_err(|e| TViewError::CatalogError {
            operation: format!("Read the metadata of tv_{entity_name}"),
            pg_error: e.to_string(),
        })?
        .ok_or_else(|| TViewError::MetadataNotFound {
            entity: entity_name.to_string(),
        })?;
    let view_oid = meta.view_oid;
    select::check_one_select(definition)?;
    // The backing view has the definition's columns: read them there, not from
    // the text, which the current search_path may resolve differently.
    let schema = ViewColumns::classify(crate::utils::column_types(view_oid)?);
    let (view_schema, view_name) = super::relation_name(view_oid)?;
    let base_table_oids = crate::dependency::find_base_tables(&view_name, Some(&view_schema))?;
    let group_keys = stored_group_keys(entity_name)?;
    // The stored policies hold; an `error` table no cascade reaches aborts the
    // re-registration (and the ALTER that caused it).
    let declarations = Declarations::of(&meta);
    let Derivation {
        lineage,
        key_mappings,
        cascade_paths,
        base_tables,
        undeclared_functions,
    } = derive(
        entity_name,
        group_keys.as_ref(),
        &base_table_oids,
        view_oid,
        &declarations,
    )?;
    key_table_on_identity(
        schema_name,
        &format!("tv_{entity_name}"),
        &lineage.identity.name,
    )?;
    let qualified_tv = crate::utils::qualified_relname_from_oid(relation_oid(
        schema_name,
        &format!("tv_{entity_name}"),
    )?)?;
    super::uncascaded::report_functions(&qualified_tv, &undeclared_functions, declarations.policy)?;
    super::uncascaded::report_time(&qualified_tv, &lineage.time_reads, &declarations)?;
    super::uncascaded::check_declared(&qualified_tv, &declarations, &lineage)?;
    let uncascaded = Uncascaded {
        tables: uncascaded_tables(&lineage),
        declarations,
        time_dependent: !lineage.time_reads.is_empty(),
    };
    super::uncascaded::report(&qualified_tv, &uncascaded)?;
    register_metadata(
        entity_name,
        view_oid,
        &format!("tv_{entity_name}"),
        definition,
        &schema,
        &cascade_paths,
        schema_name,
        group_keys.as_ref(),
        &uncascaded,
        &key_mappings,
        &lineage,
        true,
    )?;
    crate::cache::invalidate_all();
    crate::dependency::trigger_plan(&base_tables, &lineage)
}

/// Re-derive `entity`'s metadata from its stored definition and make its
/// base-table triggers match what that definition reads (issue #137).
///
/// # Errors
/// Returns an error if the TVIEW is not registered, the caller does not own it,
/// or the definition cannot be analyzed.
pub fn reregister_tview(entity: &str) -> TViewResult<()> {
    super::lock_entity(entity)?;
    let meta = crate::catalog::TviewMeta::load_by_entity(entity)?.ok_or_else(|| {
        TViewError::MetadataNotFound {
            entity: entity.to_string(),
        }
    })?;
    crate::owner::require_owner(meta.tview_oid, &format!("tv_{entity}"))?;
    let (definition, schema_name) = Spi::connect(|client| {
        let args = [crate::utils::spi::text(entity)];
        client
            .select(
                &format!(
                    "SELECT m.definition, n.nspname::text \
                     FROM {} m \
                     JOIN pg_catalog.pg_class c ON c.oid = m.table_oid \
                     JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
                     WHERE m.entity = $1",
                    crate::utils::meta_table()
                ),
                None,
                &args,
            )?
            .first()
            .get_two::<String, String>()
    })
    .map_err(|e| TViewError::CatalogError {
        operation: format!("Read the definition of TVIEW {entity}"),
        pg_error: e.to_string(),
    })?;
    let (Some(definition), Some(schema_name)) = (definition, schema_name) else {
        return Err(TViewError::MetadataNotFound {
            entity: entity.to_string(),
        });
    };
    let plan = reregister_metadata(entity, &schema_name, &definition)?;
    crate::dependency::sync_entity_triggers(&plan, entity)?;
    let _owner = crate::owner::AsOwner::of_extension()?;
    Spi::run_with_args(
        &format!(
            "UPDATE {} SET needs_reregister = false WHERE entity = $1",
            crate::utils::meta_table()
        ),
        &[crate::utils::spi::text(entity)],
    )
    .map_err(|e| TViewError::CatalogError {
        operation: format!("Clear needs_reregister of TVIEW {entity}"),
        pg_error: e.to_string(),
    })
}

/// The `group_keys` of an aggregate TVIEW (issue #58), `None` for any other.
pub(crate) fn stored_group_keys(
    entity_name: &str,
) -> TViewResult<Option<super::aggregate::GroupKeys>> {
    let stored: Option<pgrx::JsonB> = Spi::get_one_with_args(
        &format!(
            "SELECT group_keys FROM {} WHERE entity = $1",
            crate::utils::meta_table()
        ),
        &[crate::utils::spi::text(entity_name)],
    )
    .map_err(|e| TViewError::CatalogError {
        operation: "Read group_keys".to_string(),
        pg_error: e.to_string(),
    })?;
    Ok(stored.and_then(|j| serde_json::from_value(j.0).ok()))
}
