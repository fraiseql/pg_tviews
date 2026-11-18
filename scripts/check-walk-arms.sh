#!/usr/bin/env bash
# No silent fallback in the lineage walker: every catch-all under
# src/lineage/walk/ says what it does with what it does not recognise.
#
# A fallback is a `_ =>` or `Some(_) =>` arm, a `let … else`, or an
# `unwrap_or(…)` / `unwrap_or_default()` that stands in for a value the walk
# did not find. Each carries one tag, on its own line or in the comment block
# right above it (for a `let … else`, on the first line inside the `else`):
#
#   // walk: R              it refuses the definition
#   // walk: U              it makes what it reads unknown (opaque, a hole, a
#                           recompute): no column skip and no patch rests on it
#   // walk: leaf(<id>)     a node with nothing of its own to record, whose
#                           Vars the generic walk still visits; <id> is listed
#                           under [leaf.<id>] in docs/reference/dialect.toml
#   // walk: n/a(<reason>)  not query content (a catalog lookup, a malformed
#                           tree PostgreSQL never builds, the walker's own
#                           recursion)
#
# Usage: scripts/check-walk-arms.sh [dir]   (default src/lineage/walk)
set -euo pipefail

dir=${1:-src/lineage/walk}
toml=docs/reference/dialect.toml
status=0

listed=""
[[ -f "$toml" ]] && listed=$(sed -n 's/^\[leaf\.\([a-z0-9-]*\)\]$/\1/p' "$toml" | sort -u)

for f in "$dir"/*.rs; do
  report=$(awk -v file="$f" -v listed="$listed" '
    BEGIN { n = split(listed, ids, "\n"); for (i = 1; i <= n; i++) known[ids[i]] = 1 }
    { line[NR] = $0 }
    END {
      for (i = 1; i <= NR; i++) {
        l = line[i]
        site = ""
        if (l ~ /^[ \t]*(_|Some\(_\))([ \t]+if[ \t].*)?[ \t]*=>/) site = "arm"
        else if (l !~ /^[ \t]*\/\// && l !~ /^[ \t]*\}[ \t]*else/ && (l ~ /else[ \t]*\{[ \t]*$/ || (l ~ /^[ \t]*let[ \t].*[ \t]else[ \t]*\{/ && l !~ /=[ \t]*if[ \t]/))) site = "let-else"
        else if (l ~ /\.unwrap_or(_default)?\(/ && l !~ /^[ \t]*\/\//) site = "unwrap"
        if (site == "") continue
        tag = ""
        if (match(l, /\/\/ walk: .*/)) tag = substr(l, RSTART)
        if (tag == "" && site == "let-else" && line[i + 1] ~ /^[ \t]*\/\/ walk: /) tag = line[i + 1]
        if (tag == "") {
          for (j = i - 1; j > 0 && line[j] ~ /^[ \t]*\/\//; j--)
            if (line[j] ~ /^[ \t]*\/\/ walk: /) { tag = line[j]; break }
        }
        sub(/^[ \t]*/, "", tag)
        if (tag == "") { printf "%s:%d: untagged %s\n", file, i, site; continue }
        if (tag ~ /^\/\/ walk: (R|U)([ \t]|$)/ || tag ~ /^\/\/ walk: n\/a\(./) continue
        if (match(tag, /^\/\/ walk: leaf\([a-z0-9-]+\)/)) {
          id = substr(tag, 15, RLENGTH - 15)
          if (!(id in known)) printf "%s:%d: leaf(%s) is not listed in dialect.toml\n", file, i, id
          continue
        }
        printf "%s:%d: malformed tag: %s\n", file, i, tag
      }
    }' "$f")
  if [[ -n "$report" ]]; then
    echo "$report"
    status=1
  fi
done

# Every listed leaf is used somewhere.
for id in $listed; do
  if ! grep -rqF "// walk: leaf($id)" "$dir"; then
    echo "$toml: [leaf.$id] is tagged nowhere under $dir"
    status=1
  fi
done

if [[ $status -ne 0 ]]; then
  echo "check-walk-arms: tag each fallback (see the header of $0)" >&2
fi
exit $status
