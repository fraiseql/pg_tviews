#!/usr/bin/env bash
# A stored plan (pg_tview_meta.plan) is read in one place per language: in Rust
# through the typed TviewPlan (src/lineage/plan.rs), in the extension's SQL through
# the plan decoder functions (sql/install/04-plan-decoder.sql). No
# other code reads a plan's jsonb by path (`plan->'tables'`, `plan #> …`), nor
# decodes a fragment of it apart. Comments do not count.
set -u
cd "$(dirname "$0")/.."
found=$(grep -rnE "\bplan[[:space:]]*(->>?|#>>?)|->[[:space:]]*'(tables|paths|embeds|references|direct)'" \
          src sql/install --include='*.rs' --include='*.sql' \
  | grep -vE '^[^:]+:[0-9]+:[[:space:]]*(//|--)' \
  | grep -v '^sql/install/04-plan-decoder\.sql:')
if [[ -n "$found" ]]; then
  echo "plan jsonb read outside the plan decoders (use TviewPlan in Rust, pg_tviews_plan_* in SQL):"
  printf '%s\n' "$found"
  exit 1
fi
echo "plans are read through TviewPlan and the plan decoder functions only"
