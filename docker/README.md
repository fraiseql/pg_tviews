# pg_tviews Docker Benchmark Environment

This directory contains Docker configuration for running pg_tviews benchmarks in an isolated PostgreSQL environment with all required extensions pre-installed.

## Architecture

```
/home/lionel/code/
├── jsonb_delta/          ← Dependency (JSONB IVM extension)
└── pg_tviews/          ← Main project
    └── docker/         ← Docker configuration (you are here)
        ├── docker-compose.yml
        ├── dockerfile-benchmarks
        ├── benchmark-entrypoint.sh
        └── run-benchmarks-docker.sh  ← Helper script
```

## What Gets Built

The Docker image (`pg_tviews_bench`) includes:

1. **PostgreSQL 18** - Latest version
2. **pg_tviews extension** - Built from source (this project)
3. **jsonb_delta extension** - Built from source (sibling project)
4. **Rust toolchain** - For building extensions
5. **cargo-pgrx 0.16.1** - PostgreSQL extension framework
6. **Benchmark scripts** - Copied from `test/sql/comprehensive_benchmarks/`

## Quick Start

### 1. Build the Docker Image

```bash
cd docker
docker compose build
```

**Build time**: 5-10 minutes (downloads dependencies, compiles Rust extensions)

### 2. Start the Container

```bash
docker compose up -d
```

This starts PostgreSQL on **port 5433** (to avoid conflict with local PostgreSQL on 5432).

### 3. Run Benchmarks

**Option A: Using helper script** (recommended)
```bash
cd docker
./run-benchmarks-docker.sh small   # or: medium, large
```

**Option B: Manual**
```bash
# Run benchmarks inside container
docker exec -it pg_tviews_bench bash -c "
    cd /benchmarks
    ./run_benchmarks.sh --scale small
"
```

### 4. View Results

Results are mounted from the container to your host:

```bash
cat test/sql/comprehensive_benchmarks/results/benchmark_run_*.log
```

### 5. Stop the Container

```bash
cd docker
docker compose down
```

To also remove the PostgreSQL data volume:
```bash
docker compose down -v
```

## Container Details

### Ports

- **5433** (host) → **5432** (container) - PostgreSQL

### Volumes

- `test/sql/comprehensive_benchmarks/results/` - Benchmark results (mounted)
- `pgdata` - PostgreSQL data (Docker volume, persists between restarts)

### Environment Variables

- `POSTGRES_DB=pg_tviews_benchmark` - Database name
- `POSTGRES_USER=postgres` - Superuser
- `POSTGRES_PASSWORD=postgres` - Password

### Resources

- Memory: 4GB minimum, 16GB maximum
- Shared memory: 2GB (for PostgreSQL)

## Connecting to Container PostgreSQL

### From Host Machine

```bash
# Using psql
psql -h localhost -p 5433 -U postgres -d pg_tviews_benchmark

# Password: postgres
```

### From Inside Container

```bash
# Shell access
docker exec -it pg_tviews_bench bash

# Direct psql
docker exec -it pg_tviews_bench psql -U postgres -d pg_tviews_benchmark
```

## Troubleshooting

### Build Fails

**Issue**: `lstat /home/lionel/code/pg_tviews/pg_tviews: no such file or directory`

**Cause**: Context path incorrect

**Fix**: Ensure docker-compose.yml has `context: ../..` (two levels up from docker/ directory)

### Container Won't Start

```bash
# Check logs
docker logs pg_tviews_bench

# Check if another PostgreSQL is using port 5433
lsof -i :5433

# Change port in docker-compose.yml if needed
```

### Extensions Not Loading

```bash
# Verify extensions are installed
docker exec pg_tviews_bench ls -la /usr/share/postgresql/18/extension/pg_tviews*
docker exec pg_tviews_bench ls -la /usr/share/postgresql/18/extension/jsonb_delta*

# Check extension in database
docker exec pg_tviews_bench psql -U postgres -d pg_tviews_benchmark -c "\dx"
```

### Benchmarks Fail

```bash
# Check database is ready
docker exec pg_tviews_bench pg_isready -U postgres -d pg_tviews_benchmark

# Run setup manually
docker exec -it pg_tviews_bench psql -U postgres -d pg_tviews_benchmark -f /benchmarks/00_setup.sql

# Check for errors
docker logs pg_tviews_bench 2>&1 | grep -i error
```

## Development Workflow

### Rebuilding After Code Changes

When you modify pg_tviews or jsonb_delta source code:

```bash
cd docker

# Rebuild image
docker compose build --no-cache

# Restart container
docker compose down -v
docker compose up -d

# Run benchmarks
./run-benchmarks-docker.sh small
```

### Debugging Inside Container

```bash
# Get a shell
docker exec -it pg_tviews_bench bash

# Check PostgreSQL logs
tail -f /var/lib/postgresql/data/log/postgresql-*.log

# Check extension files
ls -la /usr/lib/postgresql/18/lib/pg_tviews.so
cat /usr/share/postgresql/18/extension/pg_tviews.control
cat /usr/share/postgresql/18/extension/pg_tviews--0.1.0.sql

# Test extension manually
psql -U postgres -d pg_tviews_benchmark <<EOF
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
CREATE EXTENSION pg_tviews;
\dx pg_tviews
EOF
```

## File Structure Inside Container

```
/build/
├── pg_tviews/          ← Source code (built here)
└── jsonb_delta/          ← Source code (built here)

/benchmarks/            ← Benchmark scripts (runtime)
├── 00_setup.sql
├── cleanup_schema.sql
├── data/
├── scenarios/
├── results/            ← Mounted to host
└── run_benchmarks.sh

/usr/share/postgresql/18/extension/
├── pg_tviews.control
├── pg_tviews--0.1.0.sql
├── jsonb_delta.control
└── jsonb_delta--0.1.0.sql

/usr/lib/postgresql/18/lib/
├── pg_tviews.so
└── jsonb_delta.so
```

## Performance Tuning

### For Large Benchmarks

Edit `docker-compose.yml`:

```yaml
deploy:
  resources:
    limits:
      memory: 32G      # Increase for large scale
    reservations:
      memory: 8G
shm_size: 4gb          # Increase shared memory
```

### PostgreSQL Configuration

The Dockerfile sets:
- `shared_buffers = 512MB`
- `work_mem = 256MB`
- `max_parallel_workers_per_gather = 4`
- `shared_preload_libraries = 'pg_tviews'`

To customize, edit `dockerfile-benchmarks` and rebuild.

## CI/CD Integration

### GitHub Actions Example

```yaml
name: Benchmarks
on: [push]
jobs:
  benchmark:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v3

      # Also checkout jsonb_delta sibling
      - name: Checkout jsonb_delta
        run: |
          cd ..
          git clone https://github.com/your-org/jsonb_delta.git

      - name: Build benchmark environment
        run: |
          cd docker
          docker compose build
          docker compose up -d

      - name: Run benchmarks
        run: |
          cd docker
          ./run-benchmarks-docker.sh small

      - name: Upload results
        uses: actions/upload-artifact@v3
        with:
          name: benchmark-results
          path: test/sql/comprehensive_benchmarks/results/
```

## Comparison: Docker vs Local PostgreSQL

| Aspect | Docker | Local PostgreSQL |
|--------|--------|------------------|
| **Setup** | `docker compose up` | Install extensions manually |
| **Isolation** | Complete (container) | Shared with other databases |
| **Port** | 5433 (configurable) | 5432 (default) |
| **Extensions** | Pre-installed | Manual `cargo pgrx install` |
| **Reproducibility** | High (same image) | Depends on local setup |
| **Performance** | ~95% of native | 100% (native) |
| **CI/CD** | Easy (portable) | Complex (requires setup) |

## Next Steps

1. Build image: `docker compose build`
2. Start container: `docker compose up -d`
3. Run benchmarks: `./run-benchmarks-docker.sh small`
4. View results: `cat ../test/sql/comprehensive_benchmarks/results/*.log`

---

*Last Updated: 2025-12-13*
*Docker Compose Version: 3*
*PostgreSQL Version: 18*
*pgrx Version: 0.16.1*
