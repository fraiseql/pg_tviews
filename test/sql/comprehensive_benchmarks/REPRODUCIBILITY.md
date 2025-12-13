# Benchmark Reproducibility Protocol

## Setup

1. **Fresh PostgreSQL Installation**
   ```bash
   # Install PostgreSQL 18
   sudo pacman -S postgresql

   # Stop PostgreSQL
   sudo systemctl stop postgresql

   # Remove existing data
   sudo rm -rf /var/lib/postgres/data

   # Initialize new cluster
   sudo -u postgres initdb -D /var/lib/postgres/data
   ```

2. **Apply Benchmark Configuration**
   ```bash
   # Copy optimized postgresql.conf
   sudo cp benchmark-postgresql.conf /var/lib/postgres/data/postgresql.conf

   # Start PostgreSQL
   sudo systemctl start postgresql
   ```

3. **Install Extensions**
   ```bash
   # Install pg_tviews
   cargo pgrx install --release --pg-config=/usr/bin/pg_config

   # Create benchmark database
   createdb benchmark_db
   psql benchmark_db -c "CREATE EXTENSION pg_tviews;"
   ```

## Running Benchmarks

```bash
cd test/sql/comprehensive_benchmarks

# Run full benchmark suite
python3 benchmark_runner.py --iterations 10

# Generate report
# Report is automatically generated as PERFORMANCE_VALIDATION.md
```

## Data Validation

After each benchmark run, verify:

1. **No errors in PostgreSQL log**
   ```bash
   tail -n 100 /var/lib/postgres/data/log/postgresql.log | grep ERROR
   # Should return nothing
   ```

2. **Data consistency**
   ```sql
   SELECT COUNT(*) FROM tv_benchmark_table;
   -- Should match expected row count
   ```

3. **No deadlocks**
   ```sql
   SELECT deadlocks FROM pg_stat_database WHERE datname = 'benchmark_db';
   -- Should be 0
   ```