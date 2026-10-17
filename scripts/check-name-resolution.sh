#!/usr/bin/env bash
# No SQL in src/ resolves a name to an object, outside the sites listed below.
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
pattern="to_${reg}[[:space:]]*\(|(\\\$[0-9]+|'[^']*'|\{[a-z_]*\})[[:space:]]*::[[:space:]]*(pg_catalog\.)?${reg}\b"
bad=""
while IFS= read -r hit; do
  file=${hit%%:*}
  line=${hit#*:}; line=${line#*:}
  [[ "$line" =~ ^[[:space:]]*// ]] && continue
  # Every match on the line must be a pg_catalog literal or an allowed site.
  rest=$(grep -oE "$pattern" <<<"$line" | grep -vE "^'pg_catalog(\.[a-z0-9_]+)?'")
  [[ -z "$rest" ]] && continue
  ok=""
  for entry in "${allowed[@]}"; do
    IFS='|' read -r afile atext _ <<<"$entry"
    [[ "$file" == "$afile" && "$line" == *"$atext"* ]] && { ok=1; break; }
  done
  [[ -z "$ok" ]] && bad+="$hit"$'\n'
done < <(grep -rnE "$pattern" src --include='*.rs')

if [[ -n "$bad" ]]; then
  echo "SQL resolving a name to an object (match by OID, or through pg_class/pg_namespace; see the header of $0):"
  printf '%s' "$bad"
  exit 1
fi
echo "no SQL resolves an object name outside the allowed DDL, restore and install sites"
