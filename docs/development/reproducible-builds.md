# Reproducible Builds

**Document Version:** 1.0
**Last Updated:** 2025-12-11
**Classification:** Public
**Applicable Standards:** SLSA Level 3, ISO 27001

## Overview

pg_tviews implements fully reproducible builds using Docker containers with locked dependencies and controlled environments. This ensures that anyone can rebuild the exact same artifacts from source code, providing supply chain security and build verification.

## Why Reproducible Builds?

### Security Benefits
- **Build Verification**: Independent verification of official releases
- **Supply Chain Security**: Detect tampering or malicious builds
- **Audit Compliance**: Meet regulatory requirements for build transparency
- **Trust**: Community can verify official builds match source code

### Development Benefits
- **Debugging**: Reproduce issues in controlled environments
- **Testing**: Consistent builds across different systems
- **CI/CD**: Reliable automated builds
- **Collaboration**: Team members get identical results

## Build Environment

### Docker Configuration

pg_tviews uses a locked Docker environment defined in `Dockerfile.build`:

```dockerfile
FROM rust:1.91.1-slim-bookworm

# PostgreSQL 17 (locked version)
RUN apt-get install postgresql-17 postgresql-server-dev-17

# pgrx 0.12.8 (locked version)
RUN cargo install --locked cargo-pgrx

# Reproducible build flags
ENV SOURCE_DATE_EPOCH=1
ENV RUSTFLAGS="-C opt-level=3 -C debuginfo=0 -C strip=symbols"
```

### Controlled Variables

#### Locked Versions
- **Rust**: 1.91.1 (exact compiler version)
- **PostgreSQL**: 17 (major version)
- **pgrx**: 0.12.8 (exact framework version)
- **Base OS**: Debian Bookworm slim

#### Build Flags
- **Optimization**: `-C opt-level=3` (maximum optimization)
- **Debug Info**: `-C debuginfo=0` (no debug symbols)
- **Stripping**: `-C strip=symbols` (remove symbols)
- **Timestamps**: `SOURCE_DATE_EPOCH=1` (normalized timestamps)

## Build Locally

### Prerequisites

```bash
# Install Docker
curl -fsSL https://get.docker.com | sh

# Clone repository
git clone https://github.com/your-org/pg_tviews.git
cd pg_tviews
```

### Reproducible Build Process

```bash
# Build specific version
./scripts/reproducible-build.sh 0.1.0

# Check output
ls -lh dist/
# pg_tviews-0.1.0.tar.gz
# build-info.json
# SHA256SUMS
# SHA512SUMS
```

### Verify Against Official Release

```bash
# Download official checksums
curl -fsSL https://github.com/your-org/pg_tviews/releases/download/v0.1.0/SHA256SUMS -o official-checksums.txt

# Verify your build matches
cd dist
sha256sum -c ../official-checksums.txt
```

**Expected Output:**
```
pg_tviews-0.1.0.tar.gz: OK
```

## Verify Build Reproducibility

### Multiple Build Test

```bash
# Build first time
./scripts/reproducible-build.sh 0.1.0
mv dist dist1

# Build second time
./scripts/reproducible-build.sh 0.1.0
mv dist dist2

# Compare builds (should be identical)
diff -r dist1 dist2
```

**Expected Output:** No differences (empty output)

### Checksum Consistency

```bash
# Generate checksums for both builds
cd dist1 && sha256sum pg_tviews-0.1.0.tar.gz > checksum1.txt
cd ../dist2 && sha256sum pg_tviews-0.1.0.tar.gz > checksum2.txt

# Compare checksums
diff ../dist1/checksum1.txt checksum2.txt
```

**Expected Output:** Identical checksums

## Build Metadata

### Build Information

Each reproducible build generates `build-info.json`:

```json
{
  "version": "0.1.0",
  "timestamp": "2025-12-11T10:00:00Z",
  "builder": "docker",
  "rust_version": "1.91.1",
  "postgres_version": "17",
  "pgrx_version": "0.12.8",
  "commit": "abc123...",
  "build_environment": "debian-bookworm-slim",
  "reproducible": true
}
```

### Checksum Files

Build generates multiple checksum formats:

```bash
# SHA256 checksums
cat dist/SHA256SUMS
# abc123...  pg_tviews-0.1.0.tar.gz

# SHA512 checksums
cat dist/SHA512SUMS
# def456...  pg_tviews-0.1.0.tar.gz
```

## Factors Affecting Reproducibility

### Controlled Factors ✅

- **Source Code**: Git commit hash locked
- **Dependencies**: Cargo.lock with exact versions
- **Build Environment**: Docker image with locked versions
- **Compiler**: Rust version pinned
- **Build Flags**: RUSTFLAGS environment variable
- **Timestamps**: SOURCE_DATE_EPOCH normalization

### Uncontrolled Factors ⚠️

- **System Timezone**: Use UTC in containers
- **Locale Settings**: Container uses C locale
- **File Permissions**: Docker normalizes permissions
- **Network Dependencies**: Builds are offline after dependency fetch

## Troubleshooting

### Build Fails in Docker

**Issue**: `cargo pgrx package` fails

**Solutions**:
```bash
# Check PostgreSQL is running
docker exec -it <container> pg_isready

# Initialize pgrx manually
docker exec -it <container> cargo pgrx init --pg17

# Check build logs
docker logs <container>
```

### Checksums Don't Match

**Issue**: Local build checksums differ from official

**Solutions**:
```bash
# Verify Docker image
docker build --no-cache -t pg_tviews-builder:test -f Dockerfile.build .

# Check environment variables
docker run --rm pg_tviews-builder:test env | grep -E "(SOURCE_DATE_EPOCH|RUSTFLAGS)"

# Verify Rust version
docker run --rm pg_tviews-builder:test rustc --version
```

### PostgreSQL Connection Issues

**Issue**: pgrx cannot connect to PostgreSQL

**Solutions**:
```bash
# Start PostgreSQL in container
docker run -d --name postgres postgres:17

# Link containers
docker run --link postgres:postgres pg_tviews-builder:test

# Or use host networking
docker run --network host pg_tviews-builder:test
```

### Out of Memory

**Issue**: Build fails with memory errors

**Solutions**:
```bash
# Increase Docker memory
docker run --memory=4g --memory-swap=4g pg_tviews-builder:test

# Reduce parallel jobs
docker run -e CARGO_BUILD_JOBS=1 pg_tviews-builder:test
```

## Advanced Usage

### Custom Build Environment

```bash
# Build with custom Rust flags
docker run -e RUSTFLAGS="-C opt-level=3 -C lto=fat" pg_tviews-builder:test

# Build with different PostgreSQL version
sed 's/postgresql-17/postgresql-16/g' Dockerfile.build > Dockerfile.custom
docker build -f Dockerfile.custom -t custom-builder .
```

### Integration Testing

```bash
# Build and test in one command
docker run --rm \
  -v $(pwd)/dist:/build/target \
  pg_tviews-builder:test \
  bash -c "cargo pgrx package --release && cargo test"
```

### CI/CD Integration

```yaml
# .github/workflows/verify-reproducibility.yml
name: Verify Reproducible Builds

on: [pull_request]

jobs:
  reproducible:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v3

      - name: Build reproducibly
        run: ./scripts/reproducible-build.sh ${{ github.sha }}

      - name: Verify checksums
        run: |
          cd dist
          sha256sum -c SHA256SUMS
```

## Security Considerations

### Build Environment Security

- **Minimal Base Image**: Debian slim reduces attack surface
- **Locked Dependencies**: No automatic updates during build
- **No Network Access**: Builds run offline after setup
- **Controlled Compiler**: Specific Rust version prevents compiler bugs

### Verification Best Practices

1. **Always verify checksums** after building
2. **Compare against official releases** for security
3. **Use trusted Docker images** for build environment
4. **Check build metadata** for environment consistency
5. **Report discrepancies** to maintainers immediately

## Performance Optimization

### Build Time Optimization

```bash
# Use build cache
docker build --cache-from pg_tviews-builder:latest -t pg_tviews-builder:new .

# Parallel builds
docker run -e CARGO_BUILD_JOBS=$(nproc) pg_tviews-builder:test

# Incremental builds
docker run -v $(pwd)/target:/build/target pg_tviews-builder:test
```

### Storage Optimization

```bash
# Multi-stage builds
FROM rust:1.91.1-slim-bookworm AS builder
# Build stage

FROM debian:bookworm-slim AS runtime
# Runtime stage with only artifacts
```

## Contributing

### Adding New Dependencies

When adding dependencies to `Cargo.toml`:

1. **Test reproducibility** before committing
2. **Update documentation** if build process changes
3. **Verify checksums** match after changes
4. **Update CI/CD** if new tools are required

### Modifying Build Environment

When changing `Dockerfile.build`:

1. **Test on multiple systems** (Linux, macOS, Windows)
2. **Verify reproducibility** across environments
3. **Update documentation** with new requirements
4. **Tag new versions** appropriately

## References

- [Reproducible Builds Project](https://reproducible-builds.org/)
- [SLSA Framework](https://slsa.dev/)
- [Docker Best Practices](https://docs.docker.com/develop/dev-best-practices/)
- [pg_tviews Provenance](./provenance.md)

---

**Document Control:**
- **Author**: Lionel Hamayon
- **Reviewers**: Project Contributors
- **Review Cycle**: Annual
- **Distribution**: Public