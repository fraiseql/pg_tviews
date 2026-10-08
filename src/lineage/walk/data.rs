//! The shape of the `data` output, and the identity the rows are named by.

use super::{
    Column, DataEmbed, DataField, DataShape, Flags, HashMap, Link, Maps, OutputColumn, Resolved,
    RteInfo, Scope, TViewResult, WalkedIdentity, Walker, base_read, column_read_counts, const_text,
    cstr, elements, in_clause, list_len, pg_sys, relation_entry, setop_leaves, strip_relabel, tag,
};

impl Walker<'_> {
    /// What the `data` output of the backing view's own SELECT is built of:
    /// `jsonb_build_object` keys over base columns, other TVIEWs' `data` (as a
    /// value or inside `jsonb_agg`), and anything else, which makes it opaque.
    ///
    /// SAFETY: `query` is the valid Query of the innermost level, whose RTEs are
    /// known.
    pub(super) unsafe fn data_shape(
        &mut self,
        query: *mut pg_sys::Query,
        tles: &[*mut pg_sys::TargetEntry],
    ) -> Option<DataShape> {
        // SAFETY: fields of valid target entries and of the Query.
        unsafe {
            let tle = tles
                .iter()
                .find(|t| !(***t).resjunk && cstr((***t).resname) == "data")?;
            let mut shape = DataShape::default();
            let mut fields: Vec<(Vec<String>, *mut pg_sys::Var)> = Vec::new();
            self.data_value(
                query,
                (**tle).expr.cast(),
                &mut Vec::new(),
                &mut shape,
                &mut fields,
            );
            // A column is read only as a field when the definition reads it as many
            // times as the fields copy it. A windowed or DISTINCT level, or one
            // grouped by something other than the identity, combines rows: none of
            // its columns maps one row to one field. Grouped by the identity, each
            // group is one row, and the GROUP BY entries only name the group.
            let identity_grouped = match &self.graph.identity {
                Some(Ok(identity)) => tles
                    .get(identity.position)
                    .is_some_and(|tle| in_clause((**tle).ressortgroupref, (*query).groupClause)),
                _ => false,
            };
            let grouped = (*query).hasAggs || !(*query).groupClause.is_null();
            let combined = (*query).hasWindowFuncs
                || !(*query).distinctClause.is_null()
                || (grouped && !identity_grouped);
            let reads = column_read_counts(query, grouped);
            let mut copied: HashMap<(usize, i16), usize> = HashMap::new();
            let resolved: Vec<Option<(usize, i16)>> = fields
                .iter()
                .map(|(_, var)| base_read(query, *var))
                .collect();
            for read in resolved.iter().flatten() {
                *copied.entry(*read).or_default() += 1;
            }
            for ((path, var), read) in fields.into_iter().zip(resolved) {
                match self.resolve_var(var, 0) {
                    Resolved::Col(column) => {
                        // Read off a table of this level, not through a view or a
                        // subquery whose other clauses may read it too.
                        let only_in_data = !combined
                            && read.is_some_and(|r| {
                                relation_entry(query, r.0) && reads.get(&r) == copied.get(&r)
                            });
                        let relid = self.graph.occurrences[column.occ].relid;
                        let root = matches!(&self.graph.identity,
                            Some(Ok(identity)) if identity.columns.first().is_some_and(|c| c.occ == column.occ));
                        shape.fields.push(DataField {
                            path,
                            column,
                            relid,
                            root,
                            only_in_data,
                        });
                    }
                    _ => shape.opaque = true,
                }
            }
            Some(shape)
        }
    }

    /// Read one value of `data` at `path`.
    ///
    /// SAFETY: `node` is a valid expression of `query`, the innermost level.
    pub(super) unsafe fn data_value(
        &mut self,
        query: *mut pg_sys::Query,
        node: *mut pg_sys::Node,
        path: &mut Vec<String>,
        shape: &mut DataShape,
        fields: &mut Vec<(Vec<String>, *mut pg_sys::Var)>,
    ) {
        // SAFETY: checked by tag before each cast.
        unsafe {
            let node = strip_relabel(node);
            match tag(node) {
                Some(pg_sys::NodeTag::T_FuncExpr) => {
                    let f = node.cast::<pg_sys::FuncExpr>();
                    let args = elements::<pg_sys::Node>((*f).args);
                    if self.function((*f).funcid).0 != "pg_catalog.jsonb_build_object"
                        || !args.len().is_multiple_of(2)
                    {
                        shape.opaque = true;
                        return;
                    }
                    for pair in args.chunks(2) {
                        let Some(key) = const_text(pair[0]) else {
                            shape.opaque = true;
                            continue;
                        };
                        path.push(key);
                        self.data_value(query, pair[1], path, shape, fields);
                        path.pop();
                    }
                }
                Some(pg_sys::NodeTag::T_Var) => {
                    let var = node.cast::<pg_sys::Var>();
                    if (*var).varlevelsup == 0
                        && let Some(entity) = base_read(query, var)
                            .and_then(|(varno, attno)| self.tview_data(query, varno, attno))
                    {
                        shape.embeds.push(DataEmbed {
                            entity,
                            path: path.clone(),
                            array: false,
                        });
                    } else {
                        fields.push((path.clone(), var));
                    }
                }
                // `COALESCE(jsonb_agg(…), '[]')`: the fallback is a constant.
                Some(pg_sys::NodeTag::T_CoalesceExpr) => {
                    let args =
                        elements::<pg_sys::Node>((*node.cast::<pg_sys::CoalesceExpr>()).args);
                    if args[1..]
                        .iter()
                        .any(|a| tag(strip_relabel(*a)) != Some(pg_sys::NodeTag::T_Const))
                    {
                        shape.opaque = true;
                    }
                    if let Some(&first) = args.first() {
                        self.data_value(query, first, path, shape, fields);
                    }
                }
                Some(pg_sys::NodeTag::T_Aggref) => {
                    match self.aggregated_tview_data(query, node.cast()) {
                        Some(entity) => shape.embeds.push(DataEmbed {
                            entity,
                            path: path.clone(),
                            array: true,
                        }),
                        None => shape.opaque = true,
                    }
                }
                Some(pg_sys::NodeTag::T_SubLink) => {
                    let sublink = node.cast::<pg_sys::SubLink>();
                    let sub = (*sublink).subselect.cast::<pg_sys::Query>();
                    let embedded = ((*sublink).subLinkType == pg_sys::SubLinkType::EXPR_SUBLINK
                        && tag(sub.cast()) == Some(pg_sys::NodeTag::T_Query))
                    .then(|| elements::<pg_sys::TargetEntry>((*sub).targetList))
                    .and_then(|tles| match tles[..] {
                        [tle]
                            if tag(strip_relabel((*tle).expr.cast()))
                                == Some(pg_sys::NodeTag::T_Aggref) =>
                        {
                            self.aggregated_tview_data(
                                sub,
                                strip_relabel((*tle).expr.cast()).cast(),
                            )
                        }
                        _ => None,
                    });
                    match embedded {
                        Some(entity) => shape.embeds.push(DataEmbed {
                            entity,
                            path: path.clone(),
                            array: true,
                        }),
                        None => shape.opaque = true,
                    }
                }
                Some(pg_sys::NodeTag::T_Const) => {}
                _ => shape.opaque = true,
            }
        }
    }

    /// `jsonb_agg(<a TVIEW's data>)` over a level of `query`: that TVIEW.
    ///
    /// SAFETY: `aggref` is a valid Aggref of `query`.
    pub(super) unsafe fn aggregated_tview_data(
        &mut self,
        query: *mut pg_sys::Query,
        aggref: *mut pg_sys::Aggref,
    ) -> Option<String> {
        // SAFETY: fields of a valid Aggref and its argument list.
        unsafe {
            if self.function((*aggref).aggfnoid).0 != "pg_catalog.jsonb_agg" {
                return None;
            }
            let [arg] = elements::<pg_sys::TargetEntry>((*aggref).args)[..] else {
                return None;
            };
            let value = strip_relabel((*arg).expr.cast());
            if tag(value) != Some(pg_sys::NodeTag::T_Var)
                || (*value.cast::<pg_sys::Var>()).varlevelsup != 0
            {
                return None;
            }
            let (varno, attno) = base_read(query, value.cast())?;
            self.tview_data(query, varno, attno)
        }
    }

    /// The TVIEW whose `data` column `(varno, attno)` of `query`'s own level is,
    /// read through its table or its backing view.
    ///
    /// SAFETY: `query` is a valid Query.
    pub(super) unsafe fn tview_data(
        &self,
        query: *mut pg_sys::Query,
        varno: usize,
        attno: i16,
    ) -> Option<String> {
        // SAFETY: fields of an RTE of `query`'s level.
        unsafe {
            let rte =
                *elements::<pg_sys::RangeTblEntry>((*query).rtable).get(varno.checked_sub(1)?)?;
            if (*rte).rtekind != pg_sys::RTEKind::RTE_RELATION || attno <= 0 {
                return None;
            }
            let relid = (*rte).relid;
            if cstr(pg_sys::get_attname(relid, attno, true)) != "data" {
                return None;
            }
            self.ctx
                .tview_tables
                .get(&relid)
                .or_else(|| self.ctx.tview_views.get(&relid))
                .cloned()
        }
    }

    /// Whether an equality of the graph makes `column` equal to one of `keys` on
    /// every row of `column`'s occurrence that contributes (a WHERE or inner join
    /// condition, or an outer join's with `column` on the nullable side: NULL where
    /// it has no match).
    pub(super) fn equal_to_key(&self, column: &Column, keys: &[Column]) -> bool {
        self.graph.conjuncts.iter().any(|c| {
            c.equality.as_ref().is_some_and(|(x, y)| {
                let (toward, key) = if x == column {
                    (if c.a == x.occ { c.a_to_b } else { c.b_to_a }, y)
                } else if y == column {
                    (if c.a == y.occ { c.a_to_b } else { c.b_to_a }, x)
                } else {
                    return false;
                };
                toward == Maps::Yes && keys.contains(key)
            })
        })
    }

    /// The identity of the backing view's own SELECT (ADR 0169): its DISTINCT ON
    /// key, or `pk_<entity>`.
    ///
    /// SAFETY: `query` is the valid Query of the innermost level and `tles` its
    /// target list.
    pub(super) unsafe fn identity(
        &self,
        query: *mut pg_sys::Query,
        tles: &[*mut pg_sys::TargetEntry],
    ) -> Result<WalkedIdentity, crate::lineage::IdentityError> {
        // SAFETY: fields of a valid Query and of its target entries.
        unsafe {
            let outputs: Vec<OutputColumn> = tles
                .iter()
                .map(|&tle| OutputColumn {
                    name: cstr((*tle).resname),
                    junk: (*tle).resjunk,
                    sortgroupref: (*tle).ressortgroupref,
                    column: match self.resolve_expr((*tle).expr.cast()) {
                        Resolved::Col(c) => Some(c),
                        _ => None,
                    },
                    type_oid: pg_sys::exprType((*tle).expr.cast()).to_u32(),
                })
                .collect();
            let distinct_on: Option<Vec<u32>> = (*query).hasDistinctOn.then(|| {
                elements::<pg_sys::SortGroupClause>((*query).distinctClause)
                    .iter()
                    .map(|c| (**c).tleSortGroupRef)
                    .collect()
            });
            let selected = crate::lineage::select_identity(
                self.ctx.entity,
                &outputs,
                distinct_on.as_deref(),
                &|column, key| self.equal_to_key(column, std::slice::from_ref(key)),
            )?;
            let chosen = &outputs[selected.position];
            Ok(WalkedIdentity {
                name: chosen.name.clone(),
                position: selected.position,
                type_oid: chosen.type_oid,
                kind: selected.kind,
                columns: chosen.column.iter().cloned().collect(),
            })
        }
    }

    /// The outputs of a UNION subquery: each column stands for the matching column
    /// of every branch.
    ///
    /// SAFETY: `query` is a valid set-operation Query.
    pub(super) unsafe fn union_outputs(
        &mut self,
        query: *mut pg_sys::Query,
        flags: &Flags,
    ) -> TViewResult<Vec<Resolved>> {
        // SAFETY: fields of a valid Query.
        unsafe {
            self.push_level(
                query,
                vec![RteInfo::Other; list_len((*query).rtable)],
                Link::From,
            );
            let mut leaves = Vec::new();
            setop_leaves((*query).setOperations, &mut leaves);
            let width = list_len((*query).targetList);
            let union = self.next_union();
            let mut columns: Vec<(Vec<Resolved>, Vec<Scope>)> =
                vec![(Vec::new(), Vec::new()); width];
            let result: TViewResult<()> = (|| {
                for (leaf, rtindex) in leaves.into_iter().enumerate() {
                    let Some(&rte) = elements::<pg_sys::RangeTblEntry>((*query).rtable)
                        .get(rtindex.wrapping_sub(1))
                    else {
                        continue;
                    };
                    let mut unions = flags.unions.clone();
                    unions.push((union, leaf));
                    let leaf_flags = Flags {
                        unions: unions.clone(),
                        ..flags.clone()
                    };
                    let outputs = self.level((*rte).subquery, &leaf_flags, Link::From)?;
                    for (i, out) in outputs.into_iter().take(width).enumerate() {
                        let (terms, holes) = &mut columns[i];
                        match out {
                            Resolved::Alt(ts, hs) => {
                                terms.extend(ts);
                                holes.extend(hs);
                            }
                            Resolved::Expr(e) if e.element => holes.push(unions.clone()),
                            term
                            @ (Resolved::Col(_) | Resolved::Expr(_) | Resolved::Inbound(_)) => {
                                terms.push(term);
                            }
                            Resolved::Opaque => holes.push(unions.clone()),
                        }
                    }
                }
                self.unread_ctes(flags)
            })();
            self.levels.pop();
            result?;
            Ok(columns
                .into_iter()
                .map(|(terms, holes)| {
                    if terms.is_empty() {
                        Resolved::Opaque
                    } else {
                        Resolved::Alt(terms, holes)
                    }
                })
                .collect())
        }
    }
}
