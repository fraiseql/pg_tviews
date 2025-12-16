#!/bin/bash
set -euo pipefail

echo "Testing session pooling mode..."

# Temporarily switch PgBouncer to session mode
sudo sed -i 's/pool_mode = transaction/pool_mode = session/' /etc/pgbouncer/pgbouncer.ini
sudo systemctl reload pgbouncer

export PGHOST=localhost
export PGPORT=6432

# Setup
psql <<EOF
CREATE TABLE IF NOT EXISTS tb_session_test (pk_test SERIAL PRIMARY KEY, data TEXT);
DROP TABLE IF EXISTS tv_session_test CASCADE;
CREATE TABLE tv_session_test AS SELECT pk_test, data FROM tb_session_test;
SELECT pg_tviews_convert_existing_table('tv_session_test');
EOF

echo "Test: Multiple transactions in same session"

# Run multiple transactions in same connection
psql <<EOF
BEGIN;
INSERT INTO tb_session_test (data) VALUES ('session-1');
COMMIT;

BEGIN;
INSERT INTO tb_session_test (data) VALUES ('session-2');
COMMIT;

BEGIN;
INSERT INTO tb_session_test (data) VALUES ('session-3');
COMMIT;
EOF

# Verify all refreshed
ROW_COUNT=$(psql -tAc "SELECT COUNT(*) FROM tv_session_test;")
if [ "$ROW_COUNT" -eq 3 ]; then
    echo "✅ PASS: Session pooling preserves queue state"
else
    echo "❌ FAIL: Expected 3 rows, got $ROW_COUNT"
    exit 1
fi

# Restore transaction pooling
sudo sed -i 's/pool_mode = session/pool_mode = transaction/' /etc/pgbouncer/pgbouncer.ini
sudo systemctl reload pgbouncer

echo "✅ Session pooling tests passed"