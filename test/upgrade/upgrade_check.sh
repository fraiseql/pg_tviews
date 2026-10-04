#!/usr/bin/env bash
# Check that an install of the previous release, upgraded to the commit under test,
# has exactly the catalog of a fresh install and keeps its TVIEWs working
# (ADR 0136, issue #137). Run in two steps around swapping the installed package:
#
#   test/upgrade/upgrade_check.sh before            # previous release installed
#   ...stop PostgreSQL, install the commit under test, start...
#   test/upgrade/upgrade_check.sh after update       # ALTER EXTENSION pg_tviews UPDATE
#   test/upgrade/upgrade_check.sh after migrate      # scripts/migrate-from-0.1.0.sql
#
# Honors PGHOST/PGPORT/PGUSER. Uses the databases pg_tviews_upgrade and
# pg_tviews_fresh, which it drops and re-creates.

set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
upgraded=pg_tviews_upgrade
fresh=pg_tviews_fresh
fail() { echo "FAIL: $*" >&2; exit 1; }

case "${1:-}" in
before)
    psql -X -d postgres -qc "DROP DATABASE IF EXISTS $upgraded" >/dev/null
    psql -X -d postgres -qc "CREATE DATABASE $upgraded" >/dev/null
    psql -X -d "$upgraded" -q -f "$here/fixtures.sql"
    psql -X -d "$upgraded" -q -f "$here/verify.sql"
    echo "fixtures created with pg_tviews $(psql -X -At -d "$upgraded" -c \
        "SELECT extversion FROM pg_extension WHERE extname = 'pg_tviews'")"
    ;;
after)
    mode="${2:-}"
    # The new library refuses the previous catalog until it is upgraded (when the
    # catalog revision changed; a 0.1.0 catalog always differs).
    if out="$(psql -X -d "$upgraded" -v ON_ERROR_STOP=1 \
            -c "UPDATE tb_user SET name = name WHERE pk_user = 1" 2>&1)"; then
        [[ "$mode" == update ]] || fail "a write succeeded against the 0.1.0 catalog"
        echo "catalog revision unchanged: writes work before ALTER EXTENSION"
    else
        grep -q "catalog revision" <<<"$out" || fail "unexpected write error: $out"
        [[ "$mode" != migrate ]] || grep -q "migrate-from-0.1.0.sql" <<<"$out" \
            || fail "the guard's hint does not name the migration script: $out"
        echo "writes refused until the upgrade: $(grep -m1 ERROR <<<"$out")"
    fi

    case "$mode" in
    migrate)
        psql -X -d "$upgraded" -v ON_ERROR_STOP=1 -f "$root/scripts/migrate-from-0.1.0.sql"
        ;;
    update)
        psql -X -d "$upgraded" -v ON_ERROR_STOP=1 -c "ALTER EXTENSION pg_tviews UPDATE"
        # Until re-registered, TVIEWs keep refreshing with their old metadata.
        psql -X -d "$upgraded" -q -f "$here/verify.sql"
        echo "TVIEWs follow their base tables before re-registration"
        psql -X -d "$upgraded" -v ON_ERROR_STOP=1 \
            -c "SELECT * FROM tviews.pg_tviews_reregister_all(strict => true)"
        # A DISTINCT ON TVIEW is keyed on its DISTINCT ON key (ADR 0169): the
        # unique index on pk_<entity> it had before is gone.
        left="$(psql -X -At -d "$upgraded" -c \
            "SELECT count(*) FROM pg_catalog.pg_class WHERE relkind = 'i' AND relname LIKE 'idx\_tv\_%\_pk\_unique'")"
        [[ "$left" == 0 ]] || fail "$left pk_unique index(es) survived re-registration"
        psql -X -d "$upgraded" -q -f "$here/verify_after.sql"
        ;;
    *) fail "usage: $0 after update|migrate" ;;
    esac

    psql -X -d postgres -qc "DROP DATABASE IF EXISTS $fresh" >/dev/null
    psql -X -d postgres -qc "CREATE DATABASE $fresh" >/dev/null
    psql -X -d "$fresh" -q -v ON_ERROR_STOP=1 \
        -c "SET client_min_messages TO WARNING" \
        -c "CREATE EXTENSION jsonb_delta" -c "CREATE EXTENSION pg_tviews"

    tmp="$(mktemp -d)"
    psql -X -At -d "$upgraded" -f "$here/catalog_snapshot.sql" >"$tmp/upgraded"
    psql -X -At -d "$fresh" -f "$here/catalog_snapshot.sql" >"$tmp/fresh"
    if ! diff -u "$tmp/fresh" "$tmp/upgraded"; then
        fail "the upgraded catalog differs from a fresh install (- fresh, + upgraded)"
    fi
    echo "catalog identical to a fresh install ($(wc -l <"$tmp/fresh") facts)"

    psql -X -d "$upgraded" -q -f "$here/verify.sql"
    echo "TVIEWs follow their base tables after the upgrade"
    ;;
*)
    fail "usage: $0 before | after update|migrate"
    ;;
esac
