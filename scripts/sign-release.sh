#!/bin/bash
set -e

VERSION="${1:?Usage: $0 <version>}"
GPG_KEY="${GPG_KEY_ID:-9E57E2899574FA24DB1F1651C8FCB4AB8FDB6DB4}"

echo "Signing pg_tviews v${VERSION} with GPG key ${GPG_KEY}"

# Check if GPG key is available and usable
if ! gpg --list-secret-keys "${GPG_KEY}" >/dev/null 2>&1; then
    echo "❌ GPG key ${GPG_KEY} not found or not usable"
    echo "Please ensure the GPG key is properly configured and available"
    echo "For CI/CD, the key should be available via gpg-agent or GPG_KEY_ID variable"
    exit 1
fi

# Sign tarball
echo "→ Signing release tarball..."
if gpg --local-user "${GPG_KEY}" \
    --armor \
    --detach-sign \
    --output "pg_tviews-${VERSION}.tar.gz.asc" \
    "pg_tviews-${VERSION}.tar.gz"; then
    echo "✓ Tarball signed successfully"
else
    echo "❌ Failed to sign tarball"
    exit 1
fi

# Sign SBOM files (if they exist)
if [ -f "sbom/pg_tviews-${VERSION}.spdx.json" ]; then
    echo "→ Signing SPDX SBOM..."
    gpg --local-user "${GPG_KEY}" \
        --armor \
        --detach-sign \
        --output "sbom/pg_tviews-${VERSION}.spdx.json.asc" \
        "sbom/pg_tviews-${VERSION}.spdx.json"
    echo "✓ SPDX SBOM signed successfully"
fi

if [ -f "sbom/pg_tviews-${VERSION}.cyclonedx.json" ]; then
    echo "→ Signing CycloneDX SBOM..."
    gpg --local-user "${GPG_KEY}" \
        --armor \
        --detach-sign \
        --output "sbom/pg_tviews-${VERSION}.cyclonedx.json.asc" \
        "sbom/pg_tviews-${VERSION}.cyclonedx.json"
    echo "✓ CycloneDX SBOM signed successfully"
fi

# Generate checksums
echo "→ Generating checksums..."
sha256sum pg_tviews-${VERSION}.tar.gz > pg_tviews-${VERSION}.sha256
sha512sum pg_tviews-${VERSION}.tar.gz > pg_tviews-${VERSION}.sha512

# Sign checksums
echo "→ Signing checksums..."
gpg --local-user "${GPG_KEY}" \
    --clearsign \
    --output "pg_tviews-${VERSION}.sha256.asc" \
    "pg_tviews-${VERSION}.sha256"

gpg --local-user "${GPG_KEY}" \
    --clearsign \
    --output "pg_tviews-${VERSION}.sha512.asc" \
    "pg_tviews-${VERSION}.sha512"

echo "✓ All signatures created successfully:"
ls -lh pg_tviews-${VERSION}.* 2>/dev/null || true
ls -lh sbom/pg_tviews-${VERSION}.*.asc 2>/dev/null || true