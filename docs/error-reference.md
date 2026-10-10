# Error reference

Every error pg_tviews raises carries a SQLSTATE a client can catch
(`EXCEPTION WHEN undefined_object`, `WHEN sqlstate '42704'`), a one-line
message, and where it helps a DETAIL (the definition or query involved) and a
HINT (what to do). Internal errors keep `XX000`.

This page is generated from `src/error/` (`src/error/reference.rs`); a unit test
fails when it is out of date.

## Errors of pg_tviews functions

Placeholders stand for the values each message carries.

| SQLSTATE | Condition | Error | Message | Hint |
|---|---|---|---|---|
| `42704` | `undefined_object` | `MetadataNotFound` | TVIEW metadata not found for entity '&lt;entity&gt;' | SELECT entity FROM tviews.pg_tview_meta lists the registered TVIEWs. |
| `42P07` | `duplicate_table` | `RelationExists` | TVIEW &lt;name&gt; already exists | pg_tviews_create_or_replace() changes an existing TVIEW. |
| `22023` | `invalid_parameter_value` | `InvalidInput` | Invalid input for parameter '&lt;parameter&gt;': &lt;reason&gt; |  |
| `0A000` | `feature_not_supported` | `DefinitionRefused` | &lt;reason&gt; |  |
| `42804` | `datatype_mismatch` | `KeyTypeRefused` | &lt;column&gt; is &lt;type&gt;: a TVIEW's pk_&lt;entity&gt; must be an integer key (smallint, integer or bigint) | Key the rows on an integer column, and keep a uuid key in the id column. |
| `42809` | `wrong_object_type` | `ColumnDdlRefused` | &lt;statement&gt; on TVIEW &lt;table&gt; is refused: a TVIEW's columns are its definition's | Change the definition with tviews.pg_tviews_create_or_replace(): the table follows it. |
| `42501` | `insufficient_privilege` | `PermissionDenied` | &lt;reason&gt; |  |
| `42P17` | `invalid_object_definition` | `DependencyCycle` | relations would read each other in a cycle: &lt;relation&gt;, &lt;relation&gt; |  |
| `54001` | `statement_too_complex` | `DepthExceeded` | dependency depth 11 exceeds the maximum of 10 | Raise pg_tviews.max_dependency_depth, or flatten the views the TVIEW reads. |
| `42601` | `syntax_error` | `InvalidSelectStatement` | Invalid SELECT statement: &lt;reason&gt; |  |
| `42703` | `undefined_column` | `RequiredColumnMissing` | Required column '&lt;column&gt;' missing in &lt;context&gt; |  |
| `42883` | `undefined_function` | `JsonbDeltaMissing` | Required extension 'jsonb_delta' is not installed | CREATE EXTENSION jsonb_delta; |
| `54000` | `program_limit_exceeded` | `QueueFull` | refresh queue backpressure: queue size (10001) would exceed max_queue_size (10000) | Raise pg_tviews.max_queue_size, or write in smaller transactions. |
| `55000` | `object_not_in_prerequisite_state` | `WrongState` | &lt;reason&gt; |  |
| `XX000` | `internal_error` | `CatalogError` | Catalog operation '&lt;operation&gt;' failed: &lt;error&gt; | tviews.pg_tviews_reregister(name) re-derives a TVIEW's metadata. |
| `XX000` | `internal_error` | `SpiError` | SPI query failed: &lt;error&gt; |  |
| `XX000` | `internal_error` | `SerializationError` | Serialization error: &lt;message&gt; | tviews.pg_tviews_reregister(name) re-derives a TVIEW's metadata. |

## Errors raised by triggers, hooks and the commit

| SQLSTATE | Condition | When |
|---|---|---|
| `40001` | `t_r_serialization_failure` | Under `REPEATABLE READ`, a write or a refresh needs a value lock a concurrent transaction holds (`a concurrent transaction changes rows this TVIEW refresh reads`, `… refreshes TVIEW rows from rows this write changes`): waiting could not make the transaction's snapshot see the other's change. Retry the transaction. |
| `55000` | `object_not_in_prerequisite_state` | A transaction commits with refresh work still queued (a missing or disabled flush trigger): the commit fails. |
| `55000` | `object_not_in_prerequisite_state` | The library's catalog revision does not match the installed extension, or cannot be read: run `ALTER EXTENSION pg_tviews UPDATE` (the hint names the fix). |
| `42501` | `insufficient_privilege` | The caller neither owns the TVIEW (or is a member of its owner) nor owns the extension. |
| `0A000` | `feature_not_supported` | A `CREATE TABLE tv_* AS` form pg_tviews cannot make a TVIEW of: `SELECT … INTO`, `EXPLAIN`, `EXECUTE`, `WITH NO DATA`, a temporary table, a column list, `TABLESPACE`, `USING`, a storage parameter other than `fillfactor`, a query with parameters. |
| `42P07` | `duplicate_table` | `CREATE TABLE tv_* AS` names a TVIEW that already exists. |
| `21000` | `cardinality_violation` | A UNION backing view returns two rows for one key (`pg_tviews.union_duplicate_policy = 'error'`). |
| `22023` | `invalid_parameter_value` | Under `uncascaded_policy = 'error'`: a table whose writes no cascade maps to the TVIEW's keys, a function that reads tables not declared in `function_reads`, or a definition reading the time without `time_refresh`. Under `warn` the same is a WARNING (01000). |
