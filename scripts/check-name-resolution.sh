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
# pg_catalog cannot fail. Comments do not count. test/scripts/check-name-resolution-test.sh
# runs it on a fixture of every form it must flag.
set -u
cd "$(dirname "$0")/.."

# Allowed sites, `file|text in the string literal|reason`: DDL, restore and
# install time, which run as the role doing them, and the extension's own catalog.
allowed=(
  "src/catalog/plan.rs|SELECT pg_catalog.to_regclass(t.q)::pg_catalog.oid|restore: runs as the restorer"
  "src/ddl/replace/options.rs|pg_catalog.to_regclass(\$1)|options: DDL, names the creator passed"
  "src/ddl/replace/options.rs|pg_catalog.to_regprocedure(\$1)|options: DDL, names the creator passed"
  "src/ddl/replace/rebuild.rs|c.oid IN (\$1::pg_catalog.regclass, \$2::pg_catalog.regclass)|rebuild: DDL, the TVIEW being replaced"
  "src/ddl/replace/mod.rs|SELECT \$1::pg_catalog.regclass::pg_catalog.oid,|replace: DDL, own temp view"
  "src/ddl/replace/mod.rs|'pg_temp.pg_tviews_candidate'::pg_catalog.regclass|replace: DDL, own temp view"
  "src/ddl/mod.rs|pg_catalog.to_regclass(\$1) IS NOT NULL|DDL: the table the creator names"
  "src/ddl/create/select.rs|pg_catalog.to_regtype(\$1)|create: DDL, a type the definition names"
  "src/catalog/queries.rs|pg_catalog.to_regclass(\$1) IS NOT NULL|the extension's own catalog table"
  "src/catalog/queries.rs|'{}'::pg_catalog.regnamespace|the extension's own schema"
  "src/dependency/triggers.rs|'{schema}'::pg_catalog.regnamespace|the extension's own schema"
  "src/lineage/analyze.rs|CREATE INDEX ON {} USING {}|advice in a notice, never run"
  "src/hooks/ctas.rs|{}.{}|DDL: the name the CREATE TABLE AS statement wrote"
  "src/publish.rs|p.\"id\"::pg_catalog.text FROM {} p|parent_table: named from pg_class by OID at load; renames clear the memo"
  "src/catalog/collections.rs|(SELECT p.\"id\"::pg_catalog.text FROM {} p|parent_table: named from pg_class by OID at load; renames clear the memo"
)

# The SQL is in Rust string literals: each is read whole (a `\` continuation
# joins its lines; raw strings too), outside comments, and every form below is
# looked for in it:
#   to_regclass(…) and the other to_reg*() functions;
#   a cast to a reg* type, `::regclass`, `::pg_catalog."regclass"`, `::regclass[]`,
#     `CAST(… AS regclass)`, unless what is cast is an OID: a column (`c.oid`,
#     `o.tgrelid`), a value cast to oid first (`$1::pg_catalog.oid`), or a literal
#     in pg_catalog;
#   the call form `pg_catalog.regclass($1)`;
#   the input functions `regclassin(…)` and the other reg*in();
#   a text literal or a format placeholder passed where a function takes a
#     regclass or another object's name (`pg_relation_size('…')`, `nextval('…')`,
#     `has_table_privilege(…)`, `pg_partition_root('…')`, `pg_get_viewdef('…')`,
#     `has_function_privilege(…)`);
#   a name stored in the catalog or a plan (`mapping.table`, `meta.source_table`,
#     a collection's `parent_table`) reaching a format!() whose string is SQL:
#     as an argument, as an inline capture (`{table}`) bound to one, through a
#     `concat!()` format string, or composed by a string of placeholders only
#     (`format!("{} {}", head, mapping.table)`).
bad=$(ROOT="${NAME_CHECK_ROOT:-src}" python3 - "${allowed[@]}" <<'EOF'
import os, re, sys

REG = r'reg(?:class|proc|procedure|type|oper|operator|namespace|role|collation|config|dictionary)'
SCHEMA = r'(?:"?pg_catalog"?\s*\.\s*)?'
TYPE = SCHEMA + r'"?' + REG + r'"?(?:\s*\[\s*\])?'
CAST = re.compile(r'::\s*' + TYPE + r'(?![a-z0-9_])', re.I)
CAST_AS = re.compile(r'\bCAST\s*\(\s*(.+?)\s+AS\s+' + TYPE + r'(?![a-z0-9_])', re.I)
TO_REG = re.compile(r'\bto_' + REG + r'\s*\(', re.I)
CALL = re.compile(r'(?<![:\w"])' + SCHEMA + r'"?' + REG + r'(?:in)?"?\s*\(', re.I)
TAKES_REGCLASS = (r'pg_relation_size|pg_total_relation_size|pg_table_size|pg_indexes_size|'
                  r'pg_relation_filenode|pg_relation_filepath|nextval|currval|setval|'
                  r'has_table_privilege|has_column_privilege|has_any_column_privilege|'
                  r'has_sequence_privilege|pg_get_serial_sequence|pg_partition_root|'
                  r'pg_partition_tree|pg_partition_ancestors|pg_get_viewdef|'
                  r'has_function_privilege|has_type_privilege')
# The table is the first argument, or the second of three (a role first).
NAME_ARG = re.compile(r'\b(?:' + TAKES_REGCLASS + r')\s*\(\s*(?:[^,()\']*,\s*(?=[\'{](?:[^,()]|\([^()]*\))*,))?[\'{]',
                      re.I)
# What may be cast to a reg* type: an OID column, a value cast to oid, a literal
# in pg_catalog.
OID_COLUMN = re.compile(r'(?<![\w$.\'"{}])[a-z_][a-z0-9_]*\s*\.\s*(?:[a-z_]*(?:oid|relid|objid)|inhparent|amproc|rel)\s*$', re.I)
OID_CAST = re.compile(r'::\s*(?:pg_catalog\s*\.\s*)?oid(?:\s*\[\s*\])?\s*$', re.I)
CATALOG_LITERAL = re.compile(r"'pg_catalog(?:\.[a-z0-9_]+)?'\s*$", re.I)

def oid_operand(before):
    return any(p.search(before) for p in (OID_COLUMN, OID_CAST, CATALOG_LITERAL))

def literals(text):
    """(line, content, start, end) of each string literal outside comments."""
    i, n, line = 0, len(text), 1
    while i < n:
        c = text[i]
        if c == '\n':
            line += 1; i += 1
        elif text.startswith('//', i):
            j = text.find('\n', i); i = n if j < 0 else j
        elif text.startswith('/*', i):
            j = text.find('*/', i + 2); j = n if j < 0 else j + 2
            line += text.count('\n', i, j); i = j
        elif c == "'" and re.match(r"'(\\.|[^\\'])'", text[i:]):
            i += len(re.match(r"'(\\.|[^\\'])'", text[i:]).group(0))
        elif re.match(r'b?r#*"', text[i:]) and (i == 0 or not (text[i-1].isalnum() or text[i-1] == '_')):
            m = re.match(r'b?r(#*)"', text[i:]); end = '"' + m.group(1)
            j = text.find(end, i + len(m.group(0))); j = n if j < 0 else j
            yield line, text[i + len(m.group(0)):j], i, j + len(end)
            line += text.count('\n', i, j); i = j + len(end)
        elif c == '"' or (c == 'b' and text.startswith('b"', i)):
            j = i + (2 if c == 'b' else 1); out = []
            while j < n and text[j] != '"':
                if text[j] == '\\' and j + 1 < n:
                    if text[j + 1] == '\n':
                        j += 2
                        while j < n and text[j] in ' \t\n':
                            j += 1
                        continue
                    out.append(text[j:j + 2]); j += 2
                else:
                    out.append(text[j]); j += 1
            yield line, ''.join(out).replace('\\"', '"'), i, j + 1
            line += text.count('\n', i, j); i = j + 1
        else:
            i += 1

# A name stored in the catalog or a plan (`mapping.table`, `meta.source_table`,
# `x.relname`) names whatever holds that name now: SQL gets the relation's OID
# (`{r:relid}`), never the stored name.
STORED_NAME = re.compile(r'\.\s*(?:table|source_table|parent_table|relname|tview_name|qualified\s*\()(?![\w])', re.I)
SQL_LIKE = re.compile(r'\b(?:SELECT|FROM|JOIN|UPDATE|INSERT|DELETE|EXPLAIN|TRUNCATE|LOCK|ALTER|CREATE|DROP)\b')
FORMAT = re.compile(r'\b(?:format|write|writeln)!\s*\(\s*(?:[\w.&]+\s*,\s*)?$')
# A literal inside `format!(concat!(…), …)`: the format string is the concatenation.
CONCAT = re.compile(r'\b(?:format|write|writeln)!\s*\(\s*(?:[\w.&]+\s*,\s*)?concat!\s*\(\s*(?:"(?:\\.|[^"\\])*"\s*,\s*)*$')
# An inline capture, `{name}` or `{name:?}`, and where it is bound.
CAPTURE = re.compile(r'(?<!\{)\{([a-z_][a-z0-9_]*)(?::[^{}]*)?\}', re.I)
WORD = re.compile(r'[A-Za-z]{3,}')

def format_args(text, end):
    """The arguments after a format string literal ending at `end`, to the call's ')'."""
    depth, i, n = 0, end, len(text)
    while i < n:
        c = text[i]
        if c == '"':
            j = i + 1
            while j < n and text[j] != '"':
                j += 2 if text[j] == '\\' else 1
            i = j + 1
            continue
        if c in '([{':
            depth += 1
        elif c in ')]}':
            if depth == 0:
                return text[end:i]
            depth -= 1
        i += 1
    return text[end:]

def binding(text, start, name):
    """The right-hand side of the nearest `let name = …;` before `start`."""
    found = None
    for m in re.finditer(r'\blet\s+(?:mut\s+)?' + re.escape(name) + r'\s*(?::[^=;]+)?=\s*([^;]*);',
                         text[max(start - 4000, 0):start]):
        found = m.group(1)
    return found

def stored_names(text, start, end, sql):
    """A stored name reaching a format!() string that is SQL: passed as an argument,
    bound to an inline capture, through a concat!() format string, or composed
    into SQL by a string that holds only placeholders."""
    before = text[max(start - 400, 0):start]
    if FORMAT.search(before[-80:]):
        args_from = end
    elif CONCAT.search(before):
        # The arguments follow the concat!(): skip to its closing parenthesis.
        rest = format_args(text, end)
        args_from = end + len(rest) + 1
        sql = sql + ' ' + ' '.join(m.group(1) for m in re.finditer(r'"((?:\\.|[^"\\])*)"', rest))
    else:
        return
    composed = not WORD.search(CAPTURE.sub('', re.sub(r'\{[^{}]*\}', '', sql)))
    if not SQL_LIKE.search(sql) and not composed:
        return
    m = STORED_NAME.search(format_args(text, args_from))
    if m:
        yield 'stored name ' + m.group(0).strip()
        return
    for c in CAPTURE.finditer(sql):
        rhs = binding(text, start, c.group(1))
        if rhs is not None and STORED_NAME.search(rhs):
            yield 'stored name {' + c.group(1) + '} = ' + rhs.strip()
            return

def hits(sql):
    for m in TO_REG.finditer(sql):
        yield m.group(0)
    for m in CAST.finditer(sql):
        if not oid_operand(sql[:m.start()]):
            start = max(sql.rfind(' ', 0, m.start()), 0)
            yield sql[start:m.end()].strip()
    for m in CAST_AS.finditer(sql):
        if not oid_operand(m.group(1)):
            yield m.group(0)
    for m in CALL.finditer(sql):
        if not TO_REG.match(sql, max(m.start() - 3, 0)) and not sql[:m.start()].rstrip().lower().endswith('to_'):
            yield m.group(0)
    for m in NAME_ARG.finditer(sql):
        yield m.group(0)

allowed = [entry.split('|')[:2] for entry in sys.argv[1:]]
used = set()
for dirpath, _, files in os.walk(os.environ['ROOT']):
    for name in sorted(files):
        if not name.endswith('.rs'):
            continue
        path = os.path.join(dirpath, name)
        text = open(path, encoding='utf-8').read()
        for line, sql, start, end in literals(text):
            for hit in [*hits(sql), *stored_names(text, start, end, sql)]:
                site = next((i for i, (f, t) in enumerate(allowed)
                             if path == f and t in sql
                             and (hit in t or t in hit or hit.startswith('stored name'))), None)
                if site is not None:
                    used.add(site)
                    continue
                print(f'{path}:{line}: {hit}  in  "{" ".join(sql.split())[:160]}"')
                break
if os.environ['ROOT'] == 'src':
    for i, (f, t) in enumerate(allowed):
        if i not in used:
            print(f'{f}: allowed site matches nothing, remove it: {t}')
EOF
)
if [[ -n "$bad" ]]; then
  echo "SQL resolving a name to an object (match by OID, or through pg_class/pg_namespace; see the header of $0):"
  printf '%s\n' "$bad"
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
  FNR == 1 { if (name != "" && !set) print file ": " name; name = ""; file = FILENAME }
  /CREATE FUNCTION/ { if (name != "" && !set) print FILENAME ": " name; match($0, /@extschema@\.[a-z_]+/)
                      name = substr($0, RSTART + 12, RLENGTH - 12); set = 0; body = 0 }
  name != "" && /SET search_path = pg_catalog/ && !body { set = 1 }
  name != "" && /^AS / { body = 1 }
  END { if (name != "" && !set) print FILENAME ": " name }' sql/install/*.sql \
  | grep -vE ': (pg_tviews_catalog_revision|contract_version)$')
if [[ -n "${unpinned//$'\n'/}" ]]; then
  echo "entry points that do not pin search_path before running SQL (see part 2 of $0):"
  grep -v '^$' <<<"$unpinned"
  exit 1
fi
echo "every entry point pins search_path"
