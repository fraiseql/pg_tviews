# Phase 3.2: Memory Profiling

**Objective**: Profile memory usage under load and eliminate memory leaks

**Priority**: HIGH
**Estimated Time**: 1-2 days
**Blockers**: Phase 3.1 complete (benchmark validation)

---

## Context

**Current State**: Unknown memory behavior under production load

**Why This Matters**:
- Memory leaks in long-running PostgreSQL processes are critical
- Large TVIEW refreshes may consume excessive memory
- FFI code can leak if not carefully managed
- Production systems run for months without restart

**Deliverable**: Memory profile report with no leaks and optimized allocation patterns

---

## Profiling Tools

### Valgrind (Memory Leak Detection)
- Detects leaks, invalid memory access
- Slow but comprehensive

### Heaptrack (Heap Profiling)
- Tracks all allocations
- Generates flame graphs
- Better performance than Valgrind

### massif (Heap Snapshot)
- Part of Valgrind
- Shows memory usage over time

---

## Implementation Steps

### Step 1: Setup Profiling Environment

**Install tools**:
```bash
# Valgrind
sudo apt install valgrind

# Heaptrack
sudo apt install heaptrack heaptrack-gui

# PostgreSQL debug symbols
sudo apt install postgresql-17-dbgsym
```

**Build with debug symbols**:
```bash
# Debug build with optimizations
cargo pgrx install --profile=profiling

# Or add to Cargo.toml:
# [profile.profiling]
# inherits = "release"
# debug = true
```

### Step 2: Baseline Memory Usage

**Create**: `test/profiling/baseline-memory.sh`

```bash
#!/bin/bash
set -euo pipefail

echo "Measuring baseline memory usage..."

# Start PostgreSQL with memory tracking
pg_ctl start -D $PGDATA

# Wait for startup
sleep 5

# Get baseline
BASELINE=$(pmap $(pidof postgres | head -1) | tail -1 | awk '{print $2}')
echo "Baseline PostgreSQL memory: $BASELINE"

# Load extension
psql -c "CREATE EXTENSION IF NOT EXISTS pg_tviews;"

# After extension load
AFTER_EXT=$(pmap $(pidof postgres | head -1) | tail -1 | awk '{print $2}')
echo "After extension load: $AFTER_EXT"

# Create test TVIEW
psql <<EOF
CREATE TABLE tb_baseline (pk_test SERIAL PRIMARY KEY, data TEXT);
CREATE TABLE tv_baseline AS SELECT pk_test, data FROM tb_baseline;
SELECT pg_tviews_convert_existing_table('tv_baseline');
EOF

AFTER_TVIEW=$(pmap $(pidof postgres | head -1) | tail -1 | awk '{print $2}')
echo "After TVIEW creation: $AFTER_TVIEW"

# Insert 10K rows
psql -c "INSERT INTO tb_baseline (data) SELECT 'row-' || i FROM generate_series(1, 10000) i;"

AFTER_REFRESH=$(pmap $(pidof postgres | head -1) | tail -1 | awk '{print $2}')
echo "After 10K row refresh: $AFTER_REFRESH"

# Save baseline
cat > baseline-memory.txt <<EOF
Baseline PostgreSQL: $BASELINE
After extension: $AFTER_EXT
After TVIEW: $AFTER_TVIEW
After 10K refresh: $AFTER_REFRESH
EOF

echo "✅ Baseline saved to baseline-memory.txt"
```

### Step 3: Memory Leak Detection with Valgrind

**Create**: `test/profiling/valgrind-leak-test.sh`

```bash
#!/bin/bash
set -euo pipefail

echo "Running Valgrind leak detection..."

# Stop existing PostgreSQL
pg_ctl stop -D $PGDATA || true

# Start PostgreSQL under Valgrind
valgrind \
    --leak-check=full \
    --show-leak-kinds=all \
    --track-origins=yes \
    --verbose \
    --log-file=/tmp/valgrind-postgres.log \
    postgres -D $PGDATA &

VALGRIND_PID=$!

# Wait for startup
sleep 10

# Run leak test workload
psql <<EOF
CREATE EXTENSION pg_tviews;

CREATE TABLE tb_leak_test (pk_test SERIAL PRIMARY KEY, data TEXT);
CREATE TABLE tv_leak_test AS SELECT pk_test, data FROM tb_leak_test;
SELECT pg_tviews_convert_existing_table('tv_leak_test');

-- Perform 1000 refresh operations
DO \$\$
BEGIN
    FOR i IN 1..1000 LOOP
        INSERT INTO tb_leak_test (data) VALUES ('iteration-' || i);
    END LOOP;
END \$\$;

-- Drop and recreate to test cleanup
DROP TABLE tv_leak_test CASCADE;
CREATE TABLE tv_leak_test AS SELECT pk_test, data FROM tb_leak_test;
SELECT pg_tviews_convert_existing_table('tv_leak_test');
EOF

# Graceful shutdown to flush leaks
psql -c "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE pid != pg_backend_pid();"
kill -TERM $VALGRIND_PID
wait $VALGRIND_PID

# Analyze results
echo "Analyzing Valgrind output..."

DEFINITELY_LOST=$(grep "definitely lost:" /tmp/valgrind-postgres.log | tail -1 | awk '{print $4}')
INDIRECTLY_LOST=$(grep "indirectly lost:" /tmp/valgrind-postgres.log | tail -1 | awk '{print $4}')

echo "Definitely lost: $DEFINITELY_LOST bytes"
echo "Indirectly lost: $INDIRECTLY_LOST bytes"

if [ "$DEFINITELY_LOST" -eq 0 ]; then
    echo "✅ PASS: No definite memory leaks"
else
    echo "❌ FAIL: Memory leak detected ($DEFINITELY_LOST bytes)"
    echo "See /tmp/valgrind-postgres.log for details"
    exit 1
fi
```

### Step 4: Heap Profiling with Heaptrack

**Create**: `test/profiling/heaptrack-profile.sh`

```bash
#!/bin/bash
set -euo pipefail

echo "Running heap profiling with heaptrack..."

# Build test workload
cat > /tmp/heap_workload.sql <<EOF
CREATE EXTENSION pg_tviews;

-- Small TVIEW
CREATE TABLE tb_small (pk_test SERIAL PRIMARY KEY, data TEXT);
CREATE TABLE tv_small AS SELECT pk_test, data FROM tb_small;
SELECT pg_tviews_convert_existing_table('tv_small');

-- Medium TVIEW
CREATE TABLE tb_medium (pk_test SERIAL PRIMARY KEY, data JSONB);
CREATE TABLE tv_medium AS SELECT pk_test, data FROM tb_medium;
SELECT pg_tviews_convert_existing_table('tv_medium');

-- Large TVIEW
CREATE TABLE tb_large (pk_test SERIAL PRIMARY KEY, data JSONB);
CREATE TABLE tv_large AS SELECT pk_test, data FROM tb_large;
SELECT pg_tviews_convert_existing_table('tv_large');

-- Insert data
INSERT INTO tb_small (data) SELECT 'row-' || i FROM generate_series(1, 1000) i;
INSERT INTO tb_medium (data) SELECT jsonb_build_object('id', i, 'val', 'data-' || i) FROM generate_series(1, 10000) i;
INSERT INTO tb_large (data) SELECT jsonb_build_object('id', i, 'val', 'data-' || i) FROM generate_series(1, 100000) i;

-- Trigger cascade refresh
UPDATE tb_small SET data = 'updated';
EOF

# Run under heaptrack
heaptrack postgres -D $PGDATA &
HEAP_PID=$!

sleep 10

# Run workload
psql -f /tmp/heap_workload.sql

# Shutdown
pg_ctl stop -D $PGDATA

# Analyze
heaptrack --analyze /tmp/heaptrack.postgres.*.gz

echo "✅ Heap profile saved. Open with: heaptrack_gui /tmp/heaptrack.postgres.*.gz"
```

### Step 5: Long-Running Memory Test

**Create**: `test/profiling/long-running-test.sh`

```bash
#!/bin/bash
set -euo pipefail

echo "Long-running memory test (24 hours)..."

# Setup
psql <<EOF
CREATE TABLE tb_longrun (pk_test SERIAL PRIMARY KEY, data JSONB, created_at TIMESTAMPTZ DEFAULT now());
CREATE TABLE tv_longrun AS SELECT pk_test, data, created_at FROM tb_longrun;
SELECT pg_tviews_convert_existing_table('tv_longrun');
EOF

# Monitor memory every 5 minutes
echo "timestamp,rss_kb,vsz_kb,heap_mb" > /tmp/memory-over-time.csv

for i in {1..288}; do  # 24 hours / 5 minutes = 288 intervals
    # Insert batch
    psql -c "INSERT INTO tb_longrun (data) SELECT jsonb_build_object('iter', $i, 'data', repeat('x', 1000)) FROM generate_series(1, 100);" &>/dev/null

    # Get PostgreSQL memory
    PG_PID=$(pidof postgres | cut -d' ' -f1)
    RSS=$(ps -p $PG_PID -o rss= | awk '{print $1}')
    VSZ=$(ps -p $PG_PID -o vsz= | awk '{print $1}')

    # Get heap size from /proc
    HEAP=$(grep "^Heap:" /proc/$PG_PID/status | awk '{print $2}')

    echo "$(date +%s),$RSS,$VSZ,$HEAP" >> /tmp/memory-over-time.csv

    echo "[$i/288] RSS: ${RSS}KB, VSZ: ${VSZ}KB, Heap: ${HEAP}MB"

    sleep 300  # 5 minutes
done

echo "✅ Long-running test complete. Results in /tmp/memory-over-time.csv"

# Analyze for memory growth
python3 <<EOF
import pandas as pd
import matplotlib.pyplot as plt

df = pd.read_csv('/tmp/memory-over-time.csv')
df['timestamp'] = pd.to_datetime(df['timestamp'], unit='s')

# Plot
fig, axes = plt.subplots(3, 1, figsize=(12, 8))

axes[0].plot(df['timestamp'], df['rss_kb'] / 1024, label='RSS (MB)')
axes[0].set_ylabel('RSS (MB)')
axes[0].legend()

axes[1].plot(df['timestamp'], df['vsz_kb'] / 1024, label='VSZ (MB)')
axes[1].set_ylabel('VSZ (MB)')
axes[1].legend()

axes[2].plot(df['timestamp'], df['heap_mb'], label='Heap (MB)')
axes[2].set_ylabel('Heap (MB)')
axes[2].set_xlabel('Time')
axes[2].legend()

plt.savefig('/tmp/memory-over-time.png')
print("Plot saved to /tmp/memory-over-time.png")

# Check for memory leak (linear growth)
from scipy.stats import linregress
slope, intercept, r_value, p_value, std_err = linregress(range(len(df)), df['rss_kb'])

if slope > 100:  # Growing more than 100KB per interval
    print(f"⚠️  WARNING: Possible memory leak (slope: {slope:.2f} KB/interval)")
else:
    print(f"✅ Memory stable (slope: {slope:.2f} KB/interval)")
EOF
```

### Step 6: Optimize Memory Usage

**Based on profiling results, optimize**:

**Common optimizations**:

1. **Reduce allocations in hot paths**
   ```rust
   // Before: Multiple allocations
   let query = format!("SELECT * FROM {}", table);
   let result = spi::query(&query)?;

   // After: Reuse buffers
   use std::fmt::Write;
   let mut query = String::with_capacity(256);
   write!(&mut query, "SELECT * FROM {}", table)?;
   ```

2. **Use arena allocators for batch operations**
   ```rust
   use bumpalo::Bump;

   fn refresh_batch(keys: &[RefreshKey]) -> TViewResult<()> {
       let arena = Bump::new();
       // Allocate temporary data in arena
       // All freed at once when arena drops
   }
   ```

3. **Clear caches periodically**
   ```rust
   // Add cache size limits
   if cache.len() > MAX_CACHE_SIZE {
       cache.clear();
   }
   ```

---

## Verification Commands

```bash
# Run all profiling tests
cd test/profiling
./baseline-memory.sh
./valgrind-leak-test.sh
./heaptrack-profile.sh

# Start long-running test (background)
nohup ./long-running-test.sh &

# Monitor memory in real-time
watch -n 5 'pmap $(pidof postgres | head -1) | tail -1'

# Check for leaks in specific function
valgrind --leak-check=full --track-origins=yes \
    psql -c "SELECT pg_tviews_refresh('large_entity');"
```

---

## Acceptance Criteria

- [ ] Valgrind reports zero definite leaks
- [ ] Long-running test (24h) shows stable memory usage
- [ ] Heap profiling identifies largest allocations
- [ ] Memory usage < 100MB for 10K row TVIEW
- [ ] Memory usage < 1GB for 1M row TVIEW
- [ ] Memory freed after TVIEW drop
- [ ] No growth in long-running test
- [ ] Optimizations implemented for hot paths

---

## Memory Budget

Target memory usage:

| Operation | Memory Budget | Current | Status |
|-----------|---------------|---------|--------|
| Extension load | < 10MB | TBD | ⏳ |
| Small TVIEW (1K rows) | < 50MB | TBD | ⏳ |
| Medium TVIEW (10K rows) | < 100MB | TBD | ⏳ |
| Large TVIEW (100K rows) | < 500MB | TBD | ⏳ |
| Cascade refresh (10 levels) | < 200MB | TBD | ⏳ |

---

## DO NOT

- ❌ Profile in debug mode - use release or profiling build
- ❌ Ignore "possibly lost" leaks - investigate all
- ❌ Test on small datasets only - profile realistic workloads
- ❌ Skip long-running tests - leaks appear over time

---

## Rollback Plan

If memory issues found:

1. Document issue in GitHub issue
2. Add workaround (e.g., manual cache clearing)
3. Optimize in follow-up PR
4. Re-run profiling

---

## Next Steps

After completion:
- Commit with message: `perf(memory): Profile and optimize memory usage [PHASE3.2]`
- Publish memory profile report
- Update documentation with memory requirements
- Proceed to **Phase 3.3: Performance Regression Testing**
