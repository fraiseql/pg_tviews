#!/usr/bin/env bash
# Open the next release right after one is tagged
# (docs/development/extension-versioning.md, "The version is the release"):
#
#   - Cargo.toml's version becomes <next>;
#   - sql/pg_tviews--<released>--<next>.sql is created, holding only a header and
#     the closing re-derivation; the previous update script is deleted (a release
#     updates only from the one before it, docs/operations/upgrade.md);
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
for old in sql/pg_tviews--*--*.sql; do
  [[ "$old" == "$script" ]] || git rm -q "$old"
done
[[ -e "$script" ]] || cat >"$script" <<EOF
-- pg_tviews $released → $next
--
-- Pending upgrade script: a pull request that changes the extension SQL adds its
-- statements above the re-derivation (docs/development/extension-versioning.md).
-- Released scripts are never edited.

-- Every TVIEW is re-derived inside the update, as its owner. The update does not
-- depend on what a TVIEW's owner chose: a TVIEW that cannot be re-derived keeps
-- needs_reregister (its refreshes skip and patch nothing) and is named in a
-- WARNING; the health check lists it (docs/operations/upgrade.md). A TVIEW whose
-- backing view stays locked past the session's lock_timeout (10 s when it set
-- none) is left so too: a lock never holds the update.
UPDATE @extschema@.pg_tview_meta SET needs_reregister = true;
DO \$reregister\$
DECLARE
    left_behind TEXT[];
    session_lock_timeout TEXT := pg_catalog.current_setting('lock_timeout');
BEGIN
    IF session_lock_timeout = '0' THEN
        PERFORM pg_catalog.set_config('lock_timeout', '10s', true);
    END IF;
    SELECT pg_catalog.array_agg(pg_catalog.format('%s: %s', m.table_oid::pg_catalog.regclass,
                                                  pg_catalog.split_part(r.status, E'\\n', 1))
                                ORDER BY r.entity)
      INTO left_behind
      FROM @extschema@.pg_tviews_reregister_all(strict => false) r
      JOIN @extschema@.pg_tview_meta m ON m.entity = r.entity
     WHERE r.status <> 'reregistered';
    PERFORM pg_catalog.set_config('lock_timeout', session_lock_timeout, true);
    IF left_behind IS NOT NULL THEN
        RAISE WARNING 'pg_tviews: % TVIEW(s) could not be re-derived and need re-registration: %',
            pg_catalog.cardinality(left_behind), pg_catalog.array_to_string(left_behind, '; ')
            USING HINT = 'Fix each and run SELECT @extschema@.pg_tviews_reregister(''<TVIEW>''); '
                         'until then its refreshes skip and patch nothing, and fail where its '
                         'definition no longer holds. @extschema@.pg_tviews_check_health() '
                         'lists them (component reregister).';
    END IF;
END
\$reregister\$;
EOF

# The badge escapes '-' as '--' (shields.io).
badge="${released//-/--}"
sed -i -E "s#(img\.shields\.io/badge/version-)[^)]*(-orange\.svg)#\1${badge}\2#" README.md
sed -i -E "s#^\*\*Current Version\*\*: \`[^\`]*\` \([^)]*\)#**Current Version**: \`$released\` ($(date +'%B %Y'))#" README.md

echo "Cargo.toml: $released -> $next; created $script (older scripts removed); README names $released."
git --no-pager diff --stat
