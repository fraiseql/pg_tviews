# Quarantined SQL tests

These files are **excluded from the CI regression suite** because they exercise
functionality that pg_tviews does not currently support (by design), not because of a
regression. Each file's header states why and links a tracking issue.

Do not wire these into CI (`test/run_regression_tests.sh` / the numbered-suite runner)
until the linked feature lands, at which point the test should be converted to the
supported API and moved back into `test/sql/`.

| File | Reason | Tracking |
|---|---|---|
| `98-unlogged-integration.sql` | legacy shape: aggregate built in a derived-table subquery, `u.pk_user AS id` with no `pk_<entity>` column. Aggregate TVIEWs themselves are supported since #58 (`pg_tviews_create_aggregate`, covered by `regress_issue_58_aggregate.sql`) | #58 |
| `99-performance-validation.sql` | window-function TVIEW (`perf_logged`, `AVG(…) OVER`, `ROW_NUMBER() OVER`): a window spans rows of other groups, so it cannot be maintained per group; rejected by design | #58 |
| `100-multi-table-integration.sql` | legacy shape: `mt_*_summary` rollups in derived-table subqueries over off-convention `mt_*` tables (no `tb_`/`pk_` naming); see `regress_issue_58_aggregate.sql` for the supported aggregate form | #58 |

Filed under the #55 test-suite health audit.
