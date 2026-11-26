# A theorems document with one defect of each kind

| ID | Assumption | Source | Checked by |
|---|---|---|---|
| PG1 | names a check that exists | x | `scripts/check-theorems.sh` |
| PG2 | names no check | x | nothing yet |
| PG3 | names a missing file | x | `test/sql/regress/no_such_test.sql` |

**T1. Decided by a function the module defines.** Fine.
*Decided by:* `decide::decided`.
*Checked by:* `scripts/check-theorems.sh`.

**T2. Names a decide function that does not exist.** Text.
*Decided by:* `decide::no_such_function`.
*Checked by:* `scripts/check-theorems.sh`.

**T3. Has no Decided-by line, and no check.** Text.

**T4. Must name a decide function, names a path.** Text.
*Decided by:* `src/lib.rs`.
*Checked by:* `scripts/check-theorems.sh`.

## End
