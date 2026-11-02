#!/usr/bin/env bash
# A stored plan (pg_tview_meta.plan) is read in one place per language: in Rust
# through the typed TviewPlan (src/lineage/plan.rs), in the extension's SQL through
# the plan decoder functions (the `plan_decoder` block of src/install_sql.rs). No
# other code reads a plan's jsonb by path (`plan->'tables'`, `plan #> …`), nor
# decodes a fragment of it apart. Comments do not count.
set -u
cd "$(dirname "$0")/.."
decoder_start=$(grep -n 'CREATE FUNCTION @extschema@.pg_tviews_plan_tables' src/install_sql.rs | cut -d: -f1)
decoder_end=$(grep -n 'name = "plan_decoder"' src/install_sql.rs | cut -d: -f1)
found=$(grep -rnE "\bplan[[:space:]]*(->>?|#>>?)|->[[:space:]]*'(tables|paths|embeds|references|direct)'" \
          src --include='*.rs' \
  | grep -vE '^[^:]+:[0-9]+:[[:space:]]*(//|--)' \
  | awk -F: -v s="$decoder_start" -v e="$decoder_end" \
        '!($1 == "src/install_sql.rs" && $2 >= s && $2 <= e)')
if [[ -n "$found" ]]; then
  echo "plan jsonb read outside the plan decoders (use TviewPlan in Rust, pg_tviews_plan_* in SQL):"
  printf '%s\n' "$found"
  exit 1
fi
echo "plans are read through TviewPlan and the plan decoder functions only"
