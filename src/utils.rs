use pgrx::prelude::*;
use pgrx::pg_sys::Oid;

/// Extracts a `pk_*` integer from NEW or OLD tuple by convention.
/// For MVP we assume the column name is literally "pk_*".
#[allow(dead_code)]
pub fn extract_pk(trigger: &PgTrigger) -> spi::Result<i64> {
    // For simplicity we assume there's a column named 'pk_*' and you know the entity.
    // For real code:
    //  - inspect relation attributes,
    //  - find first "pk_" column,
    //  - read value.
    let tuple = trigger
        .new()
        .or(trigger.old())
        .expect("Row must exist for AFTER trigger");

    // TODO: detect column name dynamically. For now, assume "pk_*" is "pk_post".
    // You might want to store the pk column name in pg_tview_meta.
    let pk: i64 = tuple
        .get_by_name("pk_post")? // <-- placeholder: replace per entity
        .expect("pk_post must not be null");
    Ok(pk)
}

/// Look up the view name from an OID
/// Used to find the backing view (v_entity) for a TVIEW
pub fn lookup_view_for_source(view_oid: Oid) -> spi::Result<String> {
    // Simply get the relation name from pg_class
    relname_from_oid(view_oid)
}

/// Look up the TVIEW table name given its OID (from pg_tview_meta).
pub fn relname_from_oid(oid: Oid) -> spi::Result<String> {
    Spi::connect(|client| {
        let mut rows = client.select(
            "SELECT relname::text AS relname FROM pg_class WHERE oid = $1",
            None,
            Some(vec![(PgOid::BuiltIn(PgBuiltInOids::OIDOID), oid.into_datum())]),
        )?;

        if let Some(row) = rows.next() {
            Ok(row["relname"].value().unwrap().unwrap())
        } else {
            error!("No pg_class entry for oid: {:?}", oid)
        }
    })
}

