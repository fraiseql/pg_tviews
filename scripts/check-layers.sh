#!/usr/bin/env bash
# The modules a write's refresh runs through only call downward: the catalog,
# the caches, the refresh paths, the queue and the UNLOGGED lifecycle never name
# the flush or anything above it (#212). Comments do not count.
set -u
cd "$(dirname "$0")/.."
lower=(catalog cache concurrency refresh propagate queue lifecycle lineage utils config error)
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
echo "the catalog, caches, refresh paths, queue and lifecycle only call downward"
