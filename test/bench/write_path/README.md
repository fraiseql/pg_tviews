# Write-path benchmark (#224)

FraiseQL-shaped TVIEWs (velocitybench's `tv_user`, `tv_post`, `tv_comment`, with the
joined entity nested as `author` / `post`) and five writes:

| write | touches |
|---|---|
| `bio` | 1 `tv_user` row, 10 `tv_post` rows (`author.bio`) |
| `username` | 1 `tv_user`, 10 `tv_post` (`author.username`), 50 `tv_comment` (`author.username`) |
| `title` | 1 `tv_post`, ~5 `tv_comment` (`post.title`) |
| `comment_insert` | 1 new `tv_comment` row |
| `bio_bulk100` | 100 `tv_user`, 1,000 `tv_post` rows in one statement |
| `untracked_insert` | nothing: `tb_audit` is read by no TVIEW (the cost of the installed extension alone) |

`run.sh [seconds] [clients] [write ...]` prints each write's tps and WAL bytes per
transaction with pg_tviews, the same for the write with `session_replication_role =
replica` (no triggers), and the ratio. `RECOMPUTE=1` makes pg_tviews recompute rows from
their views instead of patching them (`pg_tviews.direct_patch_enabled = off`).

`fanout.sh [seconds] [doc_bytes]` measures fan-out ([ADR 0233](../../../docs/adr/0233-fanout-cost-and-guard.md)):
one category embedded in N items (`fanout.sql`), `UPDATE tb_cat SET name = …`, for
N in `FANOUTS` (default `1 10 100 1000 10000`) and `MODES` (`patch recompute plain`). It
prints ms and WAL bytes per write. `doc_bytes` 1000 (default) keeps the item documents
inline; 4000 stores them TOASTed.

`shapes.sh [seconds] [shape ...]` measures the two shapes of
[#228](https://github.com/fraiseql/pg_tviews/issues/228) (`shapes.sql`, one TVIEW per
database): a username copied into arrays of embedded rows (`array`) and across two hops
(`two_hop`), with pg_tviews, with the patch path off, with the patch written by hand
(`writes/*.manual.sql`) and plain.

`compare.sh head=28971 base=28972 -- [rounds] [seconds] [clients ...] [-- write ...]`
compares builds: one cluster per build, the writes interleaved build by build (the
order alternating each round), each run started only when the 1-minute load average is
below `LOAD_MAX` (4) and kept only if it ended below `LOAD_DISCARD` (8) plus half its
clients (its own load). It prints every
run as CSV, then each build's median tps per write and client count.

Every run starts on a fresh copy of the seeded template database (`CREATE DATABASE …
STRATEGY FILE_COPY`), so no run measures a table an earlier one bloated.

## Measure a release build on a stock server

The pgrx development build is debug + cassert and several times slower: time only a
release package on a stock PostgreSQL.

1. Package against the server: `cargo pgrx package --no-default-features --features pg18
   --pg-config /usr/bin/pg_config` (one `CARGO_TARGET_DIR` per build you compare, and
   compare the `.so` md5 sums: a shared target dir can produce the same library twice).
2. Copy the package's `.so` into `<pkg>/lib` and its `extension/` files into
   `<pkg>/share/extension`, with jsonb_delta's next to them. Strip `$libdir/` from the
   `.control` files.
3. `initdb` a throwaway cluster and point it at the package:
   ```ini
   shared_preload_libraries = 'pg_tviews'
   extension_control_path = '<pkg>/share:$system'
   dynamic_library_path = '<pkg>/lib:$libdir'
   max_wal_size = 1GB
   autovacuum = off
   ```
4. Interleave the builds you compare, at least three rounds each.

The fan-out runs write a lot of WAL (4 KB documents at N = 1,000 write ~5 MB per
write): keep the cluster off a small tmpfs.

`portable/bio_trigger.sql` is research for [ADR 0232](../../../docs/adr/0232-distribution.md):
the `bio` write kept fresh by a PL/pgSQL statement trigger, on a database without
pg_tviews. Run `writes/bio.sql` against it, as against the extension.
