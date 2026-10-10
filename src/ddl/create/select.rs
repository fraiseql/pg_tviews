//! A definition as PostgreSQL reads it: one SELECT, its output columns with their
//! real types, and the role each column plays in a TVIEW by its name.

use crate::error::{TViewError, TViewResult};
use crate::utils::{quote_ident, quote_identifier};
use pgrx::pg_sys;
use std::ffi::{CStr, CString};

/// The output columns of a definition, and their roles by name: `pk_<entity>`
/// (the first such column names the entity), `id`, `identifier`, `data`, `fk_*`,
/// `*_id`, and every other column with its type.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ViewColumns {
    /// Every output column with its SQL type, in order.
    pub columns: Vec<(String, String)>,
    pub entity: Option<String>,
    pub pk: Option<String>,
    pub id: Option<String>,
    pub identifier: Option<String>,
    pub data: Option<String>,
    pub fk: Vec<String>,
    pub uuid_fk: Vec<String>,
    /// The columns without a role, with their types.
    pub additional: Vec<(String, String)>,
}

impl ViewColumns {
    /// Classify `columns` (name, SQL type) by name.
    #[must_use]
    pub fn classify(columns: Vec<(String, String)>) -> Self {
        let named = |n: &str| columns.iter().any(|(c, _)| c == n).then(|| n.to_string());
        let pk = columns
            .iter()
            .find(|(c, _)| c.starts_with("pk_"))
            .map(|(c, _)| c.clone());
        let entity = pk.as_ref().map(|c| c["pk_".len()..].to_string());
        let (id, identifier, data) = (named("id"), named("identifier"), named("data"));
        let fk: Vec<String> = columns
            .iter()
            .filter(|(c, _)| c.starts_with("fk_"))
            .map(|(c, _)| c.clone())
            .collect();
        let uuid_fk: Vec<String> = columns
            .iter()
            .filter(|(c, _)| c.ends_with("_id") && c != "id")
            .map(|(c, _)| c.clone())
            .collect();
        let roles: Vec<&String> = [&pk, &id, &identifier, &data]
            .into_iter()
            .flatten()
            .collect();
        let additional = columns
            .iter()
            .filter(|(c, _)| !roles.contains(&c) && !fk.contains(c) && !uuid_fk.contains(c))
            .cloned()
            .collect();
        Self {
            columns,
            entity,
            pk,
            id,
            identifier,
            data,
            fk,
            uuid_fk,
            additional,
        }
    }
}

/// What a definition's single SELECT outputs, read by PostgreSQL's parser under
/// the current `search_path`: `(name, SQL type)` of each column, `SELECT *`
/// expanded. Nothing is created.
///
/// # Errors
/// 42601 when the text is not exactly one SELECT; PostgreSQL's own error when it
/// does not analyze (an unknown table, a type error).
pub fn analyze(sql: &str) -> TViewResult<Vec<(String, String)>> {
    let c_sql = CString::new(sql).map_err(|_| TViewError::InvalidSelectStatement {
        sql: sql.to_string(),
        reason: "it contains a NUL byte".to_string(),
    })?;
    let raw = parse_one_select(sql, &c_sql)?;
    // SAFETY: the analyzer runs in the current memory context on the RawStmt
    // just parsed from `c_sql`; the Query is read only while that context lives.
    // Errors it raises propagate as PostgreSQL ERRORs.
    unsafe {
        let query = pg_sys::parse_analyze_fixedparams(
            raw,
            c_sql.as_ptr(),
            std::ptr::null(),
            0,
            std::ptr::null_mut(),
        );
        let mut out = Vec::new();
        let tlist = (*query).targetList;
        let len = if tlist.is_null() { 0 } else { (*tlist).length };
        for i in 0..len {
            let tle = pg_sys::list_nth(tlist, i).cast::<pg_sys::TargetEntry>();
            if (*tle).resjunk || (*tle).resname.is_null() {
                continue;
            }
            let name = CStr::from_ptr((*tle).resname)
                .to_string_lossy()
                .into_owned();
            let expr = (*tle).expr.cast::<pg_sys::Node>();
            let ty =
                crate::utils::qualified_type_name(pg_sys::exprType(expr), pg_sys::exprTypmod(expr));
            out.push((name, ty));
        }
        Ok(out)
    }
}

/// Refuse a `pk_<entity>` column that is not an integer (a domain over one is):
/// the TVIEW table keys on BIGINT, and refreshes carry keys as 64-bit integers.
///
/// # Errors
/// [`TViewError::KeyTypeRefused`] for another type, or the error resolving a
/// type name.
fn check_key_type(columns: &ViewColumns) -> TViewResult<()> {
    let Some(pk) = &columns.pk else {
        return Ok(());
    };
    let Some((_, found)) = columns.columns.iter().find(|(c, _)| c == pk) else {
        return Ok(());
    };
    if matches!(found.as_str(), "smallint" | "integer" | "bigint") {
        return Ok(());
    }
    let typid = crate::utils::spi::one::<pg_sys::Oid>(
        "SELECT pg_catalog.to_regtype($1)::pg_catalog.oid",
        &[crate::utils::spi::text(found.as_str())],
    )?
    .unwrap_or(pg_sys::InvalidOid);
    let base = if typid == pg_sys::InvalidOid {
        typid
    } else {
        // SAFETY: a syscache lookup of a type that exists (the definition analyzed).
        unsafe { pg_sys::getBaseType(typid) }
    };
    if matches!(base, pg_sys::INT2OID | pg_sys::INT4OID | pg_sys::INT8OID) {
        return Ok(());
    }
    Err(TViewError::KeyTypeRefused {
        column: pk.clone(),
        found: found
            .strip_prefix("pg_catalog.")
            .unwrap_or(found)
            .to_string(),
    })
}

/// Check that an aggregate definition can be maintained group by group, as
/// PostgreSQL analyzes it: no window function (a window spans rows of other
/// groups), no set operation, and a `pk_<entity>` output that is a plain column and
/// a `GROUP BY` key, so a refresh's `WHERE pk_<entity> = ANY(…)` narrows the
/// aggregate to the touched groups.
///
/// # Errors
/// [`TViewError::InvalidInput`] naming what is not supported; what [`analyze`]
/// returns for a definition PostgreSQL cannot read.
pub fn check_aggregate(sql: &str, entity: &str) -> TViewResult<()> {
    let refuse = |reason: String| TViewError::InvalidInput {
        parameter: "aggregate definition".to_string(),
        reason,
    };
    let c_sql = CString::new(sql).map_err(|_| refuse("it contains a NUL byte".to_string()))?;
    let raw = parse_one_select(sql, &c_sql)?;
    let pk = format!("pk_{entity}");
    // SAFETY: the Query just analyzed from `raw`, read in the current memory context.
    unsafe {
        let query = pg_sys::parse_analyze_fixedparams(
            raw,
            c_sql.as_ptr(),
            std::ptr::null(),
            0,
            std::ptr::null_mut(),
        );
        if (*query).hasWindowFuncs {
            return Err(refuse(
                "window functions (OVER …) are not supported in aggregate TVIEWs: a window \
                 spans rows of other groups, so a group cannot be recomputed on its own"
                    .to_string(),
            ));
        }
        if !(*query).setOperations.is_null() {
            return Err(refuse(
                "the definition must be a single SELECT with GROUP BY (no UNION)".to_string(),
            ));
        }
        if (*query).groupClause.is_null() {
            return Err(refuse("the definition must have a GROUP BY".to_string()));
        }
        let tlist = (*query).targetList;
        let len = if tlist.is_null() { 0 } else { (*tlist).length };
        let key = (0..len)
            .map(|i| pg_sys::list_nth(tlist, i).cast::<pg_sys::TargetEntry>())
            .find(|tle| {
                !(**tle).resjunk
                    && !(**tle).resname.is_null()
                    && CStr::from_ptr((**tle).resname).to_bytes() == pk.as_bytes()
            })
            .ok_or_else(|| {
                refuse(format!(
                    "the definition must output a {pk} column (the group key)"
                ))
            })?;
        let expr = (*key).expr.cast::<pg_sys::Node>();
        let mut plain = expr;
        while (*plain).type_ == pg_sys::NodeTag::T_RelabelType {
            #[allow(clippy::cast_ptr_alignment)] // Reason: PostgreSQL Node* → RelabelType* cast
            {
                plain = (*plain.cast::<pg_sys::RelabelType>()).arg.cast();
            }
        }
        if (*plain).type_ != pg_sys::NodeTag::T_Var {
            return Err(refuse(format!(
                "{pk} must be a plain column that is also a GROUP BY key, not an expression"
            )));
        }
        let grouped = (*key).ressortgroupref != 0
            && !pg_sys::get_sortgroupref_clause_noerr((*key).ressortgroupref, (*query).groupClause)
                .is_null();
        if !grouped {
            return Err(refuse(format!(
                "{pk} must also be a GROUP BY key, so a refresh recomputes only the touched groups"
            )));
        }
        Ok(())
    }
}

/// Check that `sql` is one SELECT, as PostgreSQL parses it, without resolving a
/// name: a stored definition is checked this way whatever the `search_path`.
///
/// # Errors
/// 42601 when it is not exactly one SELECT (PostgreSQL's own syntax error first).
pub fn check_one_select(sql: &str) -> TViewResult<()> {
    let c_sql = CString::new(sql).map_err(|_| TViewError::InvalidSelectStatement {
        sql: sql.to_string(),
        reason: "it contains a NUL byte".to_string(),
    })?;
    parse_one_select(sql, &c_sql).map(|_| ())
}

/// The single `RawStmt` of `c_sql`, a SELECT.
fn parse_one_select(sql: &str, c_sql: &CStr) -> TViewResult<*mut pg_sys::RawStmt> {
    let not_one_select = |reason: &str| TViewError::InvalidSelectStatement {
        sql: sql.to_string(),
        reason: reason.to_string(),
    };
    // SAFETY: the parser runs in the current memory context on a NUL-terminated
    // string; its List and RawStmt live as long as that context. Errors it raises
    // propagate as PostgreSQL ERRORs.
    unsafe {
        let stmts = pg_sys::raw_parser(c_sql.as_ptr(), pg_sys::RawParseMode::RAW_PARSE_DEFAULT);
        let n = if stmts.is_null() { 0 } else { (*stmts).length };
        if n != 1 {
            return Err(not_one_select("a TVIEW is defined by exactly one SELECT"));
        }
        let raw = pg_sys::list_nth(stmts, 0).cast::<pg_sys::RawStmt>();
        if (*(*raw).stmt).type_ != pg_sys::NodeTag::T_SelectStmt {
            return Err(not_one_select("a TVIEW is defined by a SELECT"));
        }
        Ok(raw)
    }
}

/// The definition as stored and its columns: `SELECT * FROM …` written out with
/// its columns, and a SELECT without a `pk_*` column rewritten to the
/// `pk_<entity>, id, data` shape.
///
/// # Errors
/// What [`analyze`] returns, 42601 when a raw SELECT has no column to key on, or
/// 0A000 when its key is not an integer.
pub fn normalize(entity: &str, select_sql: &str) -> TViewResult<(String, ViewColumns)> {
    let columns = ViewColumns::classify(analyze(select_sql)?);
    if columns.entity.is_some() {
        check_key_type(&columns)?;
        let sql = expand_star(select_sql, &columns).unwrap_or_else(|| select_sql.to_string());
        return Ok((sql, columns));
    }
    let transformed = raw_to_tview(entity, select_sql, &columns)?;
    let columns = ViewColumns::classify(analyze(&transformed)?);
    check_key_type(&columns)?;
    Ok((transformed, columns))
}

/// `SELECT <its columns> FROM …` for a definition that starts `SELECT * FROM`, so
/// the stored definition does not grow when a table it reads gains a column.
fn expand_star(sql: &str, columns: &ViewColumns) -> Option<String> {
    let rest = after_star(sql)?;
    let list: Vec<String> = columns
        .columns
        .iter()
        .map(|(c, _)| quote_ident(c))
        .collect();
    Some(format!("SELECT {} {rest}", list.join(", ")))
}

/// The `FROM …` of a definition that starts `SELECT * FROM`.
fn after_star(sql: &str) -> Option<&str> {
    let rest = strip_keyword(sql.trim_start(), "select")?.trim_start();
    let rest = rest.strip_prefix('*')?.trim_start();
    strip_keyword(rest, "from")?;
    Some(rest)
}

/// `text` without its leading `keyword` (any case) when a word boundary follows.
fn strip_keyword<'a>(text: &'a str, keyword: &str) -> Option<&'a str> {
    let head = text.get(..keyword.len())?;
    let rest = &text[keyword.len()..];
    (head.eq_ignore_ascii_case(keyword)
        && rest
            .chars()
            .next()
            .is_some_and(|c| !c.is_alphanumeric() && c != '_'))
    .then_some(rest)
}

/// A SELECT without a `pk_*` column, rewritten to the TVIEW shape: its key column
/// (`pk`, else the first integer column, else `id`) as `pk_<entity>`, a generated
/// `id`, and every column under its own name in `data`.
fn raw_to_tview(entity: &str, select_sql: &str, columns: &ViewColumns) -> TViewResult<String> {
    let integer = |t: &str| matches!(t, "smallint" | "integer" | "bigint");
    let key = columns
        .columns
        .iter()
        .find(|(c, _)| c == "pk")
        .or_else(|| columns.columns.iter().find(|(_, t)| integer(t)))
        .or_else(|| columns.columns.iter().find(|(c, _)| c == "id"))
        .map(|(c, _)| c.clone())
        .ok_or_else(|| TViewError::InvalidSelectStatement {
            sql: select_sql.to_string(),
            reason: "no column to key the rows on (need 'pk', an integer column, or 'id')"
                .to_string(),
        })?;
    let fields: Vec<String> = columns
        .columns
        .iter()
        .map(|(c, _)| {
            format!(
                "{}, source.{}",
                crate::utils::quote_literal(c),
                quote_ident(c)
            )
        })
        .collect();
    Ok(format!(
        "SELECT source.{} AS {}, gen_random_uuid() AS id, jsonb_build_object({}) AS data \
         FROM ({select_sql}) AS source",
        quote_ident(&key),
        quote_identifier(&format!("pk_{entity}")),
        fields.join(", ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cols(names: &[&str]) -> Vec<(String, String)> {
        names
            .iter()
            .map(|n| ((*n).to_string(), "text".to_string()))
            .collect()
    }

    #[test]
    fn roles_by_name() {
        let c = ViewColumns::classify(cols(&[
            "pk_post", "id", "fk_user", "user_id", "data", "title",
        ]));
        assert_eq!(c.entity.as_deref(), Some("post"));
        assert_eq!(c.pk.as_deref(), Some("pk_post"));
        assert_eq!(c.fk, vec!["fk_user"]);
        assert_eq!(c.uuid_fk, vec!["user_id"]);
        assert_eq!(
            c.additional,
            vec![("title".to_string(), "text".to_string())]
        );
    }

    #[test]
    fn star_expansion_keeps_the_rest() {
        assert_eq!(
            after_star("select * FROM tb_x WHERE pk_x > 1"),
            Some("FROM tb_x WHERE pk_x > 1")
        );
        assert_eq!(after_star("SELECT pk_x FROM tb_x"), None);
        assert_eq!(after_star("SELECT *, 1 FROM tb_x"), None);
        assert_eq!(after_star("selection * from x"), None);
    }
}
