//! The levels of the walk: a query's range table, joins, CTEs and the roots its key comes from.

use super::{
    Column, Flags, HashSet, IdentityKind, Level, Link, MAX_DEPTH, Occurrence, Oid, Origin, Piece,
    Resolved, Root, RteInfo, Scope, TViewError, TViewResult, WINDOW_REASON, WalkedIdentity, Walker,
    conjuncts, cstr, elements, in_clause, is_required_sublink, list_len, opaque_reason,
    output_position, pg_sys, quote_if_needed, referenced_columns, setop_leaves, sql_occs, tag,
    top_opaque_reason, unnest_array, view_query, windows_partitioned,
};

impl Walker<'_> {
    /// Record the virtual generated columns among the keys and equalities.
    pub(super) fn note_virtual_columns(&mut self) {
        let graph = &self.graph;
        let found: Vec<(usize, i16)> = graph
            .roots
            .iter()
            .map(|r| &r.key)
            .chain(
                graph
                    .conjuncts
                    .iter()
                    .filter_map(|c| c.equality.as_ref())
                    .flat_map(|(x, y)| [x, y]),
            )
            .filter(|c| {
                let relid = Oid::from(graph.occurrences[c.occ].relid);
                // SAFETY: a catalog lookup by OID and attribute number.
                unsafe { pg_sys::get_attgenerated(relid, c.attnum).cast_unsigned() == b'v' }
            })
            .map(|c| (c.occ, c.attnum))
            .collect();
        self.graph.virtual_columns.extend(found);
    }

    // ── query levels ────────────────────────────────────────────────────────

    /// The top level: the backing view itself, or each branch of its UNION.
    ///
    /// SAFETY: `query` is a valid Query.
    pub(super) unsafe fn top(
        &mut self,
        query: *mut pg_sys::Query,
        flags: &Flags,
    ) -> TViewResult<()> {
        // SAFETY: fields of a valid Query.
        unsafe {
            let key_position = elements::<pg_sys::TargetEntry>((*query).targetList)
                .iter()
                .position(|tle| cstr((**tle).resname) == self.ctx.key_column);
            if (*query).setOperations.is_null() {
                // A window function, LIMIT/OFFSET, a set-returning function or
                // GROUPING SETS here change rows other than the written one: no
                // row maps to its own key, so nothing gets a key root.
                let opaque = top_opaque_reason(query);
                let flags = Flags {
                    opaque_level: opaque.clone().or_else(|| flags.opaque_level.clone()),
                    ..flags.clone()
                };
                self.identity_level = true;
                let outputs = self.level(query, &flags, Link::Top)?;
                self.graph.outputs = elements::<pg_sys::TargetEntry>((*query).targetList)
                    .iter()
                    .zip(&outputs)
                    .filter(|(tle, _)| !(***tle).resjunk)
                    .map(|(tle, out)| {
                        let column = match out {
                            Resolved::Col(c) => Some(c.clone()),
                            _ => None,
                        };
                        (cstr((**tle).resname), column)
                    })
                    .collect();
                // The key root is the identity's column (ADR 0169).
                let root_position = match &self.graph.identity {
                    Some(Ok(identity)) => Some(identity.position),
                    _ => key_position,
                };
                if opaque.is_none()
                    && let Some(key) = root_position.and_then(|p| outputs.get(p))
                {
                    self.add_roots(key, &flags.unions);
                }
                return Ok(());
            }
            self.graph.set_operation = true;
            // LIMIT/OFFSET over the whole set operation applies to every branch.
            let whole = top_opaque_reason(query);
            // UNION: each leaf is a branch with its own root. The leaves sit in
            // the rtable as subqueries, referenced from the set-operation tree.
            self.push_level(
                query,
                vec![RteInfo::Other; list_len((*query).rtable)],
                Link::Top,
            );
            let mut leaves = Vec::new();
            setop_leaves((*query).setOperations, &mut leaves);
            let union = self.next_union();
            let result: TViewResult<()> = (|| {
                for (branch, rtindex) in leaves.into_iter().enumerate() {
                    let Some(&rte) = elements::<pg_sys::RangeTblEntry>((*query).rtable)
                        .get(rtindex.wrapping_sub(1))
                    else {
                        continue;
                    };
                    let opaque = whole.clone().or_else(|| top_opaque_reason((*rte).subquery));
                    let mut unions = flags.unions.clone();
                    unions.push((union, branch));
                    let leaf_flags = Flags {
                        unions: unions.clone(),
                        opaque_level: opaque.clone().or_else(|| flags.opaque_level.clone()),
                        ..flags.clone()
                    };
                    let outputs = self.level((*rte).subquery, &leaf_flags, Link::Top)?;
                    match key_position.and_then(|p| outputs.get(p)) {
                        Some(key) if opaque.is_none() => self.add_roots(key, &unions),
                        _ => self.graph.holes.push(unions),
                    }
                }
                self.unread_ctes(flags)
            })();
            self.levels.pop();
            // A UNION's rows are named by pk_<entity> in every branch.
            let identity = key_position
                .and_then(|p| {
                    elements::<pg_sys::TargetEntry>((*query).targetList)
                        .get(p)
                        .copied()
                })
                .zip(key_position)
                .map(|(tle, position)| WalkedIdentity {
                    name: self.ctx.key_column.to_string(),
                    position,
                    type_oid: pg_sys::exprType((*tle).expr.cast()).to_u32(),
                    kind: IdentityKind::Pk,
                    columns: self.graph.roots.iter().map(|r| r.key.clone()).collect(),
                })
                .ok_or(crate::lineage::IdentityError::Missing);
            self.graph.identity = Some(identity);
            result
        }
    }

    /// Walk one query level and return what each of its output columns stands for.
    ///
    /// SAFETY: `query` is a valid Query.
    pub(super) unsafe fn level(
        &mut self,
        query: *mut pg_sys::Query,
        flags: &Flags,
        link: Link,
    ) -> TViewResult<Vec<Resolved>> {
        let wanted = self.wanted.take();
        if self.levels.len() >= MAX_DEPTH {
            return Err(TViewError::DefinitionRefused {
                reason: format!(
                    "the backing view nests views, subqueries and CTEs more than {MAX_DEPTH} levels deep"
                ),
            });
        }
        // SAFETY: fields of a valid Query.
        unsafe {
            if !(*query).setOperations.is_null() {
                return self.union_outputs(query, flags);
            }
            // Window functions all partitioned leave the partition columns visible;
            // their occurrences keep the window as the reason they are not
            // linked otherwise.
            let partitioned = link != Link::Top && windows_partitioned(query);
            let opaque = (link != Link::Top)
                .then(|| opaque_reason(query, partitioned))
                .flatten();
            let flags = Flags {
                opaque_level: opaque
                    .clone()
                    .or_else(|| partitioned.then(|| WINDOW_REASON.to_string()))
                    .or_else(|| flags.opaque_level.clone()),
                ..flags.clone()
            };
            self.push_level(query, Vec::new(), link);
            let result = self
                .level_body(query, &flags, link, opaque.is_some(), wanted.as_ref())
                .and_then(|outputs| self.unread_ctes(&flags).map(|()| outputs));
            self.levels.pop();
            result
        }
    }

    /// SAFETY: `query` is the valid Query of the innermost level.
    pub(super) unsafe fn level_body(
        &mut self,
        query: *mut pg_sys::Query,
        flags: &Flags,
        link: Link,
        opaque: bool,
        wanted: Option<&HashSet<i16>>,
    ) -> TViewResult<Vec<Resolved>> {
        // Taken before the levels below are walked.
        let identity_level = std::mem::take(&mut self.identity_level);
        // SAFETY: fields of a valid Query; RTEs and expressions belong to it.
        unsafe {
            // A view, subquery or CTE in FROM is walked only for the columns this
            // level reads of it.
            let read = referenced_columns(query);
            for (i, rte) in elements::<pg_sys::RangeTblEntry>((*query).rtable)
                .into_iter()
                .enumerate()
            {
                self.wanted = match read.get(&(i + 1)) {
                    Some(Some(columns)) => Some(columns.clone()),
                    Some(None) => None,
                    None => Some(HashSet::new()),
                };
                let info = self.rte(rte, flags);
                self.wanted = None;
                self.current().rtes.push(info?);
            }
            let jointree = (*query).jointree;
            if !jointree.is_null() {
                self.join_item(jointree.cast(), flags)?;
            }
            // Subquery expressions anywhere else in this level. Those of an output
            // column nothing above reads cannot change the TVIEW: their tables are
            // only recorded.
            let unread = Flags {
                unread: true,
                ..flags.clone()
            };
            let skipped: Vec<bool> = elements::<pg_sys::TargetEntry>((*query).targetList)
                .iter()
                .map(|tle| {
                    wanted.is_some_and(|w| !w.contains(&(**tle).resno))
                        && (**tle).ressortgroupref == 0
                        && !pg_sys::expression_returns_set((**tle).expr.cast())
                })
                .collect();
            for (tle, skip) in elements::<pg_sys::TargetEntry>((*query).targetList)
                .into_iter()
                .zip(&skipped)
            {
                let tle_flags = if *skip { &unread } else { flags };
                self.sublinks((*tle).expr.cast(), tle_flags, false)?;
            }
            self.sublinks((*query).havingQual, flags, false)?;
            self.note_functions(query.cast());
            self.note_time(query.cast());
            if identity_level {
                let tles = elements::<pg_sys::TargetEntry>((*query).targetList);
                self.graph.identity = Some(self.identity(query, &tles));
                self.graph.data = self.data_shape(query, &tles);
            }

            let grouped = (*query).hasAggs || !(*query).groupClause.is_null();
            let tles = elements::<pg_sys::TargetEntry>((*query).targetList);
            // The columns of a GROUP BY or DISTINCT ON key, and whether an output
            // column is one of them or equal to one on every row it can match:
            // `DISTINCT ON (l.fk_order) o.pk_order` with
            // `l.fk_order = o.pk_order`.
            let key_columns = |clause: *mut pg_sys::List| -> Vec<Column> {
                tles.iter()
                    .filter(|tle| in_clause((***tle).ressortgroupref, clause))
                    .filter_map(|tle| match self.resolve_expr((**tle).expr.cast()) {
                        Resolved::Col(c) => Some(c),
                        _ => None,
                    })
                    .collect()
            };
            let group_keys = key_columns((*query).groupClause);
            let distinct_keys = key_columns((*query).distinctClause);
            // A row's window values come from the rows of its partition: only a
            // column of every window's PARTITION BY passes through.
            let partitions: Vec<(*mut pg_sys::List, Vec<Column>)> =
                elements::<pg_sys::WindowClause>((*query).windowClause)
                    .iter()
                    .map(|w| ((**w).partitionClause, key_columns((**w).partitionClause)))
                    .collect();
            let keyed =
                |tle: *mut pg_sys::TargetEntry, clause: *mut pg_sys::List, keys: &[Column]| {
                    in_clause((*tle).ressortgroupref, clause)
                        || matches!(self.resolve_expr((*tle).expr.cast()),
                                Resolved::Col(c) if self.equal_to_key(&c, keys))
                };
            let pass_through: Vec<bool> = tles
                .iter()
                .zip(skipped.iter().copied())
                .map(|(&tle, skip)| {
                    !skip
                        && (link == Link::Top
                            || (!opaque
                                && (!grouped || keyed(tle, (*query).groupClause, &group_keys))
                                && (!(*query).hasDistinctOn
                                    || keyed(tle, (*query).distinctClause, &distinct_keys))
                                && partitions
                                    .iter()
                                    .all(|(clause, keys)| keyed(tle, *clause, keys))))
                })
                .collect();
            // The other columns of a first-row level link inbound only.
            let first_row = link != Link::Top
                && !opaque
                && !grouped
                && ((*query).hasDistinctOn || !partitions.is_empty());
            Ok(tles
                .iter()
                .zip(pass_through)
                .zip(skipped)
                .map(|((&tle, pass), skip)| {
                    if pass {
                        return self.output((*tle).expr.cast());
                    }
                    match self.resolve_expr((*tle).expr.cast()) {
                        Resolved::Col(c) | Resolved::Inbound(c) if first_row && !skip => {
                            Resolved::Inbound(c)
                        }
                        _ => Resolved::Opaque,
                    }
                })
                .collect())
        }
    }

    // ── the shape of `data` ──────────────────────────────────────────────────

    pub(super) fn current(&mut self) -> &mut Level {
        self.levels.last_mut().expect("inside a query level")
    }

    pub(super) const fn next_union(&mut self) -> usize {
        self.unions += 1;
        self.unions
    }

    /// The key roots an output of the TVIEW's key stands for, in rows of `scope`:
    /// a column, or an expression of one occurrence's row (a sign, an offset),
    /// once per UNION branch it comes from. A branch whose key is anything
    /// else is a hole: its rows cannot be mapped.
    pub(super) fn add_roots(&mut self, key: &Resolved, scope: &Scope) {
        match key {
            Resolved::Col(c) => self.graph.roots.push(Root {
                key: c.clone(),
                expr: None,
                scope: self.graph.occurrences[c.occ].unions.clone(),
            }),
            Resolved::Expr(e) if !e.element && sql_occs(&e.sql).len() == 1 => {
                let Some(Piece::Column { occ, attnum }) = e
                    .sql
                    .0
                    .iter()
                    .find(|p| matches!(p, Piece::Column { .. }))
                    .cloned()
                else {
                    self.graph.holes.push(scope.clone());
                    return;
                };
                let relid = Oid::from(self.graph.occurrences[occ].relid);
                // SAFETY: a catalog lookup by OID and attribute number.
                let name = cstr(unsafe { pg_sys::get_attname(relid, attnum, true) });
                self.graph.roots.push(Root {
                    key: Column { occ, attnum, name },
                    expr: Some(e.sql.clone()),
                    scope: self.graph.occurrences[occ].unions.clone(),
                });
            }
            Resolved::Alt(terms, holes) => {
                for term in terms {
                    let term_scope = term.occs().first().map_or_else(
                        || scope.clone(),
                        |&o| self.graph.occurrences[o].unions.clone(),
                    );
                    self.add_roots(term, &term_scope);
                }
                self.graph.holes.extend(holes.iter().cloned());
            }
            _ => self.graph.holes.push(scope.clone()),
        }
    }

    /// Enter a query level. Its CTE parent is the level a pending CTE lookup
    /// found the CTE in, else the enclosing level.
    pub(super) fn push_level(&mut self, query: *mut pg_sys::Query, rtes: Vec<RteInfo>, link: Link) {
        let cte_parent = self
            .cte_parent
            .take()
            .or_else(|| self.levels.len().checked_sub(1));
        self.levels.push(Level {
            query,
            rtes,
            link,
            cte_parent,
        });
    }

    /// Walk the CTEs the innermost level defines and nothing used, so that the
    /// tables they read are known (they are not tracked: they cannot change the
    /// output).
    ///
    /// SAFETY: the innermost level's query is valid.
    pub(super) unsafe fn unread_ctes(&mut self, flags: &Flags) -> TViewResult<()> {
        let index = self.levels.len() - 1;
        let query = self.levels[index].query;
        // SAFETY: the CTE list of a valid Query.
        let ctes = unsafe { elements::<pg_sys::CommonTableExpr>((*query).cteList) };
        for cte in ctes {
            // SAFETY: a valid CommonTableExpr of that list.
            let (name, body) = unsafe { (cstr((*cte).ctename), (*cte).ctequery) };
            if self.read_ctes.contains(&(query as usize, name.clone())) {
                continue;
            }
            self.read_ctes.insert((query as usize, name));
            let unread = Flags {
                unread: true,
                ..flags.clone()
            };
            self.cte_parent = Some(index);
            // SAFETY: a copy of the CTE's query.
            unsafe {
                let copy = pg_sys::copyObjectImpl(body.cast()).cast::<pg_sys::Query>();
                self.level(copy, &unread, Link::From)?;
            }
        }
        Ok(())
    }

    // ── range table ─────────────────────────────────────────────────────────

    /// SAFETY: `rte` is a valid RTE of the innermost level.
    pub(super) unsafe fn rte(
        &mut self,
        rte: *mut pg_sys::RangeTblEntry,
        flags: &Flags,
    ) -> TViewResult<RteInfo> {
        // SAFETY: fields of a valid RTE.
        unsafe {
            match (*rte).rtekind {
                pg_sys::RTEKind::RTE_RELATION => self.relation((*rte).relid, flags),
                pg_sys::RTEKind::RTE_SUBQUERY => Ok(RteInfo::Outputs(self.level(
                    (*rte).subquery,
                    flags,
                    Link::From,
                )?)),
                // The recursive term's reference to its own CTE: the CTE is being
                // walked.
                pg_sys::RTEKind::RTE_CTE if (*rte).self_reference => Ok(RteInfo::Other),
                pg_sys::RTEKind::RTE_CTE => {
                    let name = cstr((*rte).ctename);
                    let Some((cte, defined_at)) = self.cte(&name, (*rte).ctelevelsup as usize)
                    else {
                        return Ok(RteInfo::Other);
                    };
                    self.read_ctes
                        .insert((self.levels[defined_at].query as usize, name));
                    let copy =
                        pg_sys::copyObjectImpl((*cte).ctequery.cast()).cast::<pg_sys::Query>();
                    self.cte_parent = Some(defined_at);
                    if !(*cte).cterecursive {
                        return Ok(RteInfo::Outputs(self.level(copy, flags, Link::From)?));
                    }
                    // A row of a recursive CTE comes from rows of the step before:
                    // nothing links it to the rows of the tables it reads.
                    let recursive = Flags {
                        opaque_level: Some(format!(
                            "read in a recursive CTE ({})",
                            flags.via_view.as_deref().unwrap_or("the definition")
                        )),
                        ..flags.clone()
                    };
                    let width = self.level(copy, &recursive, Link::From)?.len();
                    Ok(RteInfo::Outputs(vec![Resolved::Opaque; width]))
                }
                pg_sys::RTEKind::RTE_JOIN => Ok(RteInfo::Join((*rte).joinaliasvars)),
                pg_sys::RTEKind::RTE_FUNCTION => Ok(self.function_rte(rte)),
                // PostgreSQL 18: grouped Vars point at the GROUP entry, whose
                // expressions are those of the level.
                #[cfg(feature = "pg18")]
                pg_sys::RTEKind::RTE_GROUP => Ok(RteInfo::Join((*rte).groupexprs)),
                _ => Ok(RteInfo::Other),
            }
        }
    }

    /// A relation in FROM: a base table occurrence, or a view to expand.
    pub(super) fn relation(&mut self, relid: Oid, flags: &Flags) -> TViewResult<RteInfo> {
        // SAFETY: catalog lookups by OID.
        let (relkind, relname, qualified) = unsafe {
            let relkind = pg_sys::get_rel_relkind(relid).cast_unsigned();
            let relname = cstr(pg_sys::get_rel_name(relid));
            let nsp = cstr(pg_sys::get_namespace_name(pg_sys::get_rel_namespace(relid)));
            let qualified = format!("{}.{}", quote_if_needed(&nsp), quote_if_needed(&relname));
            (relkind, relname, qualified)
        };
        match relkind {
            b'r' | b'p' | b'm' if flags.unread => {
                self.graph.unread_tables.insert(relid.to_u32());
                Ok(RteInfo::Other)
            }
            b'r' | b'p' if self.ctx.tview_tables.contains_key(&relid) => {
                let entity = self.ctx.tview_tables[&relid].clone();
                self.graph.tview_keys.entry(entity.clone()).or_default();
                self.graph.occurrences.push(Occurrence {
                    relid: relid.to_u32(),
                    relname,
                    qualified,
                    unions: flags.unions.clone(),
                    via_view: flags.via_view.clone(),
                    via_tview: flags.via_tview.clone(),
                    in_sublink: flags.in_sublink,
                    opaque_level: flags.opaque_level.clone(),
                    matview: false,
                    tview_table: Some(entity.clone()),
                });
                let occ = self.graph.occurrences.len() - 1;
                Ok(RteInfo::Tview { entity, relid, occ })
            }
            b'r' | b'p' | b'm' => {
                self.graph.occurrences.push(Occurrence {
                    relid: relid.to_u32(),
                    relname,
                    qualified,
                    unions: flags.unions.clone(),
                    via_view: flags.via_view.clone(),
                    via_tview: flags.via_tview.clone(),
                    in_sublink: flags.in_sublink,
                    opaque_level: flags.opaque_level.clone(),
                    matview: relkind == b'm',
                    tview_table: None,
                });
                Ok(RteInfo::Base(self.graph.occurrences.len() - 1))
            }
            b'v' => {
                // SAFETY: an ACL check by OID for the current user.
                let readable = unsafe {
                    pg_sys::pg_class_aclcheck(
                        relid,
                        pg_sys::GetUserId(),
                        pg_sys::AclMode::from(pg_sys::ACL_SELECT),
                    ) == pg_sys::AclResult::ACLCHECK_OK
                };
                if !readable {
                    return Err(TViewError::PermissionDenied {
                        reason: format!("permission denied to read view {qualified}"),
                    });
                }
                let inner = match self.ctx.tview_views.get(&relid) {
                    Some(entity) => Flags {
                        via_tview: flags.via_tview.clone().or_else(|| Some(entity.clone())),
                        ..flags.clone()
                    },
                    None => Flags {
                        via_view: flags.via_view.clone().or(Some(qualified)),
                        ..flags.clone()
                    },
                };
                // SAFETY: the view query is a fresh copy.
                let query = unsafe { view_query(relid)? };
                // SAFETY: a valid, copied Query.
                let outputs = unsafe { self.level(query, &inner, Link::From)? };
                if let Some(entity) = self.ctx.tview_views.get(&relid) {
                    // SAFETY: the target list of a valid Query.
                    let key = unsafe { output_position(query, &format!("pk_{entity}")) };
                    let columns = match key.and_then(|i| outputs.get(i)) {
                        Some(Resolved::Col(c)) => vec![c.clone()],
                        Some(Resolved::Alt(terms, _)) => terms
                            .iter()
                            .filter_map(|t| match t {
                                Resolved::Col(c) => Some(c.clone()),
                                _ => None,
                            })
                            .collect(),
                        _ => Vec::new(),
                    };
                    self.graph
                        .tview_keys
                        .entry(entity.clone())
                        .or_default()
                        .extend(columns);
                }
                Ok(RteInfo::Outputs(outputs))
            }
            _ => Ok(RteInfo::Other),
        }
    }

    /// A function in FROM: `unnest(<array>)` alone, without ordinality, stands for
    /// the elements of the array; anything else is opaque.
    ///
    /// SAFETY: `rte` is a valid `RTE_FUNCTION` entry of the innermost level, whose
    /// earlier entries are known.
    pub(super) unsafe fn function_rte(&mut self, rte: *mut pg_sys::RangeTblEntry) -> RteInfo {
        // SAFETY: fields of a valid RTE and of its function list.
        unsafe {
            let functions = elements::<pg_sys::RangeTblFunction>((*rte).functions);
            let [function] = functions[..] else {
                return RteInfo::Other;
            };
            if (*rte).funcordinality {
                return RteInfo::Other;
            }
            match unnest_array((*function).funcexpr) {
                Some(array) => RteInfo::Outputs(vec![self.computed(array, true)]),
                None => RteInfo::Other,
            }
        }
    }

    /// CTE `name`, defined `levelsup` levels above the innermost one, and the index
    /// of the level that defines it. Levels are counted along CTE parents: the
    /// references inside a CTE body count from where it is defined.
    pub(super) fn cte(
        &self,
        name: &str,
        levelsup: usize,
    ) -> Option<(*mut pg_sys::CommonTableExpr, usize)> {
        let mut level = self.levels.len().checked_sub(1)?;
        for _ in 0..levelsup {
            level = self.levels[level].cte_parent?;
        }
        let query = self.levels[level].query;
        // SAFETY: the CTE list of a valid Query.
        unsafe {
            elements::<pg_sys::CommonTableExpr>((*query).cteList)
                .into_iter()
                .find(|cte| cstr((**cte).ctename) == *name)
                .map(|cte| (cte, level))
        }
    }

    // ── join tree ───────────────────────────────────────────────────────────

    /// Walk a FROM item, adding its conditions; return the occurrences under it.
    ///
    /// SAFETY: `node` is a valid jointree node of the innermost level.
    pub(super) unsafe fn join_item(
        &mut self,
        node: *mut pg_sys::Node,
        flags: &Flags,
    ) -> TViewResult<HashSet<usize>> {
        // SAFETY: each node is checked by tag before it is cast.
        unsafe {
            match tag(node) {
                Some(pg_sys::NodeTag::T_RangeTblRef) => {
                    let rtindex = super::index((*node.cast::<pg_sys::RangeTblRef>()).rtindex);
                    Ok(self.occurrences_of(rtindex))
                }
                Some(pg_sys::NodeTag::T_FromExpr) => {
                    let from = node.cast::<pg_sys::FromExpr>();
                    let mut under = HashSet::new();
                    for item in elements::<pg_sys::Node>((*from).fromlist) {
                        under.extend(self.join_item(item, flags)?);
                    }
                    for qual in conjuncts((*from).quals) {
                        self.predicate(qual, Origin::Required);
                    }
                    // A positive EXISTS / IN conjunct of WHERE must hold for every row.
                    for qual in conjuncts((*from).quals) {
                        let required = is_required_sublink(qual);
                        self.sublinks(qual, flags, required)?;
                    }
                    Ok(under)
                }
                Some(pg_sys::NodeTag::T_JoinExpr) => {
                    let join = node.cast::<pg_sys::JoinExpr>();
                    let left = self.join_item((*join).larg, flags)?;
                    let right = self.join_item((*join).rarg, flags)?;
                    for qual in conjuncts((*join).quals) {
                        let origin = match (*join).jointype {
                            pg_sys::JoinType::JOIN_INNER => Origin::Required,
                            pg_sys::JoinType::JOIN_LEFT => Origin::Outer { nullable: &right },
                            pg_sys::JoinType::JOIN_RIGHT => Origin::Outer { nullable: &left },
                            _ => Origin::None,
                        };
                        self.predicate(qual, origin);
                    }
                    self.sublinks((*join).quals, flags, false)?;
                    // Above this join, its nullable side may be NULL-extended.
                    match (*join).jointype {
                        pg_sys::JoinType::JOIN_INNER => {}
                        pg_sys::JoinType::JOIN_LEFT => self.nullable.extend(&right),
                        pg_sys::JoinType::JOIN_RIGHT => self.nullable.extend(&left),
                        _ => self.nullable.extend(left.iter().chain(&right)),
                    }
                    Ok(left.union(&right).copied().collect())
                }
                _ => Ok(HashSet::new()),
            }
        }
    }

    /// The occurrences a range table entry of the innermost level stands for.
    pub(super) fn occurrences_of(&self, rtindex: usize) -> HashSet<usize> {
        let level = self.levels.last().expect("inside a query level");
        match level.rtes.get(rtindex.wrapping_sub(1)) {
            Some(RteInfo::Base(occ) | RteInfo::Tview { occ, .. }) => HashSet::from([*occ]),
            Some(RteInfo::Outputs(outputs)) => outputs.iter().flat_map(Resolved::occs).collect(),
            _ => HashSet::new(),
        }
    }

    // ── subquery expressions ────────────────────────────────────────────────
}
