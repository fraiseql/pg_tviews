#!/usr/bin/env bash
# The documentation states no guarantee the code no longer gives: backing views are
# owner-only (no grant on the table reaches them), value-lock tags are keyed with a
# server secret, and some pg_tviews locks do block readers. The security docs state
# the control file's superuser flag, and name no placeholder contact (E1-11).
set -euo pipefail
cd "$(dirname "$0")/.."

stale=(
  "follow the table's grants"
  "privileges follow the table"
  "backing view privileges follow"
  "computable without a server"
  "Readers are never blocked"
  "your-org/"
  "your-domain"
)

bad=""
for phrase in "${stale[@]}"; do
  if hits=$(grep -rniF -- "$phrase" docs README.md SECURITY.md); then
    bad+="$hits"$'\n'
  fi
done

flag=$(sed -nE 's/^superuser[[:space:]]*=[[:space:]]*(true|false).*/\1/p' pg_tviews.control)
if ! grep -qF "\`pg_tviews.control\` sets \`superuser = $flag\`" docs/security/architecture.md; then
  bad+="docs/security/architecture.md: does not say \`pg_tviews.control\` sets \`superuser = $flag\`"$'\n'
fi

if [[ -n "$bad" ]]; then
  echo "Stale documentation claims:"
  printf '%s' "$bad"
  exit 1
fi
