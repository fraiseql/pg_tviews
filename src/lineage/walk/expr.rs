//! What an expression stands for: a column, a computed value, a SQL fragment.

use super::{
    Cast, Column, Computed, Lookup, Oid, Operand, Resolved, RteInfo, Sql, Walker, collect_vars,
    cstr, elements, has_sublink, pg_sys, sql_occs, strip_casts, strip_relabel, tag, unnest_array,
};

impl Walker<'_> {
    /// What `var`, `levelsup` levels above the innermost one, stands for.
    ///
    /// SAFETY: `var` is a valid Var.
    pub(super) unsafe fn resolve_var(&self, var: *mut pg_sys::Var, levelsup: usize) -> Resolved {
        // SAFETY: fields of a valid Var; RTE lists of valid levels.
        unsafe {
            let Some(index) = self.levels.len().checked_sub(1 + levelsup) else {
                return Resolved::Opaque;
            };
            let level = &self.levels[index];
            let (Ok(rtindex), attno) = (usize::try_from((*var).varno), (*var).varattno) else {
                return Resolved::Opaque;
            };
            if attno <= 0 {
                return Resolved::Opaque;
            }
            match level.rtes.get(rtindex.wrapping_sub(1)) {
                Some(RteInfo::Base(occ) | RteInfo::Tview { occ, .. }) => {
                    let relid = Oid::from(self.graph.occurrences[*occ].relid);
                    Resolved::Col(Column {
                        occ: *occ,
                        attnum: attno,
                        name: cstr(pg_sys::get_attname(relid, attno, true)),
                    })
                }
                Some(RteInfo::Outputs(outputs)) => outputs
                    .get(attno as usize - 1)
                    .cloned()
                    .unwrap_or(Resolved::Opaque),
                Some(RteInfo::Join(aliases)) => {
                    let alias = elements::<pg_sys::Node>(*aliases)
                        .get(attno as usize - 1)
                        .copied()
                        .unwrap_or(std::ptr::null_mut());
                    self.resolve_alias(alias, index)
                }
                _ => Resolved::Opaque,
            }
        }
    }

    /// A join alias variable (or a PostgreSQL 18 group expression): a Var of the
    /// entry's own level, maybe behind a cast.
    ///
    /// SAFETY: `node` is null or a valid expression of level `index`.
    pub(super) unsafe fn resolve_alias(&self, node: *mut pg_sys::Node, index: usize) -> Resolved {
        // SAFETY: checked by tag before each cast.
        unsafe {
            let node = strip_relabel(node);
            if tag(node) != Some(pg_sys::NodeTag::T_Var) {
                return Resolved::Opaque;
            }
            let var = node.cast::<pg_sys::Var>();
            let levelsup = self.levels.len() - 1 - index + (*var).varlevelsup as usize;
            self.resolve_var(var, levelsup)
        }
    }

    /// What an output column of the innermost level stands for: a column, an
    /// expression computed from columns, an element of `unnest(<array>)`, or
    /// opaque (another set-returning function: its value is not computed from the
    /// row it comes from).
    ///
    /// SAFETY: `node` is null or a valid expression of the innermost level.
    pub(super) unsafe fn output(&mut self, node: *mut pg_sys::Node) -> Resolved {
        // SAFETY: read-only checks of a valid expression, then forwarded.
        unsafe {
            if tag(strip_relabel(node)) == Some(pg_sys::NodeTag::T_Var) {
                return self.resolve_expr(node);
            }
            // `unnest(<array>)::T`, or a cast of an element.
            let (inner, casts) = strip_casts(node);
            if pg_sys::expression_returns_set(node) {
                if pg_sys::contain_mutable_functions(node) {
                    return Resolved::Opaque;
                }
                return match unnest_array(inner) {
                    Some(array) => match self.computed(array, true) {
                        Resolved::Expr(e) => self.cast_element(e, &casts),
                        _ => Resolved::Opaque,
                    },
                    None => Resolved::Opaque,
                };
            }
            if let Some(e) = self.element_behind(inner, &casts)
                && !pg_sys::contain_mutable_functions(node)
            {
                return self.cast_element(e, &casts);
            }
            self.computed(node, false)
        }
    }

    /// `expr` of the innermost level written over the columns it reads: opaque
    /// when it is not immutable, reads no column (or an opaque one), holds a
    /// subquery or a node [`Walker::deparse`] does not write (an aggregate).
    ///
    /// SAFETY: `expr` is null or a valid expression of the innermost level.
    pub(super) unsafe fn computed(&mut self, expr: *mut pg_sys::Node, element: bool) -> Resolved {
        // SAFETY: read-only checks and a walk of a valid expression.
        unsafe {
            if expr.is_null()
                || pg_sys::contain_mutable_functions(expr)
                || has_sublink(expr)
                || pg_sys::expression_returns_set(expr)
            {
                return Resolved::Opaque;
            }
            let mut vars = Vec::new();
            collect_vars(expr, &mut vars);
            if vars.is_empty() {
                return Resolved::Opaque;
            }
            let mut terms = Vec::new();
            for var in vars {
                match self.resolve_var(var, (*var).varlevelsup as usize) {
                    term @ (Resolved::Col(_) | Resolved::Expr(_)) => terms.push(term),
                    _ => return Resolved::Opaque,
                }
            }
            let strict = !pg_sys::contain_nonstrict_functions(expr)
                && terms
                    .iter()
                    .all(|t| !matches!(t, Resolved::Expr(e) if !e.strict));
            let index = std::cell::Cell::new(0_usize);
            let term_of = |node: *mut pg_sys::Node| {
                (tag(node) == Some(pg_sys::NodeTag::T_Var))
                    .then(|| terms.get(index.get()).cloned())
                    .flatten()
            };
            let mut next_var = || index.set(index.get() + 1);
            match self.deparse(expr, &term_of, &mut next_var) {
                Some(sql) => Resolved::Expr(Computed {
                    sql,
                    element,
                    strict,
                    type_oid: pg_sys::exprType(expr),
                }),
                None => Resolved::Opaque,
            }
        }
    }

    /// An element of `unnest(<array>)` cast one cast after the other: an element of
    /// the array cast to the arrays of those types, `(<array>)::T[]`. Opaque
    /// when a type has no array type.
    pub(super) fn cast_element(&mut self, mut element: Computed, casts: &[Cast]) -> Resolved {
        for cast in casts {
            // SAFETY: a catalog lookup by type OID.
            let array_type = unsafe { pg_sys::get_array_type(cast.type_oid) };
            if array_type == Oid::INVALID {
                return Resolved::Opaque;
            }
            if array_type == element.type_oid {
                continue;
            }
            let mut sql = Sql::text("(");
            sql.push_sql(element.sql);
            sql.push_text(&format!(")::{}", self.type_name(array_type)));
            element = Computed {
                sql,
                element: true,
                strict: element.strict && cast.strict,
                type_oid: array_type,
            };
        }
        Resolved::Expr(element)
    }

    /// The `unnest` element a Var under casts stands for, when there are casts.
    ///
    /// SAFETY: `inner` is a valid expression of the innermost level.
    pub(super) unsafe fn element_behind(
        &self,
        inner: *mut pg_sys::Node,
        casts: &[Cast],
    ) -> Option<Computed> {
        if casts.is_empty() {
            return None;
        }
        // SAFETY: forwarded.
        match unsafe { self.resolve_expr(inner) } {
            Resolved::Expr(e) if e.element => Some(e),
            _ => None,
        }
    }

    /// What an output expression stands for: a Var (maybe behind a cast) or opaque.
    ///
    /// SAFETY: `node` is null or a valid expression of the innermost level.
    pub(super) unsafe fn resolve_expr(&self, node: *mut pg_sys::Node) -> Resolved {
        // SAFETY: checked by tag before each cast.
        unsafe {
            let node = strip_relabel(node);
            if tag(node) == Some(pg_sys::NodeTag::T_Var) {
                let var = node.cast::<pg_sys::Var>();
                self.resolve_var(var, (*var).varlevelsup as usize)
            } else {
                Resolved::Opaque
            }
        }
    }

    // ── deparsing ───────────────────────────────────────────────────────────

    /// Write a predicate as SQL, with the expressions it looks rows up by. As
    /// [`Walker::deparse`], except that `=` with an element of `unnest(<array>)` is
    /// written as array membership, `x = ANY (<array>)`, and membership with the
    /// element type's equality also as containment, `<array> @> ARRAY[x]`, which a
    /// GIN index on the array serves.
    ///
    /// SAFETY: `expr` is a valid expression.
    pub(super) unsafe fn conjunct_sql(
        &mut self,
        expr: *mut pg_sys::Node,
        term_of: &dyn Fn(*mut pg_sys::Node) -> Option<Resolved>,
        next_var: &mut dyn FnMut(),
    ) -> Option<(Sql, Vec<Lookup>)> {
        // SAFETY: each node is checked by tag before it is cast.
        unsafe {
            match tag(expr)? {
                pg_sys::NodeTag::T_OpExpr => {
                    let op = expr.cast::<pg_sys::OpExpr>();
                    if let [l, r] = elements::<pg_sys::Node>((*op).args)[..]
                        && cstr(pg_sys::get_opname((*op).opno)) == "="
                    {
                        let l = self.operand(l, term_of, next_var)?;
                        let r = self.operand(r, term_of, next_var)?;
                        return match (l.element, r.element) {
                            (false, false) => Some(self.comparison((*op).opno, &l, &r)),
                            (false, true) => self.membership((*op).opno, &l, &r),
                            (true, false) => {
                                let commutator = pg_sys::get_commutator((*op).opno);
                                (commutator != Oid::INVALID)
                                    .then(|| self.membership(commutator, &r, &l))
                                    .flatten()
                            }
                            (true, true) => None,
                        };
                    }
                }
                pg_sys::NodeTag::T_ScalarArrayOpExpr => {
                    let op = expr.cast::<pg_sys::ScalarArrayOpExpr>();
                    if let [l, r] = elements::<pg_sys::Node>((*op).args)[..]
                        && (*op).useOr
                    {
                        let l = self.operand(l, term_of, next_var)?;
                        let r = self.operand(r, term_of, next_var)?;
                        if l.element || r.element {
                            return None;
                        }
                        return self.membership((*op).opno, &l, &r);
                    }
                }
                _ => {}
            }
            Some((self.deparse(expr, term_of, next_var)?, Vec::new()))
        }
    }

    /// One side of a comparison: written by [`Walker::deparse`], or the array of an
    /// `unnest` element.
    ///
    /// SAFETY: `arg` is a valid expression; its Vars are the next ones `term_of`
    /// gives.
    pub(super) unsafe fn operand(
        &mut self,
        arg: *mut pg_sys::Node,
        term_of: &dyn Fn(*mut pg_sys::Node) -> Option<Resolved>,
        next_var: &mut dyn FnMut(),
    ) -> Option<Operand> {
        // SAFETY: checked by tag; `term_of` only reads the node.
        unsafe {
            // An element under casts compares as an element of the cast array.
            let (bare, casts) = strip_casts(arg);
            let term_at = |node: *mut pg_sys::Node| match tag(node) {
                Some(pg_sys::NodeTag::T_Var | pg_sys::NodeTag::T_Param) => term_of(node),
                _ => None,
            };
            if let Some(Resolved::Expr(e)) = term_at(bare)
                && e.element
            {
                if pg_sys::contain_mutable_functions(arg) {
                    return None;
                }
                let Resolved::Expr(e) = self.cast_element(e, &casts) else {
                    return None;
                };
                if tag(bare) == Some(pg_sys::NodeTag::T_Var) {
                    next_var();
                }
                return Some(Operand {
                    sql: e.sql,
                    type_oid: e.type_oid,
                    column: false,
                    element: true,
                });
            }
            let column = matches!(
                term_at(strip_relabel(arg)),
                Some(Resolved::Col(_) | Resolved::Inbound(_))
            );
            Some(Operand {
                sql: self.deparse(arg, term_of, next_var)?,
                type_oid: pg_sys::exprType(arg),
                column,
                element: false,
            })
        }
    }

    /// `l = r`; a computed side compared with a column is looked up by (btree).
    pub(super) fn comparison(&mut self, opno: Oid, l: &Operand, r: &Operand) -> (Sql, Vec<Lookup>) {
        let name = self.operator_name(opno);
        let mut sql = Sql::text("(");
        sql.push_sql(l.sql.clone());
        sql.push_text(&format!(" OPERATOR({name}) "));
        sql.push_sql(r.sql.clone());
        sql.push_text(")");
        let mut lookups = Vec::new();
        for (side, other) in [(l, r), (r, l)] {
            if !side.column
                && other.column
                && let [occ] = sql_occs(&side.sql)[..]
            {
                lookups.push(Lookup {
                    occ,
                    expr: side.sql.clone(),
                    gin: false,
                });
            }
        }
        (sql, lookups)
    }

    /// `scalar op ANY (array)`. With the element type's own equality it also
    /// writes `array @> ARRAY[scalar]`: true whenever the membership is, and served
    /// by a GIN index on the array.
    pub(super) fn membership(
        &mut self,
        opno: Oid,
        scalar: &Operand,
        array: &Operand,
    ) -> Option<(Sql, Vec<Lookup>)> {
        let name = self.operator_name(opno);
        let mut sql = Sql::text("(");
        sql.push_sql(scalar.sql.clone());
        sql.push_text(&format!(" OPERATOR({name}) ANY ("));
        sql.push_sql(array.sql.clone());
        sql.push_text("))");
        // SAFETY: catalog lookups by type OID.
        let containment = unsafe {
            let element = pg_sys::get_element_type(array.type_oid);
            element != Oid::INVALID
                && element == scalar.type_oid
                && (*pg_sys::lookup_type_cache(element, pg_sys::TYPECACHE_EQ_OPR.cast_signed()))
                    .eq_opr
                    == opno
        };
        let mut lookups = Vec::new();
        if containment {
            let mut both = Sql::text("(");
            both.push_sql(sql);
            both.push_text(" AND (");
            both.push_sql(array.sql.clone());
            both.push_text(") OPERATOR(pg_catalog.@>) ARRAY[");
            both.push_sql(scalar.sql.clone());
            both.push_text("])");
            sql = both;
            if let [occ] = sql_occs(&array.sql)[..] {
                lookups.push(Lookup {
                    occ,
                    expr: array.sql.clone(),
                    gin: true,
                });
            }
        }
        Some((sql, lookups))
    }
}
