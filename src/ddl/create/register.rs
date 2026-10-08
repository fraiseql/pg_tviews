//! The TVIEW's catalog row, written in one parameterized statement.

use super::ViewColumns;
use super::derive::aggregate_embeds;
use super::indexes::create_embed_lookup_indexes;
use crate::cascade_path;
use crate::ddl::uncascaded::Uncascaded;
use crate::error::TViewError;
use crate::error::TViewResult;
use pgrx::pg_sys;
use pgrx::prelude::Spi;

/// Register the TVIEW in metadata tables
#[allow(clippy::too_many_arguments)] // Reason: all args are distinct registration fields with no natural grouping
pub(crate) fn register_metadata(
    entity_name: &str,
    view_oid: pg_sys::Oid,
    tview_name: &str,
    definition_sql: &str,
    schema: &ViewColumns,
    cascade_paths: &[cascade_path::CascadePath],
    schema_name: &str,
    group_keys: Option<&crate::ddl::aggregate::GroupKeys>,
    uncascaded: &Uncascaded,
    key_mappings: &serde_json::Value,
    lineage: &crate::lineage::Lineage,
    replace: bool,
) -> TViewResult<()> {
    let identity = &lineage.identity;
    // A set operation (UNION, INTERSECT, EXCEPT): its rows are recomputed, never
    // patched.
    let is_union = lineage.set_operation;

    // The TVIEWs it embeds (read with an output column equal to their key).
    // Until the propagation plan stores them as such, an embed whose column is
    // `fk_<child>` goes with its kind and path into `fk_columns`, any other into
    // the explicit lookup map beside the aggregate embeds (issue #126).
    let embeds = lineage.embeds(entity_name);
    let named: Vec<&crate::lineage::Embed> = embeds
        .iter()
        .filter(|e| e.lookup == format!("fk_{}", e.entity))
        .collect();
    let mut aggregate_embeds = aggregate_embeds(lineage, entity_name)?;
    for embed in embeds
        .iter()
        .filter(|e| e.lookup != format!("fk_{}", e.entity))
    {
        aggregate_embeds
            .entry(embed.entity.clone())
            .or_insert_with(|| embed.lookup.clone());
    }
    create_embed_lookup_indexes(&aggregate_embeds, schema, tview_name, schema_name)?;

    // The direct-patch column→key map (issue #56), read from the `data`
    // expression: empty ⇒ the fast path never engages for this entity.
    let direct_map = lineage.direct_fields();
    let (direct_map_columns, direct_map_keys): (Vec<String>, Vec<String>) =
        direct_map.into_iter().unzip();
    let cascade_paths: Vec<String> = cascade_paths
        .iter()
        .map(serde_json::to_string)
        .collect::<Result<_, _>>()?;

    // Get the OID of the table (schema-qualified, parameterized to prevent injection)
    let table_oid_args = vec![
        crate::utils::spi::text(tview_name),
        crate::utils::spi::text(schema_name),
    ];
    let table_oid_result = Spi::get_one_with_args::<pg_sys::Oid>(
        "SELECT c.oid FROM pg_class c \
         JOIN pg_namespace n ON c.relnamespace = n.oid \
         WHERE c.relname = $1 AND n.nspname = $2 AND c.relkind = 'r'",
        &table_oid_args,
    )
    .map_err(|e| TViewError::CatalogError {
        operation: format!("Get OID for table {schema_name}.{tview_name}"),
        pg_error: e.to_string(),
    })?;

    let table_oid = table_oid_result.ok_or_else(|| TViewError::CatalogError {
        operation: format!("Find table {schema_name}.{tview_name}"),
        pg_error: "Table OID not found".to_string(),
    })?;

    // A re-registration (after a column rename, or by pg_tviews_reregister)
    // replaces every derived column and keeps created_at, graphql_typename and
    // needs_reregister: only pg_tviews_reregister, which also re-installs the
    // triggers, clears the flag.
    let on_conflict = if replace {
        "ON CONFLICT (entity) DO UPDATE SET \
            view_oid = EXCLUDED.view_oid, table_oid = EXCLUDED.table_oid, \
            definition = EXCLUDED.definition, cascade_paths = EXCLUDED.cascade_paths, \
            fk_columns = EXCLUDED.fk_columns, uuid_fk_columns = EXCLUDED.uuid_fk_columns, \
            dependency_types = EXCLUDED.dependency_types, \
            dependency_paths = EXCLUDED.dependency_paths, \
            array_match_keys = EXCLUDED.array_match_keys, \
            distinct_on_keys = '{}', distinct_on_output_keys = '{}', \
            direct_map_columns = EXCLUDED.direct_map_columns, \
            direct_map_keys = EXCLUDED.direct_map_keys, is_union = EXCLUDED.is_union, \
            group_keys = EXCLUDED.group_keys, aggregate_embeds = EXCLUDED.aggregate_embeds, \
            uncascaded_oids = EXCLUDED.uncascaded_oids, key_mappings = EXCLUDED.key_mappings, \
            identity = EXCLUDED.identity, time_refresh = EXCLUDED.time_refresh, \
            time_dependent = EXCLUDED.time_dependent"
    } else {
        "ON CONFLICT (entity) DO NOTHING"
    };

    // One parameterized INSERT: every value is a parameter (Q22).
    let meta = crate::utils::meta_table();
    let insert_meta_sql = format!(
        "INSERT INTO {meta} (
            entity, view_oid, table_oid, definition, cascade_paths,
            fk_columns, uuid_fk_columns, dependency_types, dependency_paths, array_match_keys,
            direct_map_columns, direct_map_keys, is_union, group_keys, aggregate_embeds,
            uncascaded_oids, uncascaded_policy, key_mappings, identity,
            uncascaded_table_oids, uncascaded_table_policies,
            function_read_functions, function_read_tables, time_refresh, time_dependent
        ) VALUES (
            $1, $2::pg_catalog.oid::pg_catalog.regclass, $3::pg_catalog.oid::pg_catalog.regclass,
            $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15,
            $16::pg_catalog.oid[]::pg_catalog.regclass[], $17, $18,
            pg_catalog.jsonb_build_object('kind', $19::pg_catalog.text, 'columns',
                pg_catalog.jsonb_build_array(pg_catalog.jsonb_build_object(
                    'name', $20::pg_catalog.text,
                    'type', pg_catalog.format_type($21::pg_catalog.oid, NULL)))),
            $22::pg_catalog.oid[]::pg_catalog.regclass[], $23,
            $24, $25::pg_catalog.oid[]::pg_catalog.regclass[], $26, $27)
        {on_conflict}"
    );

    let group_keys_json =
        group_keys.map(|keys| pgrx::JsonB(serde_json::to_value(keys).unwrap_or_default()));
    let (function_read_functions, function_read_tables) =
        uncascaded.declarations.function_read_pairs();
    let embed_column = |f: fn(&crate::lineage::Embed) -> String| -> Vec<String> {
        named.iter().map(|e| f(e)).collect()
    };
    let args = [
        crate::utils::spi::text(entity_name),
        crate::utils::spi::oid(view_oid),
        crate::utils::spi::oid(table_oid),
        crate::utils::spi::text(definition_sql),
        crate::utils::spi::text_array(cascade_paths),
        crate::utils::spi::text_array(embed_column(|e| e.lookup.clone())),
        crate::utils::spi::text_array(schema.uuid_fk.clone()),
        crate::utils::spi::text_array(embed_column(|e| e.kind.stored().to_string())),
        crate::utils::spi::text_array(embed_column(|e| e.path.join("."))),
        crate::utils::spi::text_array(embed_column(|e| {
            if e.kind == crate::lineage::EmbedKind::Array {
                "id"
            } else {
                ""
            }
            .to_string()
        })),
        crate::utils::spi::text_array(direct_map_columns),
        crate::utils::spi::text_array(direct_map_keys),
        crate::utils::spi::boolean(is_union),
        crate::utils::spi::jsonb(group_keys_json),
        crate::utils::spi::jsonb(pgrx::JsonB(
            serde_json::to_value(&aggregate_embeds).unwrap_or_default(),
        )),
        crate::utils::spi::oid_array(uncascaded.oids()),
        crate::utils::spi::text(uncascaded.declarations.policy.as_str()),
        crate::utils::spi::jsonb(pgrx::JsonB(key_mappings.clone())),
        crate::utils::spi::text(identity.kind.name()),
        crate::utils::spi::text(identity.name.as_str()),
        crate::utils::spi::oid(pg_sys::Oid::from(identity.type_oid)),
        crate::utils::spi::oid_array(
            uncascaded
                .declarations
                .tables
                .iter()
                .map(|(oid, _)| *oid)
                .collect::<Vec<_>>(),
        ),
        crate::utils::spi::text_array(
            uncascaded
                .declarations
                .tables
                .iter()
                .map(|(_, policy)| policy.as_str().to_string())
                .collect::<Vec<_>>(),
        ),
        crate::utils::spi::text_array(function_read_functions),
        crate::utils::spi::oid_array(function_read_tables),
        crate::utils::spi::text(uncascaded.time_refresh()),
        crate::utils::spi::boolean(uncascaded.time_dependent),
    ];
    // The catalog is written as the extension's owner; the caller's right to
    // change this TVIEW was checked before (issue #134).
    let _owner = crate::owner::AsOwner::of_extension()?;
    Spi::run_with_args(&insert_meta_sql, &args).map_err(|e| TViewError::SpiError {
        query: insert_meta_sql,
        error: e.to_string(),
    })?;

    // TVIEWs that read each other in a cycle could never be refreshed in order:
    // refuse the definition that closes one, before any row is written.
    crate::queue::graph::EntityDepGraph::load()?;

    Ok(())
}
