#!/usr/bin/env bash
# The documentation states no guarantee the code no longer gives: backing views are
# owner-only (no grant on the table reaches them), value-lock tags are keyed with a
# server secret, and some pg_tviews locks do block readers.
set -euo pipefail
cd "$(dirname "$0")/.."

stale=(
  "follow the table's grants"
  "privileges follow the table"
  "backing view privileges follow"
  "computable without a server"
  "Readers are never blocked"
)

bad=""
for phrase in "${stale[@]}"; do
  if hits=$(grep -rniF -- "$phrase" docs README.md); then
    bad+="$hits"$'\n'
  fi
done

if [[ -n "$bad" ]]; then
  echo "Stale documentation claims:"
  printf '%s' "$bad"
  exit 1
fi
