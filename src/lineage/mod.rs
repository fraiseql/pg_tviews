//! Lineage of a TVIEW: how a write to each base table its backing view reads
//! maps to TVIEW keys (ADR 0157).
//!
//! [`walk`] reads PostgreSQL's analyzed query of the backing view (views, CTEs and
//! subqueries expanded) into a small graph: every occurrence of a base table, the
//! TVIEW key, and the predicates that link two occurrences. It is the only code
//! that touches `pg_sys` nodes. Everything else here is plain Rust over that graph:
//! each table is classified as
//!
//! - `Local(col)`: the key is a column of the changed row (the root table, a table
//!   joined on `col = <key>`); the row trigger reads it;
//! - `Mapped`: a chain of predicates links the table to the key; a query over the
//!   changed rows returns the keys;
//! - `Propagated(entity)`: read through the backing view of a TVIEW this one embeds
//!   (`fk_<entity>`): entity propagation refreshes the rows that embed it;
//! - `AllKeys`: nothing selective links it to the key; `pg_tviews.uncascaded_policy`
//!   decides.
//!
//! A predicate is used only as a *necessary* condition for a changed row to
//! contribute to a TVIEW row, so leaving one out only widens the mapping. Only
//! strict, immutable predicates over two table occurrences are kept, and only in the
//! directions in which they must hold: both ways for `WHERE` and inner-join
//! conditions, from the nullable side for an outer join, and from a subquery to the
//! query around it.

pub mod walk;

use std::collections::{BTreeMap, VecDeque};

/// A piece of generated SQL: text, or the alias of a table occurrence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Piece {
    Text(String),
    Alias(usize),
}

/// SQL with holes for occurrence aliases, filled in when a query is assembled.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Sql(pub Vec<Piece>);

impl Sql {
    pub fn text(s: impl Into<String>) -> Self {
        Self(vec![Piece::Text(s.into())])
    }

    pub fn push_text(&mut self, s: &str) {
        if let Some(Piece::Text(last)) = self.0.last_mut() {
            last.push_str(s);
        } else {
            self.0.push(Piece::Text(s.to_string()));
        }
    }

    pub fn push_sql(&mut self, other: Self) {
        for piece in other.0 {
            match piece {
                Piece::Text(t) => self.push_text(&t),
                alias @ Piece::Alias(_) => self.0.push(alias),
            }
        }
    }

    /// Render with `alias(occurrence)` for each hole.
    #[must_use]
    pub fn render(&self, alias: &dyn Fn(usize) -> String) -> String {
        self.0
            .iter()
            .map(|p| match p {
                Piece::Text(t) => t.clone(),
                Piece::Alias(occ) => alias(*occ),
            })
            .collect()
    }
}

/// The relation a mapping query reads the changed rows from: the old and new
/// images of the statement's rows (a CTE over the transition tables), or a table of
/// that name in tests.
pub const DELTA: &str = "pg_tviews_delta";

/// A column of a table occurrence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Column {
    pub occ: usize,
    pub attnum: i16,
    pub name: String,
}

impl Column {
    /// `<alias>."<name>"`.
    #[must_use]
    pub fn sql(&self) -> Sql {
        Sql(vec![
            Piece::Alias(self.occ),
            Piece::Text(format!(".{}", crate::utils::quote_identifier(&self.name))),
        ])
    }
}

/// One reference to a base table somewhere in the backing view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Occurrence {
    pub relid: u32,
    pub relname: String,
    /// Schema-qualified, quoted.
    pub qualified: String,
    /// Top-level UNION branch the occurrence belongs to (0 without UNION).
    pub branch: usize,
    /// First plain view (not a TVIEW's backing view) on the way to it.
    pub via_view: Option<String>,
    /// The TVIEW whose backing view it was read through, if any.
    pub via_tview: Option<String>,
    /// Read inside a subquery expression (`(SELECT …)`, `EXISTS`, `IN`, `ARRAY(…)`).
    pub in_sublink: bool,
    /// Why nothing passes through the query level it sits in, if so (window
    /// function, LIMIT, …): its columns are not visible outside that level.
    pub opaque_level: Option<String>,
}

/// A predicate linking two occurrences, `a` and `b`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conjunct {
    pub sql: Sql,
    pub a: usize,
    pub b: usize,
    /// It must hold for a contributing row of `a`, so it maps `a` toward `b`.
    pub a_to_b: bool,
    pub b_to_a: bool,
    /// Set when the predicate is `a.col = b.col` with `=`.
    pub equality: Option<(Column, Column)>,
}

/// The TVIEW key in one UNION branch: a column of the root occurrence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Root {
    pub branch: usize,
    pub key: Column,
}

/// What [`walk`] reads from the backing view.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Graph {
    pub occurrences: Vec<Occurrence>,
    pub conjuncts: Vec<Conjunct>,
    pub roots: Vec<Root>,
    /// Functions the view calls that may read tables `pg_tviews` does not see.
    pub untracked_functions: Vec<String>,
}

/// How a write to one occurrence maps to keys.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    Local(String),
    /// The predicates from the occurrence to the root, in order.
    Mapped(Vec<usize>),
    Propagated(String),
    AllKeys(String),
}

/// How a write to a whole table maps to keys: the kinds of its occurrences combined.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableLineage {
    pub relid: u32,
    pub relname: String,
    pub qualified: String,
    pub kind: TableKind,
    /// Mapped occurrences, each with its predicate chain (for the mapping query).
    pub paths: Vec<(usize, Vec<usize>)>,
    /// `mapped`: the query over [`DELTA`] returning the keys its rows can affect.
    pub sql: Option<String>,
    /// Columns of the table the TVIEW reads; empty when unknown.
    pub columns: Vec<String>,
    /// Tables the mapping query joins, with the columns it looks up in each.
    pub lookups: Vec<(String, Vec<String>)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TableKind {
    Local(String),
    Mapped,
    Propagated(String),
    AllKeys(String),
}

impl TableKind {
    /// The name `tviews.registry.cascade_kinds` reports.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Local(_) => "local",
            Self::Mapped => "mapped",
            Self::Propagated(_) => "propagated",
            Self::AllKeys(_) => "all_keys",
        }
    }
}

impl Graph {
    fn root_of(&self, occ: usize) -> Option<&Root> {
        let branch = self.occurrences[occ].branch;
        self.roots.iter().find(|r| r.branch == branch)
    }

    /// Classify one occurrence. `propagates(entity, relid)` tells whether entity
    /// propagation from that TVIEW covers a write to `relid` read through its view:
    /// this TVIEW embeds it, and it maps the table itself.
    #[must_use]
    pub fn classify(&self, occ: usize, propagates: &dyn Fn(&str, u32) -> bool) -> Kind {
        let o = &self.occurrences[occ];
        let Some(root) = self.root_of(occ) else {
            return Kind::AllKeys("the TVIEW key is not a column of a base table".to_string());
        };
        if root.key.occ == occ {
            return Kind::Local(root.key.name.clone());
        }
        if let Some(entity) = &o.via_tview
            && propagates(entity, o.relid)
        {
            return Kind::Propagated(entity.clone());
        }
        match self.path(occ, root.key.occ) {
            Some(path) => match self.local_column(&path, root) {
                Some(col) => Kind::Local(col),
                None => Kind::Mapped(path),
            },
            None => Kind::AllKeys(self.unlinked_reason(occ)),
        }
    }

    /// The shortest chain of usable predicates from `from` to `to`.
    fn path(&self, from: usize, to: usize) -> Option<Vec<usize>> {
        let mut previous: BTreeMap<usize, (usize, usize)> = BTreeMap::new();
        let mut queue = VecDeque::from([from]);
        while let Some(at) = queue.pop_front() {
            if at == to {
                let mut path = Vec::new();
                let mut cur = to;
                while cur != from {
                    let (prev, conjunct) = previous[&cur];
                    path.push(conjunct);
                    cur = prev;
                }
                path.reverse();
                return Some(path);
            }
            // Equalities first: `l.fk = o.pk` makes `l` local even when `l.pos > o.min`
            // links the same two occurrences.
            let mut order: Vec<usize> = (0..self.conjuncts.len()).collect();
            order.sort_by_key(|&i| self.conjuncts[i].equality.is_none());
            for i in order {
                let c = &self.conjuncts[i];
                let next = if c.a == at && c.a_to_b {
                    c.b
                } else if c.b == at && c.b_to_a {
                    c.a
                } else {
                    continue;
                };
                if next != from && !previous.contains_key(&next) {
                    previous.insert(next, (at, i));
                    queue.push_back(next);
                }
            }
        }
        None
    }

    /// The column of the occurrence equal to the key, when the chain is that single
    /// equality: the key is then read off the changed row.
    fn local_column(&self, path: &[usize], root: &Root) -> Option<String> {
        let [only] = path else { return None };
        let (x, y) = self.conjuncts[*only].equality.as_ref()?;
        if *y == root.key {
            Some(x.name.clone())
        } else if *x == root.key {
            Some(y.name.clone())
        } else {
            None
        }
    }

    fn unlinked_reason(&self, occ: usize) -> String {
        let o = &self.occurrences[occ];
        let mut how = Vec::new();
        if let Some(why) = &o.opaque_level {
            how.push(why.clone());
        }
        if o.in_sublink {
            how.push("read in a subquery".to_string());
        }
        if let Some(view) = &o.via_view {
            how.push(format!("read through view {view}"));
        }
        let how = if how.is_empty() {
            String::new()
        } else {
            format!("{}, ", how.join(", "))
        };
        format!("{how}with no condition linking it to the TVIEW key")
    }

    /// The mapping query of a table: the keys its changed rows ([`DELTA`]) can
    /// affect, one `SELECT DISTINCT` per occurrence, combined with `UNION`.
    #[must_use]
    pub fn mapping_sql(&self, paths: &[(usize, Vec<usize>)]) -> String {
        let mut queries: Vec<String> = paths
            .iter()
            .filter_map(|(occ, path)| self.path_sql(*occ, path))
            .collect();
        queries.dedup();
        queries.join(" UNION ")
    }

    /// For each table a mapping query joins (not the changed one), the columns its
    /// conditions look up: what an index should cover.
    #[must_use]
    pub fn lookups(&self, paths: &[(usize, Vec<usize>)]) -> Vec<(String, Vec<String>)> {
        let mut lookups: Vec<(String, Vec<String>)> = Vec::new();
        for (occ, path) in paths {
            for &i in self.kept_conditions(*occ, path) {
                let Some((x, y)) = &self.conjuncts[i].equality else {
                    continue;
                };
                for column in [x, y] {
                    if column.occ == *occ {
                        continue;
                    }
                    let table = &self.occurrences[column.occ].qualified;
                    match lookups.iter_mut().find(|(t, _)| t == table) {
                        Some((_, cols)) if !cols.contains(&column.name) => {
                            cols.push(column.name.clone());
                        }
                        Some(_) => {}
                        None => lookups.push((table.clone(), vec![column.name.clone()])),
                    }
                }
            }
        }
        lookups
    }

    /// The conditions of `path` its mapping query keeps: all but a last one that
    /// only copies the key (the root is then left out).
    fn kept_conditions<'p>(&self, occ: usize, path: &'p [usize]) -> &'p [usize] {
        match (self.root_of(occ), path.split_last()) {
            (Some(root), Some((&last, rest)))
                if self.conjuncts[last]
                    .equality
                    .as_ref()
                    .is_some_and(|(x, y)| *x == root.key || *y == root.key) =>
            {
                rest
            }
            _ => path,
        }
    }

    /// `SELECT DISTINCT <key> FROM <delta>, <tables on the path> WHERE <conditions>`.
    /// The root is left out when the last condition copies its key verbatim.
    fn path_sql(&self, occ: usize, path: &[usize]) -> Option<String> {
        let root = self.root_of(occ)?;
        // The occurrences in path order, starting at the changed table.
        let mut chain = vec![occ];
        for &i in path {
            let c = &self.conjuncts[i];
            let at = *chain.last()?;
            chain.push(if c.a == at { c.b } else { c.a });
        }
        let mut conditions: Vec<&Conjunct> = path.iter().map(|&i| &self.conjuncts[i]).collect();
        let mut key = root.key.clone();
        if chain.len() > 1
            && let Some(last) = conditions.last()
            && let Some((x, y)) = &last.equality
        {
            let copied = if *y == root.key {
                Some(x)
            } else if *x == root.key {
                Some(y)
            } else {
                None
            };
            if let Some(column) = copied {
                key = column.clone();
                conditions.pop();
                chain.pop();
            }
        }
        if chain.len() == 1 && occ == root.key.occ {
            key = root.key.clone();
        }
        let alias = |o: usize| {
            if o == occ {
                "d".to_string()
            } else {
                format!("o{}", chain.iter().position(|c| *c == o).unwrap_or(o))
            }
        };
        let from = chain
            .iter()
            .map(|&o| {
                if o == occ {
                    format!("{DELTA} d")
                } else {
                    format!("{} {}", self.occurrences[o].qualified, alias(o))
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        let mut sql = format!("SELECT DISTINCT {} FROM {from}", key.sql().render(&alias));
        if !conditions.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(
                &conditions
                    .iter()
                    .map(|c| c.sql.render(&alias))
                    .collect::<Vec<_>>()
                    .join(" AND "),
            );
        }
        Some(sql)
    }

    /// Classify every base table, combining the kinds of its occurrences: any
    /// `AllKeys` wins; occurrences propagation covers need nothing more; the rest
    /// are `Local` when they all read the key from the same column, else `Mapped`.
    #[must_use]
    pub fn tables(&self, propagates: &dyn Fn(&str, u32) -> bool) -> Vec<TableLineage> {
        let mut by_table: Vec<(u32, Vec<(usize, Kind)>)> = Vec::new();
        for (occ, o) in self.occurrences.iter().enumerate() {
            let kind = self.classify(occ, propagates);
            match by_table.iter_mut().find(|(relid, _)| *relid == o.relid) {
                Some((_, kinds)) => kinds.push((occ, kind)),
                None => by_table.push((o.relid, vec![(occ, kind)])),
            }
        }
        by_table
            .into_iter()
            .map(|(relid, kinds)| {
                let o = &self.occurrences[kinds[0].0];
                let mut paths = Vec::new();
                let kind = if let Some(reason) = kinds.iter().find_map(|(_, k)| match k {
                    Kind::AllKeys(r) => Some(r.clone()),
                    _ => None,
                }) {
                    TableKind::AllKeys(reason)
                } else {
                    let direct: Vec<&(usize, Kind)> = kinds
                        .iter()
                        .filter(|(_, k)| !matches!(k, Kind::Propagated(_)))
                        .collect();
                    let columns: Vec<&String> = direct
                        .iter()
                        .filter_map(|(_, k)| match k {
                            Kind::Local(c) => Some(c),
                            _ => None,
                        })
                        .collect();
                    if direct.is_empty() {
                        match &kinds[0].1 {
                            Kind::Propagated(e) => TableKind::Propagated(e.clone()),
                            _ => unreachable!("every occurrence is propagated"),
                        }
                    } else if columns.len() == direct.len()
                        && columns.iter().all(|c| *c == columns[0])
                    {
                        TableKind::Local(columns[0].clone())
                    } else {
                        for (occ, k) in direct {
                            paths.push((
                                *occ,
                                match k {
                                    Kind::Mapped(p) => p.clone(),
                                    _ => Vec::new(),
                                },
                            ));
                        }
                        TableKind::Mapped
                    }
                };
                let sql = (kind == TableKind::Mapped).then(|| self.mapping_sql(&paths));
                let lookups = self.lookups(&paths);
                TableLineage {
                    relid,
                    relname: o.relname.clone(),
                    qualified: o.qualified.clone(),
                    kind,
                    paths,
                    sql,
                    columns: Vec::new(),
                    lookups,
                }
            })
            .collect()
    }
}

// ── analysis of a registered or new TVIEW ───────────────────────────────────

/// The lineage of one TVIEW: every base table and how its writes map to keys.
#[derive(Debug, Clone)]
pub struct Lineage {
    pub tables: Vec<TableLineage>,
}

impl Lineage {
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
                        TableKind::AllKeys(reason) => entry["reason"] = reason.clone().into(),
                        TableKind::Mapped => {
                            entry["sql"] = t.sql.clone().unwrap_or_default().into();
                        }
                    }
                    entry["columns"] = t.columns.clone().into();
                    entry
                })
                .collect(),
        )
    }
}

/// Analyze the backing view `view_oid` of `entity`.
///
/// `embeds` lists the TVIEWs it embeds (`fk_<entity>` columns and aggregate
/// embeds); `base_tables` is what `pg_depend` says the view reads, which the
/// analysis must find exactly.
///
/// # Errors
/// Returns an error if the view cannot be analyzed, or if the analysis and
/// `pg_depend` disagree on the tables the view reads.
pub fn analyze(
    entity: &str,
    view_oid: pgrx::pg_sys::Oid,
    base_tables: &[pgrx::pg_sys::Oid],
    embeds: &[String],
) -> crate::TViewResult<Lineage> {
    use pgrx::prelude::*;
    use std::collections::{HashMap, HashSet};

    let catalog = |e: pgrx::spi::Error| crate::TViewError::CatalogError {
        operation: format!("Read the TVIEW catalog to analyze tv_{entity}"),
        pg_error: e.to_string(),
    };
    // Every other registered TVIEW: its table, its view, what it maps.
    let mut tview_tables: HashSet<pgrx::pg_sys::Oid> = HashSet::new();
    let mut tview_views: HashMap<pgrx::pg_sys::Oid, String> = HashMap::new();
    let mut mapped_by: HashMap<String, (HashSet<u32>, bool)> = HashMap::new();
    Spi::connect(|client| {
        for row in client.select(
            &format!(
                "SELECT entity::text, table_oid::oid, view_oid::oid, key_mappings, \
                        uncascaded_policy = 'full_refresh' FROM {}",
                crate::utils::meta_table()
            ),
            None,
            &[],
        )? {
            let (Some(other), Some(table), Some(view)) = (
                row.get::<String>(1)?,
                row.get::<pgrx::pg_sys::Oid>(2)?,
                row.get::<pgrx::pg_sys::Oid>(3)?,
            ) else {
                continue;
            };
            tview_tables.insert(table);
            if other == entity {
                continue;
            }
            tview_views.insert(view, other.clone());
            let mapped: HashSet<u32> = row
                .get::<pgrx::JsonB>(4)?
                .and_then(|j| j.0.as_array().cloned())
                .unwrap_or_default()
                .iter()
                .filter(|e| e["kind"] != "all_keys")
                .filter_map(|e| e["relid"].as_u64().and_then(|r| u32::try_from(r).ok()))
                .collect();
            mapped_by.insert(other, (mapped, row.get::<bool>(5)?.unwrap_or(false)));
        }
        Ok::<_, pgrx::spi::Error>(())
    })
    .map_err(catalog)?;

    let key_column = format!("pk_{entity}");
    let graph = walk::analyze(
        view_oid,
        &walk::Context {
            tview_tables: &tview_tables,
            tview_views: &tview_views,
            key_column: &key_column,
        },
    )?;

    crate::utils::log_debug!("lineage of tv_{entity}: {graph:?}");
    // pg_depend and the query tree must agree on the tables.
    let found: HashSet<u32> = graph.occurrences.iter().map(|o| o.relid).collect();
    let expected: HashSet<u32> = base_tables.iter().map(|o| o.to_u32()).collect();
    if found != expected {
        let name = |relid: &u32| {
            crate::utils::qualified_relname_from_oid(relid.to_owned().into())
                .unwrap_or_else(|_| relid.to_string())
        };
        let missing: Vec<String> = expected.difference(&found).map(name).collect();
        let extra: Vec<String> = found.difference(&expected).map(name).collect();
        return Err(crate::TViewError::InvalidInput {
            parameter: "tview definition".to_string(),
            reason: format!(
                "pg_tviews could not follow how tv_{entity} reads its base tables \
                 (not found in the view's query: [{}]; not in pg_depend: [{}])",
                missing.join(", "),
                extra.join(", ")
            ),
        });
    }

    for function in &graph.untracked_functions {
        notice!(
            "tv_{entity} calls {function}(), which is not immutable: tables read inside \
             {function}() are not tracked, and writes to them do not refresh tv_{entity}"
        );
    }

    // Propagation from an embedded TVIEW covers a table only if that TVIEW maps it.
    let propagates = |child: &str, relid: u32| {
        embeds.iter().any(|e| e == child)
            && mapped_by
                .get(child)
                .is_some_and(|(mapped, full)| *full || mapped.contains(&relid))
    };
    let mut tables = graph.tables(&propagates);
    for table in &mut tables {
        table.columns = referenced_columns(view_oid, table.relid).map_err(catalog)?;
        if let Some(sql) = &table.sql {
            explain(entity, table, sql)?;
        }
    }
    Ok(Lineage { tables })
}

/// Rows above which a sequential scan in a mapping query is worth an index.
const LARGE_TABLE_ROWS: f64 = 1000.0;

/// Plan the mapping query of `table` and say which index would avoid a
/// sequential scan of a large table. Fails when the query does not plan: a
/// mapping `pg_tviews` cannot run must not be registered.
fn explain(entity: &str, table: &TableLineage, sql: &str) -> crate::TViewResult<()> {
    use pgrx::prelude::*;
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
    for (relation, rows) in scans {
        if rows < LARGE_TABLE_ROWS {
            continue;
        }
        let Some((_, columns)) = table
            .lookups
            .iter()
            .find(|(t, _)| t.rsplit('.').next() == Some(relation.as_str()))
        else {
            continue;
        };
        notice!(
            "writes to {} map to tv_{entity} keys with a sequential scan of {} (about {rows} rows); \
             an index on {} ({}) would make them cheaper",
            table.qualified,
            relation,
            relation,
            columns.join(", ")
        );
    }
    Ok(())
}

/// `(relation, estimated rows)` of every sequential scan in an EXPLAIN JSON plan.
fn seq_scans(node: &serde_json::Value, out: &mut Vec<(String, f64)>) {
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

/// The columns of table `relid` that the view `view_oid`, or a view it reads,
/// references (`pg_depend`).
fn referenced_columns(view_oid: pgrx::pg_sys::Oid, relid: u32) -> pgrx::spi::Result<Vec<String>> {
    use pgrx::prelude::*;
    Spi::connect(|client| {
        let mut columns = Vec::new();
        for row in client.select(
            "WITH RECURSIVE views(oid) AS ( \
                 SELECT $1::pg_catalog.oid \
               UNION \
                 SELECT d.refobjid FROM views v \
                 JOIN pg_catalog.pg_rewrite w ON w.ev_class = v.oid \
                 JOIN pg_catalog.pg_depend d \
                   ON d.classid = 'pg_catalog.pg_rewrite'::pg_catalog.regclass AND d.objid = w.oid \
                  AND d.refclassid = 'pg_catalog.pg_class'::pg_catalog.regclass \
                 JOIN pg_catalog.pg_class c ON c.oid = d.refobjid AND c.relkind = 'v' \
             ) \
             SELECT DISTINCT a.attname::pg_catalog.text, a.attnum FROM views v \
             JOIN pg_catalog.pg_rewrite w ON w.ev_class = v.oid \
             JOIN pg_catalog.pg_depend d \
               ON d.classid = 'pg_catalog.pg_rewrite'::pg_catalog.regclass AND d.objid = w.oid \
              AND d.refobjid = $2 AND d.refobjsubid > 0 \
             JOIN pg_catalog.pg_attribute a ON a.attrelid = d.refobjid AND a.attnum = d.refobjsubid \
             ORDER BY 2",
            None,
            // SAFETY: plain OIDs.
            &[
                unsafe {
                    pgrx::datum::DatumWithOid::new(view_oid, PgOid::BuiltIn(PgBuiltInOids::OIDOID).value())
                },
                unsafe {
                    pgrx::datum::DatumWithOid::new(
                        pgrx::pg_sys::Oid::from(relid),
                        PgOid::BuiltIn(PgBuiltInOids::OIDOID).value(),
                    )
                },
            ],
        )? {
            if let Some(name) = row.get::<String>(1)? {
                columns.push(name);
            }
        }
        Ok(columns)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn occ(relid: u32, relname: &str) -> Occurrence {
        Occurrence {
            relid,
            relname: relname.to_string(),
            qualified: format!("public.{relname}"),
            branch: 0,
            via_view: None,
            via_tview: None,
            in_sublink: false,
            opaque_level: None,
        }
    }

    fn col(occ: usize, name: &str) -> Column {
        Column {
            occ,
            attnum: 1,
            name: name.to_string(),
        }
    }

    fn eq(a: Column, b: Column, a_to_b: bool, b_to_a: bool) -> Conjunct {
        let mut sql = a.sql();
        sql.push_text(" OPERATOR(pg_catalog.=) ");
        sql.push_sql(b.sql());
        Conjunct {
            sql,
            a: a.occ,
            b: b.occ,
            a_to_b,
            b_to_a,
            equality: Some((a, b)),
        }
    }

    fn graph(occurrences: Vec<Occurrence>, conjuncts: Vec<Conjunct>, key: Column) -> Graph {
        Graph {
            occurrences,
            conjuncts,
            roots: vec![Root { branch: 0, key }],
            untracked_functions: vec![],
        }
    }

    const NONE: &dyn Fn(&str, u32) -> bool = &|_, _| false;

    #[test]
    fn root_is_local_on_its_key() {
        let g = graph(vec![occ(1, "tb_order")], vec![], col(0, "pk_order"));
        assert_eq!(g.classify(0, NONE), Kind::Local("pk_order".into()));
    }

    #[test]
    fn direct_fk_is_local() {
        // tb_order o LEFT JOIN tb_line l ON l.fk_order = o.pk_order
        let g = graph(
            vec![occ(1, "tb_order"), occ(2, "tb_line")],
            vec![eq(col(1, "fk_order"), col(0, "pk_order"), true, false)],
            col(0, "pk_order"),
        );
        assert_eq!(g.classify(1, NONE), Kind::Local("fk_order".into()));
    }

    #[test]
    fn two_hops_are_mapped() {
        // tb_order o JOIN tb_line l ON l.fk_order = o.pk_order JOIN tb_sku s ON s.pk_sku = l.fk_sku
        let g = graph(
            vec![occ(1, "tb_order"), occ(2, "tb_line"), occ(3, "tb_sku")],
            vec![
                eq(col(1, "fk_order"), col(0, "pk_order"), true, true),
                eq(col(2, "pk_sku"), col(1, "fk_sku"), true, true),
            ],
            col(0, "pk_order"),
        );
        assert_eq!(g.classify(2, NONE), Kind::Mapped(vec![1, 0]));
    }

    #[test]
    fn a_preserved_side_does_not_map_through_an_outer_join() {
        // tb_line l LEFT JOIN tb_order o ON o.pk_order = l.fk_order, key on o
        let g = graph(
            vec![occ(1, "tb_order"), occ(2, "tb_line")],
            vec![eq(col(0, "pk_order"), col(1, "fk_order"), true, false)],
            col(0, "pk_order"),
        );
        assert!(matches!(g.classify(1, NONE), Kind::AllKeys(_)));
    }

    #[test]
    fn an_unlinked_subquery_is_all_keys_with_the_reason() {
        let mut line = occ(2, "tb_line");
        line.in_sublink = true;
        let g = graph(vec![occ(1, "tb_order"), line], vec![], col(0, "pk_order"));
        assert_eq!(
            g.classify(1, NONE),
            Kind::AllKeys(
                "read in a subquery, with no condition linking it to the TVIEW key".into()
            )
        );
    }

    #[test]
    fn an_embedded_tview_s_table_is_propagated() {
        let mut user = occ(3, "tb_user");
        user.via_tview = Some("user".into());
        let g = graph(
            vec![occ(1, "tb_post"), user],
            vec![eq(col(1, "pk_user"), col(0, "fk_user"), true, false)],
            col(0, "pk_post"),
        );
        assert_eq!(
            g.classify(1, &|e, _| e == "user"),
            Kind::Propagated("user".into())
        );
        assert!(matches!(g.classify(1, NONE), Kind::Mapped(_)));
    }

    #[test]
    fn an_equality_wins_over_another_link() {
        // EXISTS (SELECT 1 FROM tb_line l WHERE l.pos > o.min_pos AND l.fk_order = o.pk_order)
        let mut other = eq(col(1, "pos"), col(0, "min_pos"), true, false);
        other.equality = None;
        let g = graph(
            vec![occ(1, "tb_order"), occ(2, "tb_line")],
            vec![
                other,
                eq(col(1, "fk_order"), col(0, "pk_order"), true, false),
            ],
            col(0, "pk_order"),
        );
        assert_eq!(g.classify(1, NONE), Kind::Local("fk_order".into()));
    }

    // ── mapping queries (golden) ────────────────────────────────────────────

    fn ne(a: Column, b: Column, op: &str) -> Conjunct {
        let mut c = eq(a, b, true, false);
        c.equality = None;
        let Piece::Text(t) = &mut c.sql.0[1] else {
            unreachable!()
        };
        *t = t.replace('=', op);
        c
    }

    #[test]
    fn local_copy_of_the_key_skips_the_root() {
        // #157: ARRAY(SELECT … FROM tb_line l WHERE l.fk_order = o.pk_order), as a
        // mapped table (with another occurrence) keeps the one-step simplification.
        let g = graph(
            vec![occ(1, "tb_order"), occ(2, "tb_line")],
            vec![eq(col(1, "fk_order"), col(0, "pk_order"), true, false)],
            col(0, "pk_order"),
        );
        assert_eq!(
            g.mapping_sql(&[(1, vec![0])]),
            r#"SELECT DISTINCT d."fk_order" FROM pg_tviews_delta d"#
        );
    }

    #[test]
    fn two_hops_join_the_intermediate_table() {
        let g = graph(
            vec![occ(1, "tb_order"), occ(2, "tb_line"), occ(3, "tb_sku")],
            vec![
                eq(col(1, "fk_order"), col(0, "pk_order"), true, true),
                eq(col(2, "pk_sku"), col(1, "fk_sku"), true, true),
            ],
            col(0, "pk_order"),
        );
        assert_eq!(
            g.mapping_sql(&[(2, vec![1, 0])]),
            r#"SELECT DISTINCT o1."fk_order" FROM pg_tviews_delta d, public.tb_line o1 WHERE d."pk_sku" OPERATOR(pg_catalog.=) o1."fk_sku""#
        );
    }

    #[test]
    fn a_non_equality_keeps_the_root() {
        // EXISTS (SELECT 1 FROM tb_line l WHERE l.pos > o.min_pos)
        let g = graph(
            vec![occ(1, "tb_order"), occ(2, "tb_line")],
            vec![ne(col(1, "pos"), col(0, "min_pos"), ">")],
            col(0, "pk_order"),
        );
        assert_eq!(
            g.mapping_sql(&[(1, vec![0])]),
            r#"SELECT DISTINCT o1."pk_order" FROM pg_tviews_delta d, public.tb_order o1 WHERE d."pos" OPERATOR(pg_catalog.>) o1."min_pos""#
        );
    }

    #[test]
    fn several_occurrences_union_their_queries() {
        // tb_node n LEFT JOIN tb_node p ON p.pk_node = n.fk_parent
        let g = graph(
            vec![occ(1, "tb_node"), occ(1, "tb_node")],
            vec![eq(col(1, "pk_node"), col(0, "fk_parent"), true, false)],
            col(0, "pk_node"),
        );
        assert_eq!(
            g.mapping_sql(&[(0, vec![]), (1, vec![0])]),
            r#"SELECT DISTINCT d."pk_node" FROM pg_tviews_delta d UNION SELECT DISTINCT o1."pk_node" FROM pg_tviews_delta d, public.tb_node o1 WHERE d."pk_node" OPERATOR(pg_catalog.=) o1."fk_parent""#
        );
    }

    #[test]
    fn a_self_join_combines_occurrences() {
        // tb_node n LEFT JOIN tb_node p ON p.pk_node = n.fk_parent: the table is
        // the root (local) and reached through fk_parent (mapped).
        let g = graph(
            vec![occ(1, "tb_node"), occ(1, "tb_node")],
            vec![eq(col(1, "pk_node"), col(0, "fk_parent"), true, false)],
            col(0, "pk_node"),
        );
        let tables = g.tables(NONE);
        assert_eq!(tables.len(), 1);
        assert_eq!(tables[0].kind, TableKind::Mapped);
        assert_eq!(tables[0].paths, vec![(0, vec![]), (1, vec![0])]);
    }

    #[test]
    fn all_keys_wins_for_the_table() {
        let mut sub = occ(2, "tb_line");
        sub.in_sublink = true;
        let g = graph(
            vec![occ(1, "tb_order"), occ(2, "tb_line"), sub],
            vec![eq(col(1, "fk_order"), col(0, "pk_order"), true, false)],
            col(0, "pk_order"),
        );
        let tables = g.tables(NONE);
        assert!(matches!(tables[1].kind, TableKind::AllKeys(_)));
    }
}
