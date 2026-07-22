#!/bin/bash
set -euo pipefail

VERSION="${1:?Usage: $0 <version>}"

echo "Building pg_tviews ${VERSION} reproducibly..."

# Clean output dirs. `pkg/` collects the staged extension tree emitted by the
# container's `cargo pgrx package --out-dir /out`; `dist/` holds only the release
# artifacts (the SLSA workflow hashes `dist/*`, so it must contain files only).
rm -rf pkg dist
mkdir -p pkg dist

echo "→ Building in the pinned Docker environment..."
docker build -t pg_tviews-builder:${VERSION} -f docker/dockerfile-build .

# The image CMD packages into /out; mount pkg/ there to collect the artifacts.
docker run --rm -v "$(pwd)/pkg:/out" pg_tviews-builder:${VERSION}

echo "→ Packaging artifacts..."
tar czf "dist/pg_tviews-${VERSION}.tar.gz" -C pkg .

echo "→ Generating build metadata..."
cat > dist/build-info.json <<EOF
{
  "version": "${VERSION}",
  "timestamp": "$(date -u +"%Y-%m-%dT%H:%M:%SZ")",
  "builder": "docker",
  "rust_version": "1.91.1",
  "postgres_version": "18",
  "pgrx_version": "0.17.0",
  "commit": "$(git rev-parse HEAD)",
  "build_environment": "debian-bookworm-slim",
  "reproducible": true
}
EOF

echo "→ Generating checksums..."
(
  cd dist
  sha256sum "pg_tviews-${VERSION}.tar.gz" > SHA256SUMS
  sha512sum "pg_tviews-${VERSION}.tar.gz" > SHA512SUMS
)

echo "✓ Reproducible build completed in dist/"
ls -lh dist
