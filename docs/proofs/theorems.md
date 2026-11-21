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
  either `Complete(C)`, a set of column numbers, or `Unknown`. `C` holds the inputs of every
  virtual generated column it holds (such a column is NULL in the images a trigger sees, PG2).
  `R` is `Unknown` when the definition reads `T` whole, or reads a system column of `T`
  (`tableoid` only of a partitioned `T`: it is constant on any other table), or a declared
  function reads `T`.
- **Trusted.** A decision uses `C` only when it is trusted: the TVIEW's stored plan is this
  release's, its chain still reads `T` through the columns it names, and its owner may read
  them (`Env` in `src/decide/`, each fact established per statement). An untrusted read set
  counts as `Unknown`.
- **The order rule** ([shapes, D](../reference/shapes.md#d-what-a-value-depends-on-besides-rows)): every site of the definition
  whose result depends on the order of its input rows (`jsonb_agg`, `array_agg`,
  `string_agg`, `DISTINCT ON`, `LIMIT`, window functions) has an `ORDER BY` that determines
  every row. pg_tviews refuses a definition breaking it (`regress_order_rule.sql`).
- **Inputs to the decisions** are facts computed outside `src/decide/`. Each has a
  conservative value, the one that rules the shortcut out, and is computed so whenever the
  fact is not established: `key_changed`, `virtual_read`, `child_changed_in_flush` and
  `guard_met` are true, `paired` and `document_edge` false, and every `Env` fact false. The
  theorems below assume their inputs are so computed (outside the decisions' own tests).

### The two properties

- **Freshness.** For every TVIEW `V` and every committed state `σ` reached from `V`'s
  registration (or last full refresh, `pg_tviews_refresh`) by writes none of which touched a
  table declared `warn` for `V`: `tv_V(σ) = ⟦D_V⟧(σ, ω_V)`, as multisets of rows, compared
  column by column with binary equality of the rendered values. For a TVIEW that reads the
  current time, `⟦D_V⟧` reads it as the instant `B` of the last time boundary
  ([time-dependent TVIEWs](../reference/ddl.md#time-dependent-tviews)): every row equals `D_V`
  evaluated at an instant no earlier than `B`, except the keys a transaction that began
  before `B` wrote after `B` was announced, from that transaction's commit until the drain
  refreshes them.
- **Visibility.** For every role `X`, every TVIEW `V` and every committed state `σ`: every row
  `X` reads from `tv_V` equals, on every column with binary equality (`data` included), the
  row with the same key that `X` reads from `⟦D_V⟧` evaluated **as `X`**, every view of the
  chain as `X` (security-invoker semantics, the frame's settings otherwise), and `X` reads no
  row of `tv_V` whose key that evaluation does not return. If `X`'s
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
| PG2 | An `AFTER ROW` trigger fires once for every row an `INSERT`, `UPDATE`, `DELETE`, `MERGE` or `ON CONFLICT` changes, with its old and new images (a virtual generated column NULL in both; an `UPDATE` moving a row to another partition fires `DELETE` and `INSERT` on the partitions), unless `session_replication_role = replica` (non-`ALWAYS` triggers) or the trigger is disabled; `TRUNCATE` fires statement triggers only. A logical replication apply worker runs as `replica`, so TVIEWs over subscribed tables are refused (`0A000`) | `trigger.c` | `test/sql/regress/triggers/regress_one_trigger_set.sql`, `test/sql/regress/transactions/regress_replication_role.sql`, `test/sql/regress/operations/regress_health_disabled_trigger.sql`, `test/replication/logical_subscriber.sh` |
| PG3 | A statement trigger with transition tables sees every row the statement changed | `trigger.c`, `AfterTriggerSaveEvent` | `test/sql/regress/lineage/regress_statement_mapping.sql`, `test/sql/regress/transactions/regress_transition_plans_per_table.sql` |
| PG4 | `datumIsEqual(a, b)` true implies every function a definition may call (`IMMUTABLE` or `STABLE`, PG7; a definition calling a `VOLATILE` one is refused, `0A000`) returns the same value on `a` and on `b`, in the frame's settings | binary identity of datums | `test/sql/regress/freshness/regress_fanout_numeric_precision.sql` |
| PG5 | Under `READ COMMITTED`, a read-write SPI statement takes a new snapshot, which includes every transaction committed before it | `spi.c`, `snapmgr.c` | `test/isolation/specs/snapshot-writers.spec` |
| PG6 | Row security applies to role `R` on table `T` iff `check_enable_rls(T, R) = RLS_ENABLED`; the policies that then apply are those whose roles include `R` by membership at execution time | `rls.c` | `test/sql/regress/privileges/regress_row_security_refused_after_create.sql`, `test/sql/regress/privileges/regress_tv_row_security.sql` |
| PG7 | A function marked `IMMUTABLE` returns the same value for the same arguments, and reads no table or setting | `pg_proc.provolatile` | outside the theorems: the function's author promises it, and PostgreSQL does not check it |
| PG8 | Lock manager: the documented conflict table; locks held to transaction end; deadlocks detected, one victim failing with `40P01` | `lock.c` | `test/isolation/specs/value-lock-deadlock.spec`, `test/isolation/specs/lock-pass-order.spec`, `test/concurrency/fuzz/run.sh` |
| PG9 | For a `jsonb` value `v` (never SQL NULL: a patch passes its values as elements of a JSON array, a NULL column as JSON `null`) and a path `p` of object keys present in `doc` (the definition builds every level of it with `jsonb_build_object`), `jsonb_set(doc, p, v)` and `jsonb_smart_patch_scalar(doc, p, v)` equal `doc` with the value at `p` replaced by `v` and nothing else changed | `jsonfuncs.c`; jsonb_delta | `test/sql/regress/direct_patch/regress_jsonb_delta_contract.sql`, `test/sql/regress/direct_patch/regress_jsonb_delta_semantics.sql` |
| PG10 | `to_jsonb(x)` computed by PostgreSQL in the frame is the value the definition produces for the copied column `x`, for the types whose copy is exact; `numeric`, floating types and `jsonb` are not (their copy is rendered once more on the way into the patch, so its text may differ: `Copied.exact` false, never patched) | `jsonb.c` | `test/sql/regress/direct_patch/regress_direct_patch_differential.sql`, `test/sql/regress/direct_patch/regress_domain_copy.sql` |
| PG11 | Between registration and a write, DDL and DCL change a TVIEW's inputs only through statements pg_tviews' hooks intercept, through state checked per statement, or through the statements listed as "neither" below, which are outside the theorems | `src/hooks/` | each entry below |

**PG11, entry by entry.**

| Statement | How | Checked by |
|---|---|---|
| `ALTER TABLE … DROP COLUMN` | hooked | `test/sql/regress/catalog/regress_restore_dropped_columns.sql`, `test/sql/regress/catalog/regress_plan_key_column_gone.sql` |
| `ALTER TABLE … ALTER COLUMN … TYPE` | hooked | `test/sql/regress/ddl/regress_tview_column_ddl.sql`, `test/sql/regress/refresh/regress_tview_column_types.sql` |
| `ALTER TABLE … INHERIT` | hooked | `test/sql/regress/ddl/regress_untrackable_sources.sql` |
| `ALTER TABLE … RENAME COLUMN` (as a removal) | hooked | `test/sql/regress/privileges/regress_nested_ddl_followed.sql` |
| `ALTER TABLE … ADD COLUMN` on a table read whole | hooked | `test/sql/regress/lineage/regress_wholerow_root.sql` |
| `ATTACH` / `DETACH PARTITION`, `CREATE TABLE … PARTITION OF`, `DROP` of a partition | hooked (the rows that join or leave the tree are mapped to keys) | `test/sql/regress/ddl/regress_partition_truncate_attach.sql`, `test/sql/regress/ddl/regress_issue_261_partition_drop.sql` |
| `ALTER COLUMN … SET EXPRESSION`, `ALTER TYPE … RENAME VALUE` | hooked (the TVIEWs reading the values are refreshed) | `test/sql/regress/ddl/regress_value_rewriting_ddl.sql` |
| `TRUNCATE` | a statement trigger: the TVIEW is refreshed in full | `test/sql/regress/ddl/regress_partition_truncate_attach.sql`, `test/sql/regress/transactions/regress_truncate_in_writing_statement.sql` |
| `CREATE OR REPLACE VIEW`, `DROP` | hooked | `test/sql/regress/privileges/regress_row_security_beyond_plan.sql`, `test/sql/regress/ddl/regress_trigger_misuse.sql` |
| `ENABLE` / `FORCE ROW LEVEL SECURITY`, `OWNER TO`, `REASSIGN OWNED`, `ALTER ROLE`, `GRANT` / `REVOKE` of a role, `DROP ROLE`, when they make row security apply to a role a TVIEW reads a table as | refused by the hook, and checked per statement (a role change in another database reaches only the per-statement check) | `test/sql/regress/privileges/regress_row_security_refused_after_create.sql`, `test/sql/regress/privileges/regress_row_security_beyond_plan.sql`, `test/sql/regress/privileges/regress_backing_view_privileges.sql` |
| `FORCE ROW LEVEL SECURITY` on `tv_*` | checked per statement (Visibility) | `test/sql/regress/privileges/regress_tv_row_security.sql` |
| `CREATE` / `ALTER` / `DROP POLICY` | do not change whether row security applies (PG6); not relevant to Freshness | `test/sql/regress/privileges/regress_row_security_policy_other_table.sql` |
| `CREATE OR REPLACE FUNCTION`, `ALTER FUNCTION … IMMUTABLE`/`STABLE`, collation version changes, `ALTER TEXT SEARCH CONFIGURATION`, `ALTER VIEW … SET (security_invoker)` on a view of the chain, `ALTER ROLE` / `ALTER DATABASE … SET` | neither | outside the theorems: they change what a definition computes without a statement pg_tviews sees; `pg_tviews_refresh()` brings a TVIEW back |
| column `GRANT` / `REVOKE` | neither | outside the theorems for Freshness: the owner's reads fail loud (health component `owner_reads`, `test/sql/regress/privileges/regress_health_as_owner.sql`); the visibility oracle sees them |
| `CREATE TRIGGER` / `CREATE RULE` on `tv_*` | neither: the writes are refused, not the DDL | outside the theorems; the health component `tv_triggers` names them: `test/sql/regress/operations/regress_health_tv_triggers.sql` |

## 1. Theorems

**T1. Column skip (row path).** Let `R(V,T) = Complete(C)`, trusted. If an `UPDATE` of `T`
changes only columns outside `C`, binary-wise, in a row, then that row's change leaves
`⟦D_V⟧(·, ω)` unchanged; so, by induction over the rows of a statement, does a statement all
of whose rows do.
*Precondition:* row security applies to no role `D_V` reads a table as, on any table it reads
(PG6 answers `RLS_ENABLED` for none of them), checked at creation, by the hook (PG11) and per
statement; otherwise pg_tviews refuses the write or refresh and decides nothing. Freshness is
therefore stated over the data alone: `⟦D_V⟧` reads every row of `T`.
*Assumes* PG1, PG4, PG7, PG11, that the owner may read every column of `T` it reads, checked
per statement, and determinism, **order included**: every order-sensitive site of the
definition is one the order rule accepts. Without it T1 is false: an `UPDATE` of a column the
view does not read moves the row in the heap, and an unordered `jsonb_agg` then changes.
*The obligation on the analysis:* `C` contains every column of `T` that any expression of
`D_V` (all levels, every view expanded) can observe, the inputs of its virtual generated
columns included; `Complete` is never produced in the cases §0 lists, nor when an untracked
function was seen; and no arm of the walker passes over a node silently.
*Decided by:* `decide::column_skip`, `decide::trusted_read_set`.
*Checked by:* `test/sql/regress/cascade/regress_column_aware.sql`,
`test/sql/regress/cascade/regress_unchanged_reads.sql`,
`test/sql/regress/lineage/regress_order_rule.sql`, `test/sql/differential/readset-checks.sh`
(independently of the walker, the planner's check at creation and the generator's
perturbation of the columns outside `C` each catch a wrong read set), and
`scripts/check-walk-arms.sh`.

**T2. Statement-path skip.** T1, with its order assumption, for a statement's transition
tables: if `T` has no virtual generated column in `C` and every old/new pair agrees
binary-wise on `C`, the statement maps to no key. This
covers tables under the `full_refresh` policy too: there `R` is `Unknown` whenever a declared
function reads `T`.
*Decided by:* `decide::statement_skip`.
*Checked by:* `test/sql/regress/lineage/regress_statement_mapping.sql`,
`test/sql/regress/policy/regress_function_reads.sql`.

**T3. Update pair filter.** Dropping an old/new pair equal on trusted `C` from the mapping
query's input keeps, among the keys it maps to, every key whose row changes (the result is a
superset of the changed keys, as before the filter). A pair equal on `C` is equal on the
inputs of `C`'s virtual columns, which `C` holds (§0). *Assumes* the old and new images are
paired as the transition tables pair them, row by row, and T1.
*Decided by:* `decide::pair_filter`.
*Checked by:* `test/sql/regress/transactions/regress_transition_plans_per_table.sql`,
`test/sql/regress/cascade/regress_unchanged_reads.sql`.

**T4. Own-row direct patch.** Let field `f` of `data` be a direct copy of column `c` of the
root table: the walk found `c` only at path `p_f` of `data` (no other site of the definition
reads `c`), as the bare column of the same row (no cast that changes its JSON form), with a
copy that is exact (PG10), and `R(V, root) = Complete(C)`, trusted. If an `UPDATE` of a row
with key `k` leaves the key unchanged, and every column of `C` it changes is such a column
(columns outside `C` it may change, T1), with T1's precondition, then
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

**T5. Fan-out patch.** For a parent column `c` copied exactly at path `p` into every child
document that joins the parent by key `k_p`, read nowhere else in the child's definition, with
no condition of its own on the parent: after an `UPDATE` of the parent that leaves `k_p`
unchanged in every pair, pairs every row, and changes only such columns among those the child
reads (none at all: nothing to patch), the documents that can change are among those whose
stored lookup column equals the parent's key, and each changes at most at `p`. Skipping rows
whose old and new images agree on `c`, and skipping children whose stored value already
equals the new one, both require **binary** equality of `to_jsonb` values. *Assumes* T4's
assumptions and no `LEFT JOIN` filter on the parent.
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

**T7. Document-edge prune.** Let `P` read a child TVIEW `U` along a document edge: through
`tv_U`'s stored rows only, not through `U`'s base tables. Along that edge, a row of `P` reached
from a row `u` of `tv_U` needs no refresh when `u` did not change in this flush; `P`'s other
edges (`U`'s base tables read directly, say) are decided apart. *Assumes* every change to
`tv_U` inside a flush is recorded, rebuilds and refills included, so that
`child_changed_in_flush` is true whenever `u` changed.
*Decided by:* `decide::edge_prune`.
*Checked by:* `test/sql/regress/cascade/regress_prune_unchanged_parent.sql`,
`test/sql/regress/cascade/regress_multihop_prune.sql`.

**T8. Maintenance is not filtered, and is the only writer (Visibility, maintenance half).**
Every statement pg_tviews runs against `tv_*` reads and writes exactly the rows its `WHERE`
clause selects, as it would with row security off, and no other statement writes `tv_*`. *Assumes* the owner owns `tv_*` and no `tv_*` policy, permissive or
restrictive, can apply to the owner (PG6), checked per maintenance statement (`55000`) rather
than at DDL time, because `pg_restore` emits `FORCE` before the policies. A `tv_*` policy that applies to the owner is refused (`55000`) before a maintenance
statement runs, so none filters one; and a session that set `row_security = off` meets
PostgreSQL's error where a policy applies, never a filter. A user trigger or rule on `tv_*` is outside the theorem and named by the health check.
Direct writes by the owner or a grantee are refused; a session in `replica` role and logical
replication into `tv_*` bypass the refusal and are outside the theorem, as for PG2.
*Decided by:* `src/guard.rs`, `src/visibility.rs`.
*Checked by:* `test/sql/regress/privileges/regress_tv_row_security.sql`,
`test/sql/regress/freshness/regress_direct_writes_to_tview.sql`,
`test/sql/regress/operations/regress_health_tv_triggers.sql`,
`test/sql/differential/visibility.sql`.

**T9. Unread view columns and propagated tables.** Let a view `W` of `D_V`'s chain read a
table `S` only in expressions of output columns of `W` that `D_V` never reads (at any level,
with no whole-row reference to `W`), and let `W`'s rows not depend on `S`'s rows otherwise: `S`
is not joined, filtered on, grouped by, nor read in an `EXISTS` or a subquery outside those
columns. Then `S` gets no trigger, and no write to `S` can change `⟦D_V⟧`. A join that can
change `W`'s row count (a cross join, a one-to-many join, a semi-join) is a read of `S` by the
walk, so `S` gets its trigger.
*Decided by:* `src/lineage/`.
*Checked by:* `test/sql/regress/cascade/regress_unchanged_reads.sql`,
`test/sql/regress/lineage/regress_wholerow_nested_parent.sql`.

**T10. Constant-filtered lookup.** Let table `O` be read under the `full_refresh` policy, at
sites none of which links it to the TVIEW key, and let `g` be the disjunction, over those
sites, of each one's conditions on constants (the guard). If `O`'s rows reach `⟦D_V⟧` only
through rows satisfying `g`, a full refresh triggered by an `INSERT`, `UPDATE` or `DELETE` of
`O` is needed only if an old or new image of a changed row satisfies `g`. A `TRUNCATE` of `O`
refreshes the TVIEW in full (PG11), outside T10.
*Decided by:* `decide::lookup_filter`.
*Checked by:* `test/sql/regress/lineage/regress_constant_lookup.sql`,
`test/isolation/specs/guarded-lookup-vs-insert.spec`.

**T11. Suspension.** Every write recorded while refresh is suspended reaches its TVIEWs
before the transaction commits or prepares: an explicit `COMMIT` or `PREPARE TRANSACTION`
still suspended resumes and rebuilds, in the hook, each TVIEW the suspended writes touched and
every TVIEW reading one; any other commit fails with `55000`. *Assumes* the pending state
checked at `PRE_COMMIT` covers every queue: keys, patches and fan-out patches (a links-changed
flag queues nothing).
*Decided by:* `src/suspend.rs`, `src/flush/xact.rs`.
*Checked by:* `test/sql/regress/transactions/regress_suspend_resume.sql`,
`test/sql/regress/transactions/regress_suspend_implicit_commit.sql`,
`test/sql/regress/freshness/regress_fanout_unflushed_commit.sql`.

**T12. Protocol (concurrency), a safety property.** For every interleaving of `READ COMMITTED`
transactions running the value-lock protocol (ADR 0207), every committed state satisfies
Freshness (I1 below). Transactions at `REPEATABLE READ` or `SERIALIZABLE` that write a table a
TVIEW reads are refused (`0A000`), so they are not among the interleavings. Aborts leave no
committed state; which aborts may happen is a separate property, I4. *Assumes* T1–T11, order
determinism, link columns (the columns of `C` a join or a filter of `D_V` compares, which
decide which TVIEW rows a row reaches) whose `=` is binary equality or whose value lock is
taken on the type's hash, and `Unknown` read sets making every column a link. It inherits
Freshness's quantification: from registration, with no `warn` table written.
*Decided by:* `src/concurrency/`.
*Checked by:* `test/isolation/specs/crossed-value-writes.spec`,
`test/isolation/specs/created-row-vs-lock-pass.spec`,
`test/sql/regress/transactions/regress_read_committed_only.sql`,
`test/sql/regress/transactions/regress_value_locks.sql`, `test/concurrency/fuzz/run.sh`.

**T13. Read-your-writes inside a statement.** A query inside the writing statement that reads
`tv_V` (a mutation function reading its result back) sees, for every key, `⟦D_V⟧` over the
transaction's state as that query's snapshot shows it. It is conditional on the queue: the
keys the transaction's writes queued are the keys whose rows they changed (T1–T7, I2), so a
key the queue misses is a defect of those, not of T13. Freshness covers committed
states only; this is its in-transaction counterpart ([concurrency](../concurrency.md)).
*Decided by:* `src/executor.rs`.
*Checked by:* `test/sql/differential/gen/run.sh` (its read-back case),
`test/sql/regress/triggers/regress_ffi_error_unwinds.sql`.

Outside every theorem by statement: `warn` tables, writes with triggers off (PG2's exceptions,
reported by the health check), and the statements PG11 lists as "neither".

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
