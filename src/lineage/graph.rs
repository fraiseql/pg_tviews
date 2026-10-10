//! The graph the walk reads a backing view into, and how each table it reads maps
//! to the TVIEW's keys.

use super::{DELTA, IdentityError, IdentityKind, Piece, Sql, escape_template};
use std::collections::{BTreeMap, VecDeque};

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
    /// The UNION leaves it sits in, outermost first: `(union, leaf)`.
    pub unions: Scope,
    /// First plain view (not a TVIEW's backing view) on the way to it.
    pub via_view: Option<String>,
    /// The TVIEW whose backing view it was read through, if any.
    pub via_tview: Option<String>,
    /// Read inside a subquery expression (`(SELECT …)`, `EXISTS`, `IN`, `ARRAY(…)`).
    pub in_sublink: bool,
    /// Why nothing passes through the query level it sits in, if so (window
    /// function, LIMIT, …): its columns are not visible outside that level.
    pub opaque_level: Option<String>,
    /// A materialized view: `REFRESH MATERIALIZED VIEW` replaces its rows, and no
    /// trigger sees them.
    pub matview: bool,
    /// The table of another TVIEW, of that entity: its rows change when that
    /// TVIEW is refreshed.
    pub tview_table: Option<String>,
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
    /// What the predicate looks rows up by, other than plain columns.
    pub lookups: Vec<Lookup>,
}

/// An expression of one occurrence that a predicate looks its rows up by: a
/// mapping query that starts from the other occurrence scans `occ` for it, which
/// an index on the expression avoids.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lookup {
    pub occ: usize,
    pub expr: Sql,
    /// Array containment (`@>`): a GIN index; else a btree index.
    pub gin: bool,
}

/// An index on an expression that would serve a mapping query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexHint {
    /// The table, schema-qualified and quoted.
    pub table: String,
    pub relname: String,
    /// The expression, a template naming columns by attribute number (see
    /// [`render_template`]).
    pub expr: String,
    pub gin: bool,
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
    /// go on by an equality.
    IfMatched,
}

/// Where a read sits among the UNIONs of the view: `(union, leaf)` per UNION it
/// is inside, outermost first.
pub type Scope = Vec<(usize, usize)>;

/// Whether two scopes can meet in one TVIEW row: they take the same leaf of every
/// UNION both sit in.
#[must_use]
pub fn compatible(a: &Scope, b: &Scope) -> bool {
    a.iter()
        .all(|(union, leaf)| b.iter().all(|(u, l)| u != union || l == leaf))
}

/// The TVIEW key in one UNION branch: a column of the root occurrence, or an
/// immutable expression of that occurrence's row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Root {
    /// The column, or the first column the expression reads.
    pub key: Column,
    /// The key written over the root occurrence's columns, when not `key` itself.
    pub expr: Option<Sql>,
    /// The UNION leaves the key comes from: the rows it names.
    pub scope: Scope,
}

impl Root {
    /// The key written over the root occurrence's columns.
    #[must_use]
    pub fn sql(&self) -> Sql {
        self.expr.clone().unwrap_or_else(|| self.key.sql())
    }

    /// The key written over `column` instead of the root's key column, when the
    /// key reads nothing else of the root: an equality `column = key` then
    /// replaces the root in a mapping query.
    fn sql_over(&self, column: &Column) -> Option<Sql> {
        let Some(expr) = &self.expr else {
            return Some(column.sql());
        };
        let mut sql = Sql::default();
        for piece in &expr.0 {
            match piece {
                Piece::Column { occ, attnum }
                    if *occ == self.key.occ && *attnum == self.key.attnum =>
                {
                    sql.0.push(Piece::Column {
                        occ: column.occ,
                        attnum: column.attnum,
                    });
                }
                Piece::Column { .. } => return None,
                Piece::Text(t) => sql.push_text(t),
            }
        }
        Some(sql)
    }
}

/// What [`walk`] reads from the backing view.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QueryGraph {
    pub occurrences: Vec<Occurrence>,
    pub conjuncts: Vec<Conjunct>,
    pub roots: Vec<Root>,
    /// The UNION leaves whose key no table carries: their rows cannot be mapped.
    pub holes: Vec<Scope>,
    /// Functions the view calls that may read tables `pg_tviews` does not see:
    /// not immutable, outside `pg_catalog` (by OID).
    pub untracked_functions: Vec<u32>,
    /// How the view reads the current time, each construct once:
    /// `CURRENT_DATE`, `now()`…
    pub time_reads: Vec<String>,
    /// Tables read only where the output never depends on them (a CTE the view
    /// does not use): not tracked.
    pub unread_tables: std::collections::BTreeSet<u32>,
    /// The column that names the TVIEW's rows (`None` before the walk sets it).
    pub identity: Option<Result<WalkedIdentity, IdentityError>>,
    /// The backing view's own SELECT is a set operation (UNION, INTERSECT,
    /// EXCEPT).
    pub set_operation: bool,
    /// `(occurrence, attnum)` of the virtual generated columns among the keys and
    /// equalities: NULL in the rows a trigger sees, so never read off one.
    pub virtual_columns: std::collections::BTreeSet<(usize, i16)>,
    /// Every other TVIEW the view reads (its backing view or its table, found by
    /// OID), with the base columns equal to its `pk_<entity>`: the column its key
    /// stands for in its backing view, or one an equality with its table's key
    /// names.
    pub tview_keys: BTreeMap<String, Vec<Column>>,
    /// The output columns of the backing view's own SELECT (none for a set
    /// operation), with the base column each stands for.
    pub outputs: Vec<(String, Option<Column>)>,
    /// The shape of the `data` output of the backing view's own SELECT (`None` for
    /// a set operation, or without a `data` column).
    pub data: Option<DataShape>,
}

/// What the `data` output is built of, read from its expression tree.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DataShape {
    /// The fields that copy a base column as it is: their JSON path, the column,
    /// and whether the definition reads that column nowhere else (not in a join,
    /// a filter, a grouping, another output or a subquery).
    pub fields: Vec<DataField>,
    /// Where another TVIEW's `data` lands.
    pub embeds: Vec<DataEmbed>,
    /// Some part of `data` is computed in a way this reading does not follow:
    /// a patch could miss what it computes.
    pub opaque: bool,
}

/// A field of `data` copying a base column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataField {
    pub path: Vec<String>,
    pub column: Column,
    /// The table of `column`.
    pub relid: u32,
    /// `column` is read from the occurrence of the table holding the identity
    /// (not from another occurrence of it, as in a self-join).
    pub root: bool,
    pub only_in_data: bool,
}

/// Another TVIEW's `data` inside this one's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataEmbed {
    pub entity: String,
    pub path: Vec<String>,
    /// Inside `jsonb_agg`: one element per child row.
    pub array: bool,
}

/// The identity the walk found: the output column, and the base column it stands
/// for in each UNION branch (one without UNION; none where it is not a column).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalkedIdentity {
    pub name: String,
    /// Index of the output column (0-based).
    pub position: usize,
    pub type_oid: u32,
    pub kind: IdentityKind,
    pub columns: Vec<Column>,
}

/// How a write to one occurrence maps to keys.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    Local(String),
    /// The predicates from the occurrence to the root, in order.
    Mapped(Vec<usize>),
    /// One chain per UNION branch root the occurrence reaches: a read outside the
    /// UNION the key comes from.
    Branches(Vec<Vec<usize>>),
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
    /// Expressions the mapping query looks rows up by (array membership, computed
    /// columns), with the index that serves each.
    pub index_hints: Vec<IndexHint>,
    /// `mapped` through one equality onto a column of the root, `(column of this
    /// table, column of the root)`: the TVIEW rows can be found by that column
    /// when the TVIEW projects it (fan-out).
    pub hop: Option<(String, String)>,
    /// With `hop`: the output column equal to the root's column, and the `data`
    /// keys copying this table's columns, read nowhere else (`(column, key)`):
    /// what an UPDATE of it can write into every row with that value.
    pub fanout: Option<(String, Vec<(String, String)>)>,
    /// The table whose column is the key (of a branch, under UNION).
    pub root: bool,
    /// The virtual generated columns the TVIEW reads and their inputs: never
    /// copied by a fast path, which reads the changed row.
    pub virtual_reads: Vec<String>,
    /// A materialized view: no trigger can be installed on it.
    pub matview: bool,
    /// The table of another TVIEW, of that entity.
    pub tview: Option<String>,
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

impl QueryGraph {
    /// Every TVIEW the view reads, with the output columns equal to its key (one
    /// per read of it that has one): a change to its row `k` changes the rows
    /// where one of them holds `k`. Empty when no output column carries its key.
    #[must_use]
    pub fn embed_lookups(&self) -> BTreeMap<String, Vec<String>> {
        self.tview_keys
            .iter()
            .map(|(entity, keys)| {
                let mut lookups: Vec<String> = Vec::new();
                for key in keys {
                    if let Some(output) = self.output_equal_to(std::slice::from_ref(key))
                        && !lookups.contains(&output)
                    {
                        lookups.push(output);
                    }
                }
                (entity.clone(), lookups)
            })
            .collect()
    }

    /// The first output column equal to one of `keys`: the same column, or one an
    /// equality links it to.
    fn output_equal_to(&self, keys: &[Column]) -> Option<String> {
        let equal = |x: &Column, y: &Column| {
            x == y
                || self.conjuncts.iter().any(|c| {
                    c.equality
                        .as_ref()
                        .is_some_and(|(a, b)| (a == x && b == y) || (a == y && b == x))
                })
        };
        self.outputs
            .iter()
            .find(|(_, column)| {
                column
                    .as_ref()
                    .is_some_and(|c| keys.iter().any(|k| equal(c, k)))
            })
            .map(|(name, _)| name.clone())
    }

    /// The roots whose rows a read of `occ` can reach: those of the UNION leaves
    /// it can meet.
    fn roots_of(&self, occ: usize) -> Vec<&Root> {
        let scope = &self.occurrences[occ].unions;
        self.roots
            .iter()
            .filter(|r| compatible(scope, &r.scope))
            .collect()
    }

    /// Whether a read of `occ` reaches rows of a UNION leaf whose key no table
    /// carries.
    fn reaches_hole(&self, occ: usize) -> bool {
        let scope = &self.occurrences[occ].unions;
        self.holes.iter().any(|h| compatible(scope, h))
    }

    /// The root a chain from `occ` ends at.
    fn root_at(&self, occ: usize, path: &[usize]) -> Option<&Root> {
        let mut at = occ;
        for &i in path {
            let c = &self.conjuncts[i];
            at = if c.a == at { c.b } else { c.a };
        }
        self.roots_of(occ).into_iter().find(|r| r.key.occ == at)
    }

    /// Classify one occurrence. `propagates(entity, relid)` tells whether entity
    /// propagation from that TVIEW covers a write to `relid` read through its view:
    /// this TVIEW embeds it, and it maps the table itself.
    #[must_use]
    pub fn classify(&self, occ: usize, propagates: &dyn Fn(&str, u32) -> bool) -> Kind {
        let kind = self.classify_read(occ, propagates);
        match &self.occurrences[occ].tview_table {
            Some(inner) => self.tview_kind(occ, inner, kind, propagates),
            None => kind,
        }
    }

    /// The kind of a read of another TVIEW's table: `propagated` when it
    /// is joined on that TVIEW's `pk_<entity>` by a TVIEW embedding it (refreshing
    /// the inner TVIEW looks the embedding rows up); otherwise its refreshes are
    /// mapped like writes to a base table, never by a row trigger.
    fn tview_kind(
        &self,
        occ: usize,
        inner: &str,
        kind: Kind,
        propagates: &dyn Fn(&str, u32) -> bool,
    ) -> Kind {
        let path = match &kind {
            Kind::Local(_) => self.occurrence_paths(occ, &kind).pop().unwrap_or_default(),
            Kind::Mapped(path) => path.clone(),
            _ => return kind,
        };
        let key = format!("pk_{inner}");
        let by_key = path
            .first()
            .and_then(|&i| self.conjuncts[i].equality.as_ref())
            .is_some_and(|(x, y)| [x, y].iter().any(|c| c.occ == occ && c.name == key));
        if by_key && propagates(inner, self.occurrences[occ].relid) {
            Kind::Propagated(inner.to_string())
        } else {
            Kind::Mapped(path)
        }
    }

    fn classify_read(&self, occ: usize, propagates: &dyn Fn(&str, u32) -> bool) -> Kind {
        let o = &self.occurrences[occ];
        if o.matview {
            let via = o
                .via_view
                .as_ref()
                .map(|view| format!(", read through view {view}"))
                .unwrap_or_default();
            return Kind::AllKeys(format!(
                "a materialized view: REFRESH MATERIALIZED VIEW replaces its rows without \
                 firing triggers{via}"
            ));
        }
        let roots = self.roots_of(occ);
        if roots.is_empty() || self.reaches_hole(occ) {
            // A top level whose rows a write changes beyond its own (a window
            // function, LIMIT…) has no root; say why.
            return Kind::AllKeys(o.opaque_level.clone().unwrap_or_else(|| {
                if self.roots.is_empty() {
                    "the TVIEW key is not a column of a base table".to_string()
                } else {
                    "the TVIEW key is not a column of a base table in every UNION branch"
                        .to_string()
                }
            }));
        }
        if let [root] = roots[..]
            && root.key.occ == occ
        {
            // A virtual or computed key is computed by the mapping query over the
            // changed rows.
            return if self.is_virtual(&root.key) || root.expr.is_some() {
                Kind::Mapped(Vec::new())
            } else {
                Kind::Local(root.key.name.clone())
            };
        }
        if let Some(entity) = &o.via_tview
            && propagates(entity, o.relid)
        {
            return Kind::Propagated(entity.clone());
        }
        if let [root] = roots[..] {
            return match self.path(occ, root.key.occ) {
                Some(path) => match self.local_column(&path, root) {
                    Some(col) => Kind::Local(col),
                    None => Kind::Mapped(path),
                },
                None => Kind::AllKeys(self.unlinked_reason(occ)),
            };
        }
        // Rows of every branch it can meet: a chain to each branch's root.
        let paths: Option<Vec<Vec<usize>>> = roots
            .iter()
            .map(|root| {
                if root.key.occ == occ {
                    Some(Vec::new())
                } else {
                    self.path(occ, root.key.occ)
                }
            })
            .collect();
        paths.map_or_else(|| Kind::AllKeys(self.unlinked_reason(occ)), Kind::Branches)
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
    /// equality: the key is then read off the changed row (unless it is virtual).
    fn local_column(&self, path: &[usize], root: &Root) -> Option<String> {
        let [only] = path else { return None };
        if root.expr.is_some() {
            return None;
        }
        let (x, y) = self.conjuncts[*only].equality.as_ref()?;
        let own = if *y == root.key {
            x
        } else if *x == root.key {
            y
        } else {
            return None;
        };
        (!self.is_virtual(own)).then(|| own.name.clone())
    }

    fn is_virtual(&self, column: &Column) -> bool {
        self.virtual_columns.contains(&(column.occ, column.attnum))
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
        let root = self.root_at(occ, path)?;
        if root.expr.is_some() {
            return None;
        }
        let [only] = path else { return None };
        let (x, y) = self.conjuncts[*only].equality.as_ref()?;
        let (own, other) = if x.occ == occ { (x, y) } else { (y, x) };
        (other.occ == root.key.occ && *other != root.key)
            .then(|| (own.name.clone(), other.name.clone()))
    }

    /// The fan-out of a write to `occ`, one equality from a root column: the
    /// output column equal to that root column, and the top-level `data` keys
    /// that copy columns of `occ` read nowhere else.
    fn fanout(&self, occ: usize, path: &[usize]) -> Option<(String, Vec<(String, String)>)> {
        let root = self.root_at(occ, path)?;
        let [only] = path else { return None };
        let (x, y) = self.conjuncts[*only].equality.as_ref()?;
        let other = if x.occ == occ { y } else { x };
        if other.occ != root.key.occ {
            return None;
        }
        let lookup = self.output_equal_to(std::slice::from_ref(other))?;
        let data = self.data.as_ref()?;
        let fields: Vec<(String, String)> = data
            .fields
            .iter()
            .filter(|f| f.column.occ == occ && f.only_in_data && f.path.len() == 1)
            .filter(|f| {
                data.fields
                    .iter()
                    .filter(|g| g.column == f.column)
                    .all(|g| g.path.len() == 1)
            })
            .map(|f| (f.column.name.clone(), f.path[0].clone()))
            .collect();
        (!fields.is_empty()).then_some((lookup, fields))
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

    /// The expressions of other occurrences the mapping queries look rows up by.
    #[must_use]
    pub fn index_hints(&self, paths: &[(usize, Vec<usize>)]) -> Vec<IndexHint> {
        let mut hints: Vec<IndexHint> = Vec::new();
        for (occ, path) in paths {
            for &i in self.kept_conditions(*occ, path) {
                for lookup in &self.conjuncts[i].lookups {
                    if lookup.occ == *occ {
                        continue;
                    }
                    let o = &self.occurrences[lookup.occ];
                    let expr = lookup
                        .expr
                        .0
                        .iter()
                        .map(|p| match p {
                            Piece::Text(t) => escape_template(t),
                            Piece::Column { occ, attnum } => {
                                format!("{{c:{}:{attnum}}}", self.occurrences[*occ].relid)
                            }
                        })
                        .collect::<String>();
                    let hint = IndexHint {
                        table: o.qualified.clone(),
                        relname: o.relname.clone(),
                        expr,
                        gin: lookup.gin,
                    };
                    if !hints.contains(&hint) {
                        hints.push(hint);
                    }
                }
            }
        }
        hints
    }

    /// The conditions of `path` its mapping query keeps: all but a last one that
    /// only copies the key (the root is then left out).
    fn kept_conditions<'p>(&self, occ: usize, path: &'p [usize]) -> &'p [usize] {
        match (self.root_at(occ, path), path.split_last()) {
            (Some(root), Some((&last, rest))) if self.copied_key(root, last).is_some() => rest,
            _ => path,
        }
    }

    /// The key written over the other column of `conjunct` when it is an equality
    /// with the root's key column that can stand for the root.
    fn copied_key(&self, root: &Root, conjunct: usize) -> Option<Sql> {
        let (x, y) = self.conjuncts[conjunct].equality.as_ref()?;
        let other = if *y == root.key {
            x
        } else if *x == root.key {
            y
        } else {
            return None;
        };
        root.sql_over(other)
    }

    /// `SELECT DISTINCT <key> FROM <delta>, <tables on the path> WHERE <conditions>`.
    /// The root is left out when the last condition copies its key verbatim.
    fn path_sql(&self, occ: usize, path: &[usize]) -> Option<String> {
        let root = self.root_at(occ, path)?;
        // The occurrences in path order, starting at the changed table.
        let mut chain = vec![occ];
        for &i in path {
            let c = &self.conjuncts[i];
            let at = *chain.last()?;
            chain.push(if c.a == at { c.b } else { c.a });
        }
        let mut conditions: Vec<&Conjunct> = path.iter().map(|&i| &self.conjuncts[i]).collect();
        let mut key = root.sql();
        if chain.len() > 1
            && let Some(&last) = path.last()
            && let Some(copied) = self.copied_key(root, last)
        {
            key = copied;
            conditions.pop();
            chain.pop();
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
        let mut sql = format!("SELECT DISTINCT {} FROM {from}", render(&key));
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
    fn occurrence_paths(&self, occ: usize, kind: &Kind) -> Vec<Vec<usize>> {
        match kind {
            Kind::Mapped(p) => vec![p.clone()],
            Kind::Branches(ps) => ps.clone(),
            // A local occurrence other than the root reads the key through its one
            // equality.
            _ => vec![
                self.roots_of(occ)
                    .first()
                    .filter(|r| r.key.occ != occ)
                    .and_then(|r| self.path(occ, r.key.occ))
                    .unwrap_or_default(),
            ],
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
                    // is left to the TVIEW's uncascaded_policy.
                    for (occ, k) in &kinds {
                        if matches!(k, Kind::Local(_) | Kind::Mapped(_) | Kind::Branches(_)) {
                            for path in self.occurrence_paths(*occ, k) {
                                paths.push((*occ, path));
                            }
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
                            for path in self.occurrence_paths(*occ, k) {
                                paths.push((*occ, path));
                            }
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
                let index_hints = self.index_hints(&paths);
                let hop = match paths.as_slice() {
                    [(occ, path)] if kind == TableKind::Mapped => self.root_hop(*occ, path),
                    _ => None,
                };
                let fanout = match paths.as_slice() {
                    [(occ, path)] if hop.is_some() => self.fanout(*occ, path),
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
                    index_hints,
                    hop,
                    fanout,
                    virtual_reads: Vec::new(),
                    matview: o.matview,
                    tview: o.tview_table.clone(),
                    root: self
                        .roots
                        .iter()
                        .any(|r| self.occurrences[r.key.occ].relid == relid),
                }
            })
            .collect()
    }
}
