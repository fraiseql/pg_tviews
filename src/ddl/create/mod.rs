mod derive;
mod indexes;
mod json_indexes;
mod register;
mod relations;
mod select;
mod storage;

use derive::derive_as;
pub(crate) use indexes::{ManagedIndex, index_name, key_index, managed_indexes};
pub use json_indexes::JsonIndex;
pub(crate) use json_indexes::{
    apply as apply_json_indexes, declared as declared_json_indexes, present as present_json_indexes,
};
use register::Registration;
use relations::{
    create_backing_view, create_materialized_table, key_table_on_identity, populate_initial_data,
    relation_exists, relation_oid, tview_exists,
};
pub use storage::{Compression, DataStorage, Storage, TOAST_TUPLE_TARGET};

#[cfg(test)]
mod tests;

thread_local! {
    /// Set by [`rebuilt_for`]: the owner the TVIEW being created is rebuilt for.
    static REBUILT_FOR: std::cell::Cell<Option<crate::owner::Owner>> =
        const { std::cell::Cell::new(None) };
}

/// Run `create` to rebuild a TVIEW owned by `owner`, which the caller may change
/// but need not be: its definition is analyzed as `owner`, and its table is left
/// empty, for the rebuild to fill as the owner once the owner is restored. The
/// owner's view functions never run with the caller's privileges.
pub(crate) fn rebuilt_for<T>(owner: crate::owner::Owner, create: impl FnOnce() -> T) -> T {
    REBUILT_FOR.with(|w| w.set(Some(owner)));
    let result = create();
    REBUILT_FOR.with(|w| w.set(None));
    result
}

pub use select::ViewColumns;

use super::uncascaded::Declarations;
use crate::error::{TViewError, TViewResult};
use crate::utils::log_debug;
use pgrx::prelude::*;

/// Resolve the target schema for creating TVIEW objects.
///
/// Uses `current_schema()` under the caller's `search_path`, matching standard
/// `PostgreSQL` convention for unqualified DDL statements.
pub(crate) fn current_schema() -> TViewResult<String> {
    let _caller = crate::owner::CallerPath::enter();
    crate::utils::spi::get_string("SELECT current_schema()::text")
        .map_err(|e| TViewError::CatalogError {
            operation: "Get current schema".to_string(),
            pg_error: e.to_string(),
        })?
        .ok_or_else(|| TViewError::CatalogError {
            operation: "Get current schema".to_string(),
            pg_error: "current_schema() returned NULL (no schema in search_path?)".to_string(),
        })
}

/// Create a TVIEW in `schema_name` with the given storage, as an aggregate TVIEW
/// when `group_keys` is given, and return the number of rows it was populated
/// with.
///
/// Steps: normalize and analyze the definition; create the backing view
/// `v_<entity>` and the table `tv_<entity>`; populate it; register it; install
/// triggers on its base tables.
///
/// # Errors
/// Returns an error if the TVIEW exists, the definition is invalid, or creation fails.
///
/// `declarations` are the uncascaded policies to store (a rebuilt TVIEW keeps its
/// own); `None` stores the defaults.
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
        declarations.unwrap_or_else(Declarations::defaults),
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

/// The entity a new TVIEW `tview_name` names and its definition, normalized:
/// refused when the TVIEW exists, the definition has no `pk_<entity>` column, or
/// an aggregate's definition is no `GROUP BY` of its keys.
fn checked_definition(
    tview_name: &str,
    select_sql: &str,
    aggregate: bool,
) -> TViewResult<(String, String, ViewColumns)> {
    if tview_exists(tview_name)? {
        return Err(TViewError::RelationExists {
            name: tview_name.to_string(),
        });
    }
    // `tv_entity` or just `entity`.
    let named = tview_name.strip_prefix("tv_").unwrap_or(tview_name);
    // Read the definition's columns with PostgreSQL's parser, expand `SELECT *`
    // and rewrite a raw SELECT to the `pk_*, id, data` shape.
    let (final_select_sql, final_schema) = normalize_definition(named, select_sql)?;
    let entity = final_schema
        .entity
        .clone()
        .ok_or_else(|| TViewError::RequiredColumnMissing {
            column_name: crate::lineage::plan::key_column(named),
            context: format!(
                "the definition of tv_{named}: a TVIEW's definition outputs a pk_<entity> \
                 column, the first of which names the entity"
            ),
        })?;
    // The entity comes from a column alias of the definition: it names objects.
    crate::validation::validate_sql_identifier(&entity, "entity_name")?;
    if aggregate {
        select::check_aggregate(&final_select_sql, &entity)?;
    }
    Ok((entity, final_select_sql, final_schema))
}

/// Create the backing view `view_schema.view_name` of the TVIEW `tview`, taking
/// over a view a dropped TVIEW left behind; its OID.
fn create_backing_view_in(
    view_schema: &str,
    view_name: &str,
    tview: &str,
    definition: &str,
) -> TViewResult<pg_sys::Oid> {
    if relation_exists(view_schema, view_name)?
        && !super::drop::reclaim_leftover_view(view_schema, view_name)?
    {
        return Err(TViewError::DefinitionRefused {
            reason: format!(
                "the backing view of {tview}, {view_schema}.{view_name}, is already taken by \
                 another relation"
            ),
        });
    }
    super::in_extension_schema(|| create_backing_view(view_name, definition, view_schema))?;
    relation_oid(view_schema, view_name)
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
    // The role the TVIEW is analyzed as: its owner-to-be, the creating role, or
    // the owner a rebuild restores.
    let rebuilt_for = REBUILT_FOR.with(std::cell::Cell::get);
    // It keeps the text search configuration of the session creating it.
    let owner = match rebuilt_for {
        Some(owner) => owner,
        None => crate::owner::Owner::current_user()?,
    }
    .with_text_search(crate::owner::session_text_search().as_deref());
    // Calls that register, change or drop one entity run one after the other.
    super::lock_entity(tview_name.strip_prefix("tv_").unwrap_or(tview_name))?;

    let (entity_name, final_select_sql, final_schema) =
        checked_definition(tview_name, select_sql, group_keys.is_some())?;
    let entity_name = entity_name.as_str();

    // Derive the canonical materialized-table name: always tv_<entity>.
    // This normalises both calling conventions:
    //   pg_tviews_create('post', ...)   → tv_post
    //   pg_tviews_create('tv_post', ...) → tv_post
    let tv_table_name = format!("tv_{entity_name}");

    let schema_name = schema_name.to_string();

    // Create the backing view
    let (view_schema, view_name) = super::backing_view_name(&schema_name, &tv_table_name);
    let view_oid = create_backing_view_in(
        &view_schema,
        &view_name,
        &format!("{schema_name}.{tv_table_name}"),
        &final_select_sql,
    )?;
    // Every function the refresh runs as the owner resolves names in its own
    // schemas, never first in a writer's temporary one.
    crate::catalog::function_paths::check(view_oid, None)?;

    // Find base table dependencies, and how a write to each maps to keys,
    // from the view's query tree (ADR 0157), with the column that names the
    // TVIEW's rows (ADR 0169); and the local paths of its local tables (for an
    // aggregate TVIEW, one per declared group key).
    // Pass schema_name so the view OID lookup searches in the correct schema even when
    // current_schema() resolves to a different schema due to the database search_path.
    let base_table_oids = crate::dependency::find_base_tables(&view_name, &view_schema)?;
    let derivation = derive_as(
        owner,
        entity_name,
        group_keys,
        &base_table_oids,
        view_oid,
        &declarations,
    )?;
    let lineage = &derivation.lineage;

    // Create materialized table tv_<entity>, keyed on the identity, and its indexes.
    let indexes = create_materialized_table(
        &tv_table_name,
        &final_schema,
        &schema_name,
        &lineage.identity.name,
        storage,
        view_oid,
    )?;

    // Populate initial data, unless the caller fills the table itself as its
    // owner (a rebuild by another role).
    let rows = if rebuilt_for.is_some() {
        0
    } else {
        fill_new(&schema_name, &tv_table_name, view_oid, lineage)?
    };

    // Reject a TVIEW no write can ever refresh: its definition reads no
    // table. Any table it reads, whatever it is called, maps its writes
    // to the TVIEW's keys or goes through the uncascaded policy below. The objects
    // created above roll back with the ERROR.
    if group_keys.is_none() && lineage.tables.is_empty() {
        return Err(TViewError::DefinitionRefused {
            reason: format!(
                "TVIEW '{tv_table_name}' can never be refreshed: its definition reads no table"
            ),
        });
    }

    // Report the base tables no cascade reaches under
    // the policy (`error` aborts, and the objects created above roll back with it),
    // and register the TVIEW with its plan.
    Registration {
        entity: entity_name,
        schema: &schema_name,
        view_oid,
        definition: &final_select_sql,
        columns: &final_schema,
        group_keys,
        derivation: &derivation,
    }
    .write(declarations, false)?;

    let table_oid = relation_oid(&schema_name, &tv_table_name)?;
    finish(
        entity_name,
        table_oid,
        &indexes,
        &derivation,
        &final_select_sql,
    )?;
    Ok(rows)
}

/// Fill a new TVIEW's table from its backing view `view_oid`: a UNION view
/// returning one key twice is refused first, instead of failing on the primary
/// key (ADR 0216). Returns the rows written.
fn fill_new(
    schema: &str,
    table: &str,
    view_oid: pg_sys::Oid,
    lineage: &crate::lineage::Lineage,
) -> TViewResult<u64> {
    if lineage.set_operation {
        crate::refresh::refuse_duplicate_keys_in(
            &format!(
                "{}.{}",
                crate::utils::ident::quoted(schema),
                crate::utils::ident::quoted(table)
            ),
            &lineage.identity.name,
            &crate::utils::qualified_relname_from_oid(view_oid)?,
            "true",
            &[],
        )?;
    }
    populate_initial_data(table, schema, view_oid)
}

/// What follows a new TVIEW's registration: its indexes recorded, its backing
/// view given the table's readers, its rows marked trusted, the triggers on the
/// tables it reads, the caches dropped, the audit entry.
fn finish(
    entity: &str,
    table_oid: pg_sys::Oid,
    indexes: &[String],
    derivation: &derive::Derivation,
    definition: &str,
) -> TViewResult<()> {
    crate::catalog::indexes::record(table_oid, indexes)?;
    // Whoever reads the TVIEW's table reads its backing view.
    super::privileges::follow(Some(table_oid), false)?;
    // Filled in this transaction (or by the caller, before it commits): an
    // UNLOGGED table's rows can be trusted until a reset.
    crate::lifecycle::validity::mark(entity, table_oid)?;
    // Triggers on the tables it reads, as their lineage needs them: base tables,
    // and other TVIEWs' tables it maps like them.
    let plan = crate::dependency::trigger_plan(&derivation.lineage);
    crate::dependency::check_installable(&plan, &[], entity)?;
    crate::dependency::sync_tables(&plan.iter().map(|&(table, _)| table).collect::<Vec<_>>())?;
    crate::cache::invalidate_all();
    crate::audit::log_create(entity, definition)
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
    // What is derived here runs as the owner later: nothing in it depends on
    // the search_path of the role whose DDL re-derives it.
    let _path = crate::owner::CatalogPath::pin();
    let meta = crate::catalog::TviewMeta::load_to_rederive(entity_name)
        .map_err(|e| TViewError::CatalogError {
            operation: format!("Read the metadata of tv_{entity_name}"),
            pg_error: e.to_string(),
        })?
        .ok_or_else(|| TViewError::TviewNotFound {
            name: entity_name.to_string(),
        })?;
    let view_oid = meta.view_oid;
    select::check_one_select(definition)?;
    // The backing view has the definition's columns: read them there, not from
    // the text, which the current search_path may resolve differently.
    let schema = ViewColumns::classify(crate::utils::column_types(view_oid)?);
    let (view_schema, view_name) = super::relation_name(view_oid)?;
    let base_table_oids = crate::dependency::find_base_tables(&view_name, &view_schema)?;
    let group_keys = stored_group_keys(entity_name)?;
    // The stored policies hold; an `error` table no cascade reaches aborts the
    // re-registration (and the ALTER that caused it).
    let declarations = Declarations::of(&meta);
    // Analyzed as the TVIEW's owner, whoever re-derives it: planning its mapping
    // queries runs the owner's functions.
    // It keeps its text search configuration; one registered without takes the
    // session's.
    let owner = crate::owner::Owner::of_tview(&meta)?;
    let owner = match owner.text_search() {
        Some(_) => owner,
        None => owner.with_text_search(crate::owner::session_text_search().as_deref()),
    };
    let derivation = derive_as(
        owner,
        entity_name,
        group_keys.as_ref(),
        &base_table_oids,
        view_oid,
        &declarations,
    )?;
    key_table_on_identity(
        schema_name,
        &format!("tv_{entity_name}"),
        &derivation.lineage.identity.name,
    )?;
    Registration {
        entity: entity_name,
        schema: schema_name,
        view_oid,
        definition,
        columns: &schema,
        group_keys: group_keys.as_ref(),
        derivation: &derivation,
    }
    .write(declarations, true)?;
    crate::cache::invalidate_all();
    Ok(crate::dependency::trigger_plan(&derivation.lineage))
}

/// Re-derive `entity`'s metadata from its stored definition and make its
/// base-table triggers match what that definition reads.
///
/// # Errors
/// Returns an error if the TVIEW is not registered, the caller does not own it,
/// or the definition cannot be analyzed.
pub fn reregister_tview(entity: &str) -> TViewResult<()> {
    // Ownership first: a role that may not re-register the TVIEW takes no lock.
    let meta = crate::catalog::TviewMeta::load_to_rederive(entity)?.ok_or_else(|| {
        TViewError::TviewNotFound {
            name: entity.to_string(),
        }
    })?;
    crate::owner::require_owner(meta.tview_oid, &format!("tv_{entity}"))?;
    rederive(entity)
}

/// Re-derive `entity`'s metadata from its stored definition and make its
/// base-table triggers match, for a caller whose right to it was checked.
///
/// # Errors
/// Returns an error if the TVIEW is not registered or the definition cannot be
/// analyzed.
pub(crate) fn rederive(entity: &str) -> TViewResult<()> {
    super::lock_entity(entity)?;
    let (definition, schema_name) = crate::catalog::row::definition_and_schema(entity)?;
    let (Some(definition), Some(schema_name)) = (definition, schema_name) else {
        return Err(TViewError::TviewNotFound {
            name: entity.to_string(),
        });
    };
    let read_before = crate::catalog::readers::tables_of(entity)?;
    let plan = reregister_metadata(entity, &schema_name, &definition)?;
    crate::dependency::sync_read_tables(&plan, &read_before, entity)?;
    crate::catalog::row::clear_needs_reregister(entity)
}

/// The `group_keys` of an aggregate TVIEW, `None` for any other.
///
/// # Errors
/// A [`TViewError::CatalogError`] naming the entity when the catalog cannot be
/// read or the stored keys do not decode: never read as "not an aggregate".
pub(crate) fn stored_group_keys(
    entity_name: &str,
) -> TViewResult<Option<super::aggregate::GroupKeys>> {
    crate::catalog::row::group_keys(entity_name)?
        .map(|keys| {
            serde_json::from_value(keys).map_err(|e| TViewError::CatalogError {
                operation: format!("Read the group keys of tv_{entity_name}"),
                pg_error: e.to_string(),
            })
        })
        .transpose()
}

/// The plan registering `view_oid` as the TVIEW of `entity` would store, with the
/// default options, without creating anything (the plan-snapshot tests).
///
/// # Errors
/// What registration would refuse the view with.
pub(crate) fn plan_of(entity: &str, view_oid: pgrx::pg_sys::Oid) -> TViewResult<serde_json::Value> {
    let base_tables = crate::dependency::base_tables_of(view_oid)?;
    let derivation = derive_as(
        crate::owner::Owner::current_user()?,
        entity,
        None,
        &base_tables,
        view_oid,
        &Declarations::defaults(),
    )?;
    serde_json::to_value(&derivation.plan).map_err(|e| crate::TViewError::CatalogError {
        operation: format!("Encode the plan of tv_{entity}"),
        pg_error: e.to_string(),
    })
}
