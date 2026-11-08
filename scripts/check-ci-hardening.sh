#!/usr/bin/env bash
# CI builds what was tested, with least privilege: the dependency tree is locked
# and tracked, every workflow's token defaults to read-only, GH_TOKEN reaches
# only the steps that call gh, every pinned reference is a commit, the release
# checkout keeps no credentials, and no release job that holds a credential
# compiles anything.
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

while IFS= read -r uses; do
  fail "$uses: not pinned to a commit SHA"
done < <(grep -nE '^[[:space:]]*(- )?uses:' .github/workflows/*.yml .github/actions/*/action.yml \
           | grep -vE 'uses: *\./|@[0-9a-f]{40}( |$)')

# Release jobs that may write, sign or publish (a write or id-token permission)
# check out nothing but the sources and compile nothing: cargo runs there only to
# upload a crate another job verified (cargo publish --no-verify).
while IFS= read -r at; do
  fail "$at"
done < <(awk '
  function check() {
    if (job != "" && cred && build) print FILENAME ": job " job " holds a credential and " build
  }
  /^  [A-Za-z0-9_-]+:[[:space:]]*$/ { check(); job = $1; sub(":", "", job); cred = 0; build = ""; next }
  /^[^[:space:]#]/ { check(); job = "" }
  job != "" && /(contents|id-token|attestations|packages): *write/ { cred = 1 }
  job != "" && /cargo / && !/cargo publish --locked --no-verify/ { build = "runs cargo (line " NR ")" }
  job != "" && /cargo-pgrx|rustup toolchain install|uses: \.\// && !/rustup show/ { build = "builds (line " NR ")" }
  END { check() }' .github/workflows/release.yml)

if [[ -n "$bad" ]]; then
  echo "CI hardening:"
  printf '%s' "$bad"
  exit 1
fi
