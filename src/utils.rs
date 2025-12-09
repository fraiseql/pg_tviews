use pgrx::prelude::*;

/// Extracts a `pk_*` integer from NEW or OLD tuple by convention.
/// For MVP we assume the column name is literally "pk_*".
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

/// Look up the view name (v_entity) associated with a tb_* or tv_* OID.
/// In a real implementation, use pg_depend or your own mapping.
pub fn lookup_view_for_source(_source_oid: Oid) -> spi::Result<String> {
    // TODO: real logic. For the stub, we just return "v_post".
    Ok("v_post".to_string())
}

/// Look up the TVIEW table name given its OID (from pg_tview_meta).
pub fn relname_from_oid(oid: Oid) -> spi::Result<String> {
    Spi::connect(|client| {
        let row = client
            .select(
                "SELECT relname FROM pg_class WHERE oid = $1",
                None,
                Some(vec![(PgOid::BuiltIn(PgBuiltInOids::OIDOID), oid.into())]),
            )?
            .get(0)?;

        Ok(row["relname"].value().unwrap())
    })
}

