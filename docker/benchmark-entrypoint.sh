#!/bin/bash
# Custom entrypoint for pg_tviews benchmark container

set -e

# Start PostgreSQL in background
docker-entrypoint.sh postgres &
PG_PID=$!

# Wait for PostgreSQL to be ready
echo "Waiting for PostgreSQL to start..."
until pg_isready -U postgres -d postgres > /dev/null 2>&1; do
    sleep 1
done
echo "PostgreSQL is ready!"

# Create benchmark database with extensions
echo "Setting up benchmark database..."
psql -U postgres -d postgres <<-EOSQL
    CREATE DATABASE pg_tviews_benchmark;
EOSQL

psql -U postgres -d pg_tviews_benchmark <<-EOSQL
    -- Create extensions
    CREATE EXTENSION IF NOT EXISTS "uuid-ossp";
    CREATE EXTENSION IF NOT EXISTS pg_tviews;
    CREATE EXTENSION IF NOT EXISTS jsonb_ivm;

    -- Verify extensions
    SELECT extname, extversion FROM pg_extension ORDER BY extname;

    -- Verify jsonb_ivm functions are available
    SELECT
        proname,
        pg_get_functiondef(oid)::text LIKE '%Rust%' AS is_rust_implementation
    FROM pg_proc
    WHERE proname LIKE 'jsonb_%patch%'
    ORDER BY proname;
EOSQL

echo "Extensions installed:"
psql -U postgres -d pg_tviews_benchmark -c "\dx"

echo ""
echo "================================================"
echo "pg_tviews Benchmark Container Ready!"
echo "================================================"
echo ""
echo "To run benchmarks:"
echo "  docker exec -it pg_tviews_bench /benchmarks/run_benchmarks.sh --scale small"
echo "  docker exec -it pg_tviews_bench /benchmarks/run_benchmarks.sh --scale medium"
echo "  docker exec -it pg_tviews_bench /benchmarks/run_benchmarks.sh --scale large"
echo ""
echo "To view results:"
echo "  docker exec -it pg_tviews_bench cat /benchmarks/results/benchmark_run_*.log"
echo "  docker exec -it pg_tviews_bench python3 /benchmarks/generate_report.py"
echo ""
echo "To access database:"
echo "  docker exec -it pg_tviews_bench psql -U postgres -d pg_tviews_benchmark"
echo ""
echo "================================================"

# Keep PostgreSQL running
wait $PG_PID
