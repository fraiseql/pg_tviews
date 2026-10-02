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

/// A piece of generated SQL: text, or a column of a table occurrence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Piece {
    Text(String),
    Column { occ: usize, attnum: i16 },
}

/// SQL with holes for occurrence columns, filled in when a query is assembled.
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
                column @ Piece::Column { .. } => self.0.push(column),
            }
        }
    }
}

/// A stored mapping query is a template that names relations and columns by OID
/// and attribute number, `{r:<relid>}` and `{c:<relid>:<attnum>}`, so that renames
/// leave it valid; `{` and `}` of the SQL itself are doubled. [`render_template`]
/// writes the current names in.
#[must_use]
pub fn escape_template(text: &str) -> String {
    text.replace('{', "{{").replace('}', "}}")
}

/// A placeholder of a mapping-query template.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placeholder {
    Relation(u32),
    Column(u32, i16),
}

/// Fill in a template with `name(placeholder)`; `None` if a placeholder is
/// malformed or `name` has no name for it (a dropped relation or column).
pub fn fill_template(
    template: &str,
    name: &dyn Fn(Placeholder) -> Option<String>,
) -> Option<String> {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(i) = rest.find(['{', '}']) {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        if tail.starts_with("{{") || tail.starts_with("}}") {
            out.push_str(&tail[..1]);
            rest = &tail[2..];
            continue;
        }
        let end = tail.find('}')?;
        let mut fields = tail[1..end].split(':');
        let placeholder = match (fields.next()?, fields.next(), fields.next(), fields.next()) {
            ("r", Some(relid), None, None) => Placeholder::Relation(relid.parse().ok()?),
            ("c", Some(relid), Some(attnum), None) => {
                Placeholder::Column(relid.parse().ok()?, attnum.parse().ok()?)
            }
            _ => return None,
        };
        out.push_str(&name(placeholder)?);
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    Some(out)
}

/// The placeholders a template uses.
#[must_use]
pub fn template_placeholders(template: &str) -> Vec<Placeholder> {
    let found = std::cell::RefCell::new(Vec::new());
    let _ = fill_template(template, &|p| {
        found.borrow_mut().push(p);
        Some(String::new())
    });
    found.into_inner()
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
    #[must_use]
    pub fn sql(&self) -> Sql {
        Sql(vec![Piece::Column {
            occ: self.occ,
            attnum: self.attnum,
        }])
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
    /// Whether it maps a row of `a` toward `b`.
    pub a_to_b: Maps,
    pub b_to_a: Maps,
    /// Set when the predicate is `a.col = b.col` with `=`.
    pub equality: Option<(Column, Column)>,
}

/// Whether a predicate maps a changed row of one occurrence to the rows of another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Maps {
    No,
    /// It must hold for every contributing row of the first occurrence.
    Yes,
    /// An outer join's equality toward its nullable side: it holds for a row that
    /// has a match. A row with no match yields NULLs there, which no key and no
    /// further equality matches, so a path may take it and then end at the key or
    /// go on by an equality (#165).
    IfMatched,
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
    /// Tables read only where the output never depends on them (a CTE the view
    /// does not use): not tracked.
    pub unread_tables: std::collections::BTreeSet<u32>,
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
    /// `mapped`, and `all_keys` when some reads can be traced: the query over
    /// [`DELTA`] returning the keys its rows can affect.
    pub sql: Option<String>,
    /// Columns of the table the TVIEW reads (name, attnum); empty when unknown.
    pub columns: Vec<(String, i16)>,
    /// Tables the mapping query joins, with the columns it looks up in each.
    pub lookups: Vec<(String, Vec<String>)>,
    /// `mapped` through one equality onto a column of the root, `(column of this
    /// table, column of the root)`: the TVIEW rows can be found by that column
    /// when the TVIEW projects it (fan-out, issue #120).
    pub hop: Option<(String, String)>,
    /// The table whose column is the key (of a branch, under UNION).
    pub root: bool,
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
            // A top level whose rows a write changes beyond its own (a window
            // function, LIMIT…) has no root; say why.
            return Kind::AllKeys(
                o.opaque_level
                    .clone()
                    .unwrap_or_else(|| "the TVIEW key is not a column of a base table".to_string()),
            );
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
        // A state is an occurrence, and whether it was reached through an
        // `IfMatched` step. From there the path ends at the key (a row with no
        // match has a NULL key) or goes on by an equality, which fails on the
        // NULLs of a row with no match; any other predicate might hold for them.
        type State = (usize, bool);
        let mut previous: BTreeMap<State, (State, usize)> = BTreeMap::new();
        let start: State = (from, false);
        let mut queue = VecDeque::from([start]);
        while let Some(state) = queue.pop_front() {
            let (at, pending) = state;
            if at == to {
                let mut path = Vec::new();
                let mut cur = state;
                while cur != start {
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
                if pending && c.equality.is_none() {
                    continue;
                }
                let next = if c.a == at && c.a_to_b != Maps::No {
                    (c.b, c.a_to_b == Maps::IfMatched)
                } else if c.b == at && c.b_to_a != Maps::No {
                    (c.a, c.b_to_a == Maps::IfMatched)
                } else {
                    continue;
                };
                if next.0 != from && !previous.contains_key(&next) {
                    previous.insert(next, (state, i));
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

    /// `(column of occ, column of the root)` when `path` is one equality from `occ`
    /// onto a column of the root other than the key.
    fn root_hop(&self, occ: usize, path: &[usize]) -> Option<(String, String)> {
        let root = self.root_of(occ)?;
        let [only] = path else { return None };
        let (x, y) = self.conjuncts[*only].equality.as_ref()?;
        let (own, other) = if x.occ == occ { (x, y) } else { (y, x) };
        (other.occ == root.key.occ && *other != root.key)
            .then(|| (own.name.clone(), other.name.clone()))
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
        let column = |o: usize, attnum: i16| {
            format!("{}.{{c:{}:{attnum}}}", alias(o), self.occurrences[o].relid)
        };
        let render = |sql: &Sql| {
            sql.0
                .iter()
                .map(|p| match p {
                    Piece::Text(t) => escape_template(t),
                    Piece::Column { occ, attnum } => column(*occ, *attnum),
                })
                .collect::<String>()
        };
        let from = chain
            .iter()
            .map(|&o| {
                if o == occ {
                    format!("{DELTA} d")
                } else {
                    format!("{{r:{}}} {}", self.occurrences[o].relid, alias(o))
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        let mut sql = format!("SELECT DISTINCT {} FROM {from}", render(&key.sql()));
        if !conditions.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(
                &conditions
                    .iter()
                    .map(|c| render(&c.sql))
                    .collect::<Vec<_>>()
                    .join(" AND "),
            );
        }
        Some(sql)
    }

    /// The predicate chain from a `Local` or `Mapped` occurrence to its root's key
    /// (empty for the root itself).
    fn occurrence_path(&self, occ: usize, kind: &Kind) -> Vec<usize> {
        match kind {
            Kind::Mapped(p) => p.clone(),
            // A local occurrence other than the root reads the key through its one
            // equality.
            _ => self
                .root_of(occ)
                .filter(|r| r.key.occ != occ)
                .and_then(|r| self.path(occ, r.key.occ))
                .unwrap_or_default(),
        }
    }

    /// Classify every base table, combining the kinds of its occurrences: any
    /// `AllKeys` wins, keeping the mapping of the occurrences that can be traced;
    /// occurrences propagation covers need nothing more; the rest are `Local` when
    /// they all read the key from the same column, else `Mapped`.
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
                    // The reads that can be traced keep refreshing; only the rest
                    // is left to the TVIEW's uncascaded_policy (#162).
                    for (occ, k) in &kinds {
                        if matches!(k, Kind::Local(_) | Kind::Mapped(_)) {
                            paths.push((*occ, self.occurrence_path(*occ, k)));
                        }
                    }
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
                            paths.push((*occ, self.occurrence_path(*occ, k)));
                        }
                        TableKind::Mapped
                    }
                };
                let sql = match kind {
                    TableKind::Mapped => Some(self.mapping_sql(&paths)),
                    TableKind::AllKeys(_) if !paths.is_empty() => Some(self.mapping_sql(&paths)),
                    _ => None,
                };
                let lookups = self.lookups(&paths);
                let hop = match paths.as_slice() {
                    [(occ, path)] if kind == TableKind::Mapped => self.root_hop(*occ, path),
                    _ => None,
                };
                TableLineage {
                    relid,
                    relname: o.relname.clone(),
                    qualified: o.qualified.clone(),
                    kind,
                    paths,
                    sql,
                    columns: Vec::new(),
                    lookups,
                    hop,
                    root: self
                        .roots
                        .iter()
                        .any(|r| self.occurrences[r.key.occ].relid == relid),
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
    /// Tables the view reads only where its output cannot depend on them.
    pub unread: Vec<u32>,
}

impl Lineage {
    /// Whether the TVIEW is a UNION whose branches have their own root tables.
    #[must_use]
    pub fn is_union(&self) -> bool {
        self.tables.iter().filter(|t| t.root).count() > 1
    }

    /// Whether a table's writes map to keys through a query (`mapped`).
    #[must_use]
    pub fn has_mapped(&self) -> bool {
        self.tables.iter().any(|t| t.kind == TableKind::Mapped)
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
    let found: HashSet<u32> = graph
        .occurrences
        .iter()
        .map(|o| o.relid)
        .chain(graph.unread_tables.iter().copied())
        .collect();
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
    let unread = graph
        .unread_tables
        .iter()
        .copied()
        .filter(|relid| tables.iter().all(|t| t.relid != *relid))
        .collect();
    Ok(Lineage { tables, unread })
}

/// A mapping-query template with the current names of its relations and columns;
/// `None` when one of them is gone.
///
/// # Errors
/// Returns an error if the catalog cannot be read.
pub fn render_template(template: &str) -> pgrx::spi::Result<Option<String>> {
    use pgrx::prelude::*;
    let mut names: std::collections::HashMap<(u32, i16), Option<String>> =
        std::collections::HashMap::new();
    for placeholder in template_placeholders(template) {
        let (relid, attnum) = match placeholder {
            Placeholder::Relation(relid) => (relid, 0),
            Placeholder::Column(relid, attnum) => (relid, attnum),
        };
        if names.contains_key(&(relid, attnum)) {
            continue;
        }
        // SAFETY: plain OID / int2 datums.
        let args = unsafe {
            [
                pgrx::datum::DatumWithOid::new(
                    pgrx::pg_sys::Oid::from(relid),
                    PgOid::BuiltIn(PgBuiltInOids::OIDOID).value(),
                ),
                pgrx::datum::DatumWithOid::new(
                    attnum,
                    PgOid::BuiltIn(PgBuiltInOids::INT2OID).value(),
                ),
            ]
        };
        let name = Spi::get_one_with_args::<String>(
            "SELECT CASE WHEN $2 = 0 \
                    THEN pg_catalog.quote_ident(n.nspname) || '.' || pg_catalog.quote_ident(c.relname) \
                    ELSE (SELECT pg_catalog.quote_ident(a.attname) FROM pg_catalog.pg_attribute a \
                          WHERE a.attrelid = c.oid AND a.attnum = $2 AND NOT a.attisdropped) END \
             FROM pg_catalog.pg_class c \
             JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
             WHERE c.oid = $1",
            &args,
        )?;
        names.insert((relid, attnum), name);
    }
    Ok(fill_template(template, &|p| {
        let key = match p {
            Placeholder::Relation(relid) => (relid, 0),
            Placeholder::Column(relid, attnum) => (relid, attnum),
        };
        names.get(&key).cloned().flatten()
    }))
}

/// One table of a registered TVIEW's `key_mappings`.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize)]
pub struct KeyMapping {
    pub relid: u32,
    pub table: String,
    pub kind: String,
    #[serde(default)]
    pub column: Option<String>,
    #[serde(default)]
    pub entity: Option<String>,
    /// `mapped` (and `all_keys` for its traceable reads): the query template (see
    /// [`render_template`]).
    #[serde(default)]
    pub sql: Option<String>,
    /// Columns of the table the TVIEW reads; empty when unknown.
    #[serde(default)]
    pub attnums: Vec<i16>,
    #[serde(default)]
    pub columns: Vec<String>,
    /// `mapped` through one equality onto a column of the root: `(this table's
    /// column, the root's column)`.
    #[serde(default)]
    pub hop: Option<(String, String)>,
    /// The column of this table whose value the fan-out patch looks up.
    #[serde(default)]
    pub key_col: Option<String>,
    /// How an UPDATE is written into every TVIEW row it reaches (issue #120).
    #[serde(default)]
    pub fanout: Option<crate::cascade_path::FanoutPatch>,
}

impl KeyMapping {
    /// Parse `pg_tview_meta.key_mappings`; anything malformed is skipped.
    #[must_use]
    pub fn parse_all(json: &serde_json::Value) -> Vec<Self> {
        json.as_array()
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(|e| serde_json::from_value(e.clone()).ok())
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// Rows above which a sequential scan in a mapping query is worth an index.
const LARGE_TABLE_ROWS: f64 = 1000.0;

/// Plan the mapping query of `table` and say which index would avoid a
/// sequential scan of a large table. Fails when the query does not plan: a
/// mapping `pg_tviews` cannot run must not be registered.
fn explain(entity: &str, table: &TableLineage, template: &str) -> crate::TViewResult<()> {
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
fn referenced_columns(
    view_oid: pgrx::pg_sys::Oid,
    relid: u32,
) -> pgrx::spi::Result<Vec<(String, i16)>> {
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
            if let (Some(name), Some(attnum)) = (row.get::<String>(1)?, row.get::<i16>(2)?) {
                columns.push((name, attnum));
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
            a_to_b: if a_to_b { Maps::Yes } else { Maps::No },
            b_to_a: if b_to_a { Maps::Yes } else { Maps::No },
            equality: Some((a, b)),
        }
    }

    fn graph(occurrences: Vec<Occurrence>, conjuncts: Vec<Conjunct>, key: Column) -> Graph {
        Graph {
            occurrences,
            conjuncts,
            roots: vec![Root { branch: 0, key }],
            untracked_functions: vec![],
            unread_tables: std::collections::BTreeSet::new(),
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

    /// `l.fk_order = o2.pk_order` from `tb_line l LEFT JOIN tb_order o2`: it holds
    /// for rows of `o2`, and toward `o2` only for a line that has a match.
    fn outer_on(l: usize, o2: usize) -> Conjunct {
        let mut c = eq(col(l, "fk_order"), col(o2, "pk_order"), false, true);
        c.a_to_b = Maps::IfMatched;
        c
    }

    #[test]
    fn a_nullable_step_is_taken_when_the_path_goes_on() {
        // tv over o, reading v_line (l LEFT JOIN o2) where v.order_id = o.id (#165).
        let g = graph(
            vec![occ(1, "tb_order"), occ(1, "tb_order"), occ(2, "tb_line")],
            vec![outer_on(2, 1), eq(col(1, "id"), col(0, "id"), true, false)],
            col(0, "pk_order"),
        );
        assert_eq!(g.classify(2, NONE), Kind::Mapped(vec![0, 1]));
    }

    #[test]
    fn a_nullable_step_may_end_at_the_key() {
        // tb_line l LEFT JOIN tb_order o, keyed on o: a line with no order has a
        // NULL key, which is no TVIEW row; one with an order maps to it.
        let g = graph(
            vec![occ(1, "tb_order"), occ(2, "tb_line")],
            vec![outer_on(1, 0)],
            col(0, "pk_order"),
        );
        assert_eq!(g.classify(1, NONE), Kind::Local("fk_order".into()));
    }

    #[test]
    fn a_nullable_step_goes_on_only_by_an_equality() {
        // After l → o2 (nullable), only an equality is known to fail on o2's NULLs.
        let mut sql = Sql::default();
        sql.push_text("COALESCE(o2.id, 0) IS NOT DISTINCT FROM o.id");
        let loose = Conjunct {
            sql,
            a: 1,
            b: 0,
            a_to_b: Maps::Yes,
            b_to_a: Maps::No,
            equality: None,
        };
        let g = graph(
            vec![occ(1, "tb_order"), occ(1, "tb_order"), occ(2, "tb_line")],
            vec![outer_on(2, 1), loose],
            col(0, "pk_order"),
        );
        assert!(matches!(g.classify(2, NONE), Kind::AllKeys(_)));
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
    fn an_opaque_top_level_without_a_root_is_all_keys_with_the_reason() {
        // SELECT pk_win, … count(*) OVER () FROM tb_win: the walker gives the
        // opaque top level no root and stamps its occurrences.
        let mut win = occ(1, "tb_win");
        win.opaque_level = Some("read under a window function in the top-level SELECT".into());
        let g = Graph {
            occurrences: vec![win],
            conjuncts: vec![],
            roots: vec![],
            untracked_functions: vec![],
            unread_tables: std::collections::BTreeSet::new(),
        };
        assert_eq!(
            g.classify(0, NONE),
            Kind::AllKeys("read under a window function in the top-level SELECT".into())
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
            "SELECT DISTINCT d.{c:2:1} FROM pg_tviews_delta d"
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
            "SELECT DISTINCT o1.{c:2:1} FROM pg_tviews_delta d, {r:2} o1 \
             WHERE d.{c:3:1} OPERATOR(pg_catalog.=) o1.{c:2:1}"
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
            "SELECT DISTINCT o1.{c:1:1} FROM pg_tviews_delta d, {r:1} o1 \
             WHERE d.{c:2:1} OPERATOR(pg_catalog.>) o1.{c:1:1}"
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
            "SELECT DISTINCT d.{c:1:1} FROM pg_tviews_delta d UNION SELECT DISTINCT o1.{c:1:1} \
             FROM pg_tviews_delta d, {r:1} o1 WHERE d.{c:1:1} OPERATOR(pg_catalog.=) o1.{c:1:1}"
        );
    }

    #[test]
    fn templates_round_trip_braces_and_placeholders() {
        let template = format!(
            "SELECT d.{{c:7:2}} FROM {{r:7}} d WHERE d.{{c:7:3}} = {}",
            escape_template("'{r:1} }'")
        );
        let names = |p: Placeholder| match p {
            Placeholder::Relation(7) => Some("public.tb_x".to_string()),
            Placeholder::Column(7, 2) => Some("\"a\"".to_string()),
            Placeholder::Column(7, 3) => Some("b".to_string()),
            _ => None,
        };
        assert_eq!(
            fill_template(&template, &names).as_deref(),
            Some(r#"SELECT d."a" FROM public.tb_x d WHERE d.b = '{r:1} }'"#)
        );
        assert_eq!(
            template_placeholders(&template),
            vec![
                Placeholder::Column(7, 2),
                Placeholder::Relation(7),
                Placeholder::Column(7, 3)
            ]
        );
        assert_eq!(fill_template("{r:8}", &names), None);
        assert_eq!(fill_template("{x:1}", &names), None);
    }

    #[test]
    fn a_tree_reads_the_key_of_each_occurrence() {
        // SELECT pk_tree, EXISTS (SELECT 1 FROM tb_tree c WHERE c.fk_parent = t.pk_tree)
        // FROM tb_tree t: the root occurrence and a local one on another column.
        let at = |occ: usize, name: &str, attnum: i16| Column {
            occ,
            attnum,
            name: name.to_string(),
        };
        let g = graph(
            vec![occ(1, "tb_tree"), occ(1, "tb_tree")],
            vec![eq(at(1, "fk_parent", 3), at(0, "pk_tree", 1), true, false)],
            at(0, "pk_tree", 1),
        );
        let tables = g.tables(NONE);
        assert_eq!(tables[0].kind, TableKind::Mapped);
        assert_eq!(
            tables[0].sql.as_deref(),
            Some(
                "SELECT DISTINCT d.{c:1:1} FROM pg_tviews_delta d \
                 UNION SELECT DISTINCT d.{c:1:3} FROM pg_tviews_delta d"
            )
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
    fn an_all_keys_table_keeps_the_mapping_of_its_traceable_reads() {
        // tb_order is the root, and read again in a subquery nothing links (#162).
        let mut again = occ(1, "tb_order");
        again.in_sublink = true;
        let g = graph(vec![occ(1, "tb_order"), again], vec![], col(0, "pk_order"));
        let tables = g.tables(NONE);
        assert!(matches!(tables[0].kind, TableKind::AllKeys(_)));
        assert_eq!(tables[0].paths, vec![(0, vec![])]);
        assert!(tables[0].sql.is_some());
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
