#!/usr/bin/env bash
# scripts/check-name-resolution.sh flags every string literal of
# name-resolution/flagged.rs, and nothing in name-resolution/allowed.rs.
set -u
cd "$(dirname "$0")/../.."
fixtures=test/scripts/name-resolution
out=$(NAME_CHECK_ROOT="$fixtures" scripts/check-name-resolution.sh 2>&1)
rc=0
# The lines on which a string literal starts.
while IFS= read -r line; do
  grep -q "^$fixtures/flagged.rs:$line:" <<<"$out" || { echo "not flagged: flagged.rs:$line: $(sed -n "${line}p" $fixtures/flagged.rs)"; rc=1; }
done < <(grep -nE '^[[:space:]]+let _ = ' "$fixtures/flagged.rs" | cut -d: -f1)
if grep -q "^$fixtures/allowed.rs:" <<<"$out"; then
  echo "flagged in allowed.rs:"; grep "^$fixtures/allowed.rs:" <<<"$out"; rc=1
fi
[[ $rc == 0 ]] && echo "check-name-resolution flags every form and nothing else"
exit $rc
