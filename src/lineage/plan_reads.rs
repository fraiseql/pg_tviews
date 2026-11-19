//! An independent check of the read sets the walk derives, in test builds:
//! the plan `PostgreSQL` makes of the backing view names, at each scan of a
//! table, the columns it reads (`EXPLAIN VERBOSE`). Each must be in the
//! TVIEW's read set of that table, or the table must read as unknown (a
//! whole-row reference `t.*`, a system column). A column the plan reads and the
//! walk missed fails the registration with `XX000`.

use super::Lineage;
use crate::error::{TViewError, TViewResult};
use pgrx::pg_sys::Oid;
use std::collections::BTreeSet;

/// A scan of a table in the plan: the table and the name the plan gives it.
struct Scan {
    relid: Oid,
    relation: String,
    alias: String,
    /// Its output is every column of the table, in order.
    physical: bool,
}

/// Check `lineage`'s read sets against the plan of the backing view `view_oid`.
///
/// # Errors
/// [`TViewError::CatalogError`] when the plan reads a column of a table outside
/// the read set the walk derived, or a table the walk did not find; an SPI error
/// when the view does not plan.
pub fn check(
    frame: &crate::owner::OwnerFrame,
    entity: &str,
    view_oid: Oid,
    lineage: &Lineage,
) -> TViewResult<()> {
    frame.require_current()?;
    // The view's query, not the view: the frame reads its tables, as a refresh
    // does, whoever may select from the view itself.
    let query = crate::utils::spi::one::<String>(
        "SELECT pg_catalog.pg_get_viewdef($1)",
        &[crate::utils::spi::oid(view_oid)],
    )
    .map_err(|e| crate::utils::spi::catalog_error("Read a backing view's query", &e))?
    .unwrap_or_default();
    let explain = format!(
        "EXPLAIN (VERBOSE, COSTS OFF, FORMAT JSON) {}",
        query.trim().trim_end_matches(';')
    );
    let plan = crate::utils::spi::one::<pgrx::Json>(&explain, &[])
        .map_err(|e| TViewError::SpiError {
            query: explain.clone(),
            error: e.to_string(),
        })?
        .map(|j| j.0)
        .unwrap_or_default();
    let mut scans = Vec::new();
    collect_scans(&plan, &mut scans)?;
    // Row security on a table (for the frame's role, or for the owner of a view
    // of the chain) adds its policies' reads to the plan, and such a TVIEW is
    // refused by the row-security rule once registered: the check leaves it.
    let row_security = crate::utils::spi::one::<bool>(
        "SELECT pg_catalog.bool_or(relrowsecurity) FROM pg_catalog.pg_class \
         WHERE oid OPERATOR(pg_catalog.=) ANY ($1)",
        &[crate::utils::spi::oid_array(
            scans.iter().map(|s| s.relid).collect(),
        )],
    )
    .map_err(|e| crate::utils::spi::catalog_error("Read the row security of scanned tables", &e))?
    .unwrap_or(false);
    if row_security {
        return Ok(());
    }
    // A scan returning its table's whole tuple (the planner's physical target
    // list, when no projection is needed there) lists every column, read or
    // not: what is read shows in the nodes above it and in the conditions.
    let physical: BTreeSet<&str> = scans
        .iter()
        .filter(|s| s.physical)
        .map(|s| s.alias.as_str())
        .collect();
    let mut texts = Vec::new();
    collect_texts(&plan, &physical, &mut texts);
    let failure = |what: String| TViewError::CatalogError {
        operation: format!("Check the read sets of tv_{entity} against its plan"),
        pg_error: what,
    };
    for scan in scans {
        let ancestors = ancestors(scan.relid)?;
        let Some(table) = ancestors
            .iter()
            .find_map(|a| lineage.tables.iter().find(|t| t.relid == a.to_u32()))
        else {
            return Err(failure(format!(
                "the plan reads {}, a table the walk did not find",
                scan.relation
            )));
        };
        let root = Oid::from(table.relid);
        let (whole, columns) = reads_of(&scan.alias, &texts);
        let Some(read_set) = &table.read_set else {
            continue;
        };
        if whole {
            return Err(failure(format!(
                "the plan reads {} whole, which the walk read column by column",
                table.qualified
            )));
        }
        for column in columns {
            let attnum = attnum_of(root, &column)?;
            let ignored = attnum
                == i16::try_from(pgrx::pg_sys::TableOidAttributeNumber).unwrap_or(i16::MIN)
                // SAFETY: a syscache lookup by OID.
                && unsafe { pgrx::pg_sys::get_rel_relkind(root) }.cast_unsigned()
                    != pgrx::pg_sys::RELKIND_PARTITIONED_TABLE;
            if !ignored && !read_set.contains(&attnum) {
                return Err(failure(format!(
                    "the plan reads {}.{column}, outside the read set the walk derived ({:?})",
                    table.qualified, read_set
                )));
            }
        }
    }
    Ok(())
}

/// Every scan of a table in an EXPLAIN JSON plan.
fn collect_scans(node: &serde_json::Value, out: &mut Vec<Scan>) -> TViewResult<()> {
    match node {
        serde_json::Value::Array(items) => {
            for item in items {
                collect_scans(item, out)?;
            }
        }
        serde_json::Value::Object(map) => {
            let field = |k: &str| map.get(k).and_then(serde_json::Value::as_str);
            if let (Some(schema), Some(relation), Some(alias)) =
                (field("Schema"), field("Relation Name"), field("Alias"))
            {
                let relid = crate::utils::spi::one::<Oid>(
                    "SELECT c.oid FROM pg_catalog.pg_class c \
                     JOIN pg_catalog.pg_namespace n ON n.oid OPERATOR(pg_catalog.=) c.relnamespace \
                     WHERE n.nspname OPERATOR(pg_catalog.=) $1 AND c.relname OPERATOR(pg_catalog.=) $2",
                    &[crate::utils::spi::text(schema), crate::utils::spi::text(relation)],
                )
                .map_err(|e| crate::utils::spi::catalog_error("Resolve a scanned table", &e))?
                .unwrap_or(pgrx::pg_sys::InvalidOid);
                // SAFETY: a syscache lookup by OID.
                let relkind = unsafe { pgrx::pg_sys::get_rel_relkind(relid) }.cast_unsigned();
                // Tables only: a scan of a function, a CTE or VALUES names none.
                if relkind != pgrx::pg_sys::RELKIND_VIEW {
                    let output: Vec<&str> = map
                        .get("Output")
                        .and_then(serde_json::Value::as_array)
                        .map(|o| o.iter().filter_map(serde_json::Value::as_str).collect())
                        .unwrap_or_default();
                    let physical = !output.is_empty() && output == every_column(relid, alias)?;
                    out.push(Scan {
                        relid,
                        relation: format!("{schema}.{relation}"),
                        alias: alias.to_string(),
                        physical,
                    });
                }
            }
            for value in map.values() {
                collect_scans(value, out)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Every string of a plan: the expressions of its outputs and conditions, but
/// the output of a scan named in `physical`.
fn collect_texts<'a>(
    node: &'a serde_json::Value,
    physical: &BTreeSet<&str>,
    out: &mut Vec<&'a str>,
) {
    match node {
        serde_json::Value::Array(items) => {
            for item in items {
                collect_texts(item, physical, out);
            }
        }
        serde_json::Value::Object(map) => {
            let whole_tuple = map
                .get("Alias")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|a| physical.contains(a))
                && map.contains_key("Relation Name");
            for (key, value) in map {
                if !(whole_tuple && key == "Output") {
                    collect_texts(value, physical, out);
                }
            }
        }
        serde_json::Value::String(s) => out.push(s),
        _ => {}
    }
}

/// Every live column of table `relid`, in order, as a plan prints them under
/// the alias `alias`.
fn every_column(relid: Oid, alias: &str) -> TViewResult<Vec<String>> {
    let prefix = quote(alias);
    crate::utils::spi::strings(
        "SELECT pg_catalog.quote_ident(attname) FROM pg_catalog.pg_attribute \
         WHERE attrelid OPERATOR(pg_catalog.=) $1 AND attnum OPERATOR(pg_catalog.>) 0 \
           AND NOT attisdropped ORDER BY attnum",
        &[crate::utils::spi::oid(relid)],
    )
    .map(|names| names.into_iter().map(|n| format!("{prefix}.{n}")).collect())
}

/// `relid` and the tables it inherits from, nearest first.
fn ancestors(relid: Oid) -> TViewResult<Vec<Oid>> {
    crate::utils::spi::one::<Vec<Oid>>(
        "WITH RECURSIVE up(r, depth) AS ( \
             SELECT $1::pg_catalog.oid, 0 \
             UNION ALL SELECT i.inhparent, up.depth OPERATOR(pg_catalog.+) 1 \
                       FROM pg_catalog.pg_inherits i, up \
                       WHERE i.inhrelid OPERATOR(pg_catalog.=) up.r) \
         SELECT pg_catalog.array_agg(r ORDER BY depth) FROM up",
        &[crate::utils::spi::oid(relid)],
    )
    .map_err(|e| crate::utils::spi::catalog_error("Read the ancestors of a scanned table", &e))
    .map(Option::unwrap_or_default)
}

/// The attribute number of column `name` of table `relid`.
fn attnum_of(relid: Oid, name: &str) -> TViewResult<i16> {
    crate::utils::spi::one::<i16>(
        "SELECT attnum FROM pg_catalog.pg_attribute \
         WHERE attrelid OPERATOR(pg_catalog.=) $1 AND attname OPERATOR(pg_catalog.=) $2 \
           AND NOT attisdropped",
        &[crate::utils::spi::oid(relid), crate::utils::spi::text(name)],
    )
    .map_err(|e| crate::utils::spi::catalog_error("Number a scanned column", &e))?
    .ok_or_else(|| TViewError::CatalogError {
        operation: "Number a scanned column".to_string(),
        pg_error: format!("no column {name} in relation {relid:?}"),
    })
}

/// What the plan's expressions read of the scan named `alias`: whether a
/// whole row (`alias.*`), and the columns (`alias.column`).
fn reads_of(alias: &str, texts: &[&str]) -> (bool, BTreeSet<String>) {
    let quoted = quote(alias);
    let mut whole = false;
    let mut columns = BTreeSet::new();
    for text in texts {
        for prefix in [alias.to_string(), quoted.clone()] {
            let needle = format!("{prefix}.");
            let mut from = 0;
            while let Some(at) = text[from..].find(&needle) {
                let start = from + at;
                from = start + needle.len();
                // Not the tail of a longer name (`xi.` for alias `i`).
                let before = text[..start].chars().next_back();
                if before.is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '"' || c == '.') {
                    continue;
                }
                match identifier(&text[from..]) {
                    Some(Ident::Star) => whole = true,
                    Some(Ident::Name(name)) => {
                        columns.insert(name);
                    }
                    None => {}
                }
            }
        }
    }
    (whole, columns)
}

enum Ident {
    Star,
    Name(String),
}

/// The identifier `text` starts with, as `PostgreSQL` prints it: `*`, a quoted
/// name (`""` for a quote) or a lower-case one.
fn identifier(text: &str) -> Option<Ident> {
    if text.starts_with('*') {
        return Some(Ident::Star);
    }
    if let Some(rest) = text.strip_prefix('"') {
        let mut name = String::new();
        let mut chars = rest.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    name.push('"');
                } else {
                    return Some(Ident::Name(name));
                }
            } else {
                name.push(c);
            }
        }
        return None;
    }
    let name: String = text
        .chars()
        .take_while(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '_' || *c == '$')
        .collect();
    (!name.is_empty()).then_some(Ident::Name(name))
}

/// `name` as `PostgreSQL` prints an identifier that needs quoting.
fn quote(name: &str) -> String {
    let plain = name
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c == '_')
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '$');
    if plain {
        name.to_string()
    } else {
        format!("\"{}\"", name.replace('"', "\"\""))
    }
}
