//! The TVIEW's catalog row, written in one parameterized statement.

use super::ViewColumns;
use super::derive::{Derivation, uncascaded_tables};
use super::indexes::{create_embed_lookup_indexes, key_index};
use super::relations::relation_oid;
use crate::catalog::plan::TviewPlan;
use crate::ddl::uncascaded::{Declarations, Uncascaded};
use crate::error::TViewResult;
use crate::lineage::Identity;
use pgrx::pg_sys;

/// What creating and re-registering a TVIEW both write once its relations exist:
/// the tables no cascade reaches, reported under the policy (`error` aborts),
/// the indexes its embed lookups need, and its catalog row.
pub(crate) struct Registration<'a> {
    pub(crate) entity: &'a str,
    /// The schema of `tv_<entity>`.
    pub(crate) schema: &'a str,
    pub(crate) view_oid: pg_sys::Oid,
    pub(crate) definition: &'a str,
    pub(crate) columns: &'a ViewColumns,
    pub(crate) group_keys: Option<&'a crate::ddl::aggregate::GroupKeys>,
    pub(crate) derivation: &'a Derivation,
}

impl Registration<'_> {
    /// Report, index and write the catalog row (`replace`: over an existing one).
    ///
    /// # Errors
    /// An `error` policy table no cascade reaches, a refused embed, a failed write.
    pub(crate) fn write(&self, declarations: Declarations, replace: bool) -> TViewResult<()> {
        let tview = format!("tv_{}", self.entity);
        let table_oid = relation_oid(self.schema, &tview)?;
        let qualified = crate::utils::qualified_relname_from_oid(table_oid)?;
        let lineage = &self.derivation.lineage;
        crate::ddl::uncascaded::report_functions(
            &qualified,
            &self.derivation.undeclared_functions,
            declarations.policy,
        );
        crate::ddl::uncascaded::report_time(&qualified, &lineage.time_reads, &declarations)?;
        crate::ddl::uncascaded::check_declared(&qualified, &declarations, lineage)?;
        let uncascaded = Uncascaded {
            tables: uncascaded_tables(lineage),
            declarations,
            time_dependent: !lineage.time_reads.is_empty(),
        };
        crate::ddl::uncascaded::report(&qualified, &uncascaded);
        let plan = &self.derivation.plan;
        super::derive::aggregate_embeds(lineage, self.entity)?;
        let lookups = embed_lookups(plan);
        let mut indexes = if replace && crate::catalog::indexes::unrecorded(table_oid)? {
            adopted_indexes(table_oid, &tview, self.columns, &lookups)?
        } else {
            Vec::new()
        };
        indexes.extend(create_embed_lookup_indexes(
            &lookups,
            self.columns,
            &tview,
            self.schema,
        )?);
        if let Some(index) = key_index(&tview, self.columns, &lineage.identity.name) {
            indexes.extend(index.create(self.schema, &tview)?);
        }
        MetaRow {
            entity: self.entity,
            view_oid: self.view_oid,
            table_oid,
            definition: self.definition,
            plan,
            group_keys: self.group_keys,
            uncascaded: &uncascaded,
            identity: &lineage.identity,
        }
        .write(replace)?;
        crate::catalog::indexes::record(table_oid, &indexes)
    }
}

/// A TVIEW's row of `pg_tview_meta`, as registration derives it.
struct MetaRow<'a> {
    pub(crate) entity: &'a str,
    pub(crate) view_oid: pg_sys::Oid,
    pub(crate) table_oid: pg_sys::Oid,
    pub(crate) definition: &'a str,
    pub(crate) plan: &'a TviewPlan,
    pub(crate) group_keys: Option<&'a crate::ddl::aggregate::GroupKeys>,
    pub(crate) uncascaded: &'a Uncascaded,
    pub(crate) identity: &'a Identity,
}

impl MetaRow<'_> {
    /// Insert the row, or with `replace` overwrite what registration derives of an
    /// existing one (keeping `created_at`, `graphql_typename` and
    /// `needs_reregister`: only `pg_tviews_reregister`, which also re-installs the
    /// triggers, clears the flag).
    ///
    /// # Errors
    /// A failed write, or [`crate::error::TViewError::DependencyCycle`] when the
    /// TVIEW closes a cycle of TVIEWs reading each other.
    fn write(&self, replace: bool) -> TViewResult<()> {
        let declarations = &self.uncascaded.declarations;
        let group_keys = self.group_keys.map(serde_json::to_value).transpose()?;
        // The catalog is written as the extension's owner; the caller's right to
        // change this TVIEW was checked before.
        crate::catalog::queries::write_registration(
            crate::catalog::queries::Registration {
                entity: self.entity,
                view_oid: self.view_oid,
                table_oid: self.table_oid,
                definition: self.definition,
                plan: serde_json::to_value(self.plan)?,
                group_keys,
                uncascaded_oids: self.uncascaded.oids(),
                declared: declarations.catalog_row(self.uncascaded.time_refresh()),
                identity_kind: self.identity.kind.name(),
                identity_column: self.identity.name.as_str(),
                identity_type: pg_sys::Oid::from(self.identity.type_oid),
                time_dependent: self.uncascaded.time_dependent,
            },
            replace,
        )?;

        // TVIEWs that read each other in a cycle could never be refreshed in
        // order: refuse the definition that closes one. A fresh load, not the
        // cached graph: the row just written must be in it.
        crate::catalog::EntityDepGraph::load()?;
        Ok(())
    }
}

/// Every column `plan` looks up an embedded TVIEW's rows by, and its references,
/// which a reader composes by: each gets an index leading with it.
fn embed_lookups(plan: &TviewPlan) -> Vec<String> {
    plan.embeds
        .iter()
        .flat_map(|e| e.lookups.iter().cloned())
        .chain(plan.references.iter().map(|r| r.column.clone()))
        .collect()
}

/// The indexes on `table` that are exactly those `pg_tviews` creates for the
/// TVIEW under their names: what a TVIEW registered before its indexes were
/// recorded takes as `pg_tviews`' when the upgrade re-registers it. Another index
/// under one of these names stays the user's, with a warning: the name is
/// reserved, so a dump of the database would not restore it.
fn adopted_indexes(
    table: pg_sys::Oid,
    tview_name: &str,
    schema: &ViewColumns,
    lookups: &[String],
) -> TViewResult<Vec<String>> {
    let mut adopted = Vec::new();
    for index in super::managed_indexes(tview_name, schema, lookups) {
        if index.exists_on(table)? {
            adopted.push(index.name);
        } else if crate::utils::spi::one::<bool>(
            "SELECT EXISTS (SELECT 1 FROM pg_catalog.pg_index i \
             JOIN pg_catalog.pg_class c ON c.oid = i.indexrelid \
             WHERE i.indrelid = $1 AND c.relname = $2)",
            &[
                crate::utils::spi::oid(table),
                crate::utils::spi::text(index.name.as_str()),
            ],
        )? == Some(true)
        {
            pgrx::warning!(
                "index {} on {tview_name} is not the index pg_tviews creates under that name, \
                 which is reserved: rename it (ALTER INDEX … RENAME TO), or a dump of this \
                 database will not restore it",
                index.name
            );
        }
    }
    Ok(adopted)
}
