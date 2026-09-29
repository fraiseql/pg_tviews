# Physical benchmark run `baseline-beta17`

## Environment

| key | value |
|---|---|
| date | `2026-09-29T19:35:34Z` |
| commit | `v0.1.0-beta.17-4-g4d2d941` |
| cpu | `13th Gen Intel(R) Core(TM) i7-13700K` |
| cores | `24` |
| memory | `94Gi` |
| kernel | `Linux 7.0.9-arch1-1` |
| server_version | `18.1` |
| autovacuum | `on` |
| checkpoint_timeout | `300s` |
| fsync | `on` |
| full_page_writes | `on` |
| max_wal_size | `1024MB` |
| pg_tviews.audit_enabled | `off` |
| pg_tviews.batch_size | `1000` |
| pg_tviews.cache_size | `10000` |
| pg_tviews.direct_patch_enabled | `on` |
| pg_tviews.graph_cache_enabled | `on` |
| pg_tviews.log_level | `info` |
| pg_tviews.max_dependency_depth | `10` |
| pg_tviews.max_propagation_depth | `100` |
| pg_tviews.max_queue_size | `10000` |
| pg_tviews.metrics_enabled | `off` |
| pg_tviews.suspend_triggers | `off` |
| pg_tviews.table_cache_enabled | `on` |
| pg_tviews.union_duplicate_policy | `error` |
| pg_tviews.unlogged_by_default | `on` |
| shared_buffers | `163848kB` |
| synchronous_commit | `on` |
| wal_compression | `off` |
| wal_level | `replica` |
| ext_jsonb_delta | `0.3.0` |
| ext_pg_tviews | `0.1.0` |
| data_checksums | `on` |
| wal_log_hints | `off` |

## Cascade fan-out (dependents per parent key)

| scenario | edge | parents | p50 | p95 | p99 | max | mean |
|---|---|---|---|---|---|---|---|
| skewed_fanout | post->comment | 300000 | 2 | 6 | 13 | 1266 | 2.33 |
| skewed_fanout | user->comment | 9999 | 27 | 92 | 206 | 235015 | 70.01 |
| skewed_fanout | user->post | 10000 | 12 | 36 | 85 | 100000 | 30.00 |

## `product_small_delta`

Per step (WAL is cluster-wide for the step):

| mode | step | ops | ms | WAL rec | FPI | WAL MiB | WAL B/op |
|---|---|---|---|---|---|---|---|
| unlogged | update_single | 25 | 72.9 | 210 | 55 | 0.41 | 17163 |
| unlogged | update_batch | 5 | 36.9 | 180 | 0 | 0.01 | 2981 |
| unlogged | insert_single | 10 | 29.9 | 83 | 2 | 0.01 | 791 |
| unlogged | delete_single | 10 | 20.8 | 21 | 0 | 0.00 | 91 |
| logged | update_single | 25 | 65.6 | 366 | 91 | 0.72 | 30099 |
| logged | update_batch | 5 | 34.5 | 490 | 3 | 0.14 | 29047 |
| logged | insert_single | 10 | 25.4 | 134 | 2 | 0.02 | 2577 |
| logged | delete_single | 10 | 16.6 | 32 | 0 | 0.00 | 151 |

Per relation (sizes in MiB after the step, Δ in parentheses):

| mode | step | relation | upd | HOT % | newpage upd | ins | del | dead after | seq scan | idx scan | heap | index | TOAST | all-visible | B/row |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| unlogged | update_single | tv_product (u) | 25 | 0.0 | 25 | 0 | 0 | 25 | 25 | 25 | 0.80 (+0.02) | 1.93 (+0.00) | 0.02 (+0.00) | 0.7282 | 1442 |
| unlogged | update_batch | tv_product (u) | 50 | 0.0 | 48 | 0 | 0 | 75 | 5 | 65 | 0.84 (+0.04) | 1.93 (+0.00) | 0.02 (+0.00) | 0.6667 | 1462 |
| unlogged | insert_single | tv_product (u) | 0 | — | 0 | 10 | 0 | 75 | 10 | 10 | 0.85 (+0.01) | 1.93 (+0.00) | 0.02 (+0.00) | 0.6606 | 1459 |
| unlogged | delete_single | tv_product (u) | 0 | — | 0 | 0 | 10 | 85 | 10 | 10 | 0.85 (+0.00) | 1.93 (+0.00) | 0.02 (+0.00) | 0.6606 | 1466 |
| logged | update_single | tv_product (p) | 25 | 0.0 | 25 | 0 | 0 | 25 | 0 | 25 | 0.80 (+0.02) | 1.90 (+0.00) | 0.01 (+0.00) | 0.7282 | 1421 |
| logged | update_batch | tv_product (p) | 50 | 0.0 | 48 | 0 | 0 | 75 | 0 | 65 | 0.84 (+0.04) | 1.90 (+0.00) | 0.01 (+0.00) | 0.6667 | 1442 |
| logged | insert_single | tv_product (p) | 0 | — | 0 | 10 | 0 | 75 | 0 | 10 | 0.85 (+0.01) | 1.90 (+0.00) | 0.01 (+0.00) | 0.6606 | 1439 |
| logged | delete_single | tv_product (p) | 0 | — | 0 | 0 | 10 | 85 | 0 | 10 | 0.85 (+0.00) | 1.90 (+0.00) | 0.01 (+0.00) | 0.6606 | 1446 |

Flush statements of one representative refresh (auto_explain):

| mode | statement | calls | ms | rows | hit/read | dirtied/written | WAL rec | FPI | WAL B |
|---|---|---|---|---|---|---|---|---|---|
| logged | `INSERT INTO tv_product (id, pk_product, fk_category, data) SELECT id, pk_product, fk_category, data FROM v_pro` | 1 | 0.23 | 0 | 39/0 | 1/0 | 7 | 0 | 2178 |
| unlogged | `SELECT EXISTS(SELECT 1 FROM "public"."tv_product" LIMIT 1)` | 1 | 0.00 | 1 | 2/0 | 0/0 | 0 | 0 | 0 |
| unlogged | `INSERT INTO tv_product (id, pk_product, fk_category, data) SELECT id, pk_product, fk_category, data FROM v_pro` | 1 | 0.21 | 0 | 37/0 | 0/0 | 0 | 0 | 0 |

## `product_small_native`

Per step (WAL is cluster-wide for the step):

| mode | step | ops | ms | WAL rec | FPI | WAL MiB | WAL B/op |
|---|---|---|---|---|---|---|---|
| unlogged | update_single | 25 | 70.6 | 204 | 52 | 0.39 | 16404 |
| unlogged | update_batch | 5 | 35.5 | 180 | 0 | 0.01 | 2981 |
| unlogged | insert_single | 10 | 29.5 | 83 | 2 | 0.01 | 791 |
| unlogged | delete_single | 10 | 19.6 | 21 | 0 | 0.00 | 91 |
| logged | update_single | 25 | 65.5 | 360 | 87 | 0.69 | 28885 |
| logged | update_batch | 5 | 34.2 | 490 | 3 | 0.14 | 29047 |
| logged | insert_single | 10 | 25.9 | 134 | 2 | 0.02 | 2577 |
| logged | delete_single | 10 | 17.1 | 32 | 0 | 0.00 | 151 |

Per relation (sizes in MiB after the step, Δ in parentheses):

| mode | step | relation | upd | HOT % | newpage upd | ins | del | dead after | seq scan | idx scan | heap | index | TOAST | all-visible | B/row |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| unlogged | update_single | tv_product (u) | 25 | 0.0 | 25 | 0 | 0 | 25 | 25 | 25 | 0.80 (+0.02) | 1.93 (+0.00) | 0.02 (+0.00) | 0.7282 | 1442 |
| unlogged | update_batch | tv_product (u) | 50 | 0.0 | 48 | 0 | 0 | 75 | 5 | 65 | 0.84 (+0.04) | 1.93 (+0.00) | 0.02 (+0.00) | 0.6667 | 1462 |
| unlogged | insert_single | tv_product (u) | 0 | — | 0 | 10 | 0 | 75 | 10 | 10 | 0.85 (+0.01) | 1.93 (+0.00) | 0.02 (+0.00) | 0.6606 | 1459 |
| unlogged | delete_single | tv_product (u) | 0 | — | 0 | 0 | 10 | 85 | 10 | 10 | 0.85 (+0.00) | 1.93 (+0.00) | 0.02 (+0.00) | 0.6606 | 1466 |
| logged | update_single | tv_product (p) | 25 | 0.0 | 25 | 0 | 0 | 25 | 0 | 25 | 0.80 (+0.02) | 1.90 (+0.00) | 0.01 (+0.00) | 0.7282 | 1421 |
| logged | update_batch | tv_product (p) | 50 | 0.0 | 48 | 0 | 0 | 75 | 0 | 65 | 0.84 (+0.04) | 1.90 (+0.00) | 0.01 (+0.00) | 0.6667 | 1442 |
| logged | insert_single | tv_product (p) | 0 | — | 0 | 10 | 0 | 75 | 0 | 10 | 0.85 (+0.01) | 1.90 (+0.00) | 0.01 (+0.00) | 0.6606 | 1439 |
| logged | delete_single | tv_product (p) | 0 | — | 0 | 0 | 10 | 85 | 0 | 10 | 0.85 (+0.00) | 1.90 (+0.00) | 0.01 (+0.00) | 0.6606 | 1446 |

Flush statements of one representative refresh (auto_explain):

| mode | statement | calls | ms | rows | hit/read | dirtied/written | WAL rec | FPI | WAL B |
|---|---|---|---|---|---|---|---|---|---|
| logged | `INSERT INTO tv_product (id, pk_product, fk_category, data) SELECT id, pk_product, fk_category, data FROM v_pro` | 1 | 0.17 | 0 | 39/0 | 1/0 | 7 | 0 | 2178 |
| unlogged | `SELECT EXISTS(SELECT 1 FROM "public"."tv_product" LIMIT 1)` | 1 | 0.00 | 1 | 2/0 | 0/0 | 0 | 0 | 0 |
| unlogged | `INSERT INTO tv_product (id, pk_product, fk_category, data) SELECT id, pk_product, fk_category, data FROM v_pro` | 1 | 0.16 | 0 | 37/0 | 0/0 | 0 | 0 | 0 |

## `product_small_matview`

Per step (WAL is cluster-wide for the step):

| mode | step | ops | ms | WAL rec | FPI | WAL MiB | WAL B/op |
|---|---|---|---|---|---|---|---|
| logged | update_single | 5 | 124.8 | 5920 | 85 | 4.19 | 878493 |
| logged | update_batch | 5 | 120.8 | 6014 | 37 | 3.92 | 821951 |
| logged | insert_single | 5 | 120.7 | 6155 | 76 | 4.16 | 873404 |
| logged | delete_single | 5 | 122.2 | 5871 | 30 | 3.87 | 810721 |

Per relation (sizes in MiB after the step, Δ in parentheses):

| mode | step | relation | upd | HOT % | newpage upd | ins | del | dead after | seq scan | idx scan | heap | index | TOAST | all-visible | B/row |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| logged | update_single | mv_product (p) | 0 | — | 0 | 5000 | 0 | 0 | 5 | 0 | 1.00 (+0.22) | 0.04 (+0.00) | 0.01 (+0.00) | 0.0000 | 1098 |
| logged | update_batch | mv_product (p) | 0 | — | 0 | 5000 | 0 | 0 | 5 | 0 | 1.00 (+0.00) | 0.04 (+0.00) | 0.01 (+0.00) | 0.0000 | 1098 |
| logged | insert_single | mv_product (p) | 0 | — | 0 | 5015 | 0 | 0 | 5 | 0 | 1.00 (+0.00) | 0.04 (+0.00) | 0.01 (+0.00) | 0.0000 | 1092 |
| logged | delete_single | mv_product (p) | 0 | — | 0 | 5010 | 0 | 0 | 5 | 0 | 1.00 (+0.00) | 0.04 (+0.00) | 0.01 (+0.00) | 0.0000 | 1098 |

## `product_medium_delta`

Per step (WAL is cluster-wide for the step):

| mode | step | ops | ms | WAL rec | FPI | WAL MiB | WAL B/op |
|---|---|---|---|---|---|---|---|
| unlogged | update_single | 25 | 72.0 | 206 | 99 | 0.73 | 30620 |
| unlogged | update_batch | 5 | 195.7 | 2848 | 31 | 0.46 | 97198 |
| unlogged | insert_single | 10 | 30.8 | 82 | 3 | 0.01 | 1389 |
| unlogged | delete_single | 10 | 20.5 | 21 | 0 | 0.00 | 91 |
| logged | update_single | 25 | 68.8 | 360 | 166 | 1.25 | 52259 |
| logged | update_batch | 5 | 195.4 | 5953 | 110 | 2.04 | 428173 |
| logged | insert_single | 10 | 29.6 | 134 | 4 | 0.03 | 3422 |
| logged | delete_single | 10 | 18.4 | 32 | 0 | 0.00 | 151 |

Per relation (sizes in MiB after the step, Δ in parentheses):

| mode | step | relation | upd | HOT % | newpage upd | ins | del | dead after | seq scan | idx scan | heap | index | TOAST | all-visible | B/row |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| unlogged | update_single | tv_product (u) | 25 | 0.0 | 25 | 0 | 0 | 25 | 25 | 25 | 7.84 (+0.02) | 9.46 (+0.00) | 0.02 (+0.00) | 0.9721 | 908 |
| unlogged | update_batch | tv_product (u) | 500 | 0.0 | 498 | 0 | 0 | 525 | 5 | 515 | 8.23 (+0.39) | 9.51 (+0.05) | 0.02 (+0.00) | 0.8803 | 931 |
| unlogged | insert_single | tv_product (u) | 0 | — | 0 | 10 | 0 | 525 | 10 | 10 | 8.23 (+0.01) | 9.51 (+0.00) | 0.02 (+0.00) | 0.8795 | 931 |
| unlogged | delete_single | tv_product (u) | 0 | — | 0 | 0 | 10 | 535 | 10 | 10 | 8.23 (+0.00) | 9.51 (+0.00) | 0.02 (+0.00) | 0.8795 | 931 |
| logged | update_single | tv_product (p) | 25 | 0.0 | 25 | 0 | 0 | 25 | 0 | 25 | 7.84 (+0.02) | 9.43 (+0.00) | 0.01 (+0.00) | 0.9721 | 906 |
| logged | update_batch | tv_product (p) | 500 | 0.0 | 498 | 0 | 0 | 525 | 0 | 515 | 8.23 (+0.39) | 9.48 (+0.05) | 0.01 (+0.00) | 0.8803 | 929 |
| logged | insert_single | tv_product (p) | 0 | — | 0 | 10 | 0 | 525 | 0 | 10 | 8.23 (+0.01) | 9.48 (+0.00) | 0.01 (+0.00) | 0.8795 | 929 |
| logged | delete_single | tv_product (p) | 0 | — | 0 | 0 | 10 | 535 | 0 | 10 | 8.23 (+0.00) | 9.48 (+0.00) | 0.01 (+0.00) | 0.8795 | 929 |

Flush statements of one representative refresh (auto_explain):

| mode | statement | calls | ms | rows | hit/read | dirtied/written | WAL rec | FPI | WAL B |
|---|---|---|---|---|---|---|---|---|---|
| logged | `INSERT INTO tv_product (id, pk_product, fk_category, data) SELECT id, pk_product, fk_category, data FROM v_pro` | 1 | 0.25 | 0 | 34/0 | 0/0 | 6 | 0 | 2116 |
| unlogged | `SELECT EXISTS(SELECT 1 FROM "public"."tv_product" LIMIT 1)` | 1 | 0.00 | 1 | 2/0 | 0/0 | 0 | 0 | 0 |
| unlogged | `INSERT INTO tv_product (id, pk_product, fk_category, data) SELECT id, pk_product, fk_category, data FROM v_pro` | 1 | 0.27 | 0 | 32/0 | 0/0 | 0 | 0 | 0 |

## `product_medium_native`

Per step (WAL is cluster-wide for the step):

| mode | step | ops | ms | WAL rec | FPI | WAL MiB | WAL B/op |
|---|---|---|---|---|---|---|---|
| unlogged | update_single | 25 | 71.2 | 200 | 95 | 0.70 | 29437 |
| unlogged | update_batch | 5 | 196.3 | 2848 | 31 | 0.46 | 97198 |
| unlogged | insert_single | 10 | 32.6 | 82 | 3 | 0.01 | 1389 |
| unlogged | delete_single | 10 | 19.2 | 21 | 0 | 0.00 | 91 |
| logged | update_single | 25 | 67.1 | 356 | 164 | 1.23 | 51635 |
| logged | update_batch | 5 | 196.7 | 5953 | 110 | 2.04 | 428173 |
| logged | insert_single | 10 | 29.6 | 134 | 4 | 0.03 | 3422 |
| logged | delete_single | 10 | 17.6 | 32 | 0 | 0.00 | 151 |

Per relation (sizes in MiB after the step, Δ in parentheses):

| mode | step | relation | upd | HOT % | newpage upd | ins | del | dead after | seq scan | idx scan | heap | index | TOAST | all-visible | B/row |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| unlogged | update_single | tv_product (u) | 25 | 0.0 | 25 | 0 | 0 | 25 | 25 | 25 | 7.84 (+0.02) | 9.46 (+0.00) | 0.02 (+0.00) | 0.9721 | 908 |
| unlogged | update_batch | tv_product (u) | 500 | 0.0 | 498 | 0 | 0 | 525 | 5 | 515 | 8.23 (+0.39) | 9.51 (+0.05) | 0.02 (+0.00) | 0.8803 | 931 |
| unlogged | insert_single | tv_product (u) | 0 | — | 0 | 10 | 0 | 525 | 10 | 10 | 8.23 (+0.01) | 9.51 (+0.00) | 0.02 (+0.00) | 0.8795 | 931 |
| unlogged | delete_single | tv_product (u) | 0 | — | 0 | 0 | 10 | 535 | 10 | 10 | 8.23 (+0.00) | 9.51 (+0.00) | 0.02 (+0.00) | 0.8795 | 931 |
| logged | update_single | tv_product (p) | 25 | 0.0 | 25 | 0 | 0 | 25 | 0 | 25 | 7.84 (+0.02) | 9.43 (+0.00) | 0.01 (+0.00) | 0.9721 | 906 |
| logged | update_batch | tv_product (p) | 500 | 0.0 | 498 | 0 | 0 | 525 | 0 | 515 | 8.23 (+0.39) | 9.48 (+0.05) | 0.01 (+0.00) | 0.8803 | 929 |
| logged | insert_single | tv_product (p) | 0 | — | 0 | 10 | 0 | 525 | 0 | 10 | 8.23 (+0.01) | 9.48 (+0.00) | 0.01 (+0.00) | 0.8795 | 929 |
| logged | delete_single | tv_product (p) | 0 | — | 0 | 0 | 10 | 535 | 0 | 10 | 8.23 (+0.00) | 9.48 (+0.00) | 0.01 (+0.00) | 0.8795 | 929 |

Flush statements of one representative refresh (auto_explain):

| mode | statement | calls | ms | rows | hit/read | dirtied/written | WAL rec | FPI | WAL B |
|---|---|---|---|---|---|---|---|---|---|
| logged | `INSERT INTO tv_product (id, pk_product, fk_category, data) SELECT id, pk_product, fk_category, data FROM v_pro` | 1 | 0.18 | 0 | 34/0 | 0/0 | 6 | 0 | 2116 |
| unlogged | `SELECT EXISTS(SELECT 1 FROM "public"."tv_product" LIMIT 1)` | 1 | 0.00 | 1 | 2/0 | 0/0 | 0 | 0 | 0 |
| unlogged | `INSERT INTO tv_product (id, pk_product, fk_category, data) SELECT id, pk_product, fk_category, data FROM v_pro` | 1 | 0.17 | 0 | 32/0 | 0/0 | 0 | 0 | 0 |

## `product_medium_matview`

Per step (WAL is cluster-wide for the step):

| mode | step | ops | ms | WAL rec | FPI | WAL MiB | WAL B/op |
|---|---|---|---|---|---|---|---|
| logged | update_single | 5 | 925.7 | 50922 | 214 | 38.39 | 8051084 |
| logged | update_batch | 5 | 941.4 | 53687 | 196 | 38.56 | 8086853 |
| logged | insert_single | 5 | 933.8 | 50914 | 159 | 38.05 | 7980017 |
| logged | delete_single | 5 | 940.4 | 51163 | 199 | 38.34 | 8039695 |

Per relation (sizes in MiB after the step, Δ in parentheses):

| mode | step | relation | upd | HOT % | newpage upd | ins | del | dead after | seq scan | idx scan | heap | index | TOAST | all-visible | B/row |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| logged | update_single | mv_product (p) | 0 | — | 0 | 50000 | 0 | 0 | 10 | 0 | 8.00 (+0.00) | 0.23 (+0.00) | 0.01 (+0.00) | 0.0000 | 864 |
| logged | update_batch | mv_product (p) | 0 | — | 0 | 50000 | 0 | 0 | 10 | 0 | 8.00 (+0.00) | 0.23 (+0.00) | 0.01 (+0.00) | 0.0000 | 864 |
| logged | insert_single | mv_product (p) | 0 | — | 0 | 50015 | 0 | 0 | 10 | 0 | 8.00 (+0.00) | 0.23 (+0.00) | 0.01 (+0.00) | 0.0000 | 864 |
| logged | delete_single | mv_product (p) | 0 | — | 0 | 50010 | 0 | 0 | 10 | 0 | 8.00 (+0.00) | 0.23 (+0.00) | 0.01 (+0.00) | 0.0000 | 864 |

## `product_large_delta`

Per step (WAL is cluster-wide for the step):

| mode | step | ops | ms | WAL rec | FPI | WAL MiB | WAL B/op |
|---|---|---|---|---|---|---|---|
| unlogged | update_single | 25 | 75.4 | 205 | 96 | 0.71 | 29714 |
| unlogged | update_batch | 5 | 2969.4 | 30352 | 360 | 5.56 | 1166191 |
| unlogged | insert_single | 10 | 31.1 | 82 | 3 | 0.02 | 2172 |
| unlogged | delete_single | 10 | 20.8 | 21 | 0 | 0.00 | 91 |
| logged | update_single | 25 | 72.9 | 365 | 174 | 1.22 | 51372 |
| logged | update_batch | 5 | 2941.1 | 85348 | 7541 | 64.21 | 13466745 |
| logged | insert_single | 10 | 29.7 | 133 | 4 | 0.04 | 4140 |
| logged | delete_single | 10 | 18.8 | 31 | 0 | 0.00 | 145 |

Per relation (sizes in MiB after the step, Δ in parentheses):

| mode | step | relation | upd | HOT % | newpage upd | ins | del | dead after | seq scan | idx scan | heap | index | TOAST | all-visible | B/row |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| unlogged | update_single | tv_product (u) | 25 | 0.0 | 25 | 0 | 0 | 25 | 25 | 25 | 78.15 (+0.02) | 51.27 (+0.00) | 0.02 (+0.00) | 0.9972 | 1357 |
| unlogged | update_batch | tv_product (u) | 5000 | 0.0 | 3505 | 0 | 0 | 5025 | 5 | 5015 | 80.88 (+2.73) | 52.87 (+1.59) | 0.02 (+0.00) | 0.6739 | 1403 |
| unlogged | insert_single | tv_product (u) | 0 | — | 0 | 10 | 0 | 5025 | 10 | 10 | 80.89 (+0.01) | 52.87 (+0.00) | 0.02 (+0.00) | 0.6738 | 1403 |
| unlogged | delete_single | tv_product (u) | 0 | — | 0 | 0 | 10 | 5035 | 10 | 10 | 80.89 (+0.00) | 52.87 (+0.00) | 0.02 (+0.00) | 0.6738 | 1403 |
| logged | update_single | tv_product (p) | 25 | 0.0 | 25 | 0 | 0 | 25 | 0 | 25 | 78.15 (+0.02) | 51.24 (+0.00) | 0.01 (+0.00) | 0.9972 | 1357 |
| logged | update_batch | tv_product (p) | 5000 | 0.0 | 3505 | 0 | 0 | 5025 | 0 | 5015 | 80.88 (+2.73) | 52.84 (+1.59) | 0.01 (+0.00) | 0.6739 | 1402 |
| logged | insert_single | tv_product (p) | 0 | — | 0 | 10 | 0 | 5025 | 0 | 10 | 80.89 (+0.01) | 52.84 (+0.00) | 0.01 (+0.00) | 0.6738 | 1402 |
| logged | delete_single | tv_product (p) | 0 | — | 0 | 0 | 10 | 5035 | 0 | 10 | 80.89 (+0.00) | 52.84 (+0.00) | 0.01 (+0.00) | 0.6738 | 1402 |

Flush statements of one representative refresh (auto_explain):

| mode | statement | calls | ms | rows | hit/read | dirtied/written | WAL rec | FPI | WAL B |
|---|---|---|---|---|---|---|---|---|---|
| logged | `INSERT INTO tv_product (id, pk_product, fk_category, data) SELECT id, pk_product, fk_category, data FROM v_pro` | 1 | 0.29 | 0 | 43/2 | 0/0 | 7 | 0 | 2170 |
| unlogged | `SELECT EXISTS(SELECT 1 FROM "public"."tv_product" LIMIT 1)` | 1 | 0.01 | 1 | 2/0 | 0/0 | 0 | 0 | 0 |
| unlogged | `INSERT INTO tv_product (id, pk_product, fk_category, data) SELECT id, pk_product, fk_category, data FROM v_pro` | 1 | 0.28 | 0 | 43/0 | 0/0 | 0 | 0 | 0 |

## `product_large_native`

Per step (WAL is cluster-wide for the step):

| mode | step | ops | ms | WAL rec | FPI | WAL MiB | WAL B/op |
|---|---|---|---|---|---|---|---|
| unlogged | update_single | 25 | 75.2 | 209 | 98 | 0.72 | 30283 |
| unlogged | update_batch | 5 | 2900.2 | 30352 | 360 | 5.56 | 1166191 |
| unlogged | insert_single | 10 | 31.4 | 82 | 3 | 0.02 | 2172 |
| unlogged | delete_single | 10 | 19.9 | 21 | 0 | 0.00 | 91 |
| logged | update_single | 25 | 73.0 | 364 | 174 | 1.22 | 51371 |
| logged | update_batch | 5 | 2919.1 | 85349 | 7541 | 64.21 | 13466457 |
| logged | insert_single | 10 | 29.2 | 133 | 4 | 0.04 | 4140 |
| logged | delete_single | 10 | 18.8 | 31 | 0 | 0.00 | 145 |

Per relation (sizes in MiB after the step, Δ in parentheses):

| mode | step | relation | upd | HOT % | newpage upd | ins | del | dead after | seq scan | idx scan | heap | index | TOAST | all-visible | B/row |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| unlogged | update_single | tv_product (u) | 25 | 0.0 | 25 | 0 | 0 | 25 | 25 | 25 | 78.15 (+0.02) | 51.27 (+0.00) | 0.02 (+0.00) | 0.9972 | 1357 |
| unlogged | update_batch | tv_product (u) | 5000 | 0.0 | 3505 | 0 | 0 | 5025 | 5 | 5015 | 80.88 (+2.73) | 52.87 (+1.59) | 0.02 (+0.00) | 0.6739 | 1403 |
| unlogged | insert_single | tv_product (u) | 0 | — | 0 | 10 | 0 | 5025 | 10 | 10 | 80.89 (+0.01) | 52.87 (+0.00) | 0.02 (+0.00) | 0.6738 | 1403 |
| unlogged | delete_single | tv_product (u) | 0 | — | 0 | 0 | 10 | 5035 | 10 | 10 | 80.89 (+0.00) | 52.87 (+0.00) | 0.02 (+0.00) | 0.6738 | 1403 |
| logged | update_single | tv_product (p) | 25 | 0.0 | 25 | 0 | 0 | 25 | 0 | 25 | 78.15 (+0.02) | 51.24 (+0.00) | 0.01 (+0.00) | 0.9972 | 1357 |
| logged | update_batch | tv_product (p) | 5000 | 0.0 | 3505 | 0 | 0 | 5025 | 0 | 5015 | 80.88 (+2.73) | 52.84 (+1.59) | 0.01 (+0.00) | 0.6739 | 1402 |
| logged | insert_single | tv_product (p) | 0 | — | 0 | 10 | 0 | 5025 | 0 | 10 | 80.89 (+0.01) | 52.84 (+0.00) | 0.01 (+0.00) | 0.6738 | 1402 |
| logged | delete_single | tv_product (p) | 0 | — | 0 | 0 | 10 | 5035 | 0 | 10 | 80.89 (+0.00) | 52.84 (+0.00) | 0.01 (+0.00) | 0.6738 | 1402 |

Flush statements of one representative refresh (auto_explain):

| mode | statement | calls | ms | rows | hit/read | dirtied/written | WAL rec | FPI | WAL B |
|---|---|---|---|---|---|---|---|---|---|
| logged | `INSERT INTO tv_product (id, pk_product, fk_category, data) SELECT id, pk_product, fk_category, data FROM v_pro` | 1 | 0.24 | 0 | 43/2 | 0/0 | 7 | 0 | 2170 |
| unlogged | `SELECT EXISTS(SELECT 1 FROM "public"."tv_product" LIMIT 1)` | 1 | 0.01 | 1 | 2/0 | 0/0 | 0 | 0 | 0 |
| unlogged | `INSERT INTO tv_product (id, pk_product, fk_category, data) SELECT id, pk_product, fk_category, data FROM v_pro` | 1 | 0.22 | 0 | 43/0 | 0/0 | 0 | 0 | 0 |

## `product_large_matview`

Per step (WAL is cluster-wide for the step):

| mode | step | ops | ms | WAL rec | FPI | WAL MiB | WAL B/op |
|---|---|---|---|---|---|---|---|
| logged | update_single | 5 | 9794.7 | 501100 | 1479 | 381.16 | 79934173 |
| logged | update_batch | 5 | 9766.6 | 531260 | 1986 | 387.67 | 81300811 |
| logged | insert_single | 5 | 9759.1 | 518420 | 18767 | 515.27 | 108060204 |
| logged | delete_single | 5 | 9751.7 | 500923 | 1410 | 380.70 | 79839075 |

Per relation (sizes in MiB after the step, Δ in parentheses):

| mode | step | relation | upd | HOT % | newpage upd | ins | del | dead after | seq scan | idx scan | heap | index | TOAST | all-visible | B/row |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| logged | update_single | mv_product (p) | 0 | — | 0 | 500000 | 0 | 0 | 10 | 0 | 78.50 (+0.00) | 2.16 (+0.00) | 0.01 (+0.00) | 0.0000 | 846 |
| logged | update_batch | mv_product (p) | 0 | — | 0 | 500000 | 0 | 0 | 10 | 0 | 78.50 (+0.00) | 2.16 (+0.00) | 0.01 (+0.00) | 0.0000 | 846 |
| logged | insert_single | mv_product (p) | 0 | — | 0 | 500015 | 0 | 0 | 10 | 0 | 78.50 (+0.00) | 2.16 (+0.00) | 0.01 (+0.00) | 0.0000 | 846 |
| logged | delete_single | mv_product (p) | 0 | — | 0 | 500010 | 0 | 0 | 10 | 0 | 78.50 (+0.00) | 2.16 (+0.00) | 0.01 (+0.00) | 0.0000 | 846 |

## `noop`

Per step (WAL is cluster-wide for the step):

| mode | step | ops | ms | WAL rec | FPI | WAL MiB | WAL B/op |
|---|---|---|---|---|---|---|---|
| unlogged | noop_self | 1 | 342.4 | 40383 | 153 | 4.08 | 4280752 |
| unlogged | noop_repeat | 3 | 1175.6 | 122494 | 34 | 9.04 | 3157976 |
| unlogged | noop_multi_path | 1 | 18.2 | 221 | 40 | 0.31 | 329720 |
| logged | noop_self | 1 | 356.2 | 100888 | 468 | 12.98 | 13611409 |
| logged | noop_repeat | 3 | 1205.4 | 314271 | 266 | 30.64 | 10710534 |
| logged | noop_multi_path | 1 | 26.4 | 1120 | 116 | 0.93 | 977516 |

Per relation (sizes in MiB after the step, Δ in parentheses):

| mode | step | relation | upd | HOT % | newpage upd | ins | del | dead after | seq scan | idx scan | heap | index | TOAST | all-visible | B/row |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| unlogged | noop_self | tv_post (u) | 10000 | 0.0 | 10000 | 0 | 0 | 10000 | 1 | 10125 | 4.25 (+1.42) | 5.70 (+0.55) | 0.02 (+0.00) | 0.3327 | 523 |
| unlogged | noop_self | tv_user (u) | 0 | — | 0 | 0 | 0 | 0 | 0 | 0 | 0.05 (+0.00) | 0.20 (+0.00) | 0.02 (+0.00) | 1.0000 | 557 |
| unlogged | noop_repeat | tv_post (u) | 30000 | 0.3 | 29907 | 0 | 0 | 39971 | 3 | 30336 | 8.48 (+4.23) | 7.79 (+2.09) | 0.02 (+0.00) | 0.1667 | 854 |
| unlogged | noop_repeat | tv_user (u) | 0 | — | 0 | 0 | 0 | 0 | 0 | 0 | 0.05 (+0.00) | 0.20 (+0.00) | 0.02 (+0.00) | 1.0000 | 557 |
| unlogged | noop_multi_path | tv_post (u) | 120 | 16.7 | 100 | 0 | 0 | 40066 | 2 | 246 | 8.50 (+0.02) | 7.79 (+0.00) | 0.02 (+0.00) | 0.1480 | 855 |
| unlogged | noop_multi_path | tv_user (u) | 1 | 0.0 | 1 | 0 | 0 | 1 | 1 | 1 | 0.05 (+0.00) | 0.20 (+0.00) | 0.02 (+0.00) | 0.7143 | 557 |
| logged | noop_self | tv_post (p) | 10000 | 0.0 | 10000 | 0 | 0 | 10000 | 0 | 10101 | 4.24 (+1.41) | 5.66 (+0.55) | 0.01 (+0.00) | 0.3333 | 519 |
| logged | noop_self | tv_user (p) | 0 | — | 0 | 0 | 0 | 0 | 0 | 0 | 0.05 (+0.00) | 0.16 (+0.00) | 0.01 (+0.00) | 1.0000 | 475 |
| logged | noop_repeat | tv_post (p) | 30000 | 0.2 | 29946 | 0 | 0 | 39985 | 0 | 30378 | 8.48 (+4.24) | 7.76 (+2.10) | 0.01 (+0.00) | 0.1667 | 852 |
| logged | noop_repeat | tv_user (p) | 0 | — | 0 | 0 | 0 | 0 | 0 | 0 | 0.05 (+0.00) | 0.16 (+0.00) | 0.01 (+0.00) | 1.0000 | 475 |
| logged | noop_multi_path | tv_post (p) | 120 | 29.2 | 85 | 0 | 0 | 40084 | 1 | 246 | 8.51 (+0.02) | 7.76 (+0.00) | 0.01 (+0.00) | 0.1478 | 853 |
| logged | noop_multi_path | tv_user (p) | 1 | 0.0 | 1 | 0 | 0 | 1 | 0 | 1 | 0.05 (+0.00) | 0.16 (+0.00) | 0.01 (+0.00) | 0.7143 | 475 |

Flush statements of one representative refresh (auto_explain):

| mode | statement | calls | ms | rows | hit/read | dirtied/written | WAL rec | FPI | WAL B |
|---|---|---|---|---|---|---|---|---|---|
| logged | `INSERT INTO tv_post (pk_post, id, fk_user, data) SELECT pk_post, id, fk_user, data FROM v_post WHERE pk_post =` | 10 | 212.15 | 0 | 158252/0 | 185/185 | 60065 | 0 | 6789187 |
| logged | `DELETE FROM tv_post t WHERE t.pk_post = ANY($1) AND NOT EXISTS (SELECT 1 FROM v_post v WHERE v.pk_post = t.pk_` | 10 | 67.31 | 0 | 26421/0 | 0/0 | 0 | 0 | 0 |
| unlogged | `SELECT EXISTS(SELECT 1 FROM "public"."tv_post" LIMIT 1)` | 1 | 0.07 | 1 | 182/0 | 0/0 | 0 | 0 | 0 |
| unlogged | `INSERT INTO tv_post (pk_post, id, fk_user, data) SELECT pk_post, id, fk_user, data FROM v_post WHERE pk_post =` | 10 | 202.88 | 0 | 158152/0 | 181/181 | 0 | 0 | 0 |
| unlogged | `DELETE FROM tv_post t WHERE t.pk_post = ANY($1) AND NOT EXISTS (SELECT 1 FROM v_post v WHERE v.pk_post = t.pk_` | 10 | 65.55 | 0 | 26404/0 | 0/0 | 0 | 0 | 0 |

## `payload_100`

Per step (WAL is cluster-wide for the step):

| mode | step | ops | ms | WAL rec | FPI | WAL MiB | WAL B/op |
|---|---|---|---|---|---|---|---|
| unlogged | update_single | 200 | 200.4 | 661 | 60 | 0.49 | 2550 |
| unlogged | update_batch | 1 | 7.9 | 563 | 0 | 0.05 | 56136 |
| logged | update_single | 200 | 146.6 | 1670 | 134 | 1.18 | 6161 |
| logged | update_batch | 1 | 7.7 | 1605 | 0 | 0.20 | 213252 |

Per relation (sizes in MiB after the step, Δ in parentheses):

| mode | step | relation | upd | HOT % | newpage upd | ins | del | dead after | seq scan | idx scan | heap | index | TOAST | all-visible | B/row |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| unlogged | update_single | tv_doc (u) | 200 | 0.0 | 54 | 0 | 0 | 200 | 200 | 200 | 0.44 (+0.01) | 1.06 (+0.00) | 0.02 (+0.00) | 0.0000 | 795 |
| unlogged | update_batch | tv_doc (u) | 200 | 0.0 | 146 | 0 | 0 | 400 | 1 | 6 | 0.47 (+0.03) | 1.06 (+0.00) | 0.02 (+0.00) | 0.0000 | 811 |
| logged | update_single | tv_doc (p) | 200 | 0.0 | 54 | 0 | 0 | 200 | 0 | 200 | 0.44 (+0.01) | 1.03 (+0.00) | 0.01 (+0.00) | 0.0000 | 774 |
| logged | update_batch | tv_doc (p) | 200 | 0.0 | 146 | 0 | 0 | 400 | 0 | 6 | 0.47 (+0.03) | 1.03 (+0.00) | 0.01 (+0.00) | 0.0000 | 791 |

Flush statements of one representative refresh (auto_explain):

| mode | statement | calls | ms | rows | hit/read | dirtied/written | WAL rec | FPI | WAL B |
|---|---|---|---|---|---|---|---|---|---|
| logged | `UPDATE tv_doc SET data = jsonb_smart_patch_scalar(data, $1::jsonb), updated_at = now() WHERE pk_doc = ANY($2) ` | 1 | 0.15 | 1 | 18/0 | 0/0 | 5 | 0 | 846 |
| unlogged | `SELECT EXISTS(SELECT 1 FROM "public"."tv_doc" LIMIT 1)` | 1 | 0.00 | 1 | 2/0 | 0/0 | 0 | 0 | 0 |
| unlogged | `UPDATE tv_doc SET data = jsonb_smart_patch_scalar(data, $1::jsonb), updated_at = now() WHERE pk_doc = ANY($2) ` | 1 | 0.16 | 1 | 18/0 | 0/0 | 0 | 0 | 0 |

## `payload_1024`

Per step (WAL is cluster-wide for the step):

| mode | step | ops | ms | WAL rec | FPI | WAL MiB | WAL B/op |
|---|---|---|---|---|---|---|---|
| unlogged | update_single | 200 | 214.5 | 822 | 222 | 1.85 | 9707 |
| unlogged | update_batch | 1 | 9.3 | 465 | 28 | 0.26 | 273801 |
| logged | update_single | 200 | 157.9 | 1829 | 442 | 3.81 | 20001 |
| logged | update_batch | 1 | 9.6 | 1495 | 57 | 0.80 | 833829 |

Per relation (sizes in MiB after the step, Δ in parentheses):

| mode | step | relation | upd | HOT % | newpage upd | ins | del | dead after | seq scan | idx scan | heap | index | TOAST | all-visible | B/row |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| unlogged | update_single | tv_doc (u) | 200 | 0.0 | 200 | 0 | 0 | 200 | 200 | 200 | 2.46 (+0.23) | 0.58 (+0.01) | 0.02 (+0.00) | 0.2698 | 1602 |
| unlogged | update_batch | tv_doc (u) | 200 | 0.0 | 29 | 0 | 0 | 400 | 1 | 6 | 2.49 (+0.03) | 0.62 (+0.04) | 0.02 (+0.00) | 0.1787 | 1638 |
| logged | update_single | tv_doc (p) | 200 | 0.0 | 200 | 0 | 0 | 200 | 0 | 200 | 2.46 (+0.23) | 0.54 (+0.00) | 0.01 (+0.00) | 0.2698 | 1577 |
| logged | update_batch | tv_doc (p) | 200 | 0.0 | 29 | 0 | 0 | 400 | 0 | 6 | 2.49 (+0.03) | 0.58 (+0.04) | 0.01 (+0.00) | 0.1787 | 1614 |

Flush statements of one representative refresh (auto_explain):

| mode | statement | calls | ms | rows | hit/read | dirtied/written | WAL rec | FPI | WAL B |
|---|---|---|---|---|---|---|---|---|---|
| logged | `UPDATE tv_doc SET data = jsonb_smart_patch_scalar(data, $1::jsonb), updated_at = now() WHERE pk_doc = ANY($2) ` | 1 | 0.09 | 1 | 12/0 | 0/0 | 4 | 0 | 1473 |
| unlogged | `SELECT EXISTS(SELECT 1 FROM "public"."tv_doc" LIMIT 1)` | 1 | 0.00 | 1 | 2/0 | 0/0 | 0 | 0 | 0 |
| unlogged | `UPDATE tv_doc SET data = jsonb_smart_patch_scalar(data, $1::jsonb), updated_at = now() WHERE pk_doc = ANY($2) ` | 1 | 0.08 | 1 | 12/0 | 0/0 | 0 | 0 | 0 |

## `payload_4096`

Per step (WAL is cluster-wide for the step):

| mode | step | ops | ms | WAL rec | FPI | WAL MiB | WAL B/op |
|---|---|---|---|---|---|---|---|
| unlogged | update_single | 200 | 225.5 | 831 | 37 | 0.31 | 1650 |
| unlogged | update_batch | 1 | 23.8 | 611 | 3 | 0.07 | 71349 |
| logged | update_single | 200 | 172.3 | 3817 | 355 | 3.31 | 17345 |
| logged | update_batch | 1 | 25.3 | 3569 | 167 | 2.09 | 2186425 |

Per relation (sizes in MiB after the step, Δ in parentheses):

| mode | step | relation | upd | HOT % | newpage upd | ins | del | dead after | seq scan | idx scan | heap | index | TOAST | all-visible | B/row |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| unlogged | update_single | tv_doc (u) | 200 | 0.0 | 22 | 0 | 0 | 200 | 200 | 200 | 0.18 (+0.00) | 0.57 (+0.00) | 11.66 (+1.05) | 0.0000 | 6504 |
| unlogged | update_batch | tv_doc (u) | 200 | 0.0 | 177 | 0 | 0 | 400 | 1 | 6 | 0.20 (+0.02) | 0.57 (+0.00) | 12.71 (+1.05) | 0.0000 | 7066 |
| logged | update_single | tv_doc (p) | 200 | 0.0 | 30 | 0 | 0 | 200 | 0 | 200 | 0.19 (+0.01) | 0.54 (+0.00) | 11.65 (+1.05) | 0.0000 | 6488 |
| logged | update_batch | tv_doc (p) | 200 | 0.0 | 170 | 0 | 0 | 400 | 0 | 6 | 0.20 (+0.01) | 0.55 (+0.01) | 12.70 (+1.05) | 0.0000 | 7049 |

Flush statements of one representative refresh (auto_explain):

| mode | statement | calls | ms | rows | hit/read | dirtied/written | WAL rec | FPI | WAL B |
|---|---|---|---|---|---|---|---|---|---|
| logged | `UPDATE tv_doc SET data = jsonb_smart_patch_scalar(data, $1::jsonb), updated_at = now() WHERE pk_doc = ANY($2) ` | 1 | 0.33 | 1 | 58/0 | 1/1 | 14 | 0 | 5210 |
| unlogged | `SELECT EXISTS(SELECT 1 FROM "public"."tv_doc" LIMIT 1)` | 1 | 0.00 | 1 | 2/0 | 0/0 | 0 | 0 | 0 |
| unlogged | `UPDATE tv_doc SET data = jsonb_smart_patch_scalar(data, $1::jsonb), updated_at = now() WHERE pk_doc = ANY($2) ` | 1 | 0.32 | 1 | 58/0 | 1/1 | 0 | 0 | 0 |

## `payload_16384`

Per step (WAL is cluster-wide for the step):

| mode | step | ops | ms | WAL rec | FPI | WAL MiB | WAL B/op |
|---|---|---|---|---|---|---|---|
| unlogged | update_single | 200 | 260.5 | 829 | 36 | 0.31 | 1610 |
| unlogged | update_batch | 1 | 65.3 | 611 | 3 | 0.07 | 71349 |
| logged | update_single | 200 | 222.8 | 7421 | 686 | 8.89 | 46589 |
| logged | update_batch | 1 | 70.3 | 7129 | 528 | 7.72 | 8096056 |

Per relation (sizes in MiB after the step, Δ in parentheses):

| mode | step | relation | upd | HOT % | newpage upd | ins | del | dead after | seq scan | idx scan | heap | index | TOAST | all-visible | B/row |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| unlogged | update_single | tv_doc (u) | 200 | 0.0 | 22 | 0 | 0 | 200 | 200 | 200 | 0.18 (+0.00) | 0.57 (+0.00) | 35.94 (+3.26) | 0.0000 | 19235 |
| unlogged | update_batch | tv_doc (u) | 200 | 0.0 | 177 | 0 | 0 | 400 | 1 | 6 | 0.20 (+0.02) | 0.57 (+0.00) | 39.20 (+3.26) | 0.0000 | 20951 |
| logged | update_single | tv_doc (p) | 200 | 0.0 | 22 | 0 | 0 | 200 | 0 | 200 | 0.18 (+0.00) | 0.54 (+0.00) | 35.93 (+3.26) | 0.0000 | 19214 |
| logged | update_batch | tv_doc (p) | 200 | 0.0 | 177 | 0 | 0 | 400 | 0 | 6 | 0.20 (+0.02) | 0.54 (+0.00) | 39.19 (+3.26) | 0.0000 | 20931 |

Flush statements of one representative refresh (auto_explain):

| mode | statement | calls | ms | rows | hit/read | dirtied/written | WAL rec | FPI | WAL B |
|---|---|---|---|---|---|---|---|---|---|
| logged | `UPDATE tv_doc SET data = jsonb_smart_patch_scalar(data, $1::jsonb), updated_at = now() WHERE pk_doc = ANY($2) ` | 1 | 0.55 | 1 | 92/0 | 3/3 | 32 | 0 | 18608 |
| unlogged | `SELECT EXISTS(SELECT 1 FROM "public"."tv_doc" LIMIT 1)` | 1 | 0.00 | 1 | 2/0 | 0/0 | 0 | 0 | 0 |
| unlogged | `UPDATE tv_doc SET data = jsonb_smart_patch_scalar(data, $1::jsonb), updated_at = now() WHERE pk_doc = ANY($2) ` | 1 | 0.53 | 1 | 92/0 | 3/3 | 0 | 0 | 0 |

## `payload_65536`

Per step (WAL is cluster-wide for the step):

| mode | step | ops | ms | WAL rec | FPI | WAL MiB | WAL B/op |
|---|---|---|---|---|---|---|---|
| unlogged | update_single | 200 | 444.1 | 830 | 37 | 0.31 | 1616 |
| unlogged | update_batch | 1 | 233.1 | 611 | 3 | 0.07 | 71349 |
| logged | update_single | 200 | 402.7 | 21829 | 1878 | 28.34 | 148609 |
| logged | update_batch | 1 | 250.3 | 21573 | 1706 | 27.06 | 28376076 |

Per relation (sizes in MiB after the step, Δ in parentheses):

| mode | step | relation | upd | HOT % | newpage upd | ins | del | dead after | seq scan | idx scan | heap | index | TOAST | all-visible | B/row |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| unlogged | update_single | tv_doc (u) | 200 | 0.0 | 22 | 0 | 0 | 200 | 200 | 200 | 0.18 (+0.00) | 0.57 (+0.00) | 143.45 (+13.03) | 0.0000 | 75600 |
| unlogged | update_batch | tv_doc (u) | 200 | 0.0 | 177 | 0 | 0 | 400 | 1 | 6 | 0.20 (+0.02) | 0.57 (+0.00) | 156.48 (+13.04) | 0.0000 | 82444 |
| logged | update_single | tv_doc (p) | 200 | 0.0 | 22 | 0 | 0 | 200 | 0 | 200 | 0.18 (+0.00) | 0.54 (+0.00) | 143.44 (+13.03) | 0.0000 | 75579 |
| logged | update_batch | tv_doc (p) | 200 | 0.0 | 177 | 0 | 0 | 400 | 0 | 6 | 0.20 (+0.02) | 0.54 (+0.00) | 156.48 (+13.04) | 0.0000 | 82424 |

Flush statements of one representative refresh (auto_explain):

| mode | statement | calls | ms | rows | hit/read | dirtied/written | WAL rec | FPI | WAL B |
|---|---|---|---|---|---|---|---|---|---|
| logged | `UPDATE tv_doc SET data = jsonb_smart_patch_scalar(data, $1::jsonb), updated_at = now() WHERE pk_doc = ANY($2) ` | 1 | 1.44 | 1 | 200/10 | 25/9 | 104 | 0 | 72200 |
| unlogged | `SELECT EXISTS(SELECT 1 FROM "public"."tv_doc" LIMIT 1)` | 1 | 0.02 | 1 | 0/3 | 0/0 | 0 | 0 | 0 |
| unlogged | `UPDATE tv_doc SET data = jsonb_smart_patch_scalar(data, $1::jsonb), updated_at = now() WHERE pk_doc = ANY($2) ` | 1 | 1.41 | 1 | 201/9 | 25/9 | 0 | 0 | 0 |

## `skewed_fanout`

Per step (WAL is cluster-wide for the step):

| mode | step | ops | ms | WAL rec | FPI | WAL MiB | WAL B/op |
|---|---|---|---|---|---|---|---|
| unlogged | user_p50 | 5 | 180.1 | 47 | 24 | 0.16 | 33984 |
| unlogged | user_p99 | 3 | 43.7 | 5 | 2 | 0.02 | 5285 |
| unlogged | post_title | 20 | 435.5 | 101 | 51 | 0.38 | 19715 |
| unlogged | celebrity_10000 | 1 | 866.3 | 4 | 0 | 0.00 | 202 |
| unlogged | celebrity_30000 | 1 | 3059.5 | 4 | 0 | 0.00 | 204 |
| unlogged | celebrity_100000 | 1 | 9825.6 | 9288 | 9189 | 72.04 | 75535355 |
| logged | user_p50 | 5 | 175.5 | 1172 | 561 | 3.81 | 799507 |
| logged | user_p99 | 3 | 44.8 | 1580 | 864 | 5.91 | 2066743 |
| logged | post_title | 20 | 442.2 | 868 | 241 | 1.67 | 87694 |
| logged | celebrity_10000 | 1 | 1040.7 | 183506 | 31368 | 241.45 | 253178810 |
| logged | celebrity_30000 | 1 | 3244.2 | 573869 | 8913 | 146.22 | 153320686 |
| logged | celebrity_100000 | 1 | 10391.5 | 1867653 | 35681 | 505.56 | 530120343 |

Per relation (sizes in MiB after the step, Δ in parentheses):

| mode | step | relation | upd | HOT % | newpage upd | ins | del | dead after | seq scan | idx scan | heap | index | TOAST | all-visible | B/row |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| unlogged | user_p50 | tv_comment (u) | 116 | 0.0 | 113 | 0 | 0 | 116 | 20 | 116 | 151.94 (+0.03) | 102.41 (+0.02) | 0.02 (+0.00) | 0.9939 | 381 |
| unlogged | user_p50 | tv_post (u) | 60 | 0.0 | 60 | 0 | 0 | 60 | 20 | 60 | 50.95 (+0.01) | 26.28 (+0.00) | 0.02 (+0.00) | 0.9905 | 270 |
| unlogged | user_p50 | tv_user (u) | 5 | 0.0 | 4 | 0 | 0 | 5 | 5 | 5 | 1.20 (+0.00) | 3.28 (+0.00) | 0.02 (+0.00) | 0.9675 | 472 |
| unlogged | user_p99 | tv_comment (u) | 216 | 0.0 | 216 | 0 | 0 | 332 | 4 | 205 | 151.98 (+0.05) | 102.45 (+0.03) | 0.02 (+0.00) | 0.9826 | 381 |
| unlogged | user_p99 | tv_post (u) | 83 | 0.0 | 82 | 0 | 0 | 143 | 4 | 81 | 50.96 (+0.02) | 26.29 (+0.01) | 0.02 (+0.00) | 0.9775 | 270 |
| unlogged | user_p99 | tv_user (u) | 1 | 0.0 | 1 | 0 | 0 | 6 | 1 | 1 | 1.20 (+0.00) | 3.28 (+0.00) | 0.02 (+0.00) | 0.9610 | 472 |
| unlogged | post_title | tv_comment (u) | 50 | 0.0 | 50 | 0 | 0 | 382 | 79 | 50 | 151.99 (+0.01) | 102.45 (+0.00) | 0.02 (+0.00) | 0.9800 | 381 |
| unlogged | post_title | tv_post (u) | 20 | 0.0 | 20 | 0 | 0 | 163 | 20 | 20 | 50.96 (+0.00) | 26.29 (+0.00) | 0.02 (+0.00) | 0.9744 | 270 |
| unlogged | post_title | tv_user (u) | 0 | — | 0 | 0 | 0 | 6 | 0 | 0 | 1.20 (+0.00) | 3.28 (+0.00) | 0.02 (+0.00) | 0.9610 | 472 |
| unlogged | celebrity_10000 | tv_comment (u) | 22730 | 0.0 | 22469 | 0 | 0 | 23112 | 2 | 18465 | 156.73 (+4.74) | 105.80 (+3.35) | 0.02 (+0.00) | 0.2895 | 393 |
| unlogged | celebrity_10000 | tv_post (u) | 10000 | 0.0 | 9835 | 0 | 0 | 10163 | 2 | 5737 | 52.59 (+1.63) | 26.71 (+0.42) | 0.02 (+0.00) | 0.1998 | 277 |
| unlogged | celebrity_10000 | tv_user (u) | 1 | 0.0 | 1 | 0 | 0 | 7 | 1 | 1 | 1.20 (+0.00) | 3.28 (+0.00) | 0.02 (+0.00) | 0.9545 | 472 |
| unlogged | celebrity_30000 | tv_comment (u) | 70396 | 0.0 | 49815 | 0 | 0 | 93508 | 2 | 58208 | 167.26 (+10.52) | 114.79 (+8.99) | 0.02 (+0.00) | 0.0044 | 423 |
| unlogged | celebrity_30000 | tv_post (u) | 30000 | 0.0 | 20701 | 0 | 0 | 40163 | 2 | 17169 | 56.04 (+3.45) | 28.76 (+2.05) | 0.02 (+0.00) | 0.0013 | 296 |
| unlogged | celebrity_30000 | tv_user (u) | 1 | 0.0 | 0 | 0 | 0 | 8 | 1 | 1 | 1.20 (+0.00) | 3.28 (+0.00) | 0.02 (+0.00) | 0.9545 | 472 |
| unlogged | celebrity_100000 | tv_comment (u) | 235015 | 0.0 | 162664 | 0 | 0 | 328523 | 2 | 198960 | 201.60 (+34.34) | 131.49 (+16.70) | 0.02 (+0.00) | 0.0000 | 499 |
| unlogged | celebrity_100000 | tv_post (u) | 100000 | 0.0 | 69458 | 0 | 0 | 140163 | 2 | 69233 | 67.59 (+11.55) | 39.02 (+10.27) | 0.02 (+0.00) | 0.0000 | 373 |
| unlogged | celebrity_100000 | tv_user (u) | 1 | 0.0 | 0 | 0 | 0 | 9 | 1 | 1 | 1.20 (+0.00) | 3.28 (+0.00) | 0.02 (+0.00) | 0.9545 | 472 |
| logged | user_p50 | tv_comment (p) | 116 | 0.0 | 113 | 0 | 0 | 116 | 15 | 116 | 151.94 (+0.03) | 102.38 (+0.02) | 0.01 (+0.00) | 0.9939 | 381 |
| logged | user_p50 | tv_post (p) | 60 | 0.0 | 60 | 0 | 0 | 60 | 15 | 60 | 50.95 (+0.01) | 26.25 (+0.00) | 0.01 (+0.00) | 0.9905 | 270 |
| logged | user_p50 | tv_user (p) | 5 | 0.0 | 4 | 0 | 0 | 5 | 0 | 5 | 1.20 (+0.00) | 3.25 (+0.00) | 0.01 (+0.00) | 0.9675 | 468 |
| logged | user_p99 | tv_comment (p) | 216 | 0.0 | 216 | 0 | 0 | 332 | 3 | 205 | 151.98 (+0.05) | 102.41 (+0.03) | 0.01 (+0.00) | 0.9826 | 381 |
| logged | user_p99 | tv_post (p) | 83 | 0.0 | 82 | 0 | 0 | 143 | 3 | 81 | 50.96 (+0.02) | 26.26 (+0.01) | 0.01 (+0.00) | 0.9775 | 270 |
| logged | user_p99 | tv_user (p) | 1 | 0.0 | 1 | 0 | 0 | 6 | 0 | 1 | 1.20 (+0.00) | 3.25 (+0.00) | 0.01 (+0.00) | 0.9610 | 468 |
| logged | post_title | tv_comment (p) | 50 | 0.0 | 50 | 0 | 0 | 382 | 60 | 50 | 151.99 (+0.01) | 102.41 (+0.00) | 0.01 (+0.00) | 0.9800 | 381 |
| logged | post_title | tv_post (p) | 20 | 0.0 | 20 | 0 | 0 | 163 | 0 | 20 | 50.96 (+0.00) | 26.26 (+0.00) | 0.01 (+0.00) | 0.9744 | 270 |
| logged | post_title | tv_user (p) | 0 | — | 0 | 0 | 0 | 6 | 0 | 0 | 1.20 (+0.00) | 3.25 (+0.00) | 0.01 (+0.00) | 0.9610 | 468 |
| logged | celebrity_10000 | tv_comment (p) | 22730 | 0.0 | 22469 | 0 | 0 | 23112 | 1 | 18448 | 156.73 (+4.74) | 105.75 (+3.34) | 0.01 (+0.00) | 0.2895 | 393 |
| logged | celebrity_10000 | tv_post (p) | 10000 | 0.0 | 9835 | 0 | 0 | 10163 | 1 | 5720 | 52.59 (+1.63) | 26.68 (+0.42) | 0.01 (+0.00) | 0.1998 | 277 |
| logged | celebrity_10000 | tv_user (p) | 1 | 0.0 | 1 | 0 | 0 | 7 | 0 | 1 | 1.20 (+0.00) | 3.25 (+0.00) | 0.01 (+0.00) | 0.9545 | 468 |
| logged | celebrity_30000 | tv_comment (p) | 70396 | 0.0 | 49815 | 0 | 0 | 93508 | 1 | 58050 | 167.26 (+10.52) | 114.76 (+9.01) | 0.01 (+0.00) | 0.0044 | 422 |
| logged | celebrity_30000 | tv_post (p) | 30000 | 0.0 | 20701 | 0 | 0 | 40163 | 1 | 17200 | 56.04 (+3.45) | 28.70 (+2.02) | 0.01 (+0.00) | 0.0013 | 296 |
| logged | celebrity_30000 | tv_user (p) | 1 | 0.0 | 0 | 0 | 0 | 8 | 0 | 1 | 1.20 (+0.00) | 3.25 (+0.00) | 0.01 (+0.00) | 0.9545 | 468 |
| logged | celebrity_100000 | tv_comment (p) | 235015 | 0.0 | 162664 | 0 | 0 | 328523 | 1 | 199152 | 201.60 (+34.34) | 131.50 (+16.74) | 0.01 (+0.00) | 0.0000 | 499 |
| logged | celebrity_100000 | tv_post (p) | 100000 | 0.0 | 69458 | 0 | 0 | 140163 | 1 | 69150 | 67.59 (+11.55) | 38.98 (+10.28) | 0.01 (+0.00) | 0.0000 | 372 |
| logged | celebrity_100000 | tv_user (p) | 1 | 0.0 | 0 | 0 | 0 | 9 | 0 | 1 | 1.20 (+0.00) | 3.25 (+0.00) | 0.01 (+0.00) | 0.9545 | 468 |

Flush statements of one representative refresh (auto_explain):

| mode | statement | calls | ms | rows | hit/read | dirtied/written | WAL rec | FPI | WAL B |
|---|---|---|---|---|---|---|---|---|---|
| logged | `SELECT fk_user, pk_post FROM tv_post WHERE fk_user = ANY($1)` | 1 | 13.89 | 83 | 0/8651 | 1/282 | 1 | 1 | 7165 |
| logged | `UPDATE tv_post SET data = jsonb_smart_patch_nested(data, $1::jsonb, $3::text[]), updated_at = now() WHERE pk_p` | 1 | 1.90 | 83 | 1088/207 | 177/209 | 412 | 170 | 1016946 |
| logged | `SELECT fk_post, pk_comment FROM tv_comment WHERE fk_post = ANY($1)` | 1 | 21.96 | 216 | 11618/14187 | 0/282 | 0 | 0 | 0 |
| logged | `UPDATE tv_comment SET data = jsonb_smart_patch_nested(data, $1::jsonb, $3::text[]), updated_at = now() WHERE p` | 1 | 5.55 | 216 | 3098/498 | 396/509 | 1090 | 8 | 239627 |
| unlogged | `SELECT fk_user, pk_post FROM tv_post WHERE fk_user = ANY($1)` | 1 | 11.55 | 83 | 188/8463 | 0/281 | 0 | 0 | 0 |
| unlogged | `SELECT EXISTS(SELECT 1 FROM "public"."tv_post" LIMIT 1)` | 1 | 0.02 | 1 | 0/3 | 0/0 | 0 | 0 | 0 |
| unlogged | `UPDATE tv_post SET data = jsonb_smart_patch_nested(data, $1::jsonb, $3::text[]), updated_at = now() WHERE pk_p` | 1 | 1.58 | 83 | 1075/205 | 176/112 | 0 | 0 | 0 |
| unlogged | `SELECT fk_post, pk_comment FROM tv_comment WHERE fk_post = ANY($1)` | 1 | 24.12 | 216 | 5146/20659 | 0/281 | 0 | 0 | 0 |
| unlogged | `SELECT EXISTS(SELECT 1 FROM "public"."tv_comment" LIMIT 1)` | 1 | 0.01 | 1 | 1/1 | 0/1 | 0 | 0 | 0 |
| unlogged | `UPDATE tv_comment SET data = jsonb_smart_patch_nested(data, $1::jsonb, $3::text[]), updated_at = now() WHERE p` | 1 | 4.13 | 216 | 3343/236 | 121/248 | 0 | 0 | 0 |
