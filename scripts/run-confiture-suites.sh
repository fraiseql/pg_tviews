#!/usr/bin/env bash
# Run confiture's TVIEW suites against the pg_tviews this server carries, as
# confiture's own `pg-tviews` CI leg does (.github/workflows/confiture.yml runs it).
#
# Before pytest, it proves the suites can see the extension: they skip without it,
# and their unit files pass without a server, so a run against a server missing
# pg_tviews would otherwise be green. After pytest, it fails a run that skipped any
# test: every skip in these files names something the server lacks.
#
# Usage: CONFITURE_TEST_DB_URL=postgresql://... scripts/run-confiture-suites.sh <confiture checkout>
set -euo pipefail

confiture="$(cd "${1:?usage: $0 <confiture checkout>}" && pwd)"
url="${CONFITURE_TEST_DB_URL:?set CONFITURE_TEST_DB_URL to the test database}"
report="$(mktemp --suffix=.xml)"
trap 'rm -f "$report"' EXIT

# The files confiture's `pg-tviews` leg runs (quality-gate.yml at the pinned ref).
# A list confiture publishes, when it has one, wins over this copy.
suites=(
  tests/integration/test_tview_live.py
  tests/integration/test_tview_indexes_live.py
  tests/integration/test_tview_diff.py
  tests/integration/test_diff_from_a_database.py
  tests/integration/test_drift_agrees_with_diff.py
  tests/integration/test_tview_migrate.py
  tests/integration/test_tview_restore.py
  tests/integration/test_generated_examples_apply.py
  tests/unit/test_tview_model.py
  tests/unit/test_tview_ddl.py
  tests/unit/test_tview_indexes.py
  tests/unit/test_drift_tviews.py
  tests/unit/test_pg_tviews_contract.py
  tests/unit/test_pg_tviews_registry.py
)
published="$confiture/ci/pg-tviews/suites.txt"
if [[ -f "$published" ]]; then
  mapfile -t suites < <(grep -v -e '^#' -e '^[[:space:]]*$' "$published")
fi

cd "$confiture"

uv run python - "$url" <<'EOF'
import shutil
import subprocess
import sys

import psycopg

from confiture.core import live_catalog

url = sys.argv[1]
with psycopg.connect(url, autocommit=True) as conn:
    def held(sql: str) -> bool:
        return conn.execute(sql).fetchone() is not None

    assert held("SELECT 1 FROM pg_available_extensions WHERE name = 'pg_tviews'"), \
        "pg_tviews is not installed on this server"
    assert "pg_tviews" in conn.execute("SHOW shared_preload_libraries").fetchone()[0], \
        "pg_tviews is not in shared_preload_libraries"
    assert held("SELECT 1 FROM pg_available_extensions WHERE name = 'pg_trgm'"), \
        "pg_trgm (contrib) is not installed on this server"
    server = conn.info.server_version // 10000

    conn.execute("CREATE EXTENSION pg_tviews")
    try:
        live_catalog.require_supported_pg_tviews_on(conn)
    finally:
        conn.execute("DROP EXTENSION pg_tviews")

# The restore round trip runs pg_dump and pg_restore, which refuse a newer server.
for tool in ("pg_dump", "pg_restore"):
    path = shutil.which(tool)
    assert path, f"no {tool} on PATH"
    version = subprocess.run([path, "--version"], capture_output=True, text=True, check=True)
    major = int(version.stdout.split()[-1].split(".")[0])
    assert major >= server, f"{path} is PostgreSQL {major}, the server {server}"
    print(version.stdout.strip())
EOF

uv run pytest "${suites[@]}" -q -rs -p no:randomly --junitxml="$report"

uv run python - "$report" <<'EOF'
import sys
import xml.etree.ElementTree as ET

root = ET.parse(sys.argv[1]).getroot()
suite = root if root.tag == "testsuite" else root.find("testsuite")
tests, skipped = int(suite.get("tests")), int(suite.get("skipped"))
failed = int(suite.get("failures")) + int(suite.get("errors"))
passed = tests - skipped - failed
print(f"confiture TVIEW suites: {passed} passed, {skipped} skipped, {failed} failed")
if skipped or not passed:
    sys.exit("a TVIEW suite skipped: the server lacks what it needs (reasons above)")
EOF
