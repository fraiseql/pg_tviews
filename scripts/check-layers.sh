#!/usr/bin/env bash
# The modules a write's refresh runs through only call downward: the catalog,
# the caches, the refresh paths, the queue and the UNLOGGED lifecycle never name
# the flush or anything above it, and only the catalog module names its table
# (#212). Comments do not count.
set -u
cd "$(dirname "$0")/.."
lower=(catalog cache concurrency refresh propagate queue lifecycle lineage utils config error stats)
upper='flush|admin|ddl|hooks|api|trigger|replication|report|delta|rebuild_worker|suspend'
bad=""
for module in "${lower[@]}"; do
  if [[ -d "src/$module" ]]; then paths=("src/$module"); else paths=("src/$module.rs"); fi
  found=$(grep -rnE "crate::($upper)\b" "${paths[@]}" | grep -vE '^[^:]+:[0-9]+:[[:space:]]*//')
  [[ -n "$found" ]] && bad+="$found"$'\n'
done
if [[ -n "$bad" ]]; then
  echo "lower modules naming the flush or a module above it:"
  printf '%s' "$bad"
  exit 1
fi
# Only the catalog module (and the install script) names the catalog table; the
# setting descriptions in src/config/mod.rs mention it as prose.
meta=$(grep -rnE 'meta_table\(\)|pg_tview_meta' src --include='*.rs' \
  | grep -vE '^src/(catalog/|install_sql\.rs:|config/mod\.rs:)' \
  | grep -vE '^[^:]+:[0-9]+:[[:space:]]*//')
if [[ -n "$meta" ]]; then
  echo "pg_tview_meta named outside src/catalog/:"
  printf '%s\n' "$meta"
  exit 1
fi
echo "the catalog, caches, refresh paths, queue and lifecycle only call downward"
echo "only the catalog module names pg_tview_meta"
