# ADR 0078: Field dependency classes (widening the direct-patch path)

- Status: Accepted; amended by [ADR 0203](0203-propagation-plan.md): the class C fan-out patch
  (Outcome, #120) is stored in the TVIEW's plan (`tables[].fanout`), not in a cascade path.
- Issue: #78
- Survey: `test/sql/real_benchmark/survey/field_classes.py`

## Context

The direct-patch path (#56) skips the backing-view query only when a change hits an
identity-mapped top-level scalar of the entity's own table. Every other change recomputes
the row from `v_<entity>`. #78 proposed classifying each output field and dispatching
per class:

| class | example | proposed strategy |
|---|---|---|
| A direct scalar | `'title', p.title` | scalar patch (exists, #56) |
| B expression of local columns | `'full_name', first \|\| ' ' \|\| last` | evaluate on NEW, patch |
| C value of a 1:1 parent | `'author', u.data`, `'contract_name', c.name` | patch from the parent's new values |
| D keyed array of children | `'comments', jsonb_agg(…)` | keyed array delta |
| E aggregate / cross-table | `'comment_count', count(*)` | targeted recompute |
| F membership (FK, filter) | FK change | insert / delete / recompute (exists) |

The plan's decision rule: if B + C + D together are a small share (under about 20%),
stop here.

## Survey

Each top-level key of every read model's `data` object was classified from the SQL text
(`field_classes.py`; heuristic, schema-qualified names and CTEs handled, nested objects
take their costliest member's class). This is a **static** share of fields, not a share
of refresh traffic: it says what exists, not how often each field changes.

| schema | views | fields | A | B | C | D | E |
|---|---:|---:|---:|---:|---:|---:|---:|
| printoptim (production FraiseQL, `db/0_schema/02_query_side`) | 71 | 594 | 46.0% | 9.4% | 31.1% | 1.3% | 12.1% |
| velocitybench (`fraiseql_tviews.sql`, `fraiseql_cqrs_schema.sql`) | 6 | 46 | 87.0% | 0% | 13.0% | 0% | 0% |

Class C in printoptim splits into 44 parent `.id`s, 28 embedded parent `.data` documents
and 113 other parent scalars (`contract.name`, `unit.symbol`, …).

## What each class would save

Since #72 (no-op guard), #85 (no propagation past unchanged rows) and #91 (one view
evaluation, no catalog lookups), a recompute costs one indexed evaluation of the backing
view for the row plus the write. On a single-table view, 2000 single-row refreshes take
~485 ms by direct patch and ~605 ms by recompute (#91 measurement). The write, and for
large documents its TOAST rewrite, is the same either way: a `jsonb` update always writes
a whole new value (ADR 0094). A patch path saves the view evaluation only, so its value
grows with the number of joins in the view, and with the number of rows one change
reaches.

## Decision

- **A**: exists.
- **B: no-go.** 9% of printoptim fields (56), none in velocitybench. About half are
  defaults, casts or JSON extractions over local columns (`COALESCE(…)`, `x::text`,
  `->>`); a few read the clock (`start_date <= CURRENT_DATE`), which a write-driven
  refresh cannot keep current anyway. Evaluating an expression on NEW needs an SPI query per row, which is
  the cost the patch would avoid, so the saving is small.
- **C: go, as one targeted follow-up: parent-to-children scalar patch.** A parent scalar
  (113 fields) or embedded parent document (28) changes when the parent row changes, and
  today every child row that embeds it is recomputed from its view, one join each. The
  change could instead be applied to all children in one statement, keyed by the FK:
  `UPDATE tv_child SET data = jsonb_set(data, '{contract_name}', $new) WHERE fk_contract =
  $pk AND data->'contract_name' IS DISTINCT FROM $new`. This is the fan-out case
  (a celebrity parent with thousands of children) where recompute cost is highest. The
  catalog already has what it needs: the cascade path from the parent table (#63/#64)
  and, for embedded documents, the nested-object dependency paths (#56). Parent `.id`s
  (44) only change with the FK, which is membership (F), so they need nothing.
- **D: no-go.** 1.3% of printoptim fields, none in velocitybench, and
  `docs/benchmarks/array-fastpath-go-no-go.md` already concluded that the jsonb_delta
  array operations cannot sync a whole array correctly.
- **E: no-go.** A "targeted recompute of one field" is still a query over the child
  rows, which is what a recompute is. #58's aggregate TVIEWs are the better answer for
  heavy aggregates.
- **Per-field metrics / a SpecQL-provided class map: not now.** The only class worth
  building (C) is derived from what pg_tviews already stores. A class map from SpecQL
  (evoludigit/specql#32) would only be needed if more classes became worth building.

## Consequences

- One follow-up issue: parent-to-children scalar patch for class C. It should be measured
  with `scalar_cascade_fanout.sh` (one parent, 10 → 10 000 children) against today's
  recompute path.
- Re-run the survey on another production schema before extending further. The heuristic
  cannot see which fields change often, so a refresh-traffic count (instrumenting which
  class each triggering change hits) is the next measurement if C lands.

## Outcome (#120)

Class C for parent scalars landed as a one-statement fan-out patch: a cascade path of
one hop into the child's base table records which parent columns the child copies into
top-level `data` keys, and an UPDATE of those columns becomes one
`UPDATE tv_child SET data = jsonb_smart_patch_scalar(data, …) WHERE fk = $pk` at flush.
Embedded parent documents were already patched by #56's derived nested chains.

Measured on PG 18.1, one parent, per parent update, recompute vs fan-out patch (each
configuration in a fresh database): 10 children 3.5 → 1.3 ms, 100 children 14.1 →
6.9 ms, 1 000 children 49.9 → 28.4 ms, 10 000 children 362 → 184 ms. The write of each
child row is the floor. A hand-written `UPDATE … WHERE fk_user = $pk` reached 1.4,
6.5, 21.5 and 110 ms.

