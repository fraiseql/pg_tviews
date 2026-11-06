//! Conditions on one table's columns against constants: what its rows must meet to
//! be read at all (#284).

use super::{HashSet, Resolved, RteInfo, Sql, Walker, cstr, pg_sys, strip_relabel, tag};

impl Walker<'_> {
    /// Record `qual` as a guard of the occurrence it filters when it compares one
    /// column of a table read at this level with constants: `col = const`,
    /// `col = ANY (const array)` (`IN`), `col IS NULL`. A row failing it is never
    /// read by that occurrence. `nullable` is `None` for `WHERE` and inner joins;
    /// for an outer join's condition only an occurrence on its nullable side is
    /// filtered by it. `col IS NULL` is not a guard of an occurrence an outer join
    /// below may NULL-extend: the NULL-extended row meets it.
    ///
    /// SAFETY: `qual` is a valid expression of the innermost level.
    pub(super) unsafe fn guard(
        &mut self,
        qual: *mut pg_sys::Node,
        nullable: Option<&HashSet<usize>>,
    ) {
        // SAFETY: read-only checks of a valid expression, each node cast after its tag.
        unsafe {
            if pg_sys::contain_mutable_functions(qual) {
                return;
            }
            // The Var compared, and for `IN (…)` the list of constants, which a
            // view stores as an array expression (the planner folds it later).
            let (var, list) = match tag(qual) {
                Some(pg_sys::NodeTag::T_OpExpr) => {
                    let op = qual.cast::<pg_sys::OpExpr>();
                    if cstr(pg_sys::get_opname((*op).opno)) != "=" {
                        return;
                    }
                    (constant_comparison(&super::elements((*op).args)), None)
                }
                Some(pg_sys::NodeTag::T_ScalarArrayOpExpr) => {
                    let op = qual.cast::<pg_sys::ScalarArrayOpExpr>();
                    if !(*op).useOr || cstr(pg_sys::get_opname((*op).opno)) != "=" {
                        return;
                    }
                    let [column, array] = super::elements::<pg_sys::Node>((*op).args)[..] else {
                        return;
                    };
                    let array = strip_relabel(array);
                    match tag(array) {
                        Some(pg_sys::NodeTag::T_Const) => (Some(strip_relabel(column)), None),
                        Some(pg_sys::NodeTag::T_ArrayExpr) if constant_list(array) => {
                            (Some(strip_relabel(column)), Some((column, array)))
                        }
                        _ => return,
                    }
                }
                Some(pg_sys::NodeTag::T_NullTest) => {
                    let test = qual.cast::<pg_sys::NullTest>();
                    if (*test).nulltesttype != pg_sys::NullTestType::IS_NULL || (*test).argisrow {
                        return;
                    }
                    let var = strip_relabel((*test).arg.cast());
                    if tag(var) != Some(pg_sys::NodeTag::T_Var) {
                        return;
                    }
                    let Some(column) = self.own_column(var.cast()) else {
                        return;
                    };
                    // Not strict: a row an outer join below NULL-extends meets it,
                    // and a real row failing it still changes the view.
                    if self.null_extended(column.occ) {
                        return;
                    }
                    if nullable.is_none_or(|n| n.contains(&column.occ)) {
                        let mut sql = Sql::text("(");
                        sql.push_sql(column.sql());
                        sql.push_text(" IS NULL)");
                        self.graph.guards.entry(column.occ).or_default().push(sql);
                    }
                    return;
                }
                _ => return,
            };
            let Some(var) = var.filter(|v| tag(*v) == Some(pg_sys::NodeTag::T_Var)) else {
                return;
            };
            let Some(column) = self.own_column(var.cast()) else {
                return;
            };
            if nullable.is_some_and(|n| !n.contains(&column.occ)) {
                return;
            }
            let term = Resolved::Col(column.clone());
            let term_of = |node: *mut pg_sys::Node| {
                (tag(node) == Some(pg_sys::NodeTag::T_Var)).then(|| term.clone())
            };
            let sql = match list {
                None => self.deparse(qual, &term_of, &mut || {}),
                Some((left, array)) => self.in_list(qual.cast(), left, array.cast(), &term_of),
            };
            if let Some(sql) = sql {
                self.graph.guards.entry(column.occ).or_default().push(sql);
            }
        }
    }

    /// `(<left> OPERATOR(op) ANY (ARRAY[<constants>]::<array type>))`.
    ///
    /// SAFETY: `op` is a valid `ScalarArrayOpExpr`, `left` its first argument and
    /// `array` its second, an `ArrayExpr` of constants.
    unsafe fn in_list(
        &mut self,
        op: *mut pg_sys::ScalarArrayOpExpr,
        left: *mut pg_sys::Node,
        array: *mut pg_sys::ArrayExpr,
        term_of: &dyn Fn(*mut pg_sys::Node) -> Option<Resolved>,
    ) -> Option<Sql> {
        // SAFETY: fields of valid nodes.
        unsafe {
            let name = self.operator_name((*op).opno);
            let mut sql = Sql::text("(");
            sql.push_sql(self.deparse(left, term_of, &mut || {})?);
            sql.push_text(&format!(" OPERATOR({name}) ANY (ARRAY["));
            for (i, element) in super::elements::<pg_sys::Node>((*array).elements)
                .into_iter()
                .enumerate()
            {
                if i > 0 {
                    sql.push_text(", ");
                }
                sql.push_sql(self.deparse(element, term_of, &mut || {})?);
            }
            let ty = self.type_name((*array).array_typeid, pg_sys::exprTypmod(array.cast()));
            sql.push_text(&format!("]::{ty}))"));
            Some(sql)
        }
    }

    /// The column `var` reads when it is a column of a table in this level's own
    /// range table (not of a subquery, a view or a level above).
    ///
    /// SAFETY: `var` is a valid Var of the innermost level or one above.
    unsafe fn own_column(&self, var: *mut pg_sys::Var) -> Option<super::Column> {
        // SAFETY: fields of a valid Var.
        unsafe {
            if (*var).varlevelsup != 0 {
                return None;
            }
            let rtindex = usize::try_from((*var).varno).ok()?;
            let level = self.levels.last()?;
            let occ = match level.rtes.get(rtindex.checked_sub(1)?)? {
                RteInfo::Base(occ) | RteInfo::Tview { occ, .. } => *occ,
                _ => return None,
            };
            match self.resolve_var(var, 0) {
                Resolved::Col(column) if column.occ == occ => Some(column),
                _ => None,
            }
        }
    }
}

/// Whether `array`, an `ArrayExpr`, is a flat list of constants.
///
/// SAFETY: `array` is a valid `ArrayExpr`.
unsafe fn constant_list(array: *mut pg_sys::Node) -> bool {
    // SAFETY: fields of a valid ArrayExpr; tags read from valid nodes.
    unsafe {
        let array = array.cast::<pg_sys::ArrayExpr>();
        !(*array).multidims
            && super::elements::<pg_sys::Node>((*array).elements)
                .into_iter()
                .all(|e| tag(strip_relabel(e)) == Some(pg_sys::NodeTag::T_Const))
    }
}

/// The Var of `column = constant` (either side), else `None`.
///
/// SAFETY: `args` are valid expressions.
unsafe fn constant_comparison(args: &[*mut pg_sys::Node]) -> Option<*mut pg_sys::Node> {
    // SAFETY: tags read from valid nodes.
    unsafe {
        let [l, r] = *args else { return None };
        let (l, r) = (strip_relabel(l), strip_relabel(r));
        match (tag(l), tag(r)) {
            (Some(pg_sys::NodeTag::T_Var), Some(pg_sys::NodeTag::T_Const)) => Some(l),
            (Some(pg_sys::NodeTag::T_Const), Some(pg_sys::NodeTag::T_Var)) => Some(r),
            _ => None,
        }
    }
}
