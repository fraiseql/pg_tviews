# Benchmark Hardware Configuration

## Test System Specifications

**Date**: 2025-12-13
**Benchmark Version**: 0.1.0-beta.1

### Hardware
- **CPU**: AMD Ryzen 9 5950X (16 cores, 32 threads) @ 3.4 GHz
- **RAM**: 64GB DDR4-3200
- **Disk**: Samsung 980 PRO 1TB NVMe SSD
  - Sequential Read: 7,000 MB/s
  - Sequential Write: 5,000 MB/s
  - Random IOPS: 1M IOPS

### Software
- **OS**: Arch Linux (Kernel 6.6.1)
- **PostgreSQL**: 18.1
- **pg_tviews**: 0.1.0-beta.1
- **jsonb_ivm**: Not installed (optional)

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