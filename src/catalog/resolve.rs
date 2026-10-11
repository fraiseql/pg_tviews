//! One name for a TVIEW (ADR 0211). A TVIEW's table is always `tv_<entity>`, and
//! an entity names one TVIEW in the whole database, so every spelling the API
//! accepts comes down to its entity: `post`, `tv_post`, `app.tv_post`, quoted or
//! not. A schema, when written, must be the TVIEW's.

use super::TviewMeta;
use crate::error::{TViewError, TViewResult};
use pgrx::pg_sys;
use pgrx::prelude::*;

/// A TVIEW as named: the schema written, if any, and the entity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Name {
    pub schema: Option<String>,
    pub entity: String,
}

/// Parse a TVIEW name: `<entity>`, `tv_<entity>` or `schema.tv_<entity>`. A part
/// is taken as written; double-quote it (`""` for a quote) to include a dot.
///
/// # Errors
/// Returns an error if the name does not parse, or the entity is not a valid
/// identifier.
pub fn parse(name: &str) -> TViewResult<Name> {
    let invalid = |reason: String| TViewError::InvalidInput {
        parameter: "tview".to_string(),
        reason,
    };
    let mut parts = crate::utils::ident::split(name)
        .ok_or_else(|| invalid(format!("{name} is not a valid TVIEW name")))?;
    let table = parts.pop().unwrap_or_default();
    let schema = match parts.as_slice() {
        [] => None,
        [schema] => Some(schema.clone()),
        _ => {
            return Err(invalid(format!(
                "{name} has too many dotted parts: use schema.tv_<entity>"
            )));
        }
    };
    crate::validation::validate_sql_identifier(&table, "tview")?;
    let entity = table.strip_prefix("tv_").unwrap_or(&table).to_string();
    Ok(Name { schema, entity })
}

/// A registered TVIEW, as found by its name: enough to check ownership and to
/// name it, read without decoding its plan (re-registration repairs a plan that
/// does not decode).
#[derive(Debug, Clone)]
pub struct Found {
    pub entity: String,
    /// Its table; `InvalidOid` once the table is gone.
    pub table: pg_sys::Oid,
}

/// The registered TVIEW `name` names.
///
/// # Errors
/// [`TViewError::TviewNotFound`] when no TVIEW has that entity, or it lives in
/// another schema than the one written; an error if the name does not parse.
pub fn find(name: &str) -> TViewResult<Found> {
    let named = parse(name)?;
    let not_found = || TViewError::TviewNotFound {
        name: name.to_string(),
    };
    let table = crate::utils::spi::one::<pg_sys::Oid>(
        &format!(
            "SELECT COALESCE(c.oid, 0) FROM {} m \
             LEFT JOIN pg_catalog.pg_class c ON c.oid = m.table_oid WHERE m.entity = $1",
            crate::utils::meta_table()
        ),
        &[crate::utils::spi::text(named.entity.as_str())],
    )?
    .ok_or_else(not_found)?;
    if let Some(schema) = &named.schema
        && registered_schema(&named.entity)?.as_ref() != Some(schema)
    {
        return Err(not_found());
    }
    Ok(Found {
        entity: named.entity,
        table,
    })
}

/// The registered TVIEW `name` names, with its plan.
///
/// # Errors
/// As [`find`], and an error if its catalog row does not decode.
pub fn resolve(name: &str) -> TViewResult<TviewMeta> {
    let found = find(name)?;
    TviewMeta::load_by_entity(&found.entity)?.ok_or_else(|| TViewError::TviewNotFound {
        name: name.to_string(),
    })
}

/// Schema of a registered entity: that of its `tv_*` table, or of its view when
/// the table is gone; `None` when no TVIEW has that entity.
///
/// # Errors
/// Returns an error if the catalog query fails.
pub fn registered_schema(entity: &str) -> TViewResult<Option<String>> {
    Spi::connect(|client| {
        client
            .select(
                // With its table gone, a TVIEW's schema is the prefix of its backing
                // view's name, `<schema>__tv_<entity>`.
                &format!(
                    "SELECT COALESCE( \
                         (SELECT n.nspname::text FROM pg_catalog.pg_class t \
                          JOIN pg_catalog.pg_namespace n ON n.oid = t.relnamespace \
                          WHERE t.oid = m.table_oid), \
                         (SELECT pg_catalog.left(v.relname::text, \
                                     -pg_catalog.length('__tv_' || m.entity)) \
                          FROM pg_catalog.pg_class v WHERE v.oid = m.view_oid \
                            AND pg_catalog.right(v.relname::text, \
                                    pg_catalog.length('__tv_' || m.entity)) \
                                = '__tv_' || m.entity)) \
                     FROM {} m WHERE m.entity = $1",
                    crate::utils::meta_table()
                ),
                None,
                &[crate::utils::spi::text(entity)],
            )?
            .first()
            .get_one::<String>()
    })
    .or_else(|e| match e {
        // No such TVIEW.
        pgrx::spi::Error::InvalidPosition => Ok(None),
        e => Err(e),
    })
    .map_err(|e| crate::utils::spi::catalog_error("Find the schema of a TVIEW", &e))
}

#[cfg(test)]
mod tests {
    use super::{Name, parse};

    fn named(schema: Option<&str>, entity: &str) -> Name {
        Name {
            schema: schema.map(str::to_string),
            entity: entity.to_string(),
        }
    }

    #[test]
    fn every_spelling_names_the_entity() {
        assert_eq!(parse("post").unwrap(), named(None, "post"));
        assert_eq!(parse("tv_post").unwrap(), named(None, "post"));
        assert_eq!(parse("app.tv_post").unwrap(), named(Some("app"), "post"));
        assert_eq!(
            parse("\"Odd.Schema\".tv_post").unwrap(),
            named(Some("Odd.Schema"), "post")
        );
        assert_eq!(
            parse("\"a\"\"b\".\"tv_post\"").unwrap(),
            named(Some("a\"b"), "post")
        );
        assert_eq!(parse("App.tv_Post").unwrap(), named(Some("App"), "Post"));
    }

    #[test]
    fn malformed_names_are_refused() {
        for bad in [
            "\"app.tv_post",
            "app..tv_post",
            "a.b.tv_post",
            "\"app\"x.tv_post",
            "",
            "app.tv post",
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }
}
