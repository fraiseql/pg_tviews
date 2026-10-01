#!/usr/bin/env bash
# pg_tviews pre-upgrade checks. Read-only.
#
#   PGDATABASE=<database> docs/operations/upgrade/scripts/pre-upgrade-checks.sh
#
# Honors the usual libpq variables (PGHOST, PGPORT, PGUSER, PGDATABASE). Exits 1
# when the database cannot be upgraded as it is, 0 otherwise (warnings included).

set -euo pipefail

q() { psql -X -At -v ON_ERROR_STOP=1 -c "$1"; }
problems=0
say() { printf '%-12s %s\n' "$1" "$2"; }

say "database" "$(q "SELECT current_database()")"

# pg_tviews supports PostgreSQL 16, 17 and 18.
version_num="$(q "SELECT current_setting('server_version_num')::int")"
if (( version_num < 160000 )); then
    say "FAIL" "PostgreSQL $(q "SHOW server_version") is not supported: upgrade PostgreSQL to 16 or later first"
    problems=1
else
    say "ok" "PostgreSQL $(q "SHOW server_version")"
fi

installed="$(q "SELECT extversion FROM pg_extension WHERE extname = 'pg_tviews'")"
if [[ -z "$installed" ]]; then
    say "FAIL" "pg_tviews is not installed in this database"
    exit 1
fi
available="$(q "SELECT default_version FROM pg_available_extensions WHERE name = 'pg_tviews'")"
say "ok" "pg_tviews $installed installed, $available available"
if [[ "$installed" == "0.1.0" ]]; then
    say "note" "0.1.0 installs move with scripts/migrate-from-0.1.0.sql, not ALTER EXTENSION UPDATE"
fi

# Problems the health check already sees: fix them before upgrading.
errors="$(q "SELECT count(*) FROM tviews.pg_tviews_health_check() WHERE severity = 'error'")"
if (( errors > 0 )); then
    say "FAIL" "the health check reports $errors error(s):"
    q "SELECT '  ' || component || ': ' || message FROM tviews.pg_tviews_health_check() WHERE severity = 'error'"
    problems=1
else
    say "ok" "health check: no errors"
fi

say "info" "$(q "SELECT count(*) FROM tviews.registry") TVIEW(s) registered, $(q "SELECT count(*) FROM tviews.registry WHERE needs_reregister") waiting for pg_tviews_reregister_all()"

# A prepared transaction keeps its pending refreshes until COMMIT PREPARED.
prepared="$(q "SELECT count(*) FROM pg_prepared_xacts WHERE database = current_database()")"
if (( prepared > 0 )); then
    say "WARN" "$prepared prepared transaction(s): finish them before swapping the library"
fi

say "reminder" "take a backup (pg_dump -Fc) before ALTER EXTENSION pg_tviews UPDATE"
exit "$problems"
