use pgrx::prelude::*;
use pgrx::pg_sys::Oid;
use pgrx::JsonB;

use crate::catalog::TviewMeta;
use crate::propagate::propagate_from_row;
use crate::utils::{lookup_view_for_source, relname_from_oid};

/// Represents a materialized view row pulled from v_entity.
pub struct ViewRow {
    pub entity_name: String,
    pub pk: i64,
    pub tview_oid: Oid,
    pub view_oid: Oid,
    pub data: JsonB,
    pub fk_values: Vec<(String, i64)>,    // e.g. [("fk_user", 7)]
    pub uuid_fk_values: Vec<(String, String)>, // e.g. [("user_id", "...")]
}

pub fn refresh_pk(source_oid: Oid, pk: i64) -> spi::Result<()> {
    // 1. Find TVIEW metadata (tview_oid, view_oid, entity_name, etc.)
    let meta = TviewMeta::load_for_source(source_oid)?;
    let meta = match meta {
        Some(m) => m,
        None => {
            error!("No TVIEW metadata for source_oid: {:?}", source_oid);
        }
    };

    // 2. Recompute row from v_entity
    let view_row = recompute_view_row(&meta, pk)?;

    // 3. Patch tv_entity using jsonb_ivm
    apply_patch(&view_row)?;

    // 4. Propagate to parent entities
    propagate_from_row(&view_row)?;

    Ok(())
}

/// Recompute view row from v_entity WHERE pk = $1
fn recompute_view_row(meta: &TviewMeta, pk: i64) -> spi::Result<ViewRow> {
    let view_name = lookup_view_for_source(meta.view_oid)?;
    let pk_col = format!("pk_{}", meta.entity_name); // e.g. pk_post

    let sql = format!(
        "SELECT * FROM {} WHERE {} = $1",
        view_name, pk_col,
    );

    Spi::connect(|client| {
        let rows = client.select(
            &sql,
            None,
            Some(vec![(PgOid::BuiltIn(PgBuiltInOids::INT8OID), pk.into_datum())]),
        )?;

        let mut row = None;
        for r in rows {
            row = Some(r);
            break;
        }
        let row = match row {
            Some(r) => r,
            None => error!("No row in v_* for given pk: {}", pk),
        };
        // You can either fetch the whole row as JSONB, or separate fields.
        // Here we assume 'data' is the JSONB column name.
        let data: JsonB = row["data"].value().unwrap().unwrap();

        // TODO: also load fk_* columns and uuid fk columns into fk_values / uuid_fk_values.
        Ok(ViewRow {
            entity_name: meta.entity_name.clone(),
            pk,
            tview_oid: meta.tview_oid,
            view_oid: meta.view_oid,
            data,
            fk_values: Vec::new(),
            uuid_fk_values: Vec::new(),
        })
    })
}

/// Apply JSON patch to tv_entity for pk using jsonb_ivm_patch.
/// For now, this stub replaces the JSON instead of calling jsonb_ivm_patch.
fn apply_patch(row: &ViewRow) -> spi::Result<()> {
    let tv_name = relname_from_oid(row.tview_oid)?;
    let pk_col = format!("pk_{}", row.entity_name);

    // TODO: call jsonb_ivm_patch(data, $1) instead of direct replacement
    let sql = format!(
        "UPDATE {} \
         SET data = $1, updated_at = now() \
         WHERE {} = $2",
        tv_name, pk_col
    );

    Spi::connect(|mut client| {
        client.update(
            &sql,
            None,
            Some(vec![
                (PgOid::BuiltIn(PgBuiltInOids::JSONBOID), JsonB(row.data.0.clone()).into_datum()),
                (PgOid::BuiltIn(PgBuiltInOids::INT8OID), row.pk.into_datum()),
            ]),
        )?;
        Ok(())
    })
}

