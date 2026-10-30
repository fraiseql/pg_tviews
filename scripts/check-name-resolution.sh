#!/usr/bin/env bash
# 1. No SQL in src/ resolves a name to an object, outside the sites listed below.
# 2. Every entry point pins search_path before it runs SQL (see the second part).
#
# A schema-qualified name lookup (`to_regclass(text)`, `'s.t'::regclass`,
# `$1::regtype` on a text parameter, ...) checks USAGE on the schema and raises
# even inside `to_regclass`. A write loads the catalog rows of every TVIEW, so one
# such lookup on that path makes every write depend on the writer's privileges on
# schemas it never touches (#251). Catalog SQL matches by OID, or by name through
# pg_class/pg_namespace. A cast of an OID (`x.oid::regclass`,
# `$1::pg_catalog.oid::pg_catalog.regclass`) resolves nothing; a literal in
# pg_catalog cannot fail. Comments do not count.
set -u
cd "$(dirname "$0")/.."

# Allowed sites, `file|text on the line|reason`: DDL, restore and install time,
# which run as the role doing them, and the extension's own catalog.
allowed=(
  "src/install_sql.rs|pg_catalog.to_regclass(x.e->>'table')|restore trigger: runs as the restorer"
  "src/install_sql.rs|'pg_temp.pg_tviews_rename_check'::regclass|rename check: DDL, own temp view"
  "src/ddl/replace/options.rs|pg_catalog.to_regclass(\$1)|options: DDL, names the creator passed"
  "src/ddl/replace/options.rs|pg_catalog.to_regprocedure(\$1)|options: DDL, names the creator passed"
  "src/ddl/replace/rebuild.rs|c.oid IN (\$1::pg_catalog.regclass, \$2::pg_catalog.regclass)|rebuild: DDL, the TVIEW being replaced"
  "src/ddl/replace/mod.rs|\"SELECT \$1::pg_catalog.regclass::pg_catalog.oid, \\|replace: DDL, own temp view"
  "src/ddl/replace/mod.rs|\$1::pg_catalog.regclass::pg_catalog.oid, 0\"|replace: DDL, own temp view"
  "src/ddl/replace/mod.rs|'pg_temp.pg_tviews_candidate'::pg_catalog.regclass|replace: DDL, own temp view"
  "src/ddl/mod.rs|pg_catalog.to_regclass(\$1) IS NOT NULL|DDL: the table the creator names"
  "src/ddl/create/select.rs|pg_catalog.to_regtype(\$1)|create: DDL, a type the definition names"
  "src/catalog/queries.rs|pg_catalog.to_regclass(\$1) IS NOT NULL|the extension's own catalog table"
  "src/catalog/queries.rs|pg_catalog.to_regclass('{}')|the extension's own catalog table"
  "src/catalog/queries.rs|'{}'::pg_catalog.regnamespace|the extension's own schema"
  "src/dependency/triggers.rs|'{schema}'::pg_catalog.regnamespace|the extension's own schema"
)

reg='reg(class|proc|procedure|type|oper|operator|namespace|role|collation|config|dictionary)'
# A literal, a format placeholder or a parameter, maybe cast to a text type
# first, cast to reg*; or to_reg*(). Any case.
text_cast='([[:space:]]*::[[:space:]]*(pg_catalog\.)?(text|varchar|name|"char"))?'
pattern="to_${reg}[[:space:]]*\(|(\\\$[0-9]+|'[^']*'|\{[a-z0-9_]*\})${text_cast}[[:space:]]*::[[:space:]]*(pg_catalog\.)?${reg}\b"
bad=""
while IFS= read -r hit; do
  file=${hit%%:*}
  line=${hit#*:}; line=${line#*:}
  [[ "$line" =~ ^[[:space:]]*// ]] && continue
  # Every match on the line must be a pg_catalog literal, or inside an allowed
  # site's text.
  while IFS= read -r match; do
    [[ -z "$match" ]] && continue
    shopt -s nocasematch
    [[ "$match" =~ ^\'pg_catalog(\.[a-z0-9_]+)?\' ]] && { shopt -u nocasematch; continue; }
    shopt -u nocasematch
    ok=""
    for entry in "${allowed[@]}"; do
      IFS='|' read -r afile atext _ <<<"$entry"
      [[ "$file" == "$afile" && "$line" == *"$atext"* && "$atext" == *"$match"* ]] && { ok=1; break; }
    done
    [[ -z "$ok" ]] && { bad+="$hit"$'\n'; break; }
  done < <(grep -oiE "$pattern" <<<"$line")
done < <(grep -rniE "$pattern" src --include='*.rs')

if [[ -n "$bad" ]]; then
  echo "SQL resolving a name to an object (match by OID, or through pg_class/pg_namespace; see the header of $0):"
  printf '%s' "$bad"
  exit 1
fi
echo "no SQL resolves an object name outside the allowed DDL, restore and install sites"

# 2. The catalog SQL of pg_tviews compares values pg_catalog has no exact operator
# for (oid with regclass, oid with an integer literal): an exact one in any schema
# on the search_path would win over pg_catalog's. So every way SQL reaches the
# library pins search_path to `pg_catalog, pg_temp` first (owner::CatalogPath),
# and resolves the names its caller wrote in an owner::CallerPath; the extension's
# own SQL functions set it.
pin='let _path = crate::owner::CatalogPath::pin();'
unpinned=""
# The first statement (after `use` items) of each #[pg_extern] / #[pg_trigger]
# function. pg_tviews_version() runs no SQL.
unpinned+=$(awk -v pin="$pin" '
  FNR == 1 { state = 0 }
  /^#\[pg_(extern|trigger)/ { state = 1; name = ""; next }
  state == 1 && match($0, /^(pub(\([a-z]+\))? )?fn [a-z_0-9]+/) {
    name = substr($0, RSTART, RLENGTH); sub(/.*fn /, "", name)
  }
  state == 1 && /\{$/ && !/^[[:space:]]/ { state = 2; next }
  state == 2 && /^[[:space:]]*use / { next }
  state == 2 {
    if (index($0, pin) == 0 && name != "pg_tviews_version") print FILENAME ": " name "()"
    state = 0
  }' $(grep -rlE '^#\[pg_(extern|trigger)' src))
# Entry points that are not SQL-callable functions: `file|function|text it holds`.
entries=(
  "src/hooks/mod.rs|tview_process_utility_hook|$pin"
  "src/trigger.rs|flush_after_statement|$pin"
  "src/flush/mod.rs|flush_refresh_queue|$pin"
  "src/rebuild_worker.rs|pg_tviews_rebuild_worker_main|set_config('search_path', 'pg_catalog, pg_temp', true)"
)
for entry in "${entries[@]}"; do
  IFS='|' read -r file fn text <<<"$entry"
  awk -v fn="$fn" -v text="$text" '
    $0 ~ "fn " fn "[(<]" { inside = 1 }
    inside && index($0, text) { found = 1 }
    inside && /^}/ { exit }
    END { exit !found }' "$file" || unpinned+=$'\n'"$file: $fn()"
done
# The extension's SQL functions. Constant ones read nothing; the rename check
# resolves a candidate's names as the renamer wrote them, and qualifies its
# operators.
unpinned+=$'\n'$(awk '
  /CREATE FUNCTION/ { if (name != "" && !set) print FILENAME ": " name; match($0, /@extschema@\.[a-z_]+/)
                      name = substr($0, RSTART + 12, RLENGTH - 12); set = 0; body = 0 }
  name != "" && /SET search_path = pg_catalog/ && !body { set = 1 }
  name != "" && /^AS / { body = 1 }
  END { if (name != "" && !set) print FILENAME ": " name }' src/install_sql.rs \
  | grep -vE ': (pg_tviews_catalog_revision|contract_version|pg_tviews_defines_view)$')
if [[ -n "${unpinned//$'\n'/}" ]]; then
  echo "entry points that do not pin search_path before running SQL (see part 2 of $0):"
  grep -v '^$' <<<"$unpinned"
  exit 1
fi
echo "every entry point pins search_path"
