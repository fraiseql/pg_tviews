# The owner frame

Every computation of a TVIEW's rows runs in its owner's frame, whoever wrote and from whatever
session: creation, the refreshes a write causes, `pg_tviews_refresh()`, the time refresh,
`create_or_replace` and the rebuild. A TVIEW is fresh when `tv_<entity>` holds exactly what its
definition returns **in this frame**. This page is the frame's specification: the tests' oracle
is generated from it (`test/sql/lib/gen_frame.py`), and a unit test fails when it and the code
(`src/owner.rs`) disagree.

## Who and where

| Item | Value |
|---|---|
| role | the TVIEW's owner (the owner of `tv_<entity>`), inside a security-restricted operation |
| `search_path` | `pg_catalog, pg_temp` |
| `default_text_search_config` | the creating session's, kept with the TVIEW |

## Render settings

How a value renders as text or JSON. Fixed values:

| Setting | Value |
|---|---|
| `TimeZone` | `UTC` |
| `DateStyle` | `ISO, YMD` |
| `IntervalStyle` | `postgres` |
| `extra_float_digits` | `1` |
| `bytea_output` | `hex` |
| `lc_monetary` | `C` |
| `lc_numeric` | `C` |
| `lc_time` | `C` |
| `timezone_abbreviations` | `Default` |
| `array_nulls` | `on` |
| `quote_all_identifiers` | `off` |
| `transform_null_equals` | `off` |
| `standard_conforming_strings` | `on` |
| `xmlbinary` | `base64` |
| `xmloption` | `content` |
| `gin_fuzzy_search_limit` | `0` |

## Plan settings

How the frame's statements are planned. Fixed values:

| Setting | Value |
|---|---|
| `plan_cache_mode` | `auto` |
| `jit` | `off` |

## Planner settings

The server's value: the configuration file's, the command line's, `ALTER SYSTEM`'s, `ALTER ROLE
ALL … SET`'s or `ALTER DATABASE … SET`'s, else the built-in default. A role's or a session's
value is not used.

| Setting |
|---|
| `enable_async_append` |
| `enable_bitmapscan` |
| `enable_distinct_reordering` |
| `enable_gathermerge` |
| `enable_group_by_reordering` |
| `enable_hashagg` |
| `enable_hashjoin` |
| `enable_incremental_sort` |
| `enable_indexonlyscan` |
| `enable_indexscan` |
| `enable_material` |
| `enable_memoize` |
| `enable_mergejoin` |
| `enable_nestloop` |
| `enable_parallel_append` |
| `enable_parallel_hash` |
| `enable_partition_pruning` |
| `enable_partitionwise_aggregate` |
| `enable_partitionwise_join` |
| `enable_presorted_aggregate` |
| `enable_self_join_elimination` |
| `enable_seqscan` |
| `enable_sort` |
| `enable_tidscan` |
| `random_page_cost` |
| `seq_page_cost` |
| `cpu_tuple_cost` |
| `cpu_index_tuple_cost` |
| `cpu_operator_cost` |
| `effective_cache_size` |
| `work_mem` |
| `hash_mem_multiplier` |
| `join_collapse_limit` |
| `from_collapse_limit` |
| `geqo` |
| `geqo_threshold` |
| `cursor_tuple_fraction` |
| `constraint_exclusion` |
| `max_parallel_workers_per_gather` |
| `default_statistics_target` |

## Other settings

Settings outside these lists are not part of the contract.
