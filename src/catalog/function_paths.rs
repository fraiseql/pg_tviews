//! The `search_path` of the functions a TVIEW's refresh runs as its owner.
//!
//! PostgreSQL looks up relations and types in the session's temporary schema
//! before any other, unless the `search_path` names `pg_temp` explicitly: then
//! where it names it. The temporary schema belongs to the session, so to the
//! writer whose statement the refresh runs in. A function that sets its own
//! `search_path` therefore resolves its unqualified names where its owner meant
//! them only when that path ends in `pg_temp`.
//!
//! The functions checked are those the backing view calls (directly, through the
//! operators and aggregates it uses, or through the views it reads) and the user
//! triggers on the TVIEW's table. Functions of `pg_catalog` and of `pg_tviews`
//! itself set no such path.

use crate::error::{TViewError, TViewResult};
use pgrx::pg_sys::Oid;

/// A function whose `search_path` setting does not end in `pg_temp`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathIssue {
    /// The function, `schema.name(argument types)`.
    pub function: String,
    /// The `search_path` it sets, as stored.
    pub path: String,
    /// The same path with `pg_temp` last.
    pub fix: String,
}

impl PathIssue {
    /// The error refusing a TVIEW that would run this function.
    #[must_use]
    pub fn refusal(self) -> TViewError {
        TViewError::FunctionSearchPath {
            function: self.function,
            path: self.path,
            fix: self.fix,
        }
    }
}

/// The functions run for the TVIEW whose backing view is `view` and whose table
/// is `table` (none yet at create) whose `search_path` does not end in `pg_temp`.
///
/// # Errors
/// A failed catalog read.
pub fn issues(view: Oid, table: Option<Oid>) -> TViewResult<Vec<PathIssue>> {
    let found = crate::utils::spi::catalog_rows(
        "WITH RECURSIVE views(relid) AS ( \
                 SELECT $1::pg_catalog.oid \
                 UNION \
                 SELECT d.refobjid FROM views v \
                 JOIN pg_catalog.pg_rewrite r ON r.ev_class = v.relid \
                 JOIN pg_catalog.pg_depend d ON d.classid = 'pg_catalog.pg_rewrite'::pg_catalog.regclass \
                  AND d.objid = r.oid AND d.refclassid = 'pg_catalog.pg_class'::pg_catalog.regclass \
                 JOIN pg_catalog.pg_class c ON c.oid = d.refobjid AND c.relkind IN ('v', 'm') \
             ), used(fn) AS ( \
                 SELECT d.refobjid FROM views v \
                 JOIN pg_catalog.pg_rewrite r ON r.ev_class = v.relid \
                 JOIN pg_catalog.pg_depend d ON d.classid = 'pg_catalog.pg_rewrite'::pg_catalog.regclass \
                  AND d.objid = r.oid AND d.refclassid = 'pg_catalog.pg_proc'::pg_catalog.regclass \
                 UNION \
                 SELECT o.oprcode::pg_catalog.oid FROM views v \
                 JOIN pg_catalog.pg_rewrite r ON r.ev_class = v.relid \
                 JOIN pg_catalog.pg_depend d ON d.classid = 'pg_catalog.pg_rewrite'::pg_catalog.regclass \
                  AND d.objid = r.oid AND d.refclassid = 'pg_catalog.pg_operator'::pg_catalog.regclass \
                 JOIN pg_catalog.pg_operator o ON o.oid = d.refobjid \
                 UNION \
                 SELECT t.tgfoid FROM pg_catalog.pg_trigger t \
                 WHERE t.tgrelid OPERATOR(pg_catalog.=) $2 AND NOT t.tgisinternal \
             ), called(fn) AS ( \
                 SELECT fn FROM used \
                 UNION \
                 SELECT f FROM used u JOIN pg_catalog.pg_aggregate a ON a.aggfnoid = u.fn, \
                 LATERAL (VALUES (a.aggtransfn::pg_catalog.oid), (a.aggfinalfn::pg_catalog.oid), \
                                 (a.aggcombinefn::pg_catalog.oid)) x(f) \
             ) \
             SELECT p.oid::pg_catalog.regprocedure::pg_catalog.text, \
                    pg_catalog.substr(s, pg_catalog.length('search_path=') + 1) \
             FROM called k JOIN pg_catalog.pg_proc p ON p.oid = k.fn \
             CROSS JOIN LATERAL pg_catalog.unnest(p.proconfig) s \
             WHERE pg_catalog.starts_with(s, 'search_path=') \
               AND p.pronamespace <> 'pg_catalog'::pg_catalog.regnamespace \
               AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_depend e \
                               JOIN pg_catalog.pg_extension x ON x.oid = e.refobjid \
                               WHERE e.classid = 'pg_catalog.pg_proc'::pg_catalog.regclass \
                                 AND e.objid = p.oid AND e.deptype = 'e' \
                                 AND x.extname = 'pg_tviews') \
             ORDER BY 1",
        &[
            crate::utils::spi::oid(view),
            crate::utils::spi::oid(table.unwrap_or(pgrx::pg_sys::InvalidOid)),
        ],
        |row| Ok((row.get::<String>(1)?, row.get::<String>(2)?)),
    )?;
    Ok(found
        .into_iter()
        .filter_map(|(function, path)| {
            let (function, path) = (function?, path?);
            let fix = path_issue(&path)?;
            Some(PathIssue {
                function,
                path,
                fix,
            })
        })
        .collect())
}

/// Refuse the TVIEW of `view` and `table` when a function it runs sets a
/// `search_path` that does not end in `pg_temp`.
///
/// # Errors
/// `FunctionSearchPath` for the first such function; a failed catalog read.
pub fn check(view: Oid, table: Option<Oid>) -> TViewResult<()> {
    match issues(view, table)?.into_iter().next() {
        Some(issue) => Err(issue.refusal()),
        None => Ok(()),
    }
}

/// The fixed path when the `search_path` value `path` (as `SET` stores it) does
/// not end in `pg_temp`, `None` when it does.
#[must_use]
pub fn path_issue(path: &str) -> Option<String> {
    let schemas = split_identifiers(path);
    let temp = |s: &String| s == "pg_temp";
    if schemas.last().is_some_and(temp) && schemas.iter().filter(|s| temp(s)).count() == 1 {
        return None;
    }
    let mut fixed: Vec<String> = schemas
        .iter()
        .filter(|s| !temp(s))
        .map(|s| quote(s))
        .collect();
    fixed.push("pg_temp".to_string());
    Some(fixed.join(", "))
}

/// The identifiers of a comma-separated list, as `SplitIdentifierString` reads
/// them: double-quoted ones as written (`""` for a quote), others lowercased,
/// surrounding whitespace dropped.
fn split_identifiers(list: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = list.chars().peekable();
    loop {
        while chars.next_if(|c| c.is_whitespace()).is_some() {}
        let mut name = String::new();
        if chars.next_if_eq(&'"').is_some() {
            while let Some(c) = chars.next() {
                if c == '"' {
                    if chars.next_if_eq(&'"').is_some() {
                        name.push('"');
                    } else {
                        break;
                    }
                } else {
                    name.push(c);
                }
            }
        } else {
            while let Some(c) = chars.next_if(|&c| c != ',' && !c.is_whitespace()) {
                name.extend(c.to_lowercase());
            }
        }
        while chars.next_if(|c| c.is_whitespace()).is_some() {}
        if !name.is_empty() {
            out.push(name);
        }
        if chars.next().is_none() {
            return out;
        }
    }
}

/// `name` double-quoted unless it is a plain lowercase identifier.
fn quote(name: &str) -> String {
    let plain = name.starts_with(|c: char| c.is_ascii_lowercase() || c == '_')
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '$');
    if plain {
        name.to_string()
    } else {
        format!("\"{}\"", name.replace('"', "\"\""))
    }
}

#[cfg(test)]
mod tests {
    use super::path_issue;

    #[test]
    fn test_path_ending_in_pg_temp_passes() {
        assert_eq!(path_issue("app, pg_temp"), None);
        assert_eq!(path_issue("app,PG_TEMP"), None);
        assert_eq!(path_issue("\"$user\", public, \"pg_temp\""), None);
    }

    #[test]
    fn test_path_without_pg_temp_last_gets_it_last() {
        assert_eq!(path_issue("app").as_deref(), Some("app, pg_temp"));
        assert_eq!(path_issue("pg_temp, app").as_deref(), Some("app, pg_temp"));
        assert_eq!(path_issue("").as_deref(), Some("pg_temp"));
        assert_eq!(
            path_issue("pg_temp, app, pg_temp").as_deref(),
            Some("app, pg_temp")
        );
    }

    #[test]
    fn test_quoted_names_keep_their_case() {
        assert_eq!(
            path_issue("\"App\", \"PG_TEMP\"").as_deref(),
            Some("\"App\", \"PG_TEMP\", pg_temp")
        );
        assert_eq!(
            path_issue("\"$user\", public").as_deref(),
            Some("\"$user\", public, pg_temp")
        );
    }
}
