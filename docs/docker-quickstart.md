# Docker Quickstart for pg_tviews Benchmarks

## Quick Commands

```bash
# 1. Build the container (takes ~10-15 minutes)
docker-compose build pg_tviews_bench

# 2. Start the container
docker-compose up -d pg_tviews_bench

# 3. Wait for it to be ready (~30 seconds)
docker-compose ps

# 4. Run small-scale benchmarks (~30 seconds)
docker exec -it pg_tviews_bench /benchmarks/run_benchmarks.sh --scale small

# 5. View results
cat test/sql/comprehensive_benchmarks/results/benchmark_run_*.log | tail -100

# 6. Generate markdown report
docker exec -it pg_tviews_bench python3 /benchmarks/generate_report.py
```

## What Gets Installed

The Docker container includes:
- **PostgreSQL 17** (compatible with pg_ivm)
- **pg_tviews extension** (built from source)
- **pg_ivm extension** (Incremental View Maintenance)
- **jsonb_delta stubs** (PL/pgSQL fallback for JSONB operations)
- **Python 3 + psycopg** (for report generation)

## All Available Scales

```bash
# Small: 1K products (~30 seconds)
docker exec -it pg_tviews_bench /benchmarks/run_benchmarks.sh --scale small

# Medium: 100K products (~3-5 minutes)
docker exec -it pg_tviews_bench /benchmarks/run_benchmarks.sh --scale medium

# Large: 1M products (~15-20 minutes)
docker exec -it pg_tviews_bench /benchmarks/run_benchmarks.sh --scale large
```

## Useful Commands

```bash
# Check container status
docker-compose ps

# View container logs
docker-compose logs -f pg_tviews_bench

# Connect to PostgreSQL
docker exec -it pg_tviews_bench psql -U postgres -d pg_tviews_benchmark

# Check installed extensions
docker exec -it pg_tviews_bench psql -U postgres -d pg_tviews_benchmark -c "\dx"

# Open shell in container
docker exec -it pg_tviews_bench bash

# Stop container
docker-compose stop pg_tviews_bench

# Remove container (keep image)
docker-compose down

# Full cleanup (remove everything)
docker-compose down -v
docker rmi pg_tviews-pg_tviews_bench
```

## Results Location

All benchmark results are automatically saved to your host machine:
```
test/sql/comprehensive_benchmarks/results/
├── benchmark_run_YYYYMMDD_HHMMSS.log
├── benchmark_results_YYYYMMDD_HHMMSS.csv
└── BENCHMARK_REPORT_YYYYMMDD_HHMMSS.md
```

## Helper Scripts

We provide two helper scripts:

### Option 1: docker/bench.sh (Full-featured)
```bash
./docker/bench.sh build      # Build container
./docker/bench.sh start      # Start container
./docker/bench.sh run small  # Run benchmarks
./docker/bench.sh results    # View latest results
./docker/bench.sh psql       # Connect to DB
./docker/bench.sh --help     # See all commands
```

### Option 2: docker/build-and-run.sh (Simple, no docker-compose)
```bash
./docker/build-and-run.sh build      # Build image
./docker/build-and-run.sh start      # Start container
./docker/build-and-run.sh run small  # Run benchmarks
./docker/build-and-run.sh --help     # See all commands
```

## Troubleshooting

**Container won't start?**
```bash
docker-compose logs pg_tviews_bench
```

**Port 5433 already in use?**
Edit `docker-compose.yml` and change the port:
```yaml
ports:
  - "5434:5432"  # Use different port
```

**Build fails?**
```bash
# Clean rebuild
docker-compose down -v
docker-compose build --no-cache pg_tviews_bench
```

**Out of memory?**
Reduce benchmark scale or increase Docker memory limit in Docker Desktop settings.

## Full Documentation

See [docs/DOCKER_BENCHMARKS.md](docs/DOCKER_BENCHMARKS.md) for comprehensive documentation, including:
- Extension architecture details
- Technical issues and fixes
- Advanced usage and performance tuning
- CI/CD integration examples
