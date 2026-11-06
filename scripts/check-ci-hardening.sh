#!/usr/bin/env bash
# CI builds what was tested, with least privilege: the dependency tree is locked
# and tracked, every workflow's token defaults to read-only, GH_TOKEN reaches
# only the steps that call gh, every pinned reference is a commit, and the
# release checkout keeps no credentials.
set -u
cd "$(dirname "$0")/.."
bad=""
fail() { bad+="$1"$'\n'; }

git ls-files --error-unmatch Cargo.lock >/dev/null 2>&1 || fail "Cargo.lock is not tracked"
cargo metadata --locked --format-version 1 >/dev/null 2>&1 \
  || fail "Cargo.lock does not match Cargo.toml (cargo metadata --locked)"

for wf in .github/workflows/*.yml; do
  grep -qE '^permissions:' "$wf" || fail "$wf: no top-level permissions"
  # The workflow-level env block: from `env:` at column 0 to the next top-level key.
  if awk '/^env:/{e=1; next} /^[^[:space:]#]/{e=0} e' "$wf" | grep -qE '^[[:space:]]+GH_TOKEN:'; then
    fail "$wf: GH_TOKEN in the workflow-level env"
  fi
done

while IFS= read -r line; do
  value=$(sed -E 's/^[^:]+:[0-9]+:[[:space:]]*[A-Z0-9_]+_REF:[[:space:]]*"?([^"[:space:]]*)"?.*/\1/' <<<"$line")
  [[ "$value" == '${{'* || "$value" =~ ^[0-9a-f]{40}$ ]] || fail "${line%%:*}: ${line#*:} is not a commit SHA"
done < <(grep -nE '^[[:space:]]*[A-Z0-9_]+_REF:' .github/workflows/*.yml .github/actions/*/action.yml)

while IFS= read -r at; do
  fail "$at: actions/checkout without persist-credentials: false"
done < <(awk '/uses: actions\/checkout@/{c=1; n=NR; next}
              c && /persist-credentials: false/{c=0}
              c && /^[[:space:]]*- (name|uses|run):/{print FILENAME ":" n; c=0}
              END{if (c) print FILENAME ":" n}' .github/workflows/release.yml)

if [[ -n "$bad" ]]; then
  echo "CI hardening:"
  printf '%s' "$bad"
  exit 1
fi
