# ADR 0157: statement-level key mapping

`scenarios/adr_0157_mapping.sh`, PostgreSQL 18.1, jsonb_delta 0.3.0, 2026-10-02.
20k orders, 200k lines, 2k SKUs; `tv_order` lists the SKU names of its lines, so
`tb_sku` reaches it through `tb_line` (two hops). Each number is the median
end-to-end time of an autocommit statement, refresh flush included. The host was
loaded (load average 10–17), so compare ratios; two runs per build (`raw.tsv`).

| Write | rows | this release (ms) | 0.1.0-beta.20 (ms) |
|---|---:|---:|---:|
| `tb_order` (the TVIEW's own table) | 1 | 0.69 / 1.29 | 1.14 / 0.79 |
| `tb_line` (local: `fk_order` read off the row) | 1 | 1.41 / 3.25 | 1.77 / 1.77 |
| `tb_sku` (mapped, two hops) | 1 | 376 / 433 | 557 / 619 |
| `tb_line` | 100 000 | 7 454 / 6 194 | 12 234 / 6 916 |
| `tb_sku` (mapped, two hops) | 1 000 | 7 674 / 5 382 | 6 328 / 6 219 |

- Single-row writes to the TVIEW's own and local tables cost the same (row trigger,
  unchanged).
- A single-row write to a two-hop table is faster: one mapping query per statement
  replaces a hop query per row image.
- Bulk writes are dominated by recomputing the ~20 000 affected `tv_order` rows
  (a `GROUP BY` over their lines), which is the same work on both builds; finding the
  keys (one hash join over the transition table) is a small part of it.
