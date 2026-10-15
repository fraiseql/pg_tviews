# ADR 0216: One key, one row: UNION TVIEWs never pick a row

- Status: Accepted
- Fixes: #216 (`union_duplicate_policy = 'first'` fails on a single-key refresh)
- Builds on: [ADR 0169](0169-tview-row-identity.md) (row identity),
  [ADR 0157](0157-cascade-key-mapping.md) (UNION branches as roots)

## Context

A TVIEW over `UNION ALL` names its rows by `pk_<entity>` in every branch. When two branches
return the same key, the backing view has two rows for one TVIEW row. Until 0.1.0-beta.27 the
setting `pg_tviews.union_duplicate_policy` decided what happened: `error` (the default) or
`first`, "keep the first row".

Probed on 0.1.0-beta.27 with two branches sharing `pk_task = 1`:

| Path | `first` | `error` |
|---|---|---|
| `pg_tviews_create` with the duplicate present | raw `23505` on `tv_task_pkey` | raw `23505` |
| `pg_tviews_refresh(entity)`, `pg_tviews_refresh_all()` | raw `23505` | raw `23505` |
| a write refreshing one key | `21000` from `ON CONFLICT DO UPDATE … a second time` | the same |
| a write refreshing several keys (`DISTINCT ON` without `ORDER BY`) | an arbitrary row (the second branch's in the probe) | `21000` with the hint |

So the policy held on one path in five, and there "first" meant "any". A UNION TVIEW's content
was not a function of its base rows.

`first` was also a session setting deciding what an object contains: two sessions writing the
same TVIEW could disagree ([ADR 0220](0220-settings.md)).

## Decision

1. **A key returned twice is an error on every path.** Creation, every refresh (one key, many
   keys, a whole TVIEW), the refill of a reset UNLOGGED TVIEW, and the reconcile of a
   `replaced` definition raise `21000 cardinality_violation`, naming the TVIEW and the key,
   with the hint below. A full fill checks before it writes, so the error is never the
   table's `23505`.
2. **`pg_tviews.union_duplicate_policy` is removed.** Setting it fails (the `pg_tviews.`
   prefix is reserved).
3. **Keeping one row per key is written in the definition**, where its order is visible and
   stored:

   ```sql
   SELECT DISTINCT ON (u.pk_task) u.pk_task, u.id, u.data
   FROM (SELECT pk_task, id, data, 1 AS pref FROM tb_task
         UNION ALL
         SELECT pk_task, id, data, 2 AS pref FROM tb_task_copy) u
   ORDER BY u.pk_task, u.pref
   ```

   The `DISTINCT ON` key is the TVIEW's identity (ADR 0169). Its column stands for a base
   column in every branch, so writes to either table map to the key through that branch.
   Until this ADR, the identity had to be one base column, and this shape was refused ("not
   a column of a base table"). An identity is now a column in each branch, as for a top-level
   UNION.

The hint of the `21000` error: make the branches' keys disjoint (a sign or an offset per
branch), or keep one row per key with `DISTINCT ON` over the UNION, ordered by preference.

## Consequences

- A UNION TVIEW is a function of its base rows; every path computes the same rows.
- One fewer setting. A definition that relied on `first` fails at its next refresh or
  creation with the hint above. No consumer used it (confiture, fraisier, fraiseql,
  pg_treekey checked).
- The refresh paths share one source query per TVIEW (`refresh::source_sql`): the bulk,
  single-key and full fills cannot diverge again.

## Alternatives rejected

- **"first" = the first branch, kept as a policy.** Needs a hidden branch ordinal in the
  backing view. That changes `registry.query` for every UNION TVIEW, so confiture would report
  drift, and it keeps a stored choice that the definition can express.
- **A per-TVIEW option.** A stored setting for something the definition can say, and a second
  way to say it.
