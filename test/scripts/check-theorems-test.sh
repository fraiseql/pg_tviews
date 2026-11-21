#!/usr/bin/env bash
# scripts/check-theorems.sh reports each defect of theorems/flagged.md and
# theorems/decide.rs, and nothing in the real document and module.
set -u
cd "$(dirname "$0")/../.."
fixtures=test/scripts/theorems
rc=0
out=$(scripts/check-theorems.sh "$fixtures/flagged.md" "$fixtures/decide.rs" 2>&1) && { echo "the fixtures passed"; rc=1; }
for want in \
  "assumption 'PG2' names no existing check" \
  "assumption 'PG3' names no existing check" \
  "names test/sql/regress/no_such_test.sql, which does not exist" \
  "T2 names decide::no_such_function" \
  "T3 has no \"*Decided by:*\" line" \
  "T3 names no existing check" \
  "T4 names no decide function" \
  "tagged_unknown names T99" \
  "untagged carries no \"/// Theorem: T<n>\""; do
  grep -qF -- "$want" <<<"$out" || { echo "not flagged: $want"; rc=1; }
done
grep -qF "PG1" <<<"$out" && { echo "flagged PG1, which names a check"; rc=1; }
grep -qF "T1 " <<<"$out" && { echo "flagged T1, which is complete"; rc=1; }
scripts/check-theorems.sh >/dev/null || { echo "the real document fails"; rc=1; }
[[ $rc == 0 ]] && echo "check-theorems flags every defect and nothing else"
exit $rc
