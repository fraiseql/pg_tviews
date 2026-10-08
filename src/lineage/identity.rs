//! The column that names a TVIEW's rows (ADR 0169).

use super::{Column, QueryGraph, walk};

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
pub(super) fn identity_of(
    entity: &str,
    view_oid: pgrx::pg_sys::Oid,
    graph: &QueryGraph,
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
