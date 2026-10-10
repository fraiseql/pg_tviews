//! The TVIEW's catalog row, written in one parameterized statement.

use super::ViewColumns;
use super::derive::{Derivation, uncascaded_tables};
use super::indexes::create_embed_lookup_indexes;
use super::relations::relation_oid;
use crate::catalog::plan::TviewPlan;
use crate::ddl::uncascaded::{Declarations, Uncascaded};
use crate::error::TViewError;
use crate::error::TViewResult;
use crate::lineage::Identity;
use pgrx::pg_sys;
use pgrx::prelude::Spi;

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
    /// A failed write, or [`TViewError::DependencyCycle`] when the TVIEW closes a
    /// cycle of TVIEWs reading each other.
    fn write(&self, replace: bool) -> TViewResult<()> {
        let on_conflict = if replace {
            "ON CONFLICT (entity) DO UPDATE SET \
                view_oid = EXCLUDED.view_oid, table_oid = EXCLUDED.table_oid, \
                definition = EXCLUDED.definition, plan = EXCLUDED.plan, \
                group_keys = EXCLUDED.group_keys, uncascaded_oids = EXCLUDED.uncascaded_oids, \
                identity = EXCLUDED.identity, time_refresh = EXCLUDED.time_refresh, \
                time_dependent = EXCLUDED.time_dependent"
        } else {
            "ON CONFLICT (entity) DO NOTHING"
        };
        // Every value is a parameter (Q22).
        let sql = format!(
            "INSERT INTO {} (
                entity, view_oid, table_oid, definition, plan, group_keys,
                uncascaded_oids, uncascaded_policy, identity,
                uncascaded_table_oids, uncascaded_table_policies,
                function_read_functions, function_read_tables, time_refresh, time_dependent
            ) VALUES (
                $1, $2::pg_catalog.oid::pg_catalog.regclass, $3::pg_catalog.oid::pg_catalog.regclass,
                $4, $5, $6, $7::pg_catalog.oid[]::pg_catalog.regclass[], $8,
                pg_catalog.jsonb_build_object('kind', $9::pg_catalog.text, 'columns',
                    pg_catalog.jsonb_build_array(pg_catalog.jsonb_build_object(
                        'name', $10::pg_catalog.text,
                        'type', pg_catalog.format_type($11::pg_catalog.oid, NULL)))),
                $12::pg_catalog.oid[]::pg_catalog.regclass[], $13,
                $14, $15::pg_catalog.oid[]::pg_catalog.regclass[], $16, $17)
            {on_conflict}",
            crate::utils::meta_table()
        );

        let declarations = &self.uncascaded.declarations;
        let group_keys = self
            .group_keys
            .map(serde_json::to_value)
            .transpose()?
            .map(pgrx::JsonB);
        let (function_read_functions, function_read_tables) = declarations.function_read_pairs();
        let (table_oids, table_policies): (Vec<pg_sys::Oid>, Vec<String>) = declarations
            .tables
            .iter()
            .map(|(oid, policy)| (*oid, policy.as_str().to_string()))
            .unzip();
        let args = [
            crate::utils::spi::text(self.entity),
            crate::utils::spi::oid(self.view_oid),
            crate::utils::spi::oid(self.table_oid),
            crate::utils::spi::text(self.definition),
            crate::utils::spi::jsonb(pgrx::JsonB(serde_json::to_value(self.plan)?)),
            crate::utils::spi::jsonb(group_keys),
            crate::utils::spi::oid_array(self.uncascaded.oids()),
            crate::utils::spi::text(declarations.policy.as_str()),
            crate::utils::spi::text(self.identity.kind.name()),
            crate::utils::spi::text(self.identity.name.as_str()),
            crate::utils::spi::oid(pg_sys::Oid::from(self.identity.type_oid)),
            crate::utils::spi::oid_array(table_oids),
            crate::utils::spi::text_array(table_policies),
            crate::utils::spi::text_array(function_read_functions),
            crate::utils::spi::oid_array(function_read_tables),
            crate::utils::spi::text(self.uncascaded.time_refresh()),
            crate::utils::spi::boolean(self.uncascaded.time_dependent),
        ];
        // The catalog is written as the extension's owner; the caller's right to
        // change this TVIEW was checked before.
        let owner = crate::owner::AsOwner::of_extension()?;
        Spi::run_with_args(&sql, &args).map_err(|e| TViewError::SpiError {
            query: sql,
            error: e.to_string(),
        })?;
        drop(owner);

        // TVIEWs that read each other in a cycle could never be refreshed in
        // order: refuse the definition that closes one.
        crate::flush::EntityDepGraph::load()?;
        Ok(())
    }
}

/// Every column `plan` looks up an embedded TVIEW's rows by.
fn embed_lookups(plan: &TviewPlan) -> Vec<String> {
    plan.embeds
        .iter()
        .flat_map(|e| e.lookups.iter().cloned())
        .collect()
}

/// The indexes on `table` that are exactly those `pg_tviews` creates for the
/// TVIEW under their names: what a TVIEW registered before its indexes were
/// recorded takes as `pg_tviews`' when the upgrade re-registers it.
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
        }
    }
    Ok(adopted)
}
