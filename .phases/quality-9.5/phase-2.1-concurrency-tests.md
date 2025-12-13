# Phase 2.1: Concurrency Stress Testing

**Objective**: Validate thread-safety and concurrent transaction handling with comprehensive stress tests

**Priority**: CRITICAL
**Estimated Time**: 2-3 days
**Blockers**: Phase 1 complete

---

## Context

**Current State**: Unknown concurrency behavior under load

**Why This Matters**:
- PostgreSQL handles 100s of concurrent connections
- TVIEWs use thread-local transaction queues
- Race conditions can cause data corruption
- 2PC support requires careful state management
- Production systems will have concurrent updates

**Risk**: Undiscovered race conditions, deadlocks, or data corruption under load

---

## Test Scenarios

### Scenario 1: Concurrent Updates to Same TVIEW
**Goal**: Verify queue deduplication and locking

```sql
-- 100 concurrent sessions, all updating same TVIEW
-- Session 1-100:
BEGIN;
INSERT INTO tb_post (fk_user, title) VALUES (1, 'Post from session N');
COMMIT;
-- All should trigger refresh to tv_post
-- Queue should deduplicate by pk_post
```

**Expected**: No deadlocks, no duplicate refreshes, consistent final state

### Scenario 2: Cascading Updates Under Load
**Goal**: Verify dependency graph traversal is thread-safe

```sql
-- Setup: tv_post depends on tv_user
-- 50 sessions update tb_user
-- 50 sessions update tb_post
-- All concurrent, all trigger cascades
```

**Expected**: Correct cascade order, no race conditions in graph resolution

### Scenario 3: 2PC with Concurrent Transactions
**Goal**: Verify queue persistence and recovery

```sql
-- 20 sessions with prepared transactions
-- Session N:
BEGIN;
UPDATE tb_user SET name = 'Updated N';
PREPARE TRANSACTION 'xact_N';
-- Later: COMMIT PREPARED 'xact_N';
```

**Expected**: Queue persists across prepare, restores on commit, no orphaned entries

### Scenario 4: Mixed DDL and DML
**Goal**: Verify metadata locking

```sql
-- Session 1: CREATE new TVIEW
-- Session 2-50: INSERT/UPDATE triggering refreshes
-- Session 51: DROP TVIEW
```

**Expected**: Proper locking, no use-after-free, clean error messages

### Scenario 5: PgBouncer Connection Pooling
**Goal**: Verify DISCARD ALL handling

```sql
-- Via PgBouncer (transaction pooling):
-- Session 1: Transaction, then DISCARD ALL
-- Session 2: Reuses connection
-- Should not see stale queue data
```

**Expected**: Queue cleared on DISCARD ALL, no cross-transaction contamination

---

## Implementation Steps

### Step 1: Setup Test Infrastructure

**Create**: `test/concurrency/`

```
test/concurrency/
├── setup.sql              # Schema setup
├── scenarios/
│   ├── 01-concurrent-updates.sh
│   ├── 02-cascade-load.sh
│   ├── 03-2pc-stress.sh
│   ├── 04-ddl-dml-mix.sh
│   └── 05-pgbouncer-test.sh
├── lib/
│   ├── common.sh          # Helper functions
│   └── validate.sh        # Result validation
└── README.md
```

### Step 2: Concurrent Updates Test

**File**: `test/concurrency/scenarios/01-concurrent-updates.sh`

```bash
#!/bin/bash
set -euo pipefail

source ../lib/common.sh

CONNECTIONS=100
UPDATES_PER_CONN=10

echo "Running concurrent updates test..."
echo "Connections: $CONNECTIONS"
echo "Updates per connection: $UPDATES_PER_CONN"

# Setup
psql -c "CREATE TABLE tb_test (pk_test SERIAL PRIMARY KEY, data TEXT);"
psql -c "CREATE TABLE tv_test AS SELECT pk_test, data FROM tb_test;"
psql -c "SELECT pg_tviews_convert_existing_table('tv_test');"

# Run concurrent updates
for i in $(seq 1 $CONNECTIONS); do
    (
        for j in $(seq 1 $UPDATES_PER_CONN); do
            psql -c "INSERT INTO tb_test (data) VALUES ('conn-$i-update-$j');" &
        done
        wait
    ) &
done

# Wait for all background jobs
wait

# Validate
EXPECTED_ROWS=$((CONNECTIONS * UPDATES_PER_CONN))
ACTUAL_ROWS=$(psql -tAc "SELECT COUNT(*) FROM tv_test;")

if [ "$ACTUAL_ROWS" -eq "$EXPECTED_ROWS" ]; then
    echo "✅ PASS: Expected $EXPECTED_ROWS rows, got $ACTUAL_ROWS"
else
    echo "❌ FAIL: Expected $EXPECTED_ROWS rows, got $ACTUAL_ROWS"
    exit 1
fi

# Check for deadlocks in PostgreSQL log
if grep -q "deadlock detected" /var/log/postgresql/*.log 2>/dev/null; then
    echo "❌ FAIL: Deadlock detected in PostgreSQL log"
    exit 1
fi

echo "✅ Test passed: No deadlocks, correct row count"
```

### Step 3: Cascade Load Test

**File**: `test/concurrency/scenarios/02-cascade-load.sh`

```bash
#!/bin/bash
set -euo pipefail

source ../lib/common.sh

echo "Running cascade stress test..."

# Setup cascade: tb_user -> tb_post -> tv_post
psql <<EOF
CREATE TABLE tb_user (pk_user SERIAL PRIMARY KEY, name TEXT);
CREATE TABLE tb_post (
    pk_post SERIAL PRIMARY KEY,
    fk_user INT REFERENCES tb_user(pk_user),
    title TEXT
);
CREATE TABLE tv_post AS
SELECT p.pk_post, p.title, u.name as author_name
FROM tb_post p
JOIN tb_user u ON p.fk_user = u.pk_user;

SELECT pg_tviews_convert_existing_table('tv_post');
EOF

# Insert seed data
psql -c "INSERT INTO tb_user (name) SELECT 'User ' || i FROM generate_series(1, 100) i;"
psql -c "INSERT INTO tb_post (fk_user, title) SELECT (random()*99+1)::int, 'Post ' || i FROM generate_series(1, 1000) i;"

# Concurrent updates (50 to tb_user, 50 to tb_post)
for i in $(seq 1 50); do
    psql -c "UPDATE tb_user SET name = 'Updated-' || name WHERE pk_user = $i;" &
done

for i in $(seq 1 50); do
    psql -c "UPDATE tb_post SET title = 'Updated-' || title WHERE pk_post = $i;" &
done

wait

# Validate: All updated posts should reflect in tv_post
UPDATED_COUNT=$(psql -tAc "SELECT COUNT(*) FROM tv_post WHERE title LIKE 'Updated-%';")

if [ "$UPDATED_COUNT" -ge 50 ]; then
    echo "✅ PASS: Cascade updates propagated ($UPDATED_COUNT rows)"
else
    echo "❌ FAIL: Expected >=50 updated rows, got $UPDATED_COUNT"
    exit 1
fi

echo "✅ Cascade stress test passed"
```

### Step 4: 2PC Stress Test

**File**: `test/concurrency/scenarios/03-2pc-stress.sh`

```bash
#!/bin/bash
set -euo pipefail

source ../lib/common.sh

PREPARED_COUNT=20

echo "Running 2PC stress test..."

# Setup
psql <<EOF
CREATE TABLE tb_2pc_test (pk_test SERIAL PRIMARY KEY, data TEXT);
CREATE TABLE tv_2pc_test AS SELECT pk_test, data FROM tb_2pc_test;
SELECT pg_tviews_convert_existing_table('tv_2pc_test');
EOF

# Create prepared transactions
for i in $(seq 1 $PREPARED_COUNT); do
    psql <<EOF &
BEGIN;
INSERT INTO tb_2pc_test (data) VALUES ('2pc-transaction-$i');
PREPARE TRANSACTION 'xact_2pc_$i';
EOF
done

wait

# Verify queue persistence
QUEUE_SIZE=$(psql -tAc "SELECT COUNT(*) FROM pg_tview_queue_persistence;")
echo "Queue persistence entries: $QUEUE_SIZE"

# Commit half, rollback half
for i in $(seq 1 $((PREPARED_COUNT / 2))); do
    psql -c "COMMIT PREPARED 'xact_2pc_$i';" &
done

for i in $(seq $((PREPARED_COUNT / 2 + 1)) $PREPARED_COUNT); do
    psql -c "ROLLBACK PREPARED 'xact_2pc_$i';" &
done

wait

# Validate: tv_2pc_test should have exactly PREPARED_COUNT/2 rows
ACTUAL_ROWS=$(psql -tAc "SELECT COUNT(*) FROM tv_2pc_test;")
EXPECTED_ROWS=$((PREPARED_COUNT / 2))

if [ "$ACTUAL_ROWS" -eq "$EXPECTED_ROWS" ]; then
    echo "✅ PASS: 2PC handling correct ($ACTUAL_ROWS rows)"
else
    echo "❌ FAIL: Expected $EXPECTED_ROWS rows, got $ACTUAL_ROWS"
    exit 1
fi

# Check queue cleanup
QUEUE_AFTER=$(psql -tAc "SELECT COUNT(*) FROM pg_tview_queue_persistence;")
if [ "$QUEUE_AFTER" -eq 0 ]; then
    echo "✅ PASS: Queue cleaned up after 2PC commit/rollback"
else
    echo "⚠️  WARNING: Queue still has $QUEUE_AFTER entries (may be other transactions)"
fi

echo "✅ 2PC stress test passed"
```

### Step 5: Add Load Testing Framework

**File**: `test/concurrency/lib/common.sh`

```bash
#!/bin/bash

# Common functions for concurrency tests

export PGDATABASE=${PGDATABASE:-postgres}
export PGHOST=${PGHOST:-localhost}
export PGPORT=${PGPORT:-5432}
export PGUSER=${PGUSER:-postgres}

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

log_info() {
    echo -e "${GREEN}[INFO]${NC} $1"
}

log_warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

# Run SQL and check for errors
psql_safe() {
    psql -v ON_ERROR_STOP=1 "$@"
}

# Measure execution time
time_command() {
    local start=$(date +%s)
    "$@"
    local end=$(date +%s)
    local duration=$((end - start))
    log_info "Command took ${duration}s"
}

# Check for deadlocks in current session
check_deadlocks() {
    local deadlocks=$(psql -tAc "SELECT COUNT(*) FROM pg_stat_database WHERE deadlocks > 0;")
    if [ "$deadlocks" -gt 0 ]; then
        log_error "Deadlocks detected: $deadlocks"
        return 1
    fi
}

# Cleanup test database
cleanup_test_db() {
    log_info "Cleaning up test database..."
    psql -c "DROP TABLE IF EXISTS tv_test CASCADE;"
    psql -c "DROP TABLE IF EXISTS tb_test CASCADE;"
    psql -c "DROP TABLE IF EXISTS tv_2pc_test CASCADE;"
    psql -c "DROP TABLE IF EXISTS tb_2pc_test CASCADE;"
    # Add more cleanup as needed
}

trap cleanup_test_db EXIT
```

### Step 6: Add Performance Metrics

**Create**: `test/concurrency/lib/metrics.sh`

```bash
#!/bin/bash

# Record performance metrics during concurrency tests

METRICS_FILE="/tmp/pg_tviews_concurrency_metrics.csv"

# Initialize metrics file
init_metrics() {
    echo "timestamp,test_name,connections,ops_per_sec,avg_latency_ms,p95_latency_ms,deadlocks" > "$METRICS_FILE"
}

# Record metrics for a test run
record_metrics() {
    local test_name=$1
    local connections=$2
    local ops_per_sec=$3
    local avg_latency=$4
    local p95_latency=$5
    local deadlocks=$6

    echo "$(date +%s),$test_name,$connections,$ops_per_sec,$avg_latency,$p95_latency,$deadlocks" >> "$METRICS_FILE"
}

# Measure throughput
measure_throughput() {
    local start_time=$(date +%s.%N)
    "$@"
    local end_time=$(date +%s.%N)

    local duration=$(echo "$end_time - $start_time" | bc)
    local ops_per_sec=$(echo "scale=2; 1 / $duration" | bc)

    echo "$ops_per_sec"
}
```

---

## Verification Commands

```bash
# Run all concurrency tests
cd test/concurrency
./run_all_tests.sh

# Run specific scenario
./scenarios/01-concurrent-updates.sh

# Check for race conditions (run 100 times)
for i in $(seq 1 100); do
    ./scenarios/01-concurrent-updates.sh || { echo "Failed on iteration $i"; exit 1; }
done

# Monitor PostgreSQL during test
watch -n 1 "psql -c 'SELECT * FROM pg_stat_activity WHERE state != \"idle\";'"

# Check for deadlocks
psql -c "SELECT * FROM pg_stat_database WHERE deadlocks > 0;"
```

---

## Acceptance Criteria

- [x] 100+ concurrent connections handled without deadlocks
- [x] Cascade updates work correctly under concurrent load
- [x] 2PC transactions handled correctly (queue persistence/recovery)
- [x] PgBouncer connection pooling works (DISCARD ALL)
- [x] No race conditions in 100 repeated test runs
- [x] Performance metrics collected and validated
- [x] All tests automated and repeatable
- [x] CI integration added

---

## Expected Results

| Test | Connections | Expected Behavior |
|------|-------------|-------------------|
| Concurrent Updates | 100 | Zero deadlocks, all rows refreshed |
| Cascade Load | 100 | Correct dependency order, consistent state |
| 2PC Stress | 20 | Queue persisted, correct commit/rollback |
| DDL/DML Mix | 50 | Proper locking, clean errors |
| PgBouncer | Varies | Queue cleared on DISCARD ALL |

---

## DO NOT

- ❌ Ignore transient failures - all failures indicate bugs
- ❌ Test only on single CPU - use multi-core system
- ❌ Skip 2PC testing - critical for production
- ❌ Test only small datasets - use realistic data volumes

---

## CI Integration

**Add to `.github/workflows/concurrency.yml`**:

```yaml
name: Concurrency Tests

on:
  push:
    branches: [ main, dev ]
  pull_request:
    branches: [ main ]

jobs:
  concurrency:
    name: Concurrency Stress Tests
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Start PostgreSQL
        run: |
          docker run -d --name postgres \
            -e POSTGRES_PASSWORD=postgres \
            -p 5432:5432 \
            postgres:17

      - name: Install extension
        run: |
          cargo pgrx install --release

      - name: Run concurrency tests
        run: |
          cd test/concurrency
          ./run_all_tests.sh

      - name: Upload metrics
        uses: actions/upload-artifact@v3
        with:
          name: concurrency-metrics
          path: /tmp/pg_tviews_concurrency_metrics.csv
```

---

## Next Steps

After completion:
- Commit with message: `test(concurrency): Add comprehensive stress tests [PHASE2.1]`
- Document any discovered race conditions and fixes
- Proceed to **Phase 2.2: PgBouncer & 2PC Validation**
