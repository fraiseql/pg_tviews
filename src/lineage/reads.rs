//! What a view reads, directly or through other views, from `pg_depend`.

use crate::error::TViewResult;
use pgrx::pg_sys::Oid;

/// Views nested deeper than this are not followed (the analysis refuses far
/// shallower ones); it only bounds the recursion.
const MAX_NESTING: i32 = 100;

/// `WITH RECURSIVE` common table expressions over the views `seed` selects as
/// `(root, oid, 0)`: `views(root, oid, depth)` holds every view a root reads
/// through, itself included at depth 0, and `reads(root, relid, attnum, relkind,
/// depth)` every relation and column (`attnum > 0`; 0 for the relation itself) one
/// of those views reads, with that view's depth (0: the root's own rule). The
/// caller completes the statement with a `SELECT`.
#[must_use]
pub fn view_reads_cte(seed: &str) -> String {
    format!(
        "WITH RECURSIVE views(root, oid, depth) AS ( \
             {seed} \
           UNION \
             SELECT v.root, d.refobjid, v.depth + 1 FROM views v \
             JOIN pg_catalog.pg_rewrite w ON w.ev_class = v.oid \
             JOIN pg_catalog.pg_depend d \
               ON d.classid = 'pg_catalog.pg_rewrite'::pg_catalog.regclass AND d.objid = w.oid \
              AND d.refclassid = 'pg_catalog.pg_class'::pg_catalog.regclass \
              AND d.refobjid <> v.oid \
             JOIN pg_catalog.pg_class c ON c.oid = d.refobjid AND c.relkind = 'v' \
             WHERE v.depth < {MAX_NESTING} \
         ), \
         reads(root, relid, attnum, relkind, depth) AS ( \
             SELECT DISTINCT v.root, d.refobjid, d.refobjsubid, c.relkind, v.depth \
             FROM views v \
             JOIN pg_catalog.pg_rewrite w ON w.ev_class = v.oid \
             JOIN pg_catalog.pg_depend d \
               ON d.classid = 'pg_catalog.pg_rewrite'::pg_catalog.regclass AND d.objid = w.oid \
              AND d.refclassid = 'pg_catalog.pg_class'::pg_catalog.regclass \
              AND d.refobjid <> v.oid \
             JOIN pg_catalog.pg_class c ON c.oid = d.refobjid \
         ) "
    )
}

/// The seed of a single view, `$1`.
const SEED: &str = "SELECT $1::pg_catalog.oid, $1::pg_catalog.oid, 0";

/// How many views deep view `view_oid` reads through (0: it reads only tables).
///
/// # Errors
/// Returns an error if the catalog cannot be read.
pub fn view_nesting(view_oid: Oid) -> TViewResult<usize> {
    let sql = format!("{} SELECT max(depth) FROM views", view_reads_cte(SEED));
    let depth = crate::utils::spi::one::<i32>(&sql, &[crate::utils::spi::oid(view_oid)])?;
    Ok(depth.map_or(0, i32::unsigned_abs) as usize)
}

/// The columns of `relid` that view `view_oid` reads, through views, with their
/// attnums, in attnum order.
///
/// # Errors
/// Returns an error if the catalog cannot be read.
pub fn view_columns_read(view_oid: Oid, relid: Oid) -> TViewResult<Vec<(String, i16)>> {
    let sql = format!(
        "{} SELECT DISTINCT a.attname::pg_catalog.text, a.attnum FROM reads r \
         JOIN pg_catalog.pg_attribute a ON a.attrelid = r.relid AND a.attnum = r.attnum \
         WHERE r.relid = $2 AND r.attnum > 0 ORDER BY 2",
        view_reads_cte(SEED)
    );
    let rows = crate::utils::spi::rows(
        &sql,
        &[
            crate::utils::spi::oid(view_oid),
            crate::utils::spi::oid(relid),
        ],
        |row| Ok((row.get::<String>(1)?, row.get::<i16>(2)?)),
    )?;
    Ok(rows
        .into_iter()
        .filter_map(|(name, attnum)| Some((name?, attnum?)))
        .collect())
}

/// The relations view `view_oid` reads, through views (views excluded), with
/// their `relkind`.
///
/// # Errors
/// Returns an error if the catalog cannot be read.
pub fn view_relations_read(view_oid: Oid) -> TViewResult<Vec<(Oid, u8)>> {
    let sql = format!(
        "{} SELECT DISTINCT r.relid, r.relkind::pg_catalog.text FROM reads r \
         WHERE r.relkind <> 'v' ORDER BY 1",
        view_reads_cte(SEED)
    );
    let rows = crate::utils::spi::rows(&sql, &[crate::utils::spi::oid(view_oid)], |row| {
        Ok((row.get::<Oid>(1)?, row.get::<String>(2)?))
    })?;
    Ok(rows
        .into_iter()
        .filter_map(|(relid, kind)| Some((relid?, *kind?.as_bytes().first()?)))
        .collect())
}
