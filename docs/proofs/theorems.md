# pg_tviews' decisions, stated as theorems

What pg_tviews claims about the rows it maintains, written for a database researcher who
wants to check the claims, or prove them. No proof is given here. Each statement names the
code that decides it (a pure function of `src/decide/`, built and tested alone as
`decide/Cargo.toml`, or the code that carries a protocol property), what it assumes about
PostgreSQL, and the tests that exercise it. `scripts/check-theorems.sh` checks that every
assumption and every theorem names a check that exists, and that every decision of
`src/decide/` names its theorem (`/// Theorem: T<n>`).

## 0. Setting

- A **base table** `T` has columns `1..n`. A **TVIEW** `V` has a **definition** `D_V`: a
  `SELECT` over base tables, other TVIEWs' tables and views, in the dialect pg_tviews
  accepts ([shapes](../reference/shapes.md), generated from `docs/reference/dialect.toml`).
  Its result rows are keyed by `k` (`pk_<entity>`, or the `DISTINCT ON` key), and the TVIEW
  stores one row per key in its table `tv_V`.
- **Evaluation** `⟦D_V⟧(σ, ω)` is the definition's result over database state `σ` (one MVCC
  snapshot) **in the owner frame** `ω`: the TVIEW's owner as current user,
  `search_path = pg_catalog, pg_temp`, and the render, plan and planner settings pinned
  ([owner frame](../reference/owner-frame.md)). `⟦D_V⟧(σ, ω)[k]` is the row for key `k`, or ⊥.
- A **write** `w` takes `σ` to `σ'`. For an `UPDATE`, `Δ_T(w)` is the set of
  `(row, changed columns)` pairs, where "changed" means **binary inequality** of the old and
  new datum (`datumIsEqual`).
- The **read set** `R(V, T)` is computed when the TVIEW is registered, from the query tree:
  either `Complete(C)`, a set of column numbers, or `Unknown`.

### The two properties

- **Freshness.** For every committed state `σ` and every TVIEW `V`, after a write that
  touches no table declared `warn` for `V`: `tv_V(σ) = ⟦D_V⟧(σ, ω_V)`, as multisets of rows,
  compared column by column with binary equality of the rendered values. For a TVIEW that
  reads the current time, the instant is the last time boundary
  ([time-dependent TVIEWs](../reference/ddl.md#time-dependent-tviews)): every row equals
  `D_V` evaluated at an instant no earlier than it, except the keys a transaction older than
  the boundary wrote after it, from that transaction's commit until the drain refreshes them.
- **Visibility.** For every role `X`, every TVIEW `V` and every committed state `σ`: every row
  `X` reads from `tv_V` equals, on every column with binary equality (`data` included), the
  row with the same key that `X` reads from `⟦D_V⟧` evaluated **as `X`**, every view of the
  chain as `X` (security-invoker semantics, the frame's settings otherwise). If `X`'s
  privileges refuse that evaluation, `X` reads no row of `tv_V`. A reader sees neither a row,
  nor an embedded child row, nor a copied column it could not read itself. pg_tviews
  guarantees the maintenance half (T8): no `tv_*` policy filters a maintenance statement,
  and nothing else writes `tv_*`. The read half rests on the `tv_*` policies and on scoping
  each embedded collection like its parent, which pg_tviews does not write and the
  visibility oracle checks (`test/sql/differential/visibility.sql`).

### Assumptions about PostgreSQL

| ID | Assumption | Source | Checked by |
|---|---|---|---|
| PG1 | A view's stored rule (`pg_rewrite.ev_action`) is the query the executor runs for it, up to planner transformations that preserve results | rewriter, `RewriteQuery` | the differential oracle compares every TVIEW with its view: `test/sql/differential/run.sh`, `test/sql/differential/gen/run.sh` |
| PG2 | An `AFTER ROW` trigger fires once for every row an `INSERT`, `UPDATE`, `DELETE`, `MERGE` or `ON CONFLICT` changes, with its old and new images, unless `session_replication_role = replica` (non-`ALWAYS` triggers) or the trigger is disabled; `TRUNCATE` fires statement triggers only. A logical replication apply worker runs as `replica`, so TVIEWs over subscribed tables are refused (`0A000`) | `trigger.c` | `test/sql/regress/triggers/regress_one_trigger_set.sql`, `test/sql/regress/transactions/regress_replication_role.sql`, `test/sql/regress/operations/regress_health_disabled_trigger.sql`, `test/replication/logical_subscriber.sh` |
| PG3 | A statement trigger with transition tables sees every row the statement changed | `trigger.c`, `AfterTriggerSaveEvent` | `test/sql/regress/lineage/regress_statement_mapping.sql`, `test/sql/regress/transactions/regress_transition_plans_per_table.sql` |
| PG4 | `datumIsEqual(a, b)` true implies every function returns the same value on `a` and on `b` | binary identity of datums | `test/sql/regress/freshness/regress_fanout_numeric_precision.sql` |
| PG5 | Under `READ COMMITTED`, a read-write SPI statement takes a new snapshot, which includes every transaction committed before it | `spi.c`, `snapmgr.c` | `test/isolation/specs/snapshot-writers.spec` |
| PG6 | Row security applies to role `R` on table `T` iff `check_enable_rls(T, R) = RLS_ENABLED`; the policies that then apply are those whose roles include `R` by membership at execution time | `rls.c` | `test/sql/regress/privileges/regress_row_security_refused_after_create.sql`, `test/sql/regress/privileges/regress_tv_row_security.sql` |
| PG7 | A function marked `IMMUTABLE` returns the same value for the same arguments, and reads no table or setting | `pg_proc.provolatile` | outside the theorems: the function's author promises it, and PostgreSQL does not check it |
| PG8 | Lock manager: the documented conflict table; locks held to transaction end; deadlocks detected, one victim failing with `40P01` | `lock.c` | `test/isolation/specs/value-lock-deadlock.spec`, `test/isolation/specs/lock-pass-order.spec`, `test/concurrency/fuzz/run.sh` |
| PG9 | `jsonb_set(doc, p, v)` and `jsonb_smart_patch_scalar(doc, p, v)` equal `doc` with the value at path `p` replaced by `v` and nothing else changed | `jsonfuncs.c`; jsonb_delta | `test/sql/regress/direct_patch/regress_jsonb_delta_contract.sql`, `test/sql/regress/direct_patch/regress_jsonb_delta_semantics.sql` |
| PG10 | `to_jsonb(x)` computed by PostgreSQL in the frame is the value the definition produces for the copied column `x` | `jsonb.c` | `test/sql/regress/direct_patch/regress_direct_patch_differential.sql`, `test/sql/regress/direct_patch/regress_domain_copy.sql` |
| PG11 | Between registration and a write, DDL and DCL change a TVIEW's inputs only through statements pg_tviews' hooks intercept, or through state checked per statement (below) | `src/hooks/` | each entry below |

**PG11, entry by entry.**

| Statement | How | Checked by |
|---|---|---|
| `ALTER TABLE … DROP COLUMN` | hooked | `test/sql/regress/catalog/regress_restore_dropped_columns.sql`, `test/sql/regress/catalog/regress_plan_key_column_gone.sql` |
| `ALTER TABLE … ALTER COLUMN … TYPE` | hooked | `test/sql/regress/ddl/regress_tview_column_ddl.sql`, `test/sql/regress/refresh/regress_tview_column_types.sql` |
| `ALTER TABLE … INHERIT` | hooked | `test/sql/regress/ddl/regress_untrackable_sources.sql` |
| `ALTER TABLE … RENAME COLUMN` (as a removal) | hooked | `test/sql/regress/privileges/regress_nested_ddl_followed.sql` |
| `ALTER TABLE … ADD COLUMN` on a table read whole | hooked | `test/sql/regress/lineage/regress_wholerow_root.sql` |
| `CREATE OR REPLACE VIEW`, `DROP` | hooked | `test/sql/regress/privileges/regress_row_security_beyond_plan.sql`, `test/sql/regress/ddl/regress_trigger_misuse.sql` |
| `ENABLE` / `FORCE ROW LEVEL SECURITY`, `OWNER TO`, `REASSIGN OWNED`, `ALTER ROLE`, `GRANT` / `REVOKE` of a role, `DROP ROLE`, when they make row security apply to a role a TVIEW reads a table as | refused by the hook, and checked per statement (a role change in another database reaches only the per-statement check) | `test/sql/regress/privileges/regress_row_security_refused_after_create.sql`, `test/sql/regress/privileges/regress_row_security_beyond_plan.sql`, `test/sql/regress/privileges/regress_backing_view_privileges.sql` |
| `FORCE ROW LEVEL SECURITY` on `tv_*` | checked per statement (Visibility) | `test/sql/regress/privileges/regress_tv_row_security.sql` |
| `CREATE` / `ALTER` / `DROP POLICY` | do not change whether row security applies (PG6); not relevant to Freshness | `test/sql/regress/privileges/regress_row_security_policy_other_table.sql` |
| `CREATE OR REPLACE FUNCTION`, `ALTER FUNCTION … IMMUTABLE`/`STABLE`, `ALTER TYPE … RENAME VALUE`, collation version changes, `ALTER TEXT SEARCH CONFIGURATION`, `ALTER VIEW … SET (security_invoker)` on a view of the chain, `ALTER ROLE` / `ALTER DATABASE … SET` | neither | outside the theorems: they change what a definition computes without a statement pg_tviews sees; `pg_tviews_refresh()` brings a TVIEW back |
| column `GRANT` / `REVOKE` | neither | outside the theorems for Freshness: the owner's reads fail loud (health component `owner_reads`, `test/sql/regress/privileges/regress_health_as_owner.sql`); the visibility oracle sees them |
| `CREATE TRIGGER` / `CREATE RULE` on `tv_*` | neither: the writes are refused, not the DDL | outside the theorems; the health component `tv_triggers` names them: `test/sql/regress/operations/regress_health_tv_triggers.sql` |

## 1. Theorems

**T1. Column skip (row path).** Let `R(V,T) = Complete(C)`. If an `UPDATE` of `T` changes only
columns outside `C`, binary-wise, in every row, then `⟦D_V⟧(σ', ω) = ⟦D_V⟧(σ, ω)`.
*Precondition:* row security applies to no role `D_V` reads a table as, on any table it reads
(PG6 answers `RLS_ENABLED` for none of them), checked at creation, by the hook (PG11) and per
statement; otherwise pg_tviews refuses the write or refresh and decides nothing. Freshness is
therefore stated over the data alone: `⟦D_V⟧` reads every row of `T`.
*Assumes* PG1, PG4, PG7, PG11, that the owner may read every column of `T` it reads, checked
per statement, and determinism, **order included**: every order-sensitive site of the
definition is one the order rule accepts. Without it T1 is false: an `UPDATE` of a column the
view does not read moves the row in the heap, and an unordered `jsonb_agg` then changes.
*The obligation on the analysis:* `C` contains every column of `T` that any expression of
`D_V` (all levels, every view expanded) can observe; `Complete` is never produced when a
whole-row reference, a system column other than `tableoid`, a function reading `T` or an
untracked function was seen; and no arm of the walker passes over a node silently.
*Decided by:* `decide::column_skip`, `decide::trusted_read_set`.
*Checked by:* `test/sql/regress/cascade/regress_column_aware.sql`,
`test/sql/regress/cascade/regress_unchanged_reads.sql`,
`test/sql/regress/lineage/regress_order_rule.sql`, `test/sql/differential/readset-checks.sh`
(independently of the walker, the planner's check at creation and the generator's
perturbation of the columns outside `C` each catch a wrong read set), and
`scripts/check-walk-arms.sh`.

**T2. Statement-path skip.** T1, with its order assumption, for a statement's transition
tables: if every old/new pair agrees binary-wise on `C`, the statement maps to no key. This
covers tables under the `full_refresh` policy too: there `R` is `Unknown` whenever a declared
function reads `T`.
*Decided by:* `decide::statement_skip`.
*Checked by:* `test/sql/regress/lineage/regress_statement_mapping.sql`,
`test/sql/regress/policy/regress_function_reads.sql`.

**T3. Update pair filter.** Dropping an old/new pair equal on `C` from the mapping query's
input leaves the set of keys whose row changes unchanged. *Assumes* the pairs are matched by
the table's primary key, and T1.
*Decided by:* `decide::pair_filter`.
*Checked by:* `test/sql/regress/transactions/regress_transition_plans_per_table.sql`,
`test/sql/regress/cascade/regress_unchanged_reads.sql`.

**T4. Own-row direct patch.** Let field `f` of `data` be a direct copy of column `c` of the
root table: the walk found `c` only at path `p_f` of `data`, as the bare column (no cast that
changes its JSON form), and `R(V, root) = Complete`. If an `UPDATE` changes only such columns,
with T1's precondition and the owner reading every column, then
`⟦D_V⟧(σ', ω)[k] = patch(tv_V(σ)[k], {p_f ↦ to_jsonb(new c)})`, given
`tv_V(σ)[k] = ⟦D_V⟧(σ, ω)[k]` (induction on committed states).
*Assumes* PG9, PG10, that PostgreSQL renders the value (`to_jsonb`), not Rust, that no domain
or collation coercion sits between the column and its copy, that `tv_V` has no user trigger or
rule (the health component `tv_triggers` names one), and that nothing but pg_tviews writes `tv_V`.
*Decided by:* `decide::direct_patch`.
*Checked by:* `test/sql/regress/direct_patch/regress_direct_patch_differential.sql`,
`test/sql/regress/direct_patch/regress_coalesce_not_a_copy.sql`,
`test/sql/regress/direct_patch/regress_domain_copy.sql`,
`test/sql/regress/freshness/regress_direct_writes_to_tview.sql`.

**T5. Fan-out patch.** For a parent column `c` copied at path `p` into every child document
that joins the parent by key `k_p`, with no condition of its own on the parent: after an
`UPDATE` of `c`, the documents that change are exactly those whose stored lookup column equals
the parent's key, and each changes only at `p`. Skipping rows whose old and new images agree
on `c`, and skipping children whose stored value already equals the new one, both require
**binary** equality of `to_jsonb` values. *Assumes* T4's assumptions, the parent's key
unchanged, and no `LEFT JOIN` filter on the parent.
*Decided by:* `decide::fanout_patch`.
*Checked by:* `test/sql/regress/cascade/regress_fanout_patch.sql`,
`test/sql/regress/direct_patch/regress_fanout_guard.sql`,
`test/sql/regress/freshness/regress_fanout_numeric_precision.sql`,
`test/isolation/specs/fanout-vs-recompute.spec`.

**T6. No-op write guard.** Skipping the write of a computed row equal to the stored one
preserves freshness, when equality is the binary record comparison `*=`. jsonb equality would
not do: `1.5` and `1.50` are equal jsonb values with different texts. The guard compares the
row records with `*=`.
*Decided by:* `decide::noop_guard`.
*Checked by:* `test/sql/regress/refresh/regress_noop_refresh.sql`,
`test/sql/regress/freshness/regress_fanout_numeric_precision.sql`.

**T7. Document-edge prune.** A parent `P` embedding a child TVIEW `U` needs no refresh when no
row of `U` changed in this flush. *Assumes* every change to `tv_U` inside a flush is recorded,
rebuilds and refills included.
*Decided by:* `decide::edge_prune`.
*Checked by:* `test/sql/regress/cascade/regress_prune_unchanged_parent.sql`,
`test/sql/regress/cascade/regress_multihop_prune.sql`.

**T8. Maintenance is not filtered, and is the only writer (Visibility, maintenance half).**
Every statement pg_tviews runs against `tv_*` affects exactly the rows it names, and no other
statement writes `tv_*`. *Assumes* the owner owns `tv_*` and no `tv_*` policy, permissive or
restrictive, can apply to the owner (PG6), checked per maintenance statement (`55000`) rather
than at DDL time, because `pg_restore` emits `FORCE` before the policies. The frame pins
`row_security = on`, so `row_security = off` with an applicable policy is an error, never a
filter. A user trigger or rule on `tv_*` is outside the theorem and named by the health check.
Direct writes by the owner or a grantee are refused; a session in `replica` role and logical
replication into `tv_*` bypass the refusal and are outside the theorem, as for PG2.
*Decided by:* `src/guard.rs`, `src/visibility.rs`.
*Checked by:* `test/sql/regress/privileges/regress_tv_row_security.sql`,
`test/sql/regress/freshness/regress_direct_writes_to_tview.sql`,
`test/sql/regress/operations/regress_health_tv_triggers.sql`,
`test/sql/differential/visibility.sql`.

**T9. Unread view columns and propagated tables.** A table reached only through view columns
`D_V` never reads (at any level, with no whole-row reference to that view) gets no trigger, and
no write to it can change `⟦D_V⟧`.
*Decided by:* `src/lineage/`.
*Checked by:* `test/sql/regress/cascade/regress_unchanged_reads.sql`,
`test/sql/regress/lineage/regress_wholerow_nested_parent.sql`.

**T10. Constant-filtered lookup.** A full refresh triggered by a write to `O` is needed only if
an old or new image of a changed row satisfies the lookup's constant filter.
*Decided by:* `decide::lookup_filter`.
*Checked by:* `test/sql/regress/lineage/regress_constant_lookup.sql`,
`test/isolation/specs/guarded-lookup-vs-insert.spec`.

**T11. Suspension.** Every write recorded while refresh is suspended is reconciled before the
transaction commits, or the commit fails with `55000`. *Assumes* the pending state checked at
`PRE_COMMIT` covers every queue: keys and fan-out patches.
*Decided by:* `src/suspend.rs`, `src/flush/xact.rs`.
*Checked by:* `test/sql/regress/transactions/regress_suspend_resume.sql`,
`test/sql/regress/transactions/regress_suspend_implicit_commit.sql`,
`test/sql/regress/freshness/regress_fanout_unflushed_commit.sql`.

**T12. Protocol (concurrency), a safety property.** For every interleaving of `READ COMMITTED`
transactions running the value-lock protocol (ADR 0207), every committed state satisfies
Freshness (I1 below). Transactions at `REPEATABLE READ` or `SERIALIZABLE` that write a table a
TVIEW reads are refused (`0A000`), so they are not among the interleavings. Aborts leave no
committed state; which aborts may happen is a separate property, I4. *Assumes* T1–T11, order
determinism, link columns whose `=` is binary equality or whose lock form is the type's hash,
and `Unknown` read sets making every column a link.
*Decided by:* `src/concurrency/`.
*Checked by:* `test/isolation/specs/crossed-value-writes.spec`,
`test/isolation/specs/created-row-vs-lock-pass.spec`,
`test/sql/regress/transactions/regress_read_committed_only.sql`,
`test/sql/regress/transactions/regress_value_locks.sql`, `test/concurrency/fuzz/run.sh`.

**T13. Read-your-writes inside a statement.** A query inside the writing statement that reads
`tv_V` (a mutation function reading its result back) sees `⟦D_V⟧` over the transaction's
current state for every row the transaction's queued work reaches. Freshness covers committed
states only; this is its in-transaction counterpart ([concurrency](../concurrency.md)).
*Decided by:* `src/executor.rs`.
*Checked by:* `test/sql/differential/gen/run.sh` (its read-back case),
`test/sql/regress/triggers/regress_ffi_error_unwinds.sql`.

Outside every theorem by statement: `warn` tables, writes with triggers off (PG2's exceptions,
reported by the health check), function bodies changed by DDL, and enum, collation or
text-search configuration changes.

## 2. A model of the flush and locking protocol

What a model checker (TLA+/Apalache or Quint) needs, at the smallest scale that still shows the
known races (2–3 transactions, 2 tables, 1 TVIEW embedding 1 other, 2–3 keys and values). A
skeleton of it is `docs/proofs/model.md`.

- **State:** committed rows per table (versions with `xmin`/`xmax`), each transaction's
  uncommitted writes, snapshots (`READ COMMITTED`: one per statement, a new one per read-write
  SPI statement, PG5); the TVIEW tables; per-transaction pending work (key queue, fan-out
  queue, links-changed flag, suspension); the lock table (value locks `(rel, col, value)` with
  Share/Exclusive, relation intent and escalated locks, key-space locks, row locks); savepoints
  with the lock and queue marks they restore.
- **Actions:** statement start; row write (PG2: the row trigger records keys or patches);
  statement capture (look up the keys, one sorted lock pass, look up again); flush (lock the
  read set, compute in a fresh snapshot, upsert or delete, record changes, cascade to embedding
  TVIEWs); escalation past a threshold; savepoint release and rollback; `COMMIT` (the hook's
  flush, then the `PRE_COMMIT` check of pending work); `ROLLBACK`; deadlock detection (abort one
  waiter in a cycle).
- **Invariants:**
  - I1 (Freshness at every committed state): T12.
  - I2: no commit with pending work (keys or fan-out patches).
  - I3: a row of `tv_V` is created only under its key lock (ADR 0207, "Rows being created").
  - I4: deadlocks only in the classes ADR 0207 lists (multi-statement, rows found by the second
    lookup only, the flush).
- **The decisions in the model:** the model calls the decisions as an uninterpreted, sound
  oracle (T1–T11 proven separately), so the model and the decisions' proofs compose.

The concurrency fuzzer (`test/concurrency/fuzz/README.md`) checks I1 at every committed
snapshot; I2 with a prober whose flushes are skipped, every commit of which must fail or have
queued nothing (it catches the mutant `G3`, a check that does not count fan-out patches);
in `mixed` mode, it reports each deadlock by its class among I4's, telling apart all but
the second lookup from a single statement's flush; and, in `values` mode, it checks a
special case of I4 (writes of unread columns never fail). Its tables
belong to a role that is neither superuser nor the writers, as in production.
