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
    /// trigger sees them (#189).
    pub matview: bool,
    /// The table of another TVIEW, of that entity: its rows change when that
    /// TVIEW is refreshed (#191).
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
/// an index on the expression avoids (#182).
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
    /// go on by an equality (#165).
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
/// immutable expression of that occurrence's row (#188).
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

/// How a TVIEW's rows are named (ADR 0169).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityKind {
    /// `pk_<entity>`.
    Pk,
    /// The top-level DISTINCT ON key.
    DistinctOn,
}

impl IdentityKind {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Pk => "pk",
            Self::DistinctOn => "distinct_on",
        }
    }
}

/// An output column of the backing view's top level, as [`select_identity`] sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputColumn {
    pub name: String,
    /// Not part of the output (an ORDER BY or DISTINCT ON expression left unprojected).
    pub junk: bool,
    /// Its `DISTINCT ON` / `ORDER BY` reference, 0 for none.
    pub sortgroupref: u32,
    /// The base column it stands for, if it is one.
    pub column: Option<Column>,
    pub type_oid: u32,
}

/// The output column chosen as a TVIEW's identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectedIdentity {
    /// Index of the output column (0-based).
    pub position: usize,
    pub kind: IdentityKind,
}

/// Why a TVIEW has no identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityError {
    /// No `pk_<entity>` output column.
    Missing,
    /// More than one DISTINCT ON key (ADR 0169, D2).
    Composite,
    /// The DISTINCT ON key is not projected, and no projected column equals it.
    Unprojected,
    /// The DISTINCT ON key is projected but is not a column of a base table.
    NotAColumn,
}

/// Choose the output column that names a TVIEW's rows (ADR 0169): the top-level
/// DISTINCT ON key, projected or equal through `equal` (a strict equality of the top
/// level) to a projected column; `pk_<entity>` without DISTINCT ON.
///
/// # Errors
/// Returns why no output column can be the identity.
pub fn select_identity(
    entity: &str,
    outputs: &[OutputColumn],
    distinct_on: Option<&[u32]>,
    equal: &dyn Fn(&Column, &Column) -> bool,
) -> Result<SelectedIdentity, IdentityError> {
    let Some(refs) = distinct_on else {
        let key = format!("pk_{entity}");
        return outputs
            .iter()
            .position(|o| !o.junk && o.name == key)
            .map(|position| SelectedIdentity {
                position,
                kind: IdentityKind::Pk,
            })
            .ok_or(IdentityError::Missing);
    };
    let [sortgroupref] = refs else {
        return Err(IdentityError::Composite);
    };
    let Some(key) = outputs.iter().position(|o| o.sortgroupref == *sortgroupref) else {
        return Err(IdentityError::Unprojected);
    };
    let chosen = |position| {
        Ok(SelectedIdentity {
            position,
            kind: IdentityKind::DistinctOn,
        })
    };
    match (&outputs[key], outputs[key].junk) {
        (o, false) if o.column.is_some() => chosen(key),
        (_, false) => Err(IdentityError::NotAColumn),
        (
            OutputColumn {
                column: Some(column),
                ..
            },
            true,
        ) => outputs
            .iter()
            .position(|o| {
                !o.junk
                    && o.column
                        .as_ref()
                        .is_some_and(|c| c == column || equal(c, column))
            })
            .map_or(Err(IdentityError::Unprojected), chosen),
        (_, true) => Err(IdentityError::Unprojected),
    }
}

/// What [`walk`] reads from the backing view.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Graph {
    pub occurrences: Vec<Occurrence>,
    pub conjuncts: Vec<Conjunct>,
    pub roots: Vec<Root>,
    /// The UNION leaves whose key no table carries: their rows cannot be mapped.
    pub holes: Vec<Scope>,
    /// Functions the view calls that may read tables `pg_tviews` does not see:
    /// not immutable, outside `pg_catalog` (by OID).
    pub untracked_functions: Vec<u32>,
    /// How the view reads the current time, each construct once (#193):
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
    /// equalities: NULL in the rows a trigger sees, so never read off one (#179).
    pub virtual_columns: std::collections::BTreeSet<(usize, i16)>,
    /// Every other TVIEW the view reads (its backing view or its table, found by
    /// OID), with the base columns equal to its `pk_<entity>`: the column its key
    /// stands for in its backing view, or one an equality with its table's key
    /// names.
    pub tview_keys: BTreeMap<String, Vec<Column>>,
    /// The output columns of the backing view's own SELECT (none for a set
    /// operation), with the base column each stands for.
    pub outputs: Vec<(String, Option<Column>)>,
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
    /// UNION the key comes from (#188).
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
    /// when the TVIEW projects it (fan-out, issue #120).
    pub hop: Option<(String, String)>,
    /// The table whose column is the key (of a branch, under UNION).
    pub root: bool,
    /// The virtual generated columns the TVIEW reads and their inputs: never
    /// copied by a fast path, which reads the changed row (#179).
    pub virtual_reads: Vec<String>,
    /// A materialized view: no trigger can be installed on it (#189).
    pub matview: bool,
    /// The table of another TVIEW, of that entity (#191).
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

impl Graph {
    /// Every TVIEW the view reads, with the output column equal to its key: a
    /// change to its row `k` changes the rows whose column holds `k`. `None` when
    /// no output column carries its key.
    #[must_use]
    pub fn embed_lookups(&self) -> BTreeMap<String, Option<String>> {
        self.tview_keys
            .iter()
            .map(|(entity, keys)| (entity.clone(), self.output_equal_to(keys)))
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

    /// The kind of a read of another TVIEW's table (#191): `propagated` when it
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
                    // is left to the TVIEW's uncascaded_policy (#162).
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

// ── analysis of a registered or new TVIEW ───────────────────────────────────

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
    /// The aggregate TVIEWs (#58) the view reads, each with the output column
    /// equal to its key, if any (#126): no `fk_<aggregate>` column propagates a
    /// change of one of its groups, this column does.
    pub aggregate_embeds: Vec<(String, Option<String>)>,
    /// The functions it calls that may read tables it cannot see (not immutable,
    /// outside `pg_catalog`), as `(oid, schema.name(argument types))` (#193).
    pub functions: Vec<(u32, String)>,
    /// How it reads the current time (#193): its rows change with no write.
    pub time_reads: Vec<String>,
}

/// A table a function reads, declared with the TVIEW (#193).
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

/// The column that names a TVIEW's rows (ADR 0169).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    /// The output column.
    pub name: String,
    pub type_oid: u32,
    pub kind: IdentityKind,
    /// `(relid, attnum)` of the base column it stands for, per UNION branch.
    pub columns: Vec<(u32, i16)>,
}

/// The DISTINCT ON expressions of a view definition as `pg_get_viewdef` writes it
/// (`SELECT DISTINCT ON (a, f(b, c)) …`), for messages; empty without DISTINCT ON.
#[must_use]
pub fn distinct_on_list(viewdef: &str) -> Vec<String> {
    let Some(start) = viewdef.find("DISTINCT ON (") else {
        return Vec::new();
    };
    let mut items = Vec::new();
    let mut depth = 0_usize;
    let mut quote: Option<char> = None;
    let mut current = String::new();
    for ch in viewdef[start + "DISTINCT ON (".len()..].chars() {
        match (quote, ch) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '\'' | '"') => quote = Some(ch),
            (None, '(') => depth += 1,
            (None, ')') if depth == 0 => {
                items.push(current.trim().to_string());
                return items;
            }
            (None, ')') => depth -= 1,
            (None, ',') if depth == 0 => {
                items.push(current.trim().to_string());
                current.clear();
                continue;
            }
            _ => {}
        }
        current.push(ch);
    }
    Vec::new()
}

/// The refusal for a TVIEW without an identity; `keys` are its DISTINCT ON
/// expressions as written by PostgreSQL.
#[must_use]
pub fn identity_refusal(entity: &str, error: IdentityError, keys: &[String]) -> String {
    let key = keys.join(", ");
    match error {
        IdentityError::Missing => format!("tv_{entity} has no pk_{entity} output column"),
        IdentityError::Composite => format!(
            "tv_{entity} has a composite DISTINCT ON key ({key}): a TVIEW row is one entity, \
             addressed by one key (pk_<entity>, id or identifier), and its parents embed it \
             through one fk_<entity>. Model one row per ({key}) as an entity of its own, a \
             tb_<entity> table with its pk_<entity>, or DISTINCT ON one column"
        ),
        IdentityError::Unprojected => format!(
            "the DISTINCT ON key of tv_{entity} ({key}) names its rows, but it is not an output \
             column and no output column equals it: project it (… AS <name>)"
        ),
        IdentityError::NotAColumn => format!(
            "the DISTINCT ON key of tv_{entity} ({key}) names its rows, but it is not a column of \
             a base table, so writes cannot be mapped to them: DISTINCT ON a column"
        ),
    }
}

impl Lineage {
    /// Add the tables functions read (#193): no cascade reaches them, so each is
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
                    root: false,
                    virtual_reads: Vec::new(),
                    matview: read.matview,
                    tview: read.tview.clone(),
                }),
            }
        }
    }

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

/// Analyze the backing view `view_oid` of `entity`.
///
/// `embeds` lists the TVIEWs it embeds through `fk_<entity>` columns; the
/// aggregate TVIEWs it reads are embeds too, found by the walk. `base_tables` is
/// what `pg_depend` says the view reads, which the analysis must find exactly.
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
    for other in crate::catalog::registered::all()? {
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
    // pg_depend and the query tree must agree on the tables.
    let found: HashSet<u32> = graph
        .occurrences
        .iter()
        .filter(|o| o.tview_table.is_none())
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
        return Err(crate::TViewError::DefinitionRefused {
            reason: format!(
                "pg_tviews could not follow how tv_{entity} reads its base tables \
                 (not found in the view's query: [{}]; not in pg_depend: [{}])",
                missing.join(", "),
                extra.join(", ")
            ),
        });
    }

    let functions = function_signatures(&graph.untracked_functions).map_err(catalog)?;

    // The aggregate TVIEWs the view reads embed through the output equal to their
    // key (#126).
    let mut lookups = graph.embed_lookups();
    let aggregate_embeds: Vec<(String, Option<String>)> = aggregates
        .into_iter()
        .filter_map(|a| lookups.remove(&a).map(|column| (a, column)))
        .collect();
    let embeds: Vec<&str> = embeds
        .iter()
        .map(String::as_str)
        .chain(
            aggregate_embeds
                .iter()
                .filter(|(_, column)| column.is_some())
                .map(|(a, _)| a.as_str()),
        )
        .collect();
    // Propagation from an embedded TVIEW covers a table only if that TVIEW maps it.
    // Or, for a read of its table, only that it embeds it (#191).
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
    })
}

/// `(oid, schema.name(argument types))` of each function.
fn function_signatures(oids: &[u32]) -> Result<Vec<(u32, String)>, pgrx::spi::Error> {
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

/// The identity of the view `view_oid` as the TVIEW of `entity` (ADR 0169).
///
/// # Errors
/// Returns an error if the view cannot be walked, or if no column of it can name
/// the TVIEW's rows (the refusal names its DISTINCT ON key).
pub fn view_identity(entity: &str, view_oid: pgrx::pg_sys::Oid) -> crate::TViewResult<Identity> {
    let key_column = format!("pk_{entity}");
    let graph = walk::analyze(
        view_oid,
        &walk::Context {
            tview_tables: &std::collections::HashMap::new(),
            tview_views: &std::collections::HashMap::new(),
            entity,
            key_column: &key_column,
        },
    )?;
    identity_of(entity, view_oid, &graph)
}

/// The identity a walk of `view_oid` found, or the refusal naming its DISTINCT ON key.
fn identity_of(
    entity: &str,
    view_oid: pgrx::pg_sys::Oid,
    graph: &Graph,
) -> crate::TViewResult<Identity> {
    use pgrx::prelude::*;
    match graph
        .identity
        .clone()
        .unwrap_or(Err(IdentityError::Missing))
    {
        Ok(walked) => Ok(Identity {
            name: walked.name,
            type_oid: walked.type_oid,
            kind: walked.kind,
            columns: walked
                .columns
                .iter()
                .map(|c| (graph.occurrences[c.occ].relid, c.attnum))
                .collect(),
        }),
        Err(error) => {
            let viewdef = Spi::get_one_with_args::<String>(
                "SELECT pg_catalog.pg_get_viewdef($1)",
                &[crate::utils::spi::oid(view_oid)],
            )
            .map_err(|e| crate::TViewError::CatalogError {
                operation: format!("Read the definition of the view of tv_{entity}"),
                pg_error: e.to_string(),
            })?
            .unwrap_or_default();
            Err(crate::TViewError::DefinitionRefused {
                reason: identity_refusal(entity, error, &distinct_on_list(&viewdef)),
            })
        }
    }
}

/// A mapping-query template with the current names of its relations and columns;
/// `None` when one of them is gone.
///
/// # Errors
/// Returns an error if the catalog cannot be read.
pub fn render_template(template: &str) -> crate::TViewResult<Option<String>> {
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
        let args = [
            crate::utils::spi::oid(pgrx::pg_sys::Oid::from(relid)),
            crate::utils::spi::int2(attnum),
        ];
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
    /// The table of another TVIEW, of that entity (#191): refreshed first.
    #[serde(default)]
    pub tview: Option<String>,
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

/// A virtual generated column (`attnum`) and the columns its expression reads.
pub type VirtualInputs = Vec<(i16, Vec<(String, i16)>)>;

/// The columns a TVIEW reads of a table, `read`, with the inputs of every virtual
/// generated column among them (#179): a virtual column has no value in the rows a
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
fn referenced_columns(
    view_oid: pgrx::pg_sys::Oid,
    relid: u32,
) -> crate::TViewResult<Vec<(String, i16)>> {
    crate::catalog::reads::view_columns_read(view_oid, pgrx::pg_sys::Oid::from(relid))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn occ(relid: u32, relname: &str) -> Occurrence {
        Occurrence {
            relid,
            relname: relname.to_string(),
            qualified: format!("public.{relname}"),
            unions: Vec::new(),
            via_view: None,
            via_tview: None,
            in_sublink: false,
            opaque_level: None,
            matview: false,
            tview_table: None,
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
            lookups: vec![],
        }
    }

    fn graph(occurrences: Vec<Occurrence>, conjuncts: Vec<Conjunct>, key: Column) -> Graph {
        Graph {
            occurrences,
            conjuncts,
            roots: vec![Root {
                key,
                expr: None,
                scope: Vec::new(),
            }],
            holes: vec![],
            untracked_functions: vec![],
            time_reads: vec![],
            unread_tables: std::collections::BTreeSet::new(),
            identity: None,
            set_operation: false,
            virtual_columns: std::collections::BTreeSet::new(),
            tview_keys: BTreeMap::new(),
            outputs: vec![],
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
            lookups: vec![],
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
            holes: vec![],
            untracked_functions: vec![],
            time_reads: vec![],
            unread_tables: std::collections::BTreeSet::new(),
            identity: None,
            set_operation: false,
            virtual_columns: std::collections::BTreeSet::new(),
            tview_keys: BTreeMap::new(),
            outputs: vec![],
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

    /// `a.pk_node = ANY (arr(n.path)) AND arr(n.path) @> ARRAY[a.pk_node]`, with the
    /// array of `n` (occurrence `n`) looked up by a GIN index (#182).
    fn membership(a: usize, n: usize) -> Conjunct {
        let path = Column {
            occ: n,
            attnum: 3,
            name: "path".to_string(),
        };
        let mut array = Sql::text("(pg_catalog.string_to_array(");
        array.push_sql(path.sql());
        array.push_text(", '.'::pg_catalog.text))::bigint[]");
        let mut sql = Sql::text("((");
        sql.push_sql(col(a, "pk_node").sql());
        sql.push_text(" OPERATOR(pg_catalog.=) ANY (");
        sql.push_sql(array.clone());
        sql.push_text(")) AND (");
        sql.push_sql(array.clone());
        sql.push_text(") OPERATOR(pg_catalog.@>) ARRAY[");
        sql.push_sql(col(a, "pk_node").sql());
        sql.push_text("])");
        Conjunct {
            sql,
            a,
            b: n,
            a_to_b: Maps::Yes,
            b_to_a: Maps::Yes,
            equality: None,
            lookups: vec![Lookup {
                occ: n,
                expr: array,
                gin: true,
            }],
        }
    }

    #[test]
    fn an_ancestor_read_through_array_membership_is_mapped() {
        // tb_node n JOIN tb_node a ON a.pk_node = ANY (string_to_array(n.path, '.')::bigint[])
        let g = graph(
            vec![occ(7, "tb_node"), occ(7, "tb_node")],
            vec![membership(1, 0)],
            col(0, "pk_node"),
        );
        assert_eq!(g.classify(0, NONE), Kind::Local("pk_node".into()));
        assert_eq!(g.classify(1, NONE), Kind::Mapped(vec![0]));
        let tables = g.tables(NONE);
        assert_eq!(tables.len(), 1);
        assert_eq!(tables[0].kind, TableKind::Mapped);
        assert_eq!(
            tables[0].sql.as_deref(),
            Some(
                "SELECT DISTINCT d.{c:7:1} FROM pg_tviews_delta d UNION \
                 SELECT DISTINCT o1.{c:7:1} FROM pg_tviews_delta d, {r:7} o1 \
                 WHERE ((d.{c:7:1} OPERATOR(pg_catalog.=) ANY ((pg_catalog.string_to_array(o1.{c:7:3}, \
                 '.'::pg_catalog.text))::bigint[])) AND ((pg_catalog.string_to_array(o1.{c:7:3}, \
                 '.'::pg_catalog.text))::bigint[]) OPERATOR(pg_catalog.@>) ARRAY[d.{c:7:1}])"
            )
        );
    }

    #[test]
    fn a_lookup_by_expression_names_its_index() {
        let g = graph(
            vec![occ(7, "tb_node"), occ(7, "tb_node")],
            vec![membership(1, 0)],
            col(0, "pk_node"),
        );
        let tables = g.tables(NONE);
        assert_eq!(
            tables[0].index_hints,
            vec![IndexHint {
                table: "public.tb_node".into(),
                relname: "tb_node".into(),
                expr: "(pg_catalog.string_to_array({c:7:3}, '.'::pg_catalog.text))::bigint[]"
                    .into(),
                gin: true,
            }]
        );
    }

    #[test]
    fn the_changed_side_of_a_lookup_needs_no_index() {
        // A write to n maps through its own key: its array is read off the change.
        let g = graph(
            vec![occ(7, "tb_node"), occ(8, "tb_ancestor")],
            vec![membership(1, 0)],
            col(0, "pk_node"),
        );
        let tables = g.tables(NONE);
        let node = tables.iter().find(|t| t.relname == "tb_node").unwrap();
        let ancestor = tables.iter().find(|t| t.relname == "tb_ancestor").unwrap();
        assert!(node.index_hints.is_empty());
        assert_eq!(ancestor.index_hints.len(), 1);
    }

    // ── embeds of other TVIEWs (#126, #181) ─────────────────────────────────

    /// `tb_user u` (0) joined to the backing view of `user_summary`, whose key is
    /// `tb_order.fk_user` (1), projecting `outputs`.
    fn summary_graph(join: Vec<Conjunct>, outputs: Vec<(&str, Column)>) -> Graph {
        let mut g = graph(
            vec![occ(1, "tb_user"), occ(2, "tb_order")],
            join,
            col(0, "pk_user"),
        );
        g.tview_keys
            .insert("user_summary".into(), vec![col(1, "fk_user")]);
        g.outputs = outputs
            .into_iter()
            .map(|(name, c)| (name.to_string(), Some(c)))
            .collect();
        g
    }

    #[test]
    fn an_embed_is_found_through_an_equality_with_its_key() {
        // tb_user u LEFT JOIN <summary view> s ON s.pk_user_summary = u.pk_user
        let g = summary_graph(
            vec![eq(col(1, "fk_user"), col(0, "pk_user"), true, false)],
            vec![("pk_user", col(0, "pk_user")), ("id", col(0, "id"))],
        );
        assert_eq!(
            g.embed_lookups(),
            BTreeMap::from([("user_summary".to_string(), Some("pk_user".to_string()))])
        );
    }

    #[test]
    fn an_embed_lookup_uses_the_output_name() {
        // tb_post p JOIN tv_tag_count c ON p.fk_author = c.pk_tag_count, `p.fk_author AS author`
        let mut g = graph(vec![occ(1, "tb_post")], vec![], col(0, "pk_post"));
        g.tview_keys
            .insert("tag_count".into(), vec![col(0, "fk_author")]);
        g.outputs = vec![
            ("pk_post".into(), Some(col(0, "pk_post"))),
            ("author".into(), Some(col(0, "fk_author"))),
        ];
        assert_eq!(
            g.embed_lookups(),
            BTreeMap::from([("tag_count".to_string(), Some("author".to_string()))])
        );
    }

    #[test]
    fn an_embed_whose_key_no_output_carries_has_no_lookup() {
        // tb_post p JOIN <summary view> s ON s.pk_user_summary = p.fk_user, fk_user not projected
        let g = summary_graph(
            vec![eq(col(1, "fk_user"), col(0, "fk_user"), true, false)],
            vec![("pk_user", col(0, "pk_user"))],
        );
        assert_eq!(
            g.embed_lookups(),
            BTreeMap::from([("user_summary".to_string(), None)])
        );
    }

    #[test]
    fn a_tview_not_read_has_no_lookup() {
        let g = graph(vec![occ(1, "tb_user")], vec![], col(0, "pk_user"));
        assert!(g.embed_lookups().is_empty());
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

    // ── identity ────────────────────────────────────────────────────────────

    fn out(name: &str, junk: bool, sortgroupref: u32, column: Option<Column>) -> OutputColumn {
        OutputColumn {
            name: name.to_string(),
            junk,
            sortgroupref,
            column,
            type_oid: 20,
        }
    }

    fn at(occ: usize, name: &str, attnum: i16) -> Column {
        Column {
            occ,
            attnum,
            name: name.to_string(),
        }
    }

    const UNEQUAL: &dyn Fn(&Column, &Column) -> bool = &|_, _| false;

    #[test]
    fn identity_without_distinct_on_is_pk_entity() {
        let outputs = [
            out("id", false, 0, Some(at(0, "id", 2))),
            out("pk_order", false, 0, Some(at(0, "pk_order", 1))),
        ];
        assert_eq!(
            select_identity("order", &outputs, None, UNEQUAL),
            Ok(SelectedIdentity {
                position: 1,
                kind: IdentityKind::Pk
            })
        );
    }

    #[test]
    fn identity_without_pk_entity_is_missing() {
        let outputs = [out("id", false, 0, Some(at(0, "id", 2)))];
        assert_eq!(
            select_identity("order", &outputs, None, UNEQUAL),
            Err(IdentityError::Missing)
        );
    }

    #[test]
    fn identity_is_a_projected_distinct_on_root_column() {
        // DISTINCT ON (c.id_contract) c.id_contract AS pk_contract
        let outputs = [
            out("pk_contract", false, 1, Some(at(0, "id_contract", 3))),
            out("id", false, 0, Some(at(0, "id", 2))),
        ];
        assert_eq!(
            select_identity("contract", &outputs, Some(&[1]), UNEQUAL),
            Ok(SelectedIdentity {
                position: 0,
                kind: IdentityKind::DistinctOn
            })
        );
    }

    #[test]
    fn identity_is_a_projected_distinct_on_column_other_than_pk() {
        // DISTINCT ON (o.id) o.pk_order, o.id
        let outputs = [
            out("pk_order", false, 0, Some(at(0, "pk_order", 1))),
            out("id", false, 1, Some(at(0, "id", 2))),
        ];
        assert_eq!(
            select_identity("order", &outputs, Some(&[1]), UNEQUAL),
            Ok(SelectedIdentity {
                position: 1,
                kind: IdentityKind::DistinctOn
            })
        );
    }

    #[test]
    fn identity_is_a_projected_joined_column() {
        // DISTINCT ON (l.fk_order) l.fk_order AS pk_lastline, o.id … FROM tb_line l JOIN tb_order o
        let outputs = [
            out("pk_lastline", false, 1, Some(at(0, "fk_order", 3))),
            out("id", false, 0, Some(at(1, "id", 2))),
        ];
        assert_eq!(
            select_identity("lastline", &outputs, Some(&[1]), UNEQUAL),
            Ok(SelectedIdentity {
                position: 0,
                kind: IdentityKind::DistinctOn
            })
        );
    }

    #[test]
    fn identity_is_a_projected_column_equal_to_an_unprojected_key() {
        // DISTINCT ON (l.fk_order) o.pk_order … JOIN ON o.pk_order = l.fk_order
        let outputs = [
            out("pk_order", false, 0, Some(at(1, "pk_order", 1))),
            out("id", false, 0, Some(at(1, "id", 2))),
            out("fk_order", true, 1, Some(at(0, "fk_order", 3))),
        ];
        let equal = |a: &Column, b: &Column| {
            a.occ != b.occ
                && [a.name.as_str(), b.name.as_str()].contains(&"fk_order")
                && [a.name.as_str(), b.name.as_str()].contains(&"pk_order")
        };
        assert_eq!(
            select_identity("order", &outputs, Some(&[1]), &equal),
            Ok(SelectedIdentity {
                position: 0,
                kind: IdentityKind::DistinctOn
            })
        );
    }

    #[test]
    fn identity_of_an_unprojected_expression_is_refused() {
        // DISTINCT ON (lower(o.ref)) o.pk_order …
        let outputs = [
            out("pk_order", false, 0, Some(at(0, "pk_order", 1))),
            out("?column?", true, 1, None),
        ];
        assert_eq!(
            select_identity("order", &outputs, Some(&[1]), UNEQUAL),
            Err(IdentityError::Unprojected)
        );
    }

    #[test]
    fn identity_of_an_unprojected_column_nothing_equals_is_refused() {
        // DISTINCT ON (o.ref) o.pk_order …
        let outputs = [
            out("pk_order", false, 0, Some(at(0, "pk_order", 1))),
            out("ref", true, 1, Some(at(0, "ref", 3))),
        ];
        assert_eq!(
            select_identity("order", &outputs, Some(&[1]), UNEQUAL),
            Err(IdentityError::Unprojected)
        );
    }

    #[test]
    fn identity_of_a_projected_expression_is_refused() {
        // DISTINCT ON (lower(o.ref)) lower(o.ref) AS code, o.pk_order …
        let outputs = [
            out("code", false, 1, None),
            out("pk_order", false, 0, Some(at(0, "pk_order", 1))),
        ];
        assert_eq!(
            select_identity("order", &outputs, Some(&[1]), UNEQUAL),
            Err(IdentityError::NotAColumn)
        );
    }

    #[test]
    fn composite_identity_is_refused() {
        // DISTINCT ON (s.sku, s.warehouse)
        let outputs = [
            out("pk_stock", false, 0, Some(at(0, "pk_stock", 1))),
            out("sku", false, 1, Some(at(0, "sku", 3))),
            out("warehouse", false, 2, Some(at(0, "warehouse", 4))),
        ];
        assert_eq!(
            select_identity("stock", &outputs, Some(&[1, 2]), UNEQUAL),
            Err(IdentityError::Composite)
        );
    }

    #[test]
    fn distinct_on_list_splits_top_level_commas() {
        assert_eq!(
            distinct_on_list(
                " SELECT DISTINCT ON (s.sku, lower((s.warehouse)::text), f(a, ')')) s.pk_stock"
            ),
            vec!["s.sku", "lower((s.warehouse)::text)", "f(a, ')')"]
        );
        assert_eq!(
            distinct_on_list(" SELECT DISTINCT ON (o.id) o.id"),
            vec!["o.id"]
        );
        assert!(distinct_on_list(" SELECT o.id FROM t").is_empty());
    }

    #[test]
    fn a_root_keyed_on_a_virtual_column_is_mapped() {
        // DISTINCT ON (v.code) with code virtual: the row trigger cannot read it.
        let mut g = graph(vec![occ(1, "tb_ver")], vec![], col(0, "code"));
        g.virtual_columns.insert((0, 1));
        assert_eq!(g.classify(0, NONE), Kind::Mapped(vec![]));
        assert_eq!(
            g.mapping_sql(&[(0, vec![])]),
            "SELECT DISTINCT d.{c:1:1} FROM pg_tviews_delta d"
        );
    }

    #[test]
    fn a_table_joined_on_a_virtual_column_is_mapped() {
        // tb_order o LEFT JOIN tb_ref r ON r.ord = o.pk_order, ord virtual
        let ord = Column {
            occ: 1,
            attnum: 4,
            name: "ord".into(),
        };
        let mut g = graph(
            vec![occ(1, "tb_order"), occ(2, "tb_ref")],
            vec![eq(ord, col(0, "pk_order"), true, false)],
            col(0, "pk_order"),
        );
        assert_eq!(g.classify(1, NONE), Kind::Local("ord".into()));
        g.virtual_columns.insert((1, 4));
        assert_eq!(g.classify(1, NONE), Kind::Mapped(vec![0]));
        assert_eq!(
            g.mapping_sql(&[(1, vec![0])]),
            "SELECT DISTINCT d.{c:2:4} FROM pg_tviews_delta d"
        );
    }

    #[test]
    fn a_virtual_column_read_adds_its_inputs() {
        let read = vec![("pk_shop".to_string(), 1), ("code".to_string(), 3)];
        let virtual_inputs = vec![
            (3, vec![("name".to_string(), 2)]),
            (5, vec![("other".to_string(), 4)]),
        ];
        assert_eq!(
            expand_read_columns(read, &virtual_inputs),
            vec![
                ("pk_shop".to_string(), 1),
                ("name".to_string(), 2),
                ("code".to_string(), 3)
            ]
        );
    }

    #[test]
    fn virtual_reads_name_the_virtual_columns_read_and_their_inputs() {
        let read = vec![("name".to_string(), 2), ("code".to_string(), 3)];
        let virtual_inputs = vec![
            (3, vec![("name".to_string(), 2)]),
            (5, vec![("other".to_string(), 4)]),
        ];
        assert_eq!(virtual_reads(&read, &virtual_inputs), vec!["code", "name"]);
        assert!(virtual_reads(&read, &[]).is_empty());
    }

    #[test]
    fn inputs_already_read_and_no_virtual_columns_change_nothing() {
        let read = vec![("price".to_string(), 2), ("taxed".to_string(), 3)];
        assert_eq!(
            expand_read_columns(read.clone(), &[(3, vec![("price".to_string(), 2)])]),
            read
        );
        assert_eq!(expand_read_columns(read.clone(), &[]), read);
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

    // ── UNION branch keys (#188), materialized views (#189) ─────────────────

    /// `-<column>`, the line branch's key of #188.
    fn negated(c: &Column) -> Sql {
        let mut sql = Sql::text("(OPERATOR(pg_catalog.-) ");
        sql.push_sql(c.sql());
        sql.push_text(")");
        sql
    }

    /// `SELECT p.pk_product … FROM tb_product p UNION ALL SELECT -l.pk_order_line …
    /// FROM tb_order_line l`, read by a definition that joins `tb_note n` to the
    /// union's id: occurrence 0 is p (leaf 0), 1 is l (leaf 1), 2 is n (outside).
    fn two_branches() -> Graph {
        let mut p = occ(1, "tb_product");
        p.unions = vec![(1, 0)];
        let mut l = occ(2, "tb_order_line");
        l.unions = vec![(1, 1)];
        let mut g = graph(
            vec![p, l, occ(3, "tb_note")],
            vec![
                eq(col(2, "target"), col(0, "id"), true, true),
                eq(col(2, "target"), col(1, "id"), true, true),
            ],
            col(0, "pk_product"),
        );
        g.roots = vec![
            Root {
                key: col(0, "pk_product"),
                expr: None,
                scope: vec![(1, 0)],
            },
            Root {
                key: col(1, "pk_order_line"),
                expr: Some(negated(&col(1, "pk_order_line"))),
                scope: vec![(1, 1)],
            },
        ];
        g
    }

    #[test]
    fn scopes_meet_unless_they_take_different_leaves_of_one_union() {
        assert!(compatible(&vec![], &vec![(1, 0)]));
        assert!(compatible(&vec![(1, 0)], &vec![(1, 0), (2, 1)]));
        assert!(compatible(&vec![(1, 0)], &vec![(2, 1)]));
        assert!(!compatible(&vec![(1, 0)], &vec![(1, 1)]));
        assert!(!compatible(&vec![(2, 1), (1, 0)], &vec![(1, 1)]));
    }

    #[test]
    fn each_branch_root_maps_its_own_rows() {
        let g = two_branches();
        assert_eq!(g.classify(0, NONE), Kind::Local("pk_product".into()));
        // A computed key is computed by the mapping query.
        assert_eq!(g.classify(1, NONE), Kind::Mapped(vec![]));
        assert_eq!(
            g.mapping_sql(&[(1, vec![])]),
            "SELECT DISTINCT (OPERATOR(pg_catalog.-) d.{c:2:1}) FROM pg_tviews_delta d"
        );
    }

    #[test]
    fn a_read_outside_the_union_maps_to_every_branch() {
        let g = two_branches();
        assert_eq!(g.classify(2, NONE), Kind::Branches(vec![vec![0], vec![1]]));
        let tables = g.tables(NONE);
        assert_eq!(tables[2].kind, TableKind::Mapped);
        assert_eq!(
            tables[2].sql.as_deref(),
            Some(
                "SELECT DISTINCT o1.{c:1:1} FROM pg_tviews_delta d, {r:1} o1 \
                 WHERE d.{c:3:1} OPERATOR(pg_catalog.=) o1.{c:1:1} UNION \
                 SELECT DISTINCT (OPERATOR(pg_catalog.-) o1.{c:2:1}) FROM pg_tviews_delta d, {r:2} o1 \
                 WHERE d.{c:3:1} OPERATOR(pg_catalog.=) o1.{c:2:1}"
            )
        );
        // Two roots: rows are recomputed, never patched through a hop.
        assert_eq!(tables[2].hop, None);
    }

    #[test]
    fn an_equality_with_a_computed_key_column_stands_for_the_root() {
        // tb_line.fk_order_line = l.pk_order_line, in the line branch.
        let mut g = two_branches();
        let mut line = occ(4, "tb_line");
        line.unions = vec![(1, 1)];
        g.occurrences.push(line);
        g.conjuncts.push(eq(
            col(3, "fk_order_line"),
            col(1, "pk_order_line"),
            true,
            true,
        ));
        assert_eq!(g.classify(3, NONE), Kind::Mapped(vec![2]));
        assert_eq!(
            g.mapping_sql(&[(3, vec![2])]),
            "SELECT DISTINCT (OPERATOR(pg_catalog.-) d.{c:4:1}) FROM pg_tviews_delta d"
        );
    }

    #[test]
    fn a_branch_without_a_key_leaves_what_reaches_it_all_keys() {
        let mut g = two_branches();
        g.roots.pop();
        g.holes = vec![vec![(1, 1)]];
        assert_eq!(g.classify(0, NONE), Kind::Local("pk_product".into()));
        assert!(
            matches!(g.classify(1, NONE), Kind::AllKeys(r) if r.contains("every UNION branch"))
        );
        assert!(matches!(g.classify(2, NONE), Kind::AllKeys(_)));
    }

    #[test]
    fn a_materialized_view_is_all_keys_even_when_linked() {
        let mut g = graph(
            vec![occ(1, "tb_customer"), occ(2, "mv_order_count")],
            vec![eq(col(1, "fk_customer"), col(0, "pk_customer"), true, true)],
            col(0, "pk_customer"),
        );
        g.occurrences[1].matview = true;
        assert!(matches!(g.classify(1, NONE), Kind::AllKeys(r) if r.contains("materialized view")));
        assert!(g.tables(NONE)[1].matview);
    }

    // ── reads of another TVIEW's table (#191) ───────────────────────────────

    /// `tb_note n` (root) and `tv_line l`, joined on `cond`.
    fn note_and_line(cond: Conjunct) -> Graph {
        let mut line = occ(2, "tv_line");
        line.tview_table = Some("line".into());
        graph(vec![occ(1, "tb_note"), line], vec![cond], col(0, "pk_note"))
    }

    #[test]
    fn a_tview_table_joined_on_its_key_by_an_embed_is_propagated() {
        let g = note_and_line(eq(col(1, "pk_line"), col(0, "fk_line"), true, true));
        let embeds = |child: &str, _relid: u32| child == "line";
        assert_eq!(g.classify(1, &embeds), Kind::Propagated("line".into()));
        // Without the embed, its refreshes are mapped.
        assert_eq!(g.classify(1, NONE), Kind::Mapped(vec![0]));
    }

    #[test]
    fn a_tview_table_linked_otherwise_is_mapped_never_local() {
        // l.order_id = n.pk_note: an equality with the key, which a base table
        // would read off its row (local).
        let g = note_and_line(eq(col(1, "order_id"), col(0, "pk_note"), true, true));
        let embeds = |child: &str, _relid: u32| child == "line";
        assert_eq!(g.classify(1, &embeds), Kind::Mapped(vec![0]));
        let tables = g.tables(&embeds);
        assert_eq!(tables[1].kind, TableKind::Mapped);
        assert_eq!(tables[1].tview.as_deref(), Some("line"));
        assert_eq!(
            g.tables(&embeds)[1].sql.as_deref(),
            Some("SELECT DISTINCT d.{c:2:1} FROM pg_tviews_delta d")
        );
    }

    /// `p.pk_product = f.fk_product` with `f.fk_product` an inbound column of a
    /// first-row level (#194): it maps a product toward the orders carrying it,
    /// never an order away from it.
    fn inbound(p: usize, f: usize) -> Conjunct {
        let mut c = eq(col(p, "pk_product"), col(f, "fk_product"), true, false);
        c.equality = None;
        c
    }

    #[test]
    fn a_table_joined_to_an_inbound_column_maps_through_the_first_row_key() {
        // tb_customer c LEFT JOIN (first order per customer) f ON f.fk_customer = c.pk_customer
        // LEFT JOIN tb_product p ON p.pk_product = f.fk_product
        let g = graph(
            vec![
                occ(1, "tb_customer"),
                occ(2, "tb_order"),
                occ(3, "tb_product"),
            ],
            vec![
                eq(col(1, "fk_customer"), col(0, "pk_customer"), true, false),
                inbound(2, 1),
            ],
            col(0, "pk_customer"),
        );
        assert_eq!(g.classify(2, NONE), Kind::Mapped(vec![1, 0]));
        assert_eq!(g.classify(1, NONE), Kind::Local("fk_customer".into()));
    }

    #[test]
    fn a_first_row_level_does_not_map_away_through_an_inbound_column() {
        // tb_product p, (SELECT count(*) FROM first orders f WHERE f.fk_product = p.pk_product)
        let g = graph(
            vec![occ(3, "tb_product"), occ(2, "tb_order")],
            vec![inbound(0, 1)],
            col(0, "pk_product"),
        );
        assert!(matches!(g.classify(1, NONE), Kind::AllKeys(_)));
    }

    fn table(relid: u32, kind: TableKind, sql: Option<&str>) -> TableLineage {
        TableLineage {
            relid,
            relname: format!("t{relid}"),
            qualified: format!("public.t{relid}"),
            kind,
            paths: vec![],
            sql: sql.map(str::to_string),
            columns: vec![],
            lookups: vec![],
            index_hints: vec![],
            hop: None,
            root: false,
            virtual_reads: vec![],
            matview: false,
            tview: None,
        }
    }

    #[test]
    fn a_function_read_is_all_keys_and_keeps_the_traced_reads() {
        let mut lineage = Lineage {
            tables: vec![
                table(1, TableKind::Local("pk_a".into()), None),
                table(2, TableKind::Mapped, Some("SELECT 1")),
            ],
            unread: vec![],
            identity: Identity {
                name: "pk_a".into(),
                type_oid: 20,
                kind: IdentityKind::Pk,
                columns: vec![],
            },
            set_operation: false,
            aggregate_embeds: vec![],
            functions: vec![],
            time_reads: vec![],
        };
        let read = |relid: u32| FunctionRead {
            function: "public.f()".into(),
            relid,
            relname: format!("t{relid}"),
            qualified: format!("public.t{relid}"),
            matview: false,
            tview: None,
        };
        lineage.add_function_reads(&[read(1), read(2), read(3)]);
        let reason = TableKind::AllKeys("read inside public.f()".into());
        assert_eq!(lineage.tables[0].kind, reason);
        assert_eq!(
            lineage.tables[0].sql.as_deref(),
            Some("SELECT DISTINCT \"pk_a\" FROM pg_tviews_delta")
        );
        assert_eq!(lineage.tables[1].kind, reason);
        assert_eq!(lineage.tables[1].sql.as_deref(), Some("SELECT 1"));
        assert_eq!(lineage.tables[2].kind, reason);
        assert_eq!(lineage.tables[2].sql, None);
    }
}
