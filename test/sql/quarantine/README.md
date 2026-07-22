# Quarantined SQL tests

These files are **excluded from the CI regression suite** because they exercise
functionality that pg_tviews does not currently support (by design), not because of a
regression. Each file's header states why and links a tracking issue.

Do not wire these into CI (`test/run_regression_tests.sh` / the numbered-suite runner)
until the linked feature lands, at which point the test should be converted to the
supported API and moved back into `test/sql/`.

| File | Reason | Tracking |
|---|---|---|
| `98-unlogged-integration.sql` | aggregate/summary TVIEW (`user_summary`, `COUNT`/`SUM`/`GROUP BY`, no `tb_<entity>`) | #58 |
| `99-performance-validation.sql` | window-function TVIEW (`perf_logged`, `AVG(…) OVER`, `ROW_NUMBER() OVER`) | #58 |
| `100-multi-table-integration.sql` | aggregate `mt_*_summary` rollups over `mt_*` base tables | #58 |
| `60_2pc_support.sql` | tests a 2PC API (`pg_tviews_commit_prepared` / `_rollback_prepared` / `_recover_prepared_transactions`) that is not implemented; also drops an internal table and needs `max_prepared_transactions>0` | #59 |

Filed under the #55 test-suite health audit.
