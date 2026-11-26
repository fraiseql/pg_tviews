#!/usr/bin/env bash
# scripts/check-ffi-guards.sh reports every line of ffi-guards/flagged.rs marked
# FLAG, and nothing in ffi-guards/allowed.rs.
set -u
cd "$(dirname "$0")/../.."
fixtures=test/scripts/ffi-guards
rc=0
flagged=$(scripts/check-ffi-guards.sh "$fixtures/flagged.rs" 2>&1)
while IFS= read -r line; do
  grep -q "$fixtures/flagged.rs:$line:" <<<"$flagged" || { echo "not flagged: flagged.rs:$line: $(sed -n "${line}p" $fixtures/flagged.rs)"; rc=1; }
done < <(grep -n '// FLAG' "$fixtures/flagged.rs" | cut -d: -f1)
if ! allowed=$(scripts/check-ffi-guards.sh "$fixtures/allowed.rs" 2>&1); then
  echo "flagged in allowed.rs:"; echo "$allowed"; rc=1
fi
[[ $rc == 0 ]] && echo "check-ffi-guards flags every form and nothing else"
exit $rc
