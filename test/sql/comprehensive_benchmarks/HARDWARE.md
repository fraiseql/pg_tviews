# Benchmark Hardware Configuration

## Test System Specifications

**Date**: 2025-12-13
**Benchmark Version**: 0.1.0-beta.1

### Hardware
- **CPU**: Intel Core i7-13700K (16 cores, 24 threads) @ variable frequency
- **RAM**: 32GB (system total)
- **Disk**: NVMe SSD (118GB total, LVM)
  - Type: NVMe solid-state drive
  - Filesystem: ext4 (LVM)

### Software
- **OS**: Arch Linux (Kernel 6.17.9-arch1-1)
- **PostgreSQL**: 18.1
- **pg_tviews**: 0.1.0-beta.1
- **jsonb_delta**: Not installed (optional)

### PostgreSQL Configuration
```ini
shared_buffers = 16GB
effective_cache_size = 48GB
maintenance_work_mem = 2GB
checkpoint_completion_target = 0.9
wal_buffers = 16MB
default_statistics_target = 100
random_page_cost = 1.1  # SSD
effective_io_concurrency = 200
work_mem = 64MB
max_connections = 100
```

### Network
- Localhost connection (no network overhead)
- Unix domain sockets