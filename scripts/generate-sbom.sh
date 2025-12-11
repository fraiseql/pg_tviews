#!/bin/bash
set -e

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VERSION="${1:-$(grep '^version' Cargo.toml | head -1 | cut -d'"' -f2)}"
OUTPUT_DIR="${PROJECT_ROOT}/sbom"

mkdir -p "${OUTPUT_DIR}"

echo "Generating SBOM for pg_tviews v${VERSION}..."

# Generate SPDX format
echo "→ Generating SPDX SBOM..."
cargo sbom --output-format spdx_json_2_3 \
    > "${OUTPUT_DIR}/pg_tviews-${VERSION}.spdx.json"

# Generate CycloneDX format
echo "→ Generating CycloneDX SBOM..."
cargo cyclonedx \
    --format json \
    --spec-version 1.5 \
    --override-filename "pg_tviews-${VERSION}.cyclonedx"
mv "pg_tviews-${VERSION}.cyclonedx.json" "${OUTPUT_DIR}/"

# Generate human-readable summary
echo "→ Generating SBOM summary..."
cat > "${OUTPUT_DIR}/pg_tviews-${VERSION}.sbom.txt" <<EOF
Software Bill of Materials (SBOM)
Package: pg_tviews
Version: ${VERSION}
Generated: $(date -u +"%Y-%m-%dT%H:%M:%SZ")

Rust Dependencies:
$(cargo tree --depth 1 | grep -v '^pg_tviews')

Total Dependencies: $(cargo tree --depth 1 | grep -v '^pg_tviews' | wc -l)

SPDX SBOM: pg_tviews-${VERSION}.spdx.json
CycloneDX SBOM: pg_tviews-${VERSION}.cyclonedx.json
EOF

# After generating Rust SBOM, append system deps
echo "" >> "${OUTPUT_DIR}/pg_tviews-${VERSION}.sbom.txt"
./scripts/sbom-system-deps.sh >> "${OUTPUT_DIR}/pg_tviews-${VERSION}.sbom.txt"

echo "✓ SBOM generated in ${OUTPUT_DIR}/"
ls -lh "${OUTPUT_DIR}/"