# Aggregate TVIEWs

An aggregate TVIEW materializes the `GROUP BY` groups of its source tables: one row per
group, keyed by `pk_<entity>`. It has no `tb_<entity>` table of its own.

```sql
SELECT pg_tviews_create_aggregate('tv_user_summary', $$
    SELECT o.fk_user AS pk_user_summary,
           u.id,
           jsonb_build_object('name', u.name, 'orders', count(*), 'total', sum(o.total)) AS data
    FROM tb_order o JOIN tb_user u ON u.pk_user = o.fk_user
    GROUP BY o.fk_user, u.id, u.name
$$, '{"tb_order": "fk_user", "tb_user": "pk_user"}');
```

The third argument, `group_keys`, names for each source table the column whose value
**is** the group key: a change to a `tb_order` row affects the group `fk_user`, a change
to a `tb_user` row the group `pk_user`. Tables not listed do not refresh the aggregate.

## How it stays in sync

A write to a listed source table refreshes the groups its row belongs to, before and
after the write: an UPDATE that moves an order to another user refreshes both users.
Each touched group is recomputed from the backing view: a group that
appears is inserted, one that changes is updated, one that empties is deleted.

The refresh narrows the aggregate with `WHERE pk_<entity> = ANY(…)`, which PostgreSQL
pushes below the `GROUP BY`, so only the touched groups are aggregated. The cost of a
write is therefore the cost of recomputing its group: small groups are cheap, a group
with millions of rows is recomputed in full on every write to it
(`pg_tviews_profile()` reports the fan-out).

## Rules

- The definition is a single `SELECT … GROUP BY` (no UNION).
- `pk_<entity>` is a plain column that is also a `GROUP BY` key (not an expression).
- No window functions (`OVER (…)`): a window spans rows of other groups, so a group
  cannot be recomputed on its own.
- Every table in `group_keys` must be read by the definition and have the named column.
- Renaming a group key column keeps the aggregate maintained (`group_keys` follows).

## Embedding an aggregate in another TVIEW

Another TVIEW can embed an aggregate by joining its table `tv_<entity>` (or its backing view) with
an equality on `pk_<entity>`:

```sql
SELECT pg_tviews_create('tv_user', $$
    SELECT u.pk_user, u.id,
           jsonb_build_object('name', u.name, 'summary', s.data) AS data
    FROM tb_user u LEFT JOIN tv_user_summary s ON s.pk_user_summary = u.pk_user
$$);
```

When a group changes, the rows whose output column on the other side of that equality
holds the group key are refreshed: here `tv_user` rows with `pk_user` equal to the group
key, including a user whose first order just created the group. The column is recorded
in the TVIEW's plan (`pg_tview_meta.plan`, its `embeds`) and indexed if it is not the
primary key.

That column must be projected (`u.pk_user` above, possibly under an alias), and the join
must be in the `FROM` clause of a plain `SELECT` (not in a subquery or CTE). Otherwise
`pg_tviews_create` rejects the definition, because nothing could route a group change
to the rows embedding it. Create the aggregate before the TVIEWs that embed it.
