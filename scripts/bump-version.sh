#!/usr/bin/env bash
# Open the next release right after one is tagged
# (docs/development/extension-versioning.md, "The version is the release"):
#
#   - Cargo.toml's version becomes <next>;
#   - sql/pg_tviews--<released>--<next>.sql is created, holding only a header;
#   - README.md's version badge and "Current Version" line name <released>, the
#     release just tagged.
#
# It commits and tags nothing: review the diff and commit it.
#
# Usage: scripts/bump-version.sh <next-version>     e.g. 0.1.0-beta.28
set -euo pipefail

next="${1:?usage: $0 <next-version>}"
cd "$(dirname "$0")/.."

released="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
[[ -n "$released" ]] || { echo "no version in Cargo.toml"; exit 1; }
[[ "$next" != "$released" ]] || { echo "Cargo.toml is already at $next"; exit 1; }
git rev-parse -q --verify "refs/tags/v$released" >/dev/null \
  || { echo "v$released is not tagged: tag the release before opening the next one"; exit 1; }

sed -i "0,/^version = \".*\"/s//version = \"$next\"/" Cargo.toml

script="sql/pg_tviews--$released--$next.sql"
[[ -e "$script" ]] || cat >"$script" <<EOF
-- pg_tviews $released -> $next
-- Upgrade script: statements that bring an install of $released to $next.
EOF

# The badge escapes '-' as '--' (shields.io).
badge="${released//-/--}"
sed -i -E "s#(img\.shields\.io/badge/version-)[^)]*(-orange\.svg)#\1${badge}\2#" README.md
sed -i -E "s#^\*\*Current Version\*\*: \`[^\`]*\` \([^)]*\)#**Current Version**: \`$released\` ($(date +'%B %Y'))#" README.md

echo "Cargo.toml: $released -> $next; created $script; README names $released."
git --no-pager diff --stat
