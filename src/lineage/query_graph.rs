//! The graph the walk reads a backing view into: the table occurrences, the
//! predicates that link them, the TVIEW's key and data shape; and what
//! [`super::classify`] derives from it.

use super::{IdentityError, IdentityKind, Piece, Sql};
use std::collections::BTreeMap;

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

/// A relation whose rows change without firing a trigger `pg_tviews` could install.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unwatched {
    /// `REFRESH MATERIALIZED VIEW` replaces its rows; the `ProcessUtility` hook
    /// follows it.
    Matview,
    /// Its rows live on another server.
    Foreign,
    /// `nextval()` and `setval()` change it.
    Sequence,
}

impl Unwatched {
    /// What a relation of kind `relkind` is, when no trigger watches it.
    #[must_use]
    pub const fn of_relkind(relkind: u8) -> Option<Self> {
        match relkind {
            b'm' => Some(Self::Matview),
            b'f' => Some(Self::Foreign),
            b'S' => Some(Self::Sequence),
            _ => None,
        }
    }

    /// Why a write to it maps to no key.
    #[must_use]
    pub const fn reason(self) -> &'static str {
        match self {
            Self::Matview => {
                "a materialized view: REFRESH MATERIALIZED VIEW replaces its rows without firing triggers"
            }
            Self::Foreign => {
                "a foreign table: its rows change on another server, which fires no trigger here"
            }
            Self::Sequence => "a sequence: nextval() changes it without firing a trigger",
        }
    }

    /// Whether `pg_tviews` learns that it changed (a materialized view's REFRESH),
    /// so a full refresh can follow.
    #[must_use]
    pub const fn followed(self) -> bool {
        matches!(self, Self::Matview)
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
    /// A relation no trigger watches (a materialized view, a foreign table, a
    /// sequence).
    pub unwatched: Option<Unwatched>,
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
    pub(super) fn sql_over(&self, column: &Column) -> Option<Sql> {
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
    /// `(occurrence, attnum)` of the columns the rows are linked by: read in a
    /// join or filter condition, a grouping, … (ADR 0207, #263).
    pub links: std::collections::BTreeSet<(usize, i16)>,
    /// Some read in a link position could not be followed to columns: every
    /// column is a link.
    pub links_unknown: bool,
    /// Plain tables read with their inheritance children (no `ONLY`).
    pub inherited_reads: std::collections::BTreeSet<u32>,
}

/// Columns copied unchanged into `data`, each with its path there: a column
/// copied to several paths has a pair for each.
pub type CopiedPaths = Vec<(String, Vec<String>)>;

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
    /// With `sql`: what a refresh reads of the table, for value locks.
    pub reads: Vec<ReadSet>,
    /// Columns of the table the TVIEW reads (name, attnum); empty when unknown.
    pub columns: Vec<(String, i16)>,
    /// Tables the mapping query joins, with the columns it looks up in each.
    pub lookups: Vec<(String, Vec<String>)>,
    /// Expressions the mapping query looks rows up by (array membership, computed
    /// columns), with the index that serves each.
    pub index_hints: Vec<IndexHint>,
    /// `mapped` through one equality onto a column of the root, or two through a
    /// table in between, `(column of this table, column of the root)`: the TVIEW
    /// rows can be found by that column when the TVIEW projects it (fan-out).
    pub hop: Option<(String, String)>,
    /// With a `hop` through a table in between: the query from values of this
    /// table's hop column (`$1`, `int8[]`) to the root column's values they reach,
    /// as `(value, root value)` pairs.
    pub via: Option<String>,
    /// With `hop`: the output column equal to the root's column, and the paths
    /// in `data` copying this table's columns, read nowhere else (`(column,
    /// path)`, a column at each path it is copied to): what an UPDATE of it can
    /// write into every row with that value.
    pub fanout: Option<(String, CopiedPaths)>,
    /// The table whose column is the key (of a branch, under UNION).
    pub root: bool,
    /// The virtual generated columns the TVIEW reads and their inputs: never
    /// copied by a fast path, which reads the changed row.
    pub virtual_reads: Vec<String>,
    /// A relation no trigger watches: none is installed on it.
    pub unwatched: Option<Unwatched>,
    /// The table of another TVIEW, of that entity.
    pub tview: Option<String>,
    /// The columns of the table the rows are linked by ([`QueryGraph::links`]),
    /// sorted; `None` when unknown (every column is one).
    pub links: Option<Vec<i16>>,
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

/// What a refresh of a TVIEW's rows reads of a mapped table, for value locks
/// (ADR 0207): the column of the table the mapping joins on, and the query from
/// the TVIEW's keys (`$1`) to the values that column is compared with. A writer
/// of the table locks its rows' values of `attnum`; a refresh locks the values
/// this query returns, which exist even when no row of the table matches them.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ReadSet {
    /// 0 when the table is joined by no equality: the table itself is locked.
    pub attnum: i16,
    /// A template (see [`render_template`]); `None` with `attnum` 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sql: Option<String>,
}
