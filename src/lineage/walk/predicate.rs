//! Join and filter conditions: what they equate, and where they apply.

use super::{
    Conjunct, Flags, Link, Maps, Origin, Resolved, RteInfo, Site, TViewResult, Walker,
    collect_params, collect_sublinks, collect_vars, conjuncts, cstr, elements, equality,
    has_sublink, pg_sys, strip_relabel, tag,
};

impl Walker<'_> {
    /// Walk the subquery expressions in `node` (not those nested in them: their
    /// own level does).
    ///
    /// SAFETY: `node` is null or a valid expression of the innermost level.
    pub(super) unsafe fn sublinks(
        &mut self,
        node: *mut pg_sys::Node,
        flags: &Flags,
        required: bool,
    ) -> TViewResult<()> {
        let mut found: Vec<*mut pg_sys::SubLink> = Vec::new();
        // SAFETY: a read-only walk of a valid expression.
        unsafe {
            collect_sublinks(node, &mut found);
            for sublink in found {
                self.sublink(sublink, flags, required)?;
            }
        }
        Ok(())
    }

    /// SAFETY: `sublink` is a valid `SubLink` of the innermost level.
    pub(super) unsafe fn sublink(
        &mut self,
        sublink: *mut pg_sys::SubLink,
        flags: &Flags,
        required: bool,
    ) -> TViewResult<()> {
        // SAFETY: fields of a valid SubLink.
        unsafe {
            let subselect = (*sublink).subselect.cast::<pg_sys::Query>();
            if subselect.is_null() {
                return Ok(());
            }
            let required = required
                && matches!(
                    (*sublink).subLinkType,
                    pg_sys::SubLinkType::EXISTS_SUBLINK | pg_sys::SubLinkType::ANY_SUBLINK
                );
            let inner = Flags {
                in_sublink: true,
                ..flags.clone()
            };
            let outputs = self.level(subselect, &inner, Link::Sublink { required })?;
            // `x IN (SELECT y …)`: x = y holds for the matching row.
            if (*sublink).subLinkType == pg_sys::SubLinkType::ANY_SUBLINK {
                for test in conjuncts((*sublink).testexpr) {
                    self.test_predicate(test, &outputs, required);
                }
            }
            Ok(())
        }
    }

    // ── predicates ──────────────────────────────────────────────────────────

    /// Record a predicate of the innermost level if it links two occurrences and
    /// can be used: immutable, written with supported nodes, and never true on a
    /// NULL-extended row ([`Walker::null_safe`]).
    ///
    /// SAFETY: `qual` is a valid expression of the innermost level.
    pub(super) unsafe fn predicate(&mut self, qual: *mut pg_sys::Node, origin: Origin<'_>) {
        // SAFETY: read-only checks of a valid expression.
        unsafe {
            self.tview_key_equality(qual);
            if matches!(origin, Origin::None)
                || pg_sys::contain_mutable_functions(qual)
                || has_sublink(qual)
            {
                return;
            }
            let mut sites = Vec::new();
            if !self.sites(qual, &mut sites) || !self.null_safe(qual, &sites) {
                return;
            }
            self.add_conjuncts(qual, &sites, &|_| None, origin, true);
        }
    }

    /// `<tv table>.pk_<entity> = <column>`: the column carries the key of the
    /// TVIEW whose table this level reads.
    ///
    /// SAFETY: `qual` is a valid expression of the innermost level.
    pub(super) unsafe fn tview_key_equality(&mut self, qual: *mut pg_sys::Node) {
        // SAFETY: checked by tag before each cast; RTE lists of valid levels.
        unsafe {
            if tag(qual) != Some(pg_sys::NodeTag::T_OpExpr)
                || cstr(pg_sys::get_opname((*qual.cast::<pg_sys::OpExpr>()).opno)) != "="
            {
                return;
            }
            let args = elements::<pg_sys::Node>((*qual.cast::<pg_sys::OpExpr>()).args);
            let [l, r] = args[..] else { return };
            let (l, r) = (strip_relabel(l), strip_relabel(r));
            if tag(l) != Some(pg_sys::NodeTag::T_Var) || tag(r) != Some(pg_sys::NodeTag::T_Var) {
                return;
            }
            for (key, other) in [(l, r), (r, l)] {
                let (key, other) = (key.cast::<pg_sys::Var>(), other.cast::<pg_sys::Var>());
                let Some(entity) = self.tview_key_of(key) else {
                    continue;
                };
                if let Resolved::Col(c) = self.resolve_var(other, (*other).varlevelsup as usize) {
                    self.graph.tview_keys.entry(entity).or_default().push(c);
                }
            }
        }
    }

    /// The entity whose TVIEW table `var` reads the key `pk_<entity>` of.
    ///
    /// SAFETY: `var` is a valid Var of the innermost level or one above.
    pub(super) unsafe fn tview_key_of(&self, var: *mut pg_sys::Var) -> Option<String> {
        // SAFETY: fields of a valid Var; RTE lists of valid levels.
        unsafe {
            let index = self
                .levels
                .len()
                .checked_sub(1 + (*var).varlevelsup as usize)?;
            let rtindex = usize::try_from((*var).varno).ok()?;
            let Some(RteInfo::Tview { entity, relid, .. }) =
                self.levels[index].rtes.get(rtindex.wrapping_sub(1))
            else {
                return None;
            };
            (cstr(pg_sys::get_attname(*relid, (*var).varattno, true)) == format!("pk_{entity}"))
                .then(|| entity.clone())
        }
    }

    /// Whether a NULL-extended row cannot make `expr` true: `expr` is strict, or no
    /// occurrence it reads is on the nullable side of an outer join walked so far
    /// (one below the predicate). `x = ANY (string_to_array(n.path, '.'))` is not
    /// strict, and is used over inner joins.
    ///
    /// SAFETY: `expr` is a valid expression and `sites` its Vars and Params.
    pub(super) unsafe fn null_safe(&self, expr: *mut pg_sys::Node, sites: &[Site]) -> bool {
        let terms = || sites.iter().flat_map(|s| &s.candidates);
        // SAFETY: a read-only check of a valid expression.
        let strict = unsafe { !pg_sys::contain_nonstrict_functions(expr) }
            && terms().all(|t| !matches!(t, Resolved::Expr(e) if !e.strict));
        strict
            || terms()
                .flat_map(Resolved::occs)
                .all(|occ| !self.nullable.contains(&occ))
    }

    /// `lhs op Param` of an `IN (SELECT …)`: the Param stands for the subquery's
    /// output column.
    ///
    /// SAFETY: `test` is a valid expression of the innermost level.
    pub(super) unsafe fn test_predicate(
        &mut self,
        test: *mut pg_sys::Node,
        outputs: &[Resolved],
        required: bool,
    ) {
        // SAFETY: read-only checks of a valid expression.
        unsafe {
            if pg_sys::contain_mutable_functions(test) {
                return;
            }
            let mut sites = Vec::new();
            if !self.sites(test, &mut sites) {
                return;
            }
            // The subquery's columns sit one level below the innermost one.
            let param_column = |param: *mut pg_sys::Param| -> Option<Vec<Resolved>> {
                let p = &*param;
                if p.paramkind != pg_sys::ParamKind::PARAM_SUBLINK {
                    return None;
                }
                match outputs.get(usize::try_from(p.paramid).ok()?.checked_sub(1)?)? {
                    Resolved::Alt(terms, _) => Some(terms.clone()),
                    Resolved::Opaque => None,
                    term => Some(vec![term.clone()]),
                }
            };
            let mut param_sites = Vec::new();
            collect_params(test, &mut param_sites);
            for param in &param_sites {
                let Some(candidates) = param_column(*param) else {
                    return;
                };
                sites.push(Site {
                    // Below the innermost level: always "inner".
                    levelsup: usize::MAX,
                    candidates,
                });
            }
            if !self.null_safe(test, &sites) {
                return;
            }
            let lookup = |node: *mut pg_sys::Node| -> Option<usize> {
                param_sites
                    .iter()
                    .position(|p| p.cast::<pg_sys::Node>() == node)
            };
            self.add_conjuncts(test, &sites, &lookup, Origin::Required, required);
        }
    }

    /// Resolve every Var of `expr`; false if one is opaque.
    ///
    /// SAFETY: `expr` is a valid expression of the innermost level.
    pub(super) unsafe fn sites(&self, expr: *mut pg_sys::Node, sites: &mut Vec<Site>) -> bool {
        let mut vars = Vec::new();
        // SAFETY: a read-only walk of a valid expression.
        unsafe {
            collect_vars(expr, &mut vars);
            for var in vars {
                let v = &*var;
                let levelsup = v.varlevelsup as usize;
                let candidates = match self.resolve_var(var, levelsup) {
                    Resolved::Alt(terms, _) => terms,
                    Resolved::Opaque => return false,
                    term => vec![term],
                };
                sites.push(Site {
                    levelsup,
                    candidates,
                });
            }
        }
        true
    }

    /// Add one conjunct per choice of column for each site, when it links exactly
    /// two occurrences. `param_site(node)` maps a Param node to its site index past
    /// the Vars.
    ///
    /// SAFETY: `expr` is a valid expression of the innermost level and `sites` are
    /// its Vars (in walk order), then its Params.
    pub(super) unsafe fn add_conjuncts(
        &mut self,
        expr: *mut pg_sys::Node,
        sites: &[Site],
        param_site: &dyn Fn(*mut pg_sys::Node) -> Option<usize>,
        origin: Origin<'_>,
        outer_to_inner: bool,
    ) {
        if sites.is_empty() {
            return;
        }
        let combinations: usize = sites.iter().map(|s| s.candidates.len()).product();
        if combinations == 0 || combinations > 16 {
            return;
        }
        let var_count = sites.iter().filter(|s| s.levelsup != usize::MAX).count();
        for n in 0..combinations {
            let mut rest = n;
            let chosen: Vec<&Resolved> = sites
                .iter()
                .map(|s| {
                    let c = &s.candidates[rest % s.candidates.len()];
                    rest /= s.candidates.len();
                    c
                })
                .collect();
            let mut occs: Vec<usize> = chosen.iter().flat_map(|c| c.occs()).collect();
            occs.sort_unstable();
            occs.dedup();
            let [a, b] = occs[..] else { continue };
            let Some((a_to_b, b_to_a)) =
                self.directions(sites, &chosen, a, b, origin, outer_to_inner)
            else {
                continue;
            };
            let var_index = std::cell::Cell::new(0_usize);
            let term_of = |node: *mut pg_sys::Node| -> Option<Resolved> {
                // SAFETY: `node` is a Var or Param of `expr`.
                match unsafe { tag(node) } {
                    Some(pg_sys::NodeTag::T_Var) => {
                        chosen.get(var_index.get()).map(|c| (*c).clone())
                    }
                    Some(pg_sys::NodeTag::T_Param) => param_site(node)
                        .and_then(|i| chosen.get(var_count + i).map(|c| (*c).clone())),
                    _ => None,
                }
            };
            let mut next_var = || var_index.set(var_index.get() + 1);
            // SAFETY: deparse reads the same valid expression.
            let Some((sql, lookups)) =
                (unsafe { self.conjunct_sql(expr, &term_of, &mut next_var) })
            else {
                return;
            };
            // SAFETY: the same expression.
            let equality = unsafe { equality(expr, &chosen) };
            // Only an equality is known to fail on the NULLs of an unmatched row.
            let checked = |m: Maps| {
                if m == Maps::IfMatched && equality.is_none() {
                    Maps::No
                } else {
                    m
                }
            };
            let (a_to_b, b_to_a) = (checked(a_to_b), checked(b_to_a));
            if a_to_b == Maps::No && b_to_a == Maps::No {
                continue;
            }
            self.graph.conjuncts.push(Conjunct {
                sql,
                a,
                b,
                a_to_b,
                b_to_a,
                equality,
                lookups,
            });
        }
    }

    /// In which directions a predicate between occurrences `a` and `b` must hold.
    pub(super) fn directions(
        &self,
        sites: &[Site],
        chosen: &[&Resolved],
        a: usize,
        b: usize,
        origin: Origin<'_>,
        outer_to_inner: bool,
    ) -> Option<(Maps, Maps)> {
        // The level of each occurrence relative to the predicate's: 0 here, n above,
        // usize::MAX below (a subquery's output).
        let level_of = |occ: usize| {
            sites
                .iter()
                .zip(chosen)
                .filter(|(_, c)| c.occs().contains(&occ))
                .map(|(s, _)| s.levelsup)
                .min()
        };
        let (la, lb) = (level_of(a)?, level_of(b)?);
        let depth = |l: usize| {
            if l == usize::MAX {
                -1_i64
            } else {
                i64::try_from(l).unwrap_or(i64::MAX)
            }
        };
        let (da, db) = (depth(la), depth(lb));
        // A predicate always holds for rows of the deeper occurrence; for rows of
        // the outer one only if every level in between must find a row.
        let outer_ok = |outer_levelsup: i64| outer_to_inner && self.levels_required(outer_levelsup);
        let (a_to_b, b_to_a) = match da.cmp(&db) {
            std::cmp::Ordering::Equal => (true, true),
            std::cmp::Ordering::Less => (true, outer_ok(db)),
            std::cmp::Ordering::Greater => (outer_ok(da), true),
        };
        // An outer join's condition holds only for rows of its nullable side. Toward
        // that side it still maps a row that has a match.
        let maps = |holds: bool, from: usize, to: usize| match origin {
            Origin::Outer { nullable } if holds && !nullable.contains(&from) => {
                if nullable.contains(&to) {
                    Maps::IfMatched
                } else {
                    Maps::No
                }
            }
            _ if holds => Maps::Yes,
            _ => Maps::No,
        };
        let (a_to_b, b_to_a) = (maps(a_to_b, a, b), maps(b_to_a, b, a));
        // Never away from an inbound column's occurrence.
        let inbound = |occ: usize| {
            chosen
                .iter()
                .any(|c| matches!(c, Resolved::Inbound(col) if col.occ == occ))
        };
        let a_to_b = if inbound(a) { Maps::No } else { a_to_b };
        let b_to_a = if inbound(b) { Maps::No } else { b_to_a };
        (a_to_b != Maps::No || b_to_a != Maps::No).then_some((a_to_b, b_to_a))
    }

    /// Whether a row of the level `outer` levels above the innermost one exists
    /// only if every level below it, down to the innermost, returns a row: each is
    /// a required subquery expression of the level above.
    pub(super) fn levels_required(&self, outer: i64) -> bool {
        let top = i64::try_from(self.levels.len()).unwrap_or(i64::MAX) - 1;
        (0..outer).all(|up| {
            usize::try_from(top - up).is_ok_and(|index| {
                matches!(self.levels[index].link, Link::Sublink { required: true })
            })
        })
    }

    // ── Var resolution ──────────────────────────────────────────────────────
}
