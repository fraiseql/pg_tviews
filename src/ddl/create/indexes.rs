//! The indexes `pg_tviews` creates on a TVIEW's table.

use super::ViewColumns;
use crate::error::TViewError;
use crate::error::TViewResult;
use crate::utils::ident;
use pgrx::pg_sys;

/// Deterministic index name `idx_<tview>_<suffix>`, fitted to 63 bytes by
/// [`crate::utils::fit_identifier`].
pub(crate) fn index_name(tview_name: &str, suffix: &str) -> String {
    crate::utils::fit_identifier(format!("idx_{tview_name}_{suffix}"))
}

/// `CREATE INDEX IF NOT EXISTS idx_<tview>_<suffix> ON schema.tview <method>(cols)`.
fn index_ddl(
    schema_name: &str,
    tview_name: &str,
    suffix: &str,
    method: &str,
    columns: &[&str],
) -> String {
    let cols = columns
        .iter()
        .map(|c| ident::quoted(c))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "CREATE INDEX IF NOT EXISTS {} ON {}.{} {method}({cols})",
        ident::quoted(&index_name(tview_name, suffix)),
        ident::quoted(schema_name),
        ident::quoted(tview_name),
    )
}

/// An index `pg_tviews` creates on a TVIEW's table: `idx_<tview>_<suffix>` over
/// `columns`, btree or GIN.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ManagedIndex {
    pub(crate) name: String,
    suffix: String,
    gin: bool,
    pub(crate) columns: Vec<String>,
}

impl ManagedIndex {
    fn btree(tview_name: &str, suffix: String, columns: Vec<String>) -> Self {
        Self {
            name: index_name(tview_name, &suffix),
            suffix,
            gin: false,
            columns,
        }
    }

    /// The required propagation index `(column, pk)`.
    ///
    /// Cascade propagation (`src/propagate.rs`) looks up parent rows with
    /// `SELECT column, pk FROM tv WHERE column = ANY($1)`; this index makes that
    /// lookup index-only instead of a scan of the whole TVIEW.
    pub(crate) fn propagation(tview_name: &str, column: &str, pk: &str) -> Self {
        Self::btree(
            tview_name,
            format!("{column}_{pk}"),
            vec![column.to_string(), pk.to_string()],
        )
    }

    /// The opt-in GIN index on `data`.
    pub(crate) fn data_gin(tview_name: &str, data: &str) -> Self {
        let suffix = format!("{data}_gin");
        Self {
            name: index_name(tview_name, &suffix),
            suffix,
            gin: true,
            columns: vec![data.to_string()],
        }
    }

    /// Its index access method, as `pg_am.amname`.
    pub(crate) const fn method(&self) -> &'static str {
        if self.gin { "gin" } else { "btree" }
    }

    /// `CREATE INDEX IF NOT EXISTS` for this index on `schema_name.tview_name`.
    pub(crate) fn ddl(&self, schema_name: &str, tview_name: &str) -> String {
        let columns: Vec<&str> = self.columns.iter().map(String::as_str).collect();
        index_ddl(
            schema_name,
            tview_name,
            &self.suffix,
            if self.gin { "USING GIN " } else { "" },
            &columns,
        )
    }

    /// Create it on `schema_name.tview_name`; its name when this statement created
    /// it, `None` when a relation of that name was already there (`IF NOT EXISTS`
    /// then does nothing, and the relation is not `pg_tviews`').
    ///
    /// # Errors
    /// A failed lookup or `CREATE INDEX`.
    pub(crate) fn create(
        &self,
        schema_name: &str,
        tview_name: &str,
    ) -> TViewResult<Option<String>> {
        let taken = crate::utils::spi::one::<bool>(
            "SELECT EXISTS (SELECT 1 FROM pg_catalog.pg_class c \
             JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
             WHERE c.relname = $1 AND n.nspname = $2)",
            &[
                crate::utils::spi::text(self.name.as_str()),
                crate::utils::spi::text(schema_name),
            ],
        )?;
        if taken == Some(true) {
            return Ok(None);
        }
        let ddl = self.ddl(schema_name, tview_name);
        crate::utils::spi_run_ddl(&ddl).map_err(|e| TViewError::SpiError {
            query: ddl.clone(),
            error: e,
        })?;
        Ok(Some(self.name.clone()))
    }

    /// Whether `table` has an index of this name that is exactly this index: same
    /// method and key columns, default operator classes, not unique, no predicate
    /// or expression.
    ///
    /// # Errors
    /// A failed catalog query.
    pub(crate) fn exists_on(&self, table: pg_sys::Oid) -> TViewResult<bool> {
        Ok(crate::utils::spi::one::<bool>(
            "SELECT EXISTS (SELECT 1 FROM pg_catalog.pg_index i \
             JOIN pg_catalog.pg_class ic ON ic.oid = i.indexrelid \
             JOIN pg_catalog.pg_am am ON am.oid = ic.relam \
             WHERE i.indrelid = $1 AND ic.relname = $2 AND am.amname = $3 \
               AND NOT i.indisunique AND i.indpred IS NULL AND i.indexprs IS NULL \
               AND i.indnkeyatts = i.indnatts \
               AND ARRAY(SELECT a.attname::pg_catalog.text \
                         FROM pg_catalog.unnest(i.indkey::pg_catalog.int2[]) \
                              WITH ORDINALITY AS k(attnum, n) \
                         JOIN pg_catalog.pg_attribute a \
                           ON a.attrelid = i.indrelid AND a.attnum = k.attnum \
                         ORDER BY k.n) = $4 \
               AND NOT EXISTS (SELECT 1 FROM pg_catalog.unnest(i.indclass::pg_catalog.oid[]) c \
                               JOIN pg_catalog.pg_opclass o ON o.oid = c \
                               WHERE NOT o.opcdefault))",
            &[
                crate::utils::spi::oid(table),
                crate::utils::spi::text(self.name.as_str()),
                crate::utils::spi::text(self.method()),
                crate::utils::spi::text_array_of(&self.columns),
            ],
        )? == Some(true))
    }
}

/// Create the indexes a new TVIEW gets on its table; the names of those created.
pub(crate) fn create_tview_indexes(
    tview_name: &str,
    schema: &ViewColumns,
    schema_name: &str,
    data_gin: bool,
) -> TViewResult<Vec<String>> {
    let mut created = Vec::new();
    for index in tview_indexes(tview_name, schema, data_gin) {
        created.extend(index.create(schema_name, tview_name)?);
    }
    Ok(created)
}

/// Every index `pg_tviews` creates, or would create, on `tview_name` for `schema`
/// (the `data` GIN index included, whether or not the option is on) and for the
/// columns it looks up the aggregate TVIEWs it embeds by.
pub(crate) fn managed_indexes(
    tview_name: &str,
    schema: &ViewColumns,
    embed_columns: &[String],
) -> Vec<ManagedIndex> {
    let mut indexes = tview_indexes(tview_name, schema, true);
    if let Some(pk) = &schema.pk {
        for column in embed_lookup_columns(embed_columns, schema, pk) {
            indexes.push(ManagedIndex::propagation(tview_name, column, pk));
        }
    }
    indexes.dedup_by(|a, b| a.name == b.name);
    indexes
}

/// The indexes a new TVIEW gets.
///
/// HOT invariant: refreshes rewrite `data` and `updated_at`, so neither is indexed
/// (the `data` GIN only when `data_gin` is explicitly requested). An index on a
/// rewritten column makes every refresh a non-HOT update: new entries in every
/// index, a dead tuple needing index cleanup, and a cleared visibility-map bit.
pub(crate) fn tview_indexes(
    tview_name: &str,
    schema: &ViewColumns,
    data_gin: bool,
) -> Vec<ManagedIndex> {
    let mut indexes = Vec::new();

    // Trinity identifier and UUID foreign keys (filtering by public id)
    for column in schema.id.iter().chain(&schema.uuid_fk) {
        indexes.push(ManagedIndex::btree(
            tview_name,
            column.clone(),
            vec![column.clone()],
        ));
    }

    // Required propagation indexes (see `ManagedIndex::propagation`)
    if let Some(pk) = &schema.pk {
        for fk in schema.fk.iter().filter(|fk| *fk != pk) {
            indexes.push(ManagedIndex::propagation(tview_name, fk, pk));
        }
    }

    // Opt-in (option data_gin_index): top-level containment queries on data
    if data_gin && let Some(data) = &schema.data {
        indexes.push(ManagedIndex::data_gin(tview_name, data));
    }

    indexes
}

/// DDL for every index a new TVIEW gets (see [`tview_indexes`]).
#[cfg(test)]
pub(crate) fn tview_index_ddl(
    tview_name: &str,
    schema: &ViewColumns,
    schema_name: &str,
    data_gin: bool,
) -> Vec<String> {
    tview_indexes(tview_name, schema, data_gin)
        .iter()
        .map(|index| index.ddl(schema_name, tview_name))
        .collect()
}

/// `WITH (fillfactor = N)` for a new TVIEW table; empty at 100 (the heap default),
/// so opting out yields the same DDL as before the setting existed.
pub(crate) fn storage_clause(fillfactor: i32) -> String {
    if fillfactor < 100 {
        format!(" WITH (fillfactor = {fillfactor})")
    } else {
        String::new()
    }
}

/// The embed lookup columns that need an index of their own: neither the TVIEW's
/// primary key nor an `fk_*` column, which has its propagation index already.
fn embed_lookup_columns<'a>(
    lookups: &'a [String],
    schema: &'a ViewColumns,
    pk: &'a String,
) -> std::collections::BTreeSet<&'a String> {
    lookups
        .iter()
        .filter(|c| *c != pk && !schema.fk.contains(c))
        .collect()
}

/// Index each embed lookup column (see [`embed_lookup_columns`]), so propagation
/// from the embedded TVIEW does not scan the whole TVIEW; the names of the indexes
/// created.
pub(crate) fn create_embed_lookup_indexes(
    lookups: &[String],
    schema: &ViewColumns,
    tview_name: &str,
    schema_name: &str,
) -> TViewResult<Vec<String>> {
    let Some(pk) = &schema.pk else {
        return Ok(Vec::new());
    };
    let mut created = Vec::new();
    for column in embed_lookup_columns(lookups, schema, pk) {
        created.extend(
            ManagedIndex::propagation(tview_name, column, pk).create(schema_name, tview_name)?,
        );
    }
    Ok(created)
}
