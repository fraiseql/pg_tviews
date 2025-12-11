#!/bin/bash
set -e

VERSION="${1:?Usage: $0 <version>}"

echo "Building pg_tviews v${VERSION} reproducibly..."

# Create dist directory
mkdir -p dist

# Build in container for reproducibility
echo "→ Building in reproducible Docker environment..."
docker build -t pg_tviews-builder:${VERSION} -f Dockerfile.build .

# Run build in container
docker run --rm -v $(pwd)/dist:/build/target pg_tviews-builder:${VERSION}

# Generate build metadata
echo "→ Generating build metadata..."
cat > dist/build-info.json <<EOF
{
  "version": "${VERSION}",
  "timestamp": "$(date -u +"%Y-%m-%dT%H:%M:%SZ")",
  "builder": "docker",
  "rust_version": "1.91.1",
  "postgres_version": "17",
  "pgrx_version": "0.12.8",
  "commit": "$(git rev-parse HEAD)",
  "build_environment": "debian-bookworm-slim",
  "reproducible": true
}
EOF

# Generate checksums
echo "→ Generating checksums..."
cd dist
sha256sum pg_tviews-${VERSION}.tar.gz > SHA256SUMS
sha512sum pg_tviews-${VERSION}.tar.gz > SHA512SUMS

echo "✓ Reproducible build completed in dist/"
ls -lh