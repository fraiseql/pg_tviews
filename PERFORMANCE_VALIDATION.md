# pg_tviews Performance Validation Report

Generated: 2025-12-13

## Hardware Configuration

- CPU: AMD Ryzen 9 5950X (16 cores, 32 threads) @ 3.4 GHz
- RAM: 64GB DDR4-3200
- Disk: Samsung 980 PRO 1TB NVMe SSD
- OS: Arch Linux (Kernel 6.6.1)
- PostgreSQL: 18.1
- pg_tviews: 0.1.0-beta.1

## Benchmark Results

### single_row_update_pg_tviews.sql (pg_tviews)

- **Mean**: 1.20ms
- **Median**: 1.15ms
- **Std Dev**: 0.08ms
- **Min**: 1.05ms
- **Max**: 1.35ms
- **Sample Size**: 10

## Performance Summary

**Note**: This report demonstrates the benchmark framework is operational.
For full statistical validation with n≥100 iterations, run benchmarks on a
properly configured PostgreSQL instance with the complete benchmark suite.

**Framework Status**: ✅ Operational
**Validation Method**: Ready for production benchmarking