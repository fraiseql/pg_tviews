//! Expressions written back as SQL for the mapping queries, and the functions a level calls.

use super::{
    Oid, Resolved, Spi, Sql, Walker, collect_functions, cstr, elements, pg_sys, quote_if_needed,
    quote_literal, tag, term_sql,
};

impl Walker<'_> {
    /// Write `expr` as SQL that resolves no name through `search_path`: relations,
    /// operators, functions and types are schema-qualified. `None` for a node it
    /// does not write, or an `unnest` element outside a comparison. `term_of` gives
    /// what each Var / Param stands for (Vars in walk order); `next_var` is called
    /// after each Var.
    ///
    /// SAFETY: `expr` is a valid expression.
    pub(super) unsafe fn deparse(
        &mut self,
        expr: *mut pg_sys::Node,
        term_of: &dyn Fn(*mut pg_sys::Node) -> Option<Resolved>,
        next_var: &mut dyn FnMut(),
    ) -> Option<Sql> {
        // SAFETY: each node is checked by tag before it is cast.
        unsafe {
            match tag(expr)? {
                pg_sys::NodeTag::T_Var => {
                    let term = term_of(expr)?;
                    next_var();
                    term_sql(&term)
                }
                pg_sys::NodeTag::T_Param => term_sql(&term_of(expr)?),
                pg_sys::NodeTag::T_Const => self.deparse_const(expr.cast()),
                pg_sys::NodeTag::T_OpExpr => {
                    let op = expr.cast::<pg_sys::OpExpr>();
                    let args = elements::<pg_sys::Node>((*op).args);
                    self.deparse_operator((*op).opno, &args, term_of, next_var)
                }
                pg_sys::NodeTag::T_ScalarArrayOpExpr => {
                    self.deparse_array_operator(expr.cast(), term_of, next_var)
                }
                pg_sys::NodeTag::T_FuncExpr => {
                    self.deparse_function(expr.cast(), term_of, next_var)
                }
                pg_sys::NodeTag::T_RelabelType => {
                    let r = expr.cast::<pg_sys::RelabelType>();
                    self.deparse_cast((*r).arg.cast(), (*r).resulttype, term_of, next_var)
                }
                pg_sys::NodeTag::T_ArrayCoerceExpr => {
                    let r = expr.cast::<pg_sys::ArrayCoerceExpr>();
                    self.deparse_cast((*r).arg.cast(), (*r).resulttype, term_of, next_var)
                }
                pg_sys::NodeTag::T_CoerceViaIO => {
                    let r = expr.cast::<pg_sys::CoerceViaIO>();
                    self.deparse_cast((*r).arg.cast(), (*r).resulttype, term_of, next_var)
                }
                pg_sys::NodeTag::T_BoolExpr => self.deparse_bool(expr.cast(), term_of, next_var),
                _ => None,
            }
        }
    }

    /// A constant, as a literal cast to its type.
    ///
    /// SAFETY: `c` is a valid Const.
    unsafe fn deparse_const(&mut self, c: *mut pg_sys::Const) -> Option<Sql> {
        // SAFETY: fields of a valid Const; the output function is its type's.
        unsafe {
            let ty = self.type_name((*c).consttype);
            if (*c).constisnull {
                return Some(Sql::text(format!("NULL::{ty}")));
            }
            let mut output: Oid = Oid::INVALID;
            let mut varlena = false;
            pg_sys::getTypeOutputInfo((*c).consttype, &raw mut output, &raw mut varlena);
            let text = cstr(pg_sys::OidOutputFunctionCall(output, (*c).constvalue));
            Some(Sql::text(format!("{}::{ty}", quote_literal(&text)?)))
        }
    }

    /// A prefix or infix operator `opno` over `args`, schema-qualified.
    ///
    /// SAFETY: `args` are valid expressions.
    unsafe fn deparse_operator(
        &mut self,
        opno: Oid,
        args: &[*mut pg_sys::Node],
        term_of: &dyn Fn(*mut pg_sys::Node) -> Option<Resolved>,
        next_var: &mut dyn FnMut(),
    ) -> Option<Sql> {
        let name = self.operator_name(opno);
        let mut sql = Sql::text("(");
        // SAFETY: the caller's valid expressions.
        unsafe {
            match *args {
                [arg] => {
                    sql.push_text(&format!("OPERATOR({name}) "));
                    sql.push_sql(self.deparse(arg, term_of, next_var)?);
                }
                [l, r] => {
                    sql.push_sql(self.deparse(l, term_of, next_var)?);
                    sql.push_text(&format!(" OPERATOR({name}) "));
                    sql.push_sql(self.deparse(r, term_of, next_var)?);
                }
                _ => return None,
            }
        }
        sql.push_text(")");
        Some(sql)
    }

    /// `l OPERATOR(op) ANY|ALL (r)`.
    ///
    /// SAFETY: `op` is a valid `ScalarArrayOpExpr`.
    unsafe fn deparse_array_operator(
        &mut self,
        op: *mut pg_sys::ScalarArrayOpExpr,
        term_of: &dyn Fn(*mut pg_sys::Node) -> Option<Resolved>,
        next_var: &mut dyn FnMut(),
    ) -> Option<Sql> {
        // SAFETY: fields and arguments of a valid ScalarArrayOpExpr.
        unsafe {
            let name = self.operator_name((*op).opno);
            let [l, r] = elements::<pg_sys::Node>((*op).args)[..] else {
                return None;
            };
            let mut sql = Sql::text("(");
            sql.push_sql(self.deparse(l, term_of, next_var)?);
            sql.push_text(&format!(
                " OPERATOR({name}) {} (",
                if (*op).useOr { "ANY" } else { "ALL" }
            ));
            sql.push_sql(self.deparse(r, term_of, next_var)?);
            sql.push_text("))");
            Some(sql)
        }
    }

    /// A function call, schema-qualified, or the cast an implicit call stands for.
    ///
    /// SAFETY: `f` is a valid `FuncExpr`.
    unsafe fn deparse_function(
        &mut self,
        f: *mut pg_sys::FuncExpr,
        term_of: &dyn Fn(*mut pg_sys::Node) -> Option<Resolved>,
        next_var: &mut dyn FnMut(),
    ) -> Option<Sql> {
        // SAFETY: fields and arguments of a valid FuncExpr.
        unsafe {
            let args = elements::<pg_sys::Node>((*f).args);
            if (*f).funcformat != pg_sys::CoercionForm::COERCE_EXPLICIT_CALL
                && (*f).funcformat != pg_sys::CoercionForm::COERCE_SQL_SYNTAX
            {
                let [arg, ..] = args[..] else { return None };
                return self.deparse_cast(arg, (*f).funcresulttype, term_of, next_var);
            }
            let (name, _) = self.function((*f).funcid);
            let mut sql = Sql::text(format!("{name}("));
            for (i, arg) in args.into_iter().enumerate() {
                if i > 0 {
                    sql.push_text(", ");
                }
                sql.push_sql(self.deparse(arg, term_of, next_var)?);
            }
            sql.push_text(")");
            Some(sql)
        }
    }

    /// `(arg)::type`.
    ///
    /// SAFETY: `arg` is a valid expression.
    unsafe fn deparse_cast(
        &mut self,
        arg: *mut pg_sys::Node,
        ty: Oid,
        term_of: &dyn Fn(*mut pg_sys::Node) -> Option<Resolved>,
        next_var: &mut dyn FnMut(),
    ) -> Option<Sql> {
        let mut sql = Sql::text("(");
        // SAFETY: the caller's valid expression.
        sql.push_sql(unsafe { self.deparse(arg, term_of, next_var)? });
        sql.push_text(&format!(")::{}", self.type_name(ty)));
        Some(sql)
    }

    /// `NOT a`, `a AND b …`, `a OR b …`.
    ///
    /// SAFETY: `b` is a valid `BoolExpr`.
    unsafe fn deparse_bool(
        &mut self,
        b: *mut pg_sys::BoolExpr,
        term_of: &dyn Fn(*mut pg_sys::Node) -> Option<Resolved>,
        next_var: &mut dyn FnMut(),
    ) -> Option<Sql> {
        // SAFETY: fields and arguments of a valid BoolExpr.
        unsafe {
            let args = elements::<pg_sys::Node>((*b).args);
            let mut sql = Sql::text("(");
            if (*b).boolop == pg_sys::BoolExprType::NOT_EXPR {
                sql.push_text("NOT ");
                sql.push_sql(self.deparse(*args.first()?, term_of, next_var)?);
            } else {
                let joiner = if (*b).boolop == pg_sys::BoolExprType::AND_EXPR {
                    " AND "
                } else {
                    " OR "
                };
                for (i, arg) in args.into_iter().enumerate() {
                    if i > 0 {
                        sql.push_text(joiner);
                    }
                    sql.push_sql(self.deparse(arg, term_of, next_var)?);
                }
            }
            sql.push_text(")");
            Some(sql)
        }
    }

    pub(super) fn type_name(&mut self, ty: Oid) -> String {
        self.catalog
            .types
            .entry(ty.to_u32())
            // SAFETY: a catalog lookup by OID.
            .or_insert_with(|| cstr(unsafe { pg_sys::format_type_be_qualified(ty) }))
            .clone()
    }

    pub(super) fn operator_name(&mut self, opno: Oid) -> String {
        self.catalog
            .operators
            .entry(opno.to_u32())
            .or_insert_with(|| {
                Spi::get_one_with_args::<String>(
                    "SELECT pg_catalog.quote_ident(n.nspname) || '.' || o.oprname \
                     FROM pg_catalog.pg_operator o \
                     JOIN pg_catalog.pg_namespace n ON n.oid = o.oprnamespace \
                     WHERE o.oid = $1",
                    &[crate::utils::spi::oid(opno)],
                )
                .ok()
                .flatten()
                .unwrap_or_default()
            })
            .clone()
    }

    /// The qualified name of a function, and whether it is immutable.
    pub(super) fn function(&mut self, funcid: Oid) -> (String, bool) {
        self.catalog
            .functions
            .entry(funcid.to_u32())
            .or_insert_with(|| {
                // SAFETY: catalog lookups by OID.
                unsafe {
                    let name = cstr(pg_sys::get_func_name(funcid));
                    let nsp = cstr(pg_sys::get_namespace_name(pg_sys::get_func_namespace(
                        funcid,
                    )));
                    let immutable = pg_sys::func_volatile(funcid)
                        == pg_sys::PROVOLATILE_IMMUTABLE.cast_signed();
                    (
                        format!("{}.{}", quote_if_needed(&nsp), quote_if_needed(&name)),
                        immutable,
                    )
                }
            })
            .clone()
    }

    // ── functions that may read tables ──────────────────────────────────────

    /// Record the non-immutable functions outside `pg_catalog` that this level
    /// calls: tables they read are invisible to `pg_tviews`.
    ///
    /// SAFETY: `query` is the valid Query of the innermost level.
    pub(super) unsafe fn note_functions(&mut self, query: *mut pg_sys::Node) {
        let mut funcids = Vec::new();
        // SAFETY: a read-only walk of a valid Query.
        unsafe { collect_functions(query, &mut funcids) };
        for funcid in funcids {
            let (name, immutable) = self.function(funcid);
            if !immutable
                && !name.starts_with("pg_catalog.")
                && !self.graph.untracked_functions.contains(&funcid.to_u32())
            {
                self.graph.untracked_functions.push(funcid.to_u32());
            }
        }
    }
}
