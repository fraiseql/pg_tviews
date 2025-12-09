use pgrx::prelude::*;
use pgrx::pg_sys::Oid;
use pgrx::IntoDatum;
use serde::{Deserialize, Serialize};

/// Represents a row in pg_tview_meta (your own catalog table).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TviewMeta {
    pub tview_oid: Oid,
    pub view_oid: Oid,
    pub entity_name: String,
    pub sync_mode: char, // 's' = sync (default), 'a' = async (future)
}

impl TviewMeta {
    /// Look up metadata by source table OID or view OID.
    pub fn load_for_source(source_oid: Oid) -> spi::Result<Option<Self>> {
        Spi::connect(|client| {
            let rows = client.select(
                "SELECT tview_oid, view_oid, entity_name, sync_mode \
                 FROM pg_tview_meta \
                 WHERE view_oid = $1 OR tview_oid = $1",
                None,
                Some(vec![(PgOid::BuiltIn(PgBuiltInOids::OIDOID), source_oid.into_datum())]),
            )?;

            let mut result = None;
            for row in rows {
                result = Some(Self {
                    tview_oid: row["tview_oid"].value().unwrap().unwrap(),
                    view_oid: row["view_oid"].value().unwrap().unwrap(),
                    entity_name: row["entity_name"].value().unwrap().unwrap(),
                    sync_mode: row["sync_mode"].value().unwrap().unwrap(),
                });
                break; // Only get first row
            }
            Ok(result)
        })
    }

    /// TODO: function to register a new TVIEW (used by CREATE TVIEW)
    pub fn register_new(_view_oid: Oid, _tview_oid: Oid, _entity_name: &str) -> spi::Result<()> {
        // Implementation: insert into pg_tview_meta
        // This will be invoked from a CREATE TVIEW support function.
        Ok(())
    }
}

