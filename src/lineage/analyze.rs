//! The analysis of a registered or new TVIEW: its lineage, embeds and key mappings.

use super::{
    DELTA, DataEmbed, DataShape, Identity, IdentityKind, TableKind, TableLineage, escape_template,
    identity_of, render_template, walk,
};
use std::collections::{BTreeMap, BTreeSet};

/// The lineage of one TVIEW: every base table and how its writes map to keys.
#[derive(Debug, Clone)]
pub struct Lineage {
    pub tables: Vec<TableLineage>,
    /// Tables the view reads only where its output cannot depend on them.
    pub unread: Vec<u32>,
    pub identity: Identity,
    /// The backing view's own SELECT is a set operation (UNION, INTERSECT,
    /// EXCEPT), or its key comes from the branches of one: its rows are
    /// recomputed, never patched.
    pub set_operation: bool,
    /// The aggregate TVIEWs the view reads, each with the output column
    /// equal to its key, if any: no `fk_<aggregate>` column propagates a
    /// change of one of its groups, this column does.
    pub aggregate_embeds: Vec<(String, Option<String>)>,
    /// The functions it calls that may read tables it cannot see (not immutable,
    /// outside `pg_catalog`), as `(oid, schema.name(argument types))`.
    pub functions: Vec<(u32, String)>,
    /// How it reads the current time: its rows change with no write.
    pub time_reads: Vec<String>,
    /// Every other TVIEW it reads, with the output columns equal to that TVIEW's
    /// key (none when no output carries it).
    pub tview_reads: BTreeMap<String, Vec<String>>,
    /// The TVIEWs it reads whose rows are named by a column other than
    /// `pk_<entity>` (DISTINCT ON): the key it joins on is not theirs.
    pub keyed_otherwise: BTreeSet<String>,
    /// The shape of its `data` output.
    pub data: Option<DataShape>,
}

/// How a TVIEW embeds another one's rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum EmbedKind {
    /// It reads some of the child's columns, not its document.
    #[serde(rename = "scalar")]
    Scalar,
    /// The child's `data` is a value of its own `data`.
    #[serde(rename = "nested_object")]
    Nested,
    /// The children's `data` are aggregated into an array.
    #[serde(rename = "array")]
    Array,
}

/// Another TVIEW this one embeds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Embed {
    pub entity: String,
    /// The output columns of this TVIEW equal to the child's key: one per read
    /// of the child.
    pub lookups: Vec<String>,
    pub kind: EmbedKind,
    /// Where its document lands in `data` (empty for a scalar embed).
    pub path: Vec<String>,
}

impl Lineage {
    /// The TVIEWs this one embeds, in entity order.
    #[must_use]
    pub fn embeds(&self, entity: &str) -> Vec<Embed> {
        let placed = self.data.iter().flat_map(|d| &d.embeds);
        self.tview_reads
            .iter()
            .filter(|(child, _)| child.as_str() != entity)
            .filter(|(_, lookups)| !lookups.is_empty())
            .map(|(child, lookups)| {
                // The parent joins on the child's pk_<child>, which is not the key
                // of a DISTINCT ON child: its document is read, not followed.
                let placements: Vec<&DataEmbed> = if self.keyed_otherwise.contains(child) {
                    Vec::new()
                } else {
                    placed.clone().filter(|e| &e.entity == child).collect()
                };
                let kind = if placements.iter().any(|e| e.array) {
                    EmbedKind::Array
                } else if placements.is_empty() {
                    EmbedKind::Scalar
                } else {
                    EmbedKind::Nested
                };
                // A document placed at two paths has no single path to patch.
                let path = match placements.as_slice() {
                    [one] => one.path.clone(),
                    _ => Vec::new(),
                };
                Embed {
                    entity: child.clone(),
                    lookups: lookups.clone(),
                    kind,
                    path,
                }
            })
            .collect()
    }

    /// The direct-patch map: `(column, data key)` for each column of the table
    /// holding the identity that `data` copies under a top-level key and that the
    /// definition reads nowhere else. Never a virtual generated column or an input
    /// of one: the trigger sees it NULL. Empty for a set operation and a
    /// DISTINCT ON TVIEW, whose rows are recomputed.
    #[must_use]
    pub fn direct_fields(&self) -> Vec<(String, String)> {
        let Some(data) = &self.data else {
            return Vec::new();
        };
        if self.set_operation || self.identity.kind == IdentityKind::DistinctOn {
            return Vec::new();
        }
        let Some(&(root, _)) = self.identity.columns.first() else {
            return Vec::new();
        };
        let virtual_reads: Vec<&String> = self
            .tables
            .iter()
            .filter(|t| t.relid == root)
            .flat_map(|t| &t.virtual_reads)
            .collect();
        data.fields
            .iter()
            .filter(|f| f.root && f.only_in_data && f.path.len() == 1)
            .filter(|f| !virtual_reads.contains(&&f.column.name))
            .filter(|f| {
                data.fields
                    .iter()
                    .filter(|g| g.column == f.column)
                    .all(|g| g.path.len() == 1 && g.only_in_data)
            })
            .map(|f| (f.column.name.clone(), f.path[0].clone()))
            .collect()
    }
}

/// A table a function reads, declared with the TVIEW.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionRead {
    /// The function, `schema.name(argument types)`.
    pub function: String,
    pub relid: u32,
    pub relname: String,
    pub qualified: String,
    pub matview: bool,
    /// The table of another TVIEW, of that entity.
    pub tview: Option<String>,
}

impl Lineage {
    /// Add the tables functions read: no cascade reaches them, so each is
    /// `all_keys`, read inside its function. A table the view also reads keeps
    /// mapping the reads of it that can be traced.
    pub fn add_function_reads(&mut self, reads: &[FunctionRead]) {
        for read in reads {
            let reason = format!("read inside {}", read.function);
            match self.tables.iter_mut().find(|t| t.relid == read.relid) {
                Some(table) => {
                    let (kind, sql) = match (&table.kind, table.sql.take()) {
                        (TableKind::AllKeys(r), sql) => (format!("{r}; {reason}"), sql),
                        (TableKind::Local(column), _) => (
                            reason,
                            Some(format!(
                                "SELECT DISTINCT {} FROM {DELTA}",
                                escape_template(&crate::utils::quote_identifier(column))
                            )),
                        ),
                        (TableKind::Mapped, sql) => (reason, sql),
                        (TableKind::Propagated(_), _) => (reason, None),
                    };
                    table.kind = TableKind::AllKeys(kind);
                    table.sql = sql;
                    table.hop = None;
                    table.fanout = None;
                }
                None => self.tables.push(TableLineage {
                    relid: read.relid,
                    relname: read.relname.clone(),
                    qualified: read.qualified.clone(),
                    kind: TableKind::AllKeys(reason),
                    paths: Vec::new(),
                    sql: None,
                    columns: Vec::new(),
                    lookups: Vec::new(),
                    index_hints: Vec::new(),
                    hop: None,
                    fanout: None,
                    root: false,
                    virtual_reads: Vec::new(),
                    matview: read.matview,
                    tview: read.tview.clone(),
                }),
            }
        }
    }

    /// The tables no cascade reaches (`all_keys`), with the reason, which says when
    /// some reads of the table still refresh the rows they reach.
    #[must_use]
    pub fn all_keys(&self) -> Vec<(u32, String, String)> {
        self.tables
            .iter()
            .filter_map(|t| match &t.kind {
                TableKind::AllKeys(reason) => Some((
                    t.relid,
                    t.qualified.clone(),
                    if t.sql.is_some() {
                        format!("{reason}; the rows its other reads reach are still refreshed")
                    } else {
                        reason.clone()
                    },
                )),
                _ => None,
            })
            .collect()
    }

    /// `pg_tview_meta.key_mappings`: one object per base table.
    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::Value::Array(
            self.tables
                .iter()
                .map(|t| {
                    let mut entry = serde_json::json!({
                        "table": t.qualified,
                        "relid": t.relid,
                        "kind": t.kind.name(),
                    });
                    match &t.kind {
                        TableKind::Local(column) => entry["column"] = column.clone().into(),
                        TableKind::Propagated(entity) => entry["entity"] = entity.clone().into(),
                        TableKind::AllKeys(reason) => {
                            entry["reason"] = reason.clone().into();
                            if let Some(sql) = &t.sql {
                                entry["sql"] = sql.clone().into();
                            }
                        }
                        TableKind::Mapped => {
                            entry["sql"] = t.sql.clone().unwrap_or_default().into();
                            if let Some((own, root)) = &t.hop {
                                entry["hop"] = serde_json::json!([own, root]);
                            }
                        }
                    }
                    if let Some(inner) = &t.tview {
                        entry["tview"] = inner.clone().into();
                    }
                    entry["columns"] = t
                        .columns
                        .iter()
                        .map(|(name, _)| name.clone())
                        .collect::<Vec<_>>()
                        .into();
                    entry["attnums"] = t.columns.iter().map(|(_, n)| *n).collect::<Vec<_>>().into();
                    entry
                })
                .collect(),
        )
    }
}

/// `pg_depend` and the query tree must agree on the base tables `entity` reads.
fn check_tables_agree(
    entity: &str,
    graph: &super::QueryGraph,
    base_tables: &[pgrx::pg_sys::Oid],
) -> crate::TViewResult<()> {
    use std::collections::HashSet;
    let found: HashSet<u32> = graph
        .occurrences
        .iter()
        .filter(|o| o.tview_table.is_none())
        .map(|o| o.relid)
        .chain(graph.unread_tables.iter().copied())
        .collect();
    let expected: HashSet<u32> = base_tables.iter().map(|o| o.to_u32()).collect();
    if found == expected {
        return Ok(());
    }
    let name = |relid: &u32| {
        crate::utils::qualified_relname_from_oid(relid.to_owned().into())
            .unwrap_or_else(|_| relid.to_string())
    };
    let missing: Vec<String> = expected.difference(&found).map(name).collect();
    let extra: Vec<String> = found.difference(&expected).map(name).collect();
    Err(crate::TViewError::DefinitionRefused {
        reason: format!(
            "pg_tviews could not follow how tv_{entity} reads its base tables \
             (not found in the view's query: [{}]; not in pg_depend: [{}])",
            missing.join(", "),
            extra.join(", ")
        ),
    })
}

/// Analyze the backing view `view_oid` of `entity`.
///
/// The TVIEWs it embeds are the ones it reads with an output column equal to
/// their key, found by the walk. `base_tables` is what `pg_depend` says the view
/// reads, which the analysis must find exactly.
///
/// # Errors
/// Returns an error if the view cannot be analyzed, or if the analysis and
/// `pg_depend` disagree on the tables the view reads.
pub fn analyze(
    entity: &str,
    view_oid: pgrx::pg_sys::Oid,
    base_tables: &[pgrx::pg_sys::Oid],
) -> crate::TViewResult<Lineage> {
    use std::collections::{HashMap, HashSet};

    let catalog = |e: pgrx::spi::Error| crate::TViewError::CatalogError {
        operation: format!("Read the TVIEW catalog to analyze tv_{entity}"),
        pg_error: e.to_string(),
    };
    // Every other registered TVIEW: its table, its view, what it maps.
    let mut tview_tables: HashMap<pgrx::pg_sys::Oid, String> = HashMap::new();
    let mut tview_views: HashMap<pgrx::pg_sys::Oid, String> = HashMap::new();
    // Per other TVIEW: the tables it maps, and those it refreshes in full.
    let mut mapped_by: HashMap<String, (HashSet<u32>, HashSet<u32>)> = HashMap::new();
    let mut aggregates: Vec<String> = Vec::new();
    let mut keyed_otherwise: BTreeSet<String> = BTreeSet::new();
    for other in crate::catalog::registered::all()? {
        if other.keyed_otherwise {
            keyed_otherwise.insert(other.entity.clone());
        }
        tview_tables.insert(other.table_oid, other.entity.clone());
        if other.entity == entity {
            continue;
        }
        if other.aggregate {
            aggregates.push(other.entity.clone());
        }
        tview_views.insert(other.view_oid, other.entity.clone());
        mapped_by.insert(other.entity, (other.mapped, other.full_refresh));
    }

    let key_column = format!("pk_{entity}");
    let graph = walk::analyze(
        view_oid,
        &walk::Context {
            tview_tables: &tview_tables,
            tview_views: &tview_views,
            entity,
            key_column: &key_column,
        },
    )?;

    crate::utils::log_debug!("lineage of tv_{entity}: {graph:?}");
    let identity = identity_of(entity, view_oid, &graph)?;
    check_tables_agree(entity, &graph, base_tables)?;

    let functions = function_signatures(&graph.untracked_functions).map_err(catalog)?;

    // The aggregate TVIEWs the view reads embed through the output equal to their
    // key.
    let tview_reads = graph.embed_lookups();
    let mut lookups = tview_reads.clone();
    let aggregate_embeds: Vec<(String, Option<String>)> = aggregates
        .into_iter()
        .filter_map(|a| {
            lookups
                .remove(&a)
                .map(|columns| (a, columns.first().cloned()))
        })
        .collect();
    // A TVIEW read with an output column equal to its key is embedded: a refresh
    // of its rows finds the parents by that column.
    let embeds: Vec<&str> = tview_reads
        .iter()
        .filter(|(child, lookups)| !lookups.is_empty() && child.as_str() != entity)
        .map(|(child, _)| child.as_str())
        .collect();
    // Propagation from an embedded TVIEW covers a table only if that TVIEW maps it.
    // Or, for a read of its table, only that it embeds it.
    let propagates = |child: &str, relid: u32| {
        embeds.contains(&child)
            && (tview_tables
                .get(&pgrx::pg_sys::Oid::from(relid))
                .map(String::as_str)
                == Some(child)
                || mapped_by
                    .get(child)
                    .is_some_and(|(mapped, full)| full.contains(&relid) || mapped.contains(&relid)))
    };
    let mut tables = graph.tables(&propagates);
    for table in &mut tables {
        let virtual_columns = virtual_inputs(table.relid)?;
        table.columns =
            expand_read_columns(referenced_columns(view_oid, table.relid)?, &virtual_columns);
        table.virtual_reads = virtual_reads(&table.columns, &virtual_columns);
        if let Some(sql) = &table.sql {
            explain(entity, table, sql)?;
        }
    }
    let unread = graph
        .unread_tables
        .iter()
        .copied()
        .filter(|relid| tables.iter().all(|t| t.relid != *relid))
        .collect();
    Ok(Lineage {
        tables,
        unread,
        identity,
        // A UNION read through a view or subquery gives the key one root per
        // branch too: its rows are recomputed, and two rows for one key refused.
        set_operation: graph.set_operation || graph.roots.len() > 1,
        aggregate_embeds,
        functions,
        time_reads: graph.time_reads.clone(),
        tview_reads,
        keyed_otherwise,
        data: graph.data,
    })
}

/// `(oid, schema.name(argument types))` of each function.
pub(super) fn function_signatures(oids: &[u32]) -> Result<Vec<(u32, String)>, pgrx::spi::Error> {
    use pgrx::prelude::*;
    if oids.is_empty() {
        return Ok(Vec::new());
    }
    let oids: Vec<pgrx::pg_sys::Oid> = oids.iter().map(|&o| o.into()).collect();
    Spi::connect(|client| {
        let mut functions = Vec::new();
        for row in client.select(
            &format!("SELECT oid, {FUNCTION_SIGNATURE} FROM pg_catalog.pg_proc p WHERE p.oid = ANY ($1) ORDER BY 2"),
            None,
            &[crate::utils::spi::oid_array(oids)],
        )? {
            if let (Some(oid), Some(signature)) =
                (row.get::<pgrx::pg_sys::Oid>(1)?, row.get::<String>(2)?)
            {
                functions.push((oid.to_u32(), signature));
            }
        }
        Ok(functions)
    })
}

/// The signature of function `p` (a `pg_proc` row) as the `function_reads` option
/// and `tviews.registry` write it: `schema.name(argument types)`, quoted where SQL
/// needs it.
pub const FUNCTION_SIGNATURE: &str = "pg_catalog.format('%s.%s(%s)', \
     (SELECT pg_catalog.quote_ident(n.nspname::pg_catalog.text) FROM pg_catalog.pg_namespace n \
      WHERE n.oid = p.pronamespace), \
     pg_catalog.quote_ident(p.proname::pg_catalog.text), \
     pg_catalog.oidvectortypes(p.proargtypes))";

/// How writes to a table map to a registered TVIEW's keys: the stored form of a
/// [`TableKind`]. Any other name fails to decode.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MappingKind {
    /// The key is a column of the changed row (row trigger).
    #[default]
    Local,
    /// A mapping query over the transition table (`sql`).
    Mapped,
    /// Reached only through an embed: no trigger.
    Propagated,
    /// No cascade maps the table: the policy applies.
    AllKeys,
}

/// One table of a registered TVIEW's `key_mappings`.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct KeyMapping {
    pub relid: u32,
    /// The table's qualified name: what a restore rebinds `relid` from.
    pub table: String,
    pub kind: MappingKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity: Option<String>,
    /// `all_keys`: why no cascade maps the table.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// `mapped` (and `all_keys` for its traceable reads): the query template (see
    /// [`render_template`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sql: Option<String>,
    /// Columns of the table the TVIEW reads; empty when unknown.
    #[serde(default)]
    pub attnums: Vec<i16>,
    #[serde(default)]
    pub columns: Vec<String>,
    /// `mapped` through one equality onto a column of the root: `(this table's
    /// column, the root's column)`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hop: Option<(String, String)>,
    /// The column of this table whose value the fan-out patch looks up.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_col: Option<String>,
    /// How an UPDATE is written into every TVIEW row it reaches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fanout: Option<crate::catalog::plan::FanoutPatch>,
    /// The table of another TVIEW, of that entity: refreshed first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tview: Option<String>,
}

/// Rows above which a sequential scan in a mapping query is worth an index.
pub(super) const LARGE_TABLE_ROWS: f64 = 1000.0;

/// Plan the mapping query of `table` and say which index would avoid a
/// sequential scan of a large table. Fails when the query does not plan: a
/// mapping `pg_tviews` cannot run must not be registered.
pub(super) fn explain(
    entity: &str,
    table: &TableLineage,
    template: &str,
) -> crate::TViewResult<()> {
    use pgrx::prelude::*;
    let sql = render_template(template)
        .map_err(|e| crate::TViewError::CatalogError {
            operation: format!("Name the relations of the mapping of {}", table.qualified),
            pg_error: e.to_string(),
        })?
        .ok_or_else(|| crate::TViewError::CatalogError {
            operation: format!("Name the relations of the mapping of {}", table.qualified),
            pg_error: "a relation or column it reads is gone".to_string(),
        })?;
    let explain = format!(
        "EXPLAIN (FORMAT JSON) WITH {DELTA} AS (SELECT * FROM {} LIMIT 0) {sql}",
        table.qualified
    );
    let plan = Spi::get_one::<pgrx::Json>(&explain)
        .map_err(|e| crate::TViewError::SpiError {
            query: explain.clone(),
            error: e.to_string(),
        })?
        .map(|j| j.0)
        .unwrap_or_default();
    let mut scans = Vec::new();
    seq_scans(&plan, &mut scans);
    // Once per relation: the changed rows of a self-join scan it too.
    scans.sort_by(|x, y| x.0.cmp(&y.0).then(y.1.total_cmp(&x.1)));
    scans.dedup_by(|x, y| x.0 == y.0);
    for (relation, rows) in scans {
        if rows < LARGE_TABLE_ROWS {
            continue;
        }
        let mut advice: Vec<String> = table
            .lookups
            .iter()
            .filter(|(t, _)| t.rsplit('.').next() == Some(relation.as_str()))
            .map(|(_, columns)| format!("an index on {relation} ({})", columns.join(", ")))
            .collect();
        for hint in table.index_hints.iter().filter(|h| h.relname == relation) {
            let expr = render_template(&hint.expr)
                .ok()
                .flatten()
                .unwrap_or_else(|| hint.expr.clone());
            advice.push(format!(
                "CREATE INDEX ON {} USING {} (({expr}))",
                hint.table,
                if hint.gin { "gin" } else { "btree" }
            ));
        }
        if advice.is_empty() {
            continue;
        }
        notice!(
            "writes to {} map to tv_{entity} keys with a sequential scan of {} (about {rows} rows); \
             {} would make them cheaper",
            table.qualified,
            relation,
            advice.join(" or ")
        );
    }
    Ok(())
}

/// `(relation, estimated rows)` of every sequential scan in an EXPLAIN JSON plan.
pub(super) fn seq_scans(node: &serde_json::Value, out: &mut Vec<(String, f64)>) {
    match node {
        serde_json::Value::Array(items) => items.iter().for_each(|i| seq_scans(i, out)),
        serde_json::Value::Object(map) => {
            if map.get("Node Type").and_then(|t| t.as_str()) == Some("Seq Scan")
                && let Some(relation) = map.get("Relation Name").and_then(|r| r.as_str())
            {
                let rows = map
                    .get("Plan Rows")
                    .and_then(serde_json::Value::as_f64)
                    .unwrap_or(0.0);
                out.push((relation.to_string(), rows));
            }
            map.values().for_each(|v| seq_scans(v, out));
        }
        _ => {}
    }
}

/// A virtual generated column (`attnum`) and the columns its expression reads.
pub type VirtualInputs = Vec<(i16, Vec<(String, i16)>)>;

/// The columns a TVIEW reads of a table, `read`, with the inputs of every virtual
/// generated column among them: a virtual column has no value in the rows a
/// trigger sees, so a change to it is a change to its inputs. Sorted by attnum.
#[must_use]
pub fn expand_read_columns(
    mut read: Vec<(String, i16)>,
    virtual_inputs: &[(i16, Vec<(String, i16)>)],
) -> Vec<(String, i16)> {
    let virtual_read: Vec<i16> = read
        .iter()
        .map(|(_, attnum)| *attnum)
        .filter(|attnum| virtual_inputs.iter().any(|(v, _)| v == attnum))
        .collect();
    for (virtual_column, inputs) in virtual_inputs {
        if virtual_read.contains(virtual_column) {
            read.extend(inputs.iter().cloned());
        }
    }
    read.sort_by_key(|(_, attnum)| *attnum);
    read.dedup_by_key(|(_, attnum)| *attnum);
    read
}

/// The names of the virtual generated columns among `read` and of their inputs.
#[must_use]
pub fn virtual_reads(
    read: &[(String, i16)],
    virtual_inputs: &[(i16, Vec<(String, i16)>)],
) -> Vec<String> {
    let mut names = Vec::new();
    for (virtual_column, inputs) in virtual_inputs {
        if let Some((name, _)) = read.iter().find(|(_, attnum)| attnum == virtual_column) {
            names.push(name.clone());
            names.extend(inputs.iter().map(|(input, _)| input.clone()));
        }
    }
    names
}

/// The virtual generated columns of table `relid` and their inputs: the columns
/// the dependencies of their `pg_attrdef` entries name. Empty before
/// PostgreSQL 18, which has no virtual generated columns.
///
/// # Errors
/// Returns an error if the catalog cannot be read.
pub fn virtual_inputs(relid: u32) -> crate::TViewResult<VirtualInputs> {
    use pgrx::prelude::*;
    Spi::connect(|client| {
        let mut out: VirtualInputs = Vec::new();
        for row in client.select(
            "SELECT g.attnum, i.attname::pg_catalog.text, i.attnum \
             FROM pg_catalog.pg_attribute g \
             JOIN pg_catalog.pg_attrdef ad ON ad.adrelid = g.attrelid AND ad.adnum = g.attnum \
             JOIN pg_catalog.pg_depend d \
               ON d.classid = 'pg_catalog.pg_attrdef'::pg_catalog.regclass AND d.objid = ad.oid \
              AND d.refclassid = 'pg_catalog.pg_class'::pg_catalog.regclass \
              AND d.refobjid = g.attrelid AND d.refobjsubid > 0 AND d.refobjsubid <> g.attnum \
             JOIN pg_catalog.pg_attribute i ON i.attrelid = g.attrelid AND i.attnum = d.refobjsubid \
             WHERE g.attrelid = $1 AND g.attgenerated = 'v' AND NOT i.attisdropped \
             ORDER BY 1, 3",
            None,
            &[crate::utils::spi::oid(pgrx::pg_sys::Oid::from(relid))],
        )? {
            let (Some(column), Some(name), Some(input)) =
                (row.get::<i16>(1)?, row.get::<String>(2)?, row.get::<i16>(3)?)
            else {
                continue;
            };
            match out.iter_mut().find(|(c, _)| *c == column) {
                Some((_, inputs)) => inputs.push((name, input)),
                None => out.push((column, vec![(name, input)])),
            }
        }
        Ok(out)
    })
}

/// The columns of table `relid` that the view `view_oid`, or a view it reads,
/// references (`pg_depend`).
pub(super) fn referenced_columns(
    view_oid: pgrx::pg_sys::Oid,
    relid: u32,
) -> crate::TViewResult<Vec<(String, i16)>> {
    crate::catalog::reads::view_columns_read(view_oid, pgrx::pg_sys::Oid::from(relid))
}
