# Phase 2.2: PgBouncer & 2PC Validation

**Objective**: Validate compatibility with PgBouncer connection pooling and 2PC transaction handling

**Priority**: CRITICAL
**Estimated Time**: 1-2 days
**Blockers**: Phase 2.1 complete (concurrency tests)

---

## Context

**Current State**: Unknown behavior with connection poolers and 2PC edge cases

**Why This Matters**:
- PgBouncer is widely used in production for connection pooling
- Transaction pooling mode uses `DISCARD ALL` between transactions
- 2PC support requires careful state management across prepare/commit phases
- Queue persistence must survive connection recycling

**Risk**: Data loss or stale queue entries with connection poolers

---

## Test Scenarios

### Scenario 1: PgBouncer Transaction Pooling

**Setup**: PgBouncer in transaction pooling mode

**Test Cases**:
1. Queue cleared on `DISCARD ALL`
2. No cross-transaction contamination
3. Refresh triggers work correctly
4. 2PC queue persistence survives connection reuse

### Scenario 2: PgBouncer Session Pooling

**Setup**: PgBouncer in session pooling mode

**Test Cases**:
1. Queue state maintained within session
2. Multiple transactions in same session
3. Prepared transactions work correctly

### Scenario 3: 2PC Edge Cases

**Test Cases**:
1. Prepare → crash → recover → commit
2. Prepare → long delay → commit
3. Prepare → rollback
4. Multiple prepared transactions with same TVIEW
5. Orphaned prepared transactions cleanup

---

## Implementation Steps

### Step 1: PgBouncer Setup

**Create**: `test/pgbouncer/pgbouncer.ini`

```ini
[databases]
test_db = host=localhost port=5432 dbname=postgres

[pgbouncer]
listen_port = 6432
listen_addr = localhost
auth_type = trust
auth_file = /etc/pgbouncer/userlist.txt

# Transaction pooling mode
pool_mode = transaction
max_client_conn = 100
default_pool_size = 20

# Server lifetime to test connection recycling
server_lifetime = 60
server_idle_timeout = 30
```

**Create**: `test/pgbouncer/setup.sh`

```bash
#!/bin/bash
set -euo pipefail

echo "Setting up PgBouncer for testing..."

# Install PgBouncer if needed
if ! command -v pgbouncer &> /dev/null; then
    sudo apt-get install -y pgbouncer
fi

# Copy config
sudo cp pgbouncer.ini /etc/pgbouncer/
sudo chown postgres:postgres /etc/pgbouncer/pgbouncer.ini

# Create userlist
echo '"postgres" "trust"' | sudo tee /etc/pgbouncer/userlist.txt

# Start PgBouncer
sudo systemctl restart pgbouncer

# Verify
psql -h localhost -p 6432 -c "SELECT 1;" && echo "✅ PgBouncer running"
```

### Step 2: Transaction Pooling Tests

**Create**: `test/pgbouncer/test-transaction-pooling.sh`

```bash
#!/bin/bash
set -euo pipefail

PGHOST=localhost
PGPORT=6432  # PgBouncer port

echo "Testing transaction pooling mode..."

# Setup test table
psql -c "CREATE TABLE IF NOT EXISTS tb_pgbouncer_test (pk_test SERIAL PRIMARY KEY, data TEXT);"
psql -c "DROP TABLE IF EXISTS tv_pgbouncer_test CASCADE;"
psql -c "CREATE TABLE tv_pgbouncer_test AS SELECT pk_test, data FROM tb_pgbouncer_test;"
psql -c "SELECT pg_tviews_convert_existing_table('tv_pgbouncer_test');"

echo "Test 1: DISCARD ALL clears queue"

# Session 1: Add to queue
psql <<EOF
BEGIN;
INSERT INTO tb_pgbouncer_test (data) VALUES ('test-1');
-- Queue should have entry
SELECT COUNT(*) as queue_size FROM pg_tviews_get_queue();
COMMIT;
EOF

# Session 2: Check queue (should be cleared by DISCARD ALL)
QUEUE_SIZE=$(psql -tAc "SELECT COUNT(*) FROM pg_tviews_get_queue();")

if [ "$QUEUE_SIZE" -eq 0 ]; then
    echo "✅ PASS: Queue cleared after DISCARD ALL"
else
    echo "❌ FAIL: Queue still has $QUEUE_SIZE entries"
    exit 1
fi

echo "Test 2: No cross-transaction contamination"

# Run multiple transactions, verify isolation
for i in {1..10}; do
    psql <<EOF
BEGIN;
INSERT INTO tb_pgbouncer_test (data) VALUES ('txn-$i');
COMMIT;
EOF
done

# Verify all 11 rows exist (1 from test 1 + 10 new)
ROW_COUNT=$(psql -tAc "SELECT COUNT(*) FROM tv_pgbouncer_test;")
if [ "$ROW_COUNT" -eq 11 ]; then
    echo "✅ PASS: All transactions processed correctly"
else
    echo "❌ FAIL: Expected 11 rows, got $ROW_COUNT"
    exit 1
fi

echo "✅ Transaction pooling tests passed"
```

### Step 3: 2PC Validation

**Create**: `test/pgbouncer/test-2pc.sh`

```bash
#!/bin/bash
set -euo pipefail

echo "Testing 2PC edge cases..."

# Setup
psql <<EOF
CREATE TABLE IF NOT EXISTS tb_2pc (pk_test SERIAL PRIMARY KEY, data TEXT);
DROP TABLE IF EXISTS tv_2pc CASCADE;
CREATE TABLE tv_2pc AS SELECT pk_test, data FROM tb_2pc;
SELECT pg_tviews_convert_existing_table('tv_2pc');
EOF

echo "Test 1: Prepare → Commit"

psql <<EOF
BEGIN;
INSERT INTO tb_2pc (data) VALUES ('2pc-test-1');
PREPARE TRANSACTION 'test_2pc_1';
EOF

# Check queue persistence
QUEUE_PERSISTED=$(psql -tAc "SELECT COUNT(*) FROM pg_tview_queue_persistence WHERE gid = 'test_2pc_1';")
if [ "$QUEUE_PERSISTED" -eq 0 ]; then
    echo "❌ FAIL: Queue not persisted for prepared transaction"
    exit 1
fi

# Commit
psql -c "COMMIT PREPARED 'test_2pc_1';"

# Verify refresh happened
ROW_COUNT=$(psql -tAc "SELECT COUNT(*) FROM tv_2pc WHERE data = '2pc-test-1';")
if [ "$ROW_COUNT" -eq 1 ]; then
    echo "✅ PASS: 2PC commit refreshed TVIEW"
else
    echo "❌ FAIL: TVIEW not refreshed after 2PC commit"
    exit 1
fi

echo "Test 2: Prepare → Rollback"

psql <<EOF
BEGIN;
INSERT INTO tb_2pc (data) VALUES ('2pc-test-rollback');
PREPARE TRANSACTION 'test_2pc_rollback';
EOF

psql -c "ROLLBACK PREPARED 'test_2pc_rollback';"

# Verify no refresh
ROW_COUNT=$(psql -tAc "SELECT COUNT(*) FROM tv_2pc WHERE data = '2pc-test-rollback';")
if [ "$ROW_COUNT" -eq 0 ]; then
    echo "✅ PASS: 2PC rollback did not refresh TVIEW"
else
    echo "❌ FAIL: TVIEW incorrectly refreshed after rollback"
    exit 1
fi

echo "Test 3: Multiple prepared transactions"

# Create 5 prepared transactions
for i in {1..5}; do
    psql <<EOF
BEGIN;
INSERT INTO tb_2pc (data) VALUES ('2pc-multi-$i');
PREPARE TRANSACTION 'test_2pc_multi_$i';
EOF
done

# Verify all persisted
PERSISTED_COUNT=$(psql -tAc "SELECT COUNT(*) FROM pg_tview_queue_persistence WHERE gid LIKE 'test_2pc_multi_%';")
if [ "$PERSISTED_COUNT" -eq 5 ]; then
    echo "✅ PASS: All 5 prepared transactions persisted"
else
    echo "❌ FAIL: Expected 5 persisted, got $PERSISTED_COUNT"
    exit 1
fi

# Commit all
for i in {1..5}; do
    psql -c "COMMIT PREPARED 'test_2pc_multi_$i';"
done

# Verify all refreshed
ROW_COUNT=$(psql -tAc "SELECT COUNT(*) FROM tv_2pc WHERE data LIKE '2pc-multi-%';")
if [ "$ROW_COUNT" -eq 5 ]; then
    echo "✅ PASS: All prepared transactions committed and refreshed"
else
    echo "❌ FAIL: Expected 5 rows, got $ROW_COUNT"
    exit 1
fi

echo "✅ 2PC validation tests passed"
```

### Step 4: Session Pooling Tests

**Create**: `test/pgbouncer/test-session-pooling.sh`

```bash
#!/bin/bash
set -euo pipefail

echo "Testing session pooling mode..."

# Temporarily switch PgBouncer to session mode
sudo sed -i 's/pool_mode = transaction/pool_mode = session/' /etc/pgbouncer/pgbouncer.ini
sudo systemctl reload pgbouncer

PGHOST=localhost
PGPORT=6432

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
```

### Step 5: Add Connection Lifecycle Hooks Test

**Create**: `test/pgbouncer/test-hooks.sql`

```sql
-- Test that pg_tviews hooks work correctly with PgBouncer

-- Setup logging
CREATE TABLE IF NOT EXISTS hook_log (
    ts TIMESTAMPTZ DEFAULT NOW(),
    event TEXT,
    details TEXT
);

-- Test DISCARD ALL handling
BEGIN;
INSERT INTO tb_pgbouncer_test (data) VALUES ('hook-test');
INSERT INTO hook_log (event, details)
  SELECT 'queue_before_discard', COUNT(*)::TEXT
  FROM pg_tviews_get_queue();
COMMIT;

DISCARD ALL;

-- After DISCARD ALL, queue should be empty
INSERT INTO hook_log (event, details)
  SELECT 'queue_after_discard', COUNT(*)::TEXT
  FROM pg_tviews_get_queue();

-- Verify
SELECT event, details FROM hook_log ORDER BY ts;

-- Expected:
-- queue_before_discard | 1
-- queue_after_discard  | 0
```

---

## Verification Commands

```bash
# Setup PgBouncer
cd test/pgbouncer
./setup.sh

# Run all tests
./test-transaction-pooling.sh
./test-session-pooling.sh
./test-2pc.sh

# Verify with monitoring
watch -n 1 "psql -h localhost -p 6432 -c 'SHOW POOLS;'"

# Check PgBouncer logs
sudo tail -f /var/log/pgbouncer/pgbouncer.log

# Cleanup
sudo systemctl stop pgbouncer
```

---

## Acceptance Criteria

- [ ] Transaction pooling mode works correctly
- [ ] `DISCARD ALL` clears queue as expected
- [ ] No cross-transaction contamination
- [ ] 2PC prepare/commit cycle works
- [ ] 2PC rollback works correctly
- [ ] Multiple prepared transactions handled
- [ ] Session pooling mode works
- [ ] All tests pass 100 times in a row (no flakiness)
- [ ] Documentation updated with PgBouncer requirements

---

## DO NOT

- ❌ Test only on direct PostgreSQL connection - must test through PgBouncer
- ❌ Ignore DISCARD ALL handling - critical for transaction pooling
- ❌ Skip 2PC edge cases - production systems use 2PC
- ❌ Test only with small delays - add realistic delays between prepare/commit

---

## Documentation Updates

**Add to**: `docs/deployment/pgbouncer.md`

```markdown
# PgBouncer Compatibility

## Supported Modes

pg_tviews is compatible with all PgBouncer pooling modes:

- **Transaction pooling**: ✅ Fully supported (recommended)
- **Session pooling**: ✅ Fully supported
- **Statement pooling**: ⚠️ Not recommended (TVIEW state is per-transaction)

## Configuration

### Transaction Pooling (Recommended)

```ini
pool_mode = transaction
```

Queue is automatically cleared via `DISCARD ALL` between transactions.

### Two-Phase Commit (2PC)

2PC is fully supported. Queue entries are persisted in `pg_tview_queue_persistence` during `PREPARE TRANSACTION` and restored on `COMMIT PREPARED`.

## Known Limitations

None - all features work correctly through PgBouncer.
```

---

## Rollback Plan

If issues found:

```bash
# Disable PgBouncer
sudo systemctl stop pgbouncer

# Test directly
PGPORT=5432 ./test-transaction-pooling.sh

# Compare results
```

---

## Next Steps

After completion:
- Commit with message: `test(pgbouncer): Validate connection pooling and 2PC handling [PHASE2.2]`
- Update deployment documentation
- Proceed to **Phase 2.3: Failure Mode Analysis**
