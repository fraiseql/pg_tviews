# ADR 0169: TVIEW row identity from the query tree

- Status: Accepted; amended by [ADR 0203](0203-propagation-plan.md): the identity is part of the
  stored plan, parents are found through lookup columns of any name (not `fk_<child>`, Decision 7),
  and Stage 2 below is done there. The `distinct_on_keys` / `distinct_on_output_keys` columns
  (Decision 8) were dropped from `pg_tview_meta` in 0.1.0-beta.27. Amended by
  [ADR 0216](0216-union-keys.md): a DISTINCT ON key may be a column of a UNION subquery that is
  a base column in every branch.
- Issues: #170 (umbrella), #169, #171, #172, #173, #174, #175
- Supersedes in part: ADR 0157 ("`sqlparser` remains for … DISTINCT ON keys"), and the beta.22
  DISTINCT ON decisions (#164: the "key-aligned" check, `unique_root_column`, the `pk_unique` index)

## Context

ADR 0157 made the query tree the source of how a write to each base table maps to TVIEW keys.
DISTINCT ON TVIEWs kept a second key system next to it, derived from the definition's text and
keyed by column name:

| | The lineage (ADR 0157) | The DISTINCT ON path (until beta.22) |
|---|---|---|
| Source | Query tree: base columns by attribute number | SQL text, scanned twice (a hand scanner and `sqlparser`), never cross-checked |
| Key value | `RefreshKey.pk: i64` | `RefreshKey.dedup_key: Option<String>` |
| Trigger | the key mapping, over OLD and NEW | the key column read **by name** off `tb_<entity>`, **NEW else OLD** |
| Flush | batched `refresh_bulk` | only when it was the entity's single key |
| Refresh | `WHERE pk = ANY($1)`, rows locked first, typed | `WHERE key::text = $1`, no lock, `LIMIT 1` |
| Propagation | `find_parents_batch` | skipped |

Where the two met (a table a DISTINCT ON TVIEW reads through a join), name matching decided
whether the TVIEW was accepted. That glue is what #164 and #169 are about. The parallel system
also lost data silently:

- **#171** A statement writing rows of several DISTINCT ON groups refreshed none: the flush's bulk
  branch dropped every DISTINCT ON key.
- **#172** Changing a row's DISTINCT ON key left the old group's row in the TVIEW: the trigger
  enqueued the new image's key only.
- **#173** A TVIEW embedding a DISTINCT ON TVIEW was never refreshed by changes to it: propagation
  skipped DISTINCT ON keys.
- **#169** `DISTINCT ON (o.id)` was refused as soon as another table read had an `id` column.
- **#175** The same name-based lookup lost every write to a TVIEW's root table when that table
  was not named `tb_<entity>` (the row trigger strips `tb_` to find a TVIEW's own table).

**Performance (#174).** `SELECT * FROM v_contract WHERE pk_contract = ANY('{100,200}')` on
`DISTINCT ON (c.id_contract) c.id_contract AS pk_contract` pushes the filter below `Unique` and
uses the index on `id_contract`. The text path's `pk_contract::text = '100'` scanned the whole
base table on every refresh. A filter on a column that is *not* the DISTINCT ON key cannot be
pushed below `Unique`: `DISTINCT ON (o.id) … WHERE pk_order = …` sorts both joined tables in full.

## Options

- **(A) Patch the name check** (#169 as asked). Leaves #171–#173 and #175 in place, and the two
  key systems.
- **(B) Keep `pk_<entity>` as the only identity; prove the DISTINCT ON key 1:1 with it.** Needs a
  uniqueness prover (the #164/#169 logic, generalized) and a key translation on every refresh:
  the refresh filters on `pk_<entity>`, which cannot be pushed below `Unique`.
- **(C) The identity of a DISTINCT ON TVIEW is its DISTINCT ON key, derived from the query tree,
  with one runtime key path for every TVIEW.** Chosen.

## Decision

1. **One identity per TVIEW, from the query tree.** At create and re-register, the lineage walk
   derives the TVIEW's *identity*: the output column whose values name its rows and, per UNION
   branch, the base column it resolves to.
   - No top-level DISTINCT ON: the identity is `pk_<entity>`, as before.
   - Top-level DISTINCT ON: the identity is the DISTINCT ON key. It must be projected, or equal,
     through a strict equality of that level, to a projected column (`DISTINCT ON (l.fk_order)
     o.pk_order` with `l.fk_order = o.pk_order`). `pk_<entity>` is then an ordinary column:
     parents still join on it, and the entity name still comes from it.
   - Otherwise (an unprojected expression, a key with no base column) the create is refused,
     with the key named.

   No uniqueness proof is needed: a DISTINCT ON key is unique in the output by definition, and
   refreshing by it recomputes exactly one group.
2. **The lineage maps to the identity.** The walk's key root is the identity's base column; every
   `local` column, mapping query and fan-out hop yields identity values. A table a DISTINCT ON
   TVIEW reads through a join maps to its keys like any other table.
3. **Composite DISTINCT ON is refused, permanently (D2).** A TVIEW row is one entity, addressed by
   one key (`pk_<entity>`, `id` or `identifier`), and parents embed it through one `fk_<entity>`.
   "One row per (a, b)" is an entity of its own (`tb_<entity>` with its `pk_<entity>`). Such a
   definition already failed at create (a primary key on the first column only), so no installed
   TVIEW has this shape. The catalog stores the identity as a list only so that GROUP BY keys
   (stage 2) fit the same shape.
4. **One key type at run time (D3).** `RefreshKey { entity, key: KeyValue, all }` with
   `KeyValue::{Int(i64), Text(String)}`: `Int` for `int2`/`int4`/`int8` identities (the trinity
   `pk_*` hot path, no text round trip), `Text` with the canonical output text of any other type.
   Values are bound as arrays and cast on the parameter side (`= ANY($1::text[]::uuid[])`), so
   the index on the identity column is used.
5. **One trigger path.** The root table is handled through its `local` key mapping like any other
   table: the key column is read by attribute number, from the OLD **and** NEW images (#172,
   #175). Direct patches (#56) and fan-out patches (#120) keep their fast paths.
6. **One refresh path.** Refresh, bulk refresh, smart patch, the row lock and the journaled delete
   take the identity column and type. The text-keyed refresh is deleted (#171, #174).
7. **Propagation from the refreshed rows (D4).** Parents find their rows by
   `fk_<child> = child.pk_<child>`, whatever the child's identity. The refresh returns the
   `pk_<child>` values of the rows it deleted and wrote (old and new), and those are propagated
   (#173). This is the only source that is right after a DISTINCT ON winner change: the winning
   row's `pk_<child>` changes while the key does not.
8. **Deleted:** both text extractions of DISTINCT ON keys, the #164 alignment check
   (`unique_root_column`, the `pk_unique` index), the dedup key, its trigger extraction, refresh
   and DML cache, and the catalog columns `distinct_on_keys` / `distinct_on_output_keys`
   (replaced by `identity`). Whether a TVIEW is a UNION comes from the lineage too.
9. **No stopgap release (D5).** All of it ships in one beta; the affected shapes were documented
   in the issues meanwhile.

## Consequences

- **Catalog revision bump.** `pg_tview_meta.identity` replaces the two DISTINCT ON columns; every
  TVIEW is flagged `needs_reregister`, and `pg_tviews_reregister_all()` after the upgrade derives
  its identity. Until then a TVIEW keeps the runtime of its stored catalog row (an identity of
  `pk_<entity>`), which is what beta.22 did for every TVIEW but DISTINCT ON ones.
- **`pk_<entity>` is no longer necessarily the row identity.** It is still required, still what
  parents join on and what names the entity. `tviews.registry` gains an `identity` column (an
  additive change under ADR 0136, Decision 4: the contract version stays).
- **The table's primary key is the identity column**, always, whatever its name (before, a first
  key named `fk_*`, `*_id` or `identifier` got no primary key).
- **Accepted shapes grow:** a DISTINCT ON TVIEW reading tables through joins needs no alignment and
  no `full_refresh`; the #164 refusal is gone.
- **Refused shapes:** composite DISTINCT ON (named, with the modelling), an unprojected expression
  key (named).

## Stage 2 (separate ADR)

Text analysis still decides the entity name and column roles (`infer_schema`), nested-object and
array dependencies, the direct-patch and fan-out field maps, the recursive-CTE check, aggregate
`group_keys` and embed lookups. The query tree is available at each of those points, and each can
disagree with it the way DISTINCT ON did. Stage 2 moves them one at a time, starting with
aggregate GROUP BY keys as an identity (the catalog shape of this ADR fits them), and leaves
`sqlparser` only for rename text rewriting.
