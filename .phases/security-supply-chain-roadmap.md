# Security & Supply Chain Roadmap

**Goal**: Production-grade security, SBOM, signing, and supply chain hardening
**Priority**: High (Required for 1.0.0 release)
**Estimated Effort**: 40-60 hours across 5 phases
**Target Timeline**: 4-6 weeks

---

## 📋 Table of Contents

1. [Phase 1: SBOM Generation](#phase-1-sbom-generation)
2. [Phase 2: Artifact Signing & Verification](#phase-2-artifact-signing--verification)
3. [Phase 3: Dependency Security](#phase-3-dependency-security)
4. [Phase 4: Build Provenance & Reproducibility](#phase-4-build-provenance--reproducibility)
5. [Phase 5: Security Policies & Compliance](#phase-5-security-policies--compliance)

---

## Current Status Baseline

### Security Score: 70/100

| Category | Score | Status |
|----------|-------|--------|
| SBOM Generation | 0/100 | ❌ Not implemented |
| Artifact Signing | 0/100 | ❌ Not implemented |
| Dependency Security | 60/100 | ⚠️ Partial (cargo-audit only) |
| Build Reproducibility | 40/100 | ⚠️ Basic (no provenance) |
| Security Policies | 50/100 | ⚠️ Basic docs only |
| Vulnerability Disclosure | 80/100 | ✅ Process documented |
| Code Security | 85/100 | ✅ Good (clippy, no unsafe) |

**Target for 1.0.0**: 95/100 across all categories

---

# Phase 1: SBOM Generation

**Goal**: Generate comprehensive Software Bill of Materials (SBOM)
**Effort**: 8-10 hours
**Priority**: P0 (Critical for supply chain security)

## Objectives

1. Generate SBOM in industry-standard formats (SPDX, CycloneDX)
2. Include all dependencies (Rust crates, system libs, PostgreSQL)
3. Automate SBOM generation in CI/CD
4. Publish SBOM with releases
5. Provide SBOM verification tools

---

## Task 1.1: Choose SBOM Format & Tools

**Effort**: 2 hours

### Decision Matrix

| Format | Standard | Tooling | Adoption | Recommendation |
|--------|----------|---------|----------|----------------|
| **SPDX** | ISO/IEC 5962:2021 | Good | High | ✅ Primary |
| **CycloneDX** | OWASP | Excellent | Growing | ✅ Secondary |
| SWID | ISO/IEC 19770-2 | Poor | Low | ❌ Skip |

**Decision**: Generate both SPDX and CycloneDX for maximum compatibility

### Tools to Install

```bash
# SBOM generation for Rust
cargo install cargo-sbom

# CycloneDX generator
cargo install cargo-cyclonedx

# SPDX tools (if needed)
# Note: cargo-sbom outputs SPDX format
```

**Acceptance Criteria**:
- [ ] Tools evaluated and selected
- [ ] Installation documented in DEVELOPMENT.md
- [ ] Test SBOM generated for current codebase

---

## Task 1.2: Generate Rust Dependencies SBOM

**Effort**: 2 hours

### Implementation

**File**: `scripts/generate-sbom.sh`

```bash
#!/bin/bash
set -e

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VERSION="${1:-$(grep '^version' Cargo.toml | head -1 | cut -d'"' -f2)}"
OUTPUT_DIR="${PROJECT_ROOT}/sbom"

mkdir -p "${OUTPUT_DIR}"

echo "Generating SBOM for pg_tviews v${VERSION}..."

# Generate SPDX format
echo "→ Generating SPDX SBOM..."
cargo sbom --output-format spdx \
    --package-name "pg_tviews" \
    --package-version "${VERSION}" \
    > "${OUTPUT_DIR}/pg_tviews-${VERSION}.spdx.json"

# Generate CycloneDX format
echo "→ Generating CycloneDX SBOM..."
cargo cyclonedx \
    --format json \
    --output-file "${OUTPUT_DIR}/pg_tviews-${VERSION}.cyclonedx.json"

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

echo "✓ SBOM generated in ${OUTPUT_DIR}/"
ls -lh "${OUTPUT_DIR}/"
```

**Acceptance Criteria**:
- [ ] Script generates SPDX JSON
- [ ] Script generates CycloneDX JSON
- [ ] Human-readable summary included
- [ ] All Rust dependencies captured
- [ ] Version information correct

---

## Task 1.3: Include System Dependencies

**Effort**: 2 hours

### PostgreSQL & System Libraries

**File**: `scripts/sbom-system-deps.sh`

```bash
#!/bin/bash
# Extract system dependencies for SBOM

echo "System Dependencies:"
echo "===================="

# PostgreSQL version
PG_VERSION=$(pg_config --version 2>/dev/null || echo "PostgreSQL (runtime)")
echo "- ${PG_VERSION}"

# pgrx version
PGRX_VERSION=$(cargo pgrx --version 2>/dev/null || echo "pgrx 0.12.8")
echo "- ${PGRX_VERSION}"

# System libraries (from ldd on compiled .so)
if [ -f "target/release/libpg_tviews.so" ]; then
    echo ""
    echo "Linked System Libraries:"
    ldd target/release/libpg_tviews.so | grep -E "(libc|libpq|libssl)" | awk '{print "- " $1 " " $3}'
fi

# OS information
echo ""
echo "Build Environment:"
echo "- OS: $(uname -s) $(uname -r)"
echo "- Arch: $(uname -m)"
echo "- Rust: $(rustc --version)"
```

### Enhanced SBOM with System Deps

Update `scripts/generate-sbom.sh` to include:

```bash
# After generating Rust SBOM, append system deps
echo "" >> "${OUTPUT_DIR}/pg_tviews-${VERSION}.sbom.txt"
./scripts/sbom-system-deps.sh >> "${OUTPUT_DIR}/pg_tviews-${VERSION}.sbom.txt"
```

**Acceptance Criteria**:
- [ ] PostgreSQL version captured
- [ ] pgrx version captured
- [ ] System libraries documented
- [ ] Build environment recorded

---

## Task 1.4: Automate SBOM in CI/CD

**Effort**: 2 hours

**File**: `.github/workflows/sbom.yml`

```yaml
name: Generate SBOM

on:
  push:
    tags:
      - 'v*'
  workflow_dispatch:

jobs:
  sbom:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v3

      - name: Install Rust
        uses: actions-rs/toolchain@v1
        with:
          toolchain: stable

      - name: Install SBOM tools
        run: |
          cargo install cargo-sbom
          cargo install cargo-cyclonedx

      - name: Install pgrx
        run: cargo install --locked cargo-pgrx

      - name: Generate SBOM
        run: ./scripts/generate-sbom.sh

      - name: Upload SBOM artifacts
        uses: actions/upload-artifact@v3
        with:
          name: sbom
          path: sbom/*

      - name: Attach SBOM to release
        if: startsWith(github.ref, 'refs/tags/')
        uses: softprops/action-gh-release@v1
        with:
          files: |
            sbom/pg_tviews-*.spdx.json
            sbom/pg_tviews-*.cyclonedx.json
            sbom/pg_tviews-*.sbom.txt
```

**Acceptance Criteria**:
- [ ] Workflow triggers on tags
- [ ] SBOM generated automatically
- [ ] SBOM attached to GitHub releases
- [ ] Artifacts available for download

---

## Task 1.5: SBOM Verification & Documentation

**Effort**: 2 hours

**File**: `docs/security/sbom.md`

```markdown
# Software Bill of Materials (SBOM)

## Overview

pg_tviews provides comprehensive SBOM in multiple formats for supply chain security.

## Download SBOM

### From GitHub Releases

Every release includes SBOM files:
- `pg_tviews-{version}.spdx.json` - SPDX format (ISO standard)
- `pg_tviews-{version}.cyclonedx.json` - CycloneDX format (OWASP)
- `pg_tviews-{version}.sbom.txt` - Human-readable summary

Download from: https://github.com/your-org/pg_tviews/releases

### Verify SBOM

```bash
# Download SBOM
wget https://github.com/your-org/pg_tviews/releases/download/v0.1.0/pg_tviews-0.1.0.spdx.json

# Validate SPDX format
# (Install spdx-tools: pip install spdx-tools)
pyspdxtools -i pg_tviews-0.1.0.spdx.json

# View dependencies
cat pg_tviews-0.1.0.sbom.txt
```

## SBOM Contents

### Rust Dependencies
- All crates from Cargo.lock
- Transitive dependencies
- Version pinning information

### System Dependencies
- PostgreSQL version requirements
- pgrx framework version
- Linked system libraries (libc, libpq, etc.)

### Build Information
- Rust compiler version
- Build environment (OS, arch)
- Build timestamp

## Supply Chain Security

### Dependency Verification

All dependencies are:
- ✅ Sourced from crates.io (official Rust registry)
- ✅ Version-pinned in Cargo.lock
- ✅ Audited with cargo-audit
- ✅ Scanned for vulnerabilities

### SBOM Updates

SBOM is regenerated:
- On every release
- When dependencies change
- Quarterly security reviews

## Compliance

SBOM generation supports:
- **NTIA Minimum Elements** - ✅ Compliant
- **Executive Order 14028** - ✅ Federal requirements
- **ISO/IEC 5962:2021** - ✅ SPDX 2.3
- **OWASP CycloneDX 1.5** - ✅ Security-focused
```

**Acceptance Criteria**:
- [ ] SBOM documentation complete
- [ ] Verification instructions provided
- [ ] Compliance claims documented
- [ ] Links to releases correct

---

## Phase 1 Deliverables

- [ ] `scripts/generate-sbom.sh` - SBOM generation script
- [ ] `scripts/sbom-system-deps.sh` - System dependency scanner
- [ ] `.github/workflows/sbom.yml` - CI/CD automation
- [ ] `docs/security/sbom.md` - User documentation
- [ ] `sbom/` directory with generated SBOMs
- [ ] SBOM attached to v0.1.0-beta.2 release (test)

**Success Metrics**:
- SBOM generated in <2 minutes
- SBOM includes 100% of dependencies
- SPDX validation passes
- Documentation clear and actionable

---

# Phase 2: Artifact Signing & Verification

**Goal**: Cryptographically sign all release artifacts
**Effort**: 10-12 hours
**Priority**: P0 (Critical for authenticity)

## Objectives

1. Sign release binaries with GPG/Sigstore
2. Sign container images with cosign
3. Provide verification instructions
4. Automate signing in CI/CD
5. Publish signatures and public keys

---

## Task 2.1: Choose Signing Strategy

**Effort**: 2 hours

### Decision Matrix

| Method | Technology | Key Management | Verification | Recommendation |
|--------|-----------|----------------|--------------|----------------|
| **GPG** | PGP keys | Manual/Keybase | gpg --verify | ✅ Primary (releases) |
| **Sigstore** | Keyless (OIDC) | Automated | cosign verify | ✅ Primary (CI) |
| **Minisign** | Ed25519 | Simple | minisign -V | ⚠️ Optional |

**Decision**: Use both GPG (for maintainer signatures) and Sigstore (for CI automation)

### Benefits

**GPG Signing**:
- Industry standard for software releases
- Long-term key management
- Personal attestation by maintainers
- Compatible with package managers

**Sigstore/Cosign**:
- Keyless signing (no key management burden)
- Transparency log (immutable audit trail)
- Container image signing
- GitHub Actions integration

**Acceptance Criteria**:
- [ ] Signing strategy documented
- [ ] Tools selected and justified
- [ ] Key management plan defined

---

## Task 2.2: GPG Signing for Releases

**Effort**: 3 hours

### Setup GPG Key

**File**: `docs/security/maintainer-keys.md`

```markdown
# Maintainer GPG Keys

## Lionel Hamayon (Project Lead)

```
pub   rsa4096 2025-12-11 [SC]
      YOUR_KEY_FINGERPRINT_HERE
uid   Lionel Hamayon <lionel.hamayon@evolution-digitale.fr>
sub   rsa4096 2025-12-11 [E]
```

### Import Public Key

```bash
# From keyserver
gpg --keyserver keys.openpgp.org --recv-keys YOUR_KEY_FINGERPRINT

# From file
curl -fsSL https://github.com/your-org/pg_tviews/releases/download/keys/lionel.asc | gpg --import
```

### Verify Releases

```bash
# Download release and signature
wget https://github.com/your-org/pg_tviews/releases/download/v0.1.0/pg_tviews-0.1.0.tar.gz
wget https://github.com/your-org/pg_tviews/releases/download/v0.1.0/pg_tviews-0.1.0.tar.gz.asc

# Verify signature
gpg --verify pg_tviews-0.1.0.tar.gz.asc pg_tviews-0.1.0.tar.gz
```

Expected output:
```
gpg: Signature made Thu Dec 11 2025 10:00:00 AM UTC
gpg:                using RSA key YOUR_KEY_FINGERPRINT
gpg: Good signature from "Lionel Hamayon <lionel.hamayon@evolution-digitale.fr>"
```
```

### Signing Script

**File**: `scripts/sign-release.sh`

```bash
#!/bin/bash
set -e

VERSION="${1:?Usage: $0 <version>}"
GPG_KEY="${GPG_KEY_ID:-YOUR_KEY_FINGERPRINT}"

echo "Signing pg_tviews v${VERSION} with GPG key ${GPG_KEY}"

# Sign tarball
gpg --local-user "${GPG_KEY}" \
    --armor \
    --detach-sign \
    --output "pg_tviews-${VERSION}.tar.gz.asc" \
    "pg_tviews-${VERSION}.tar.gz"

# Sign SBOM
gpg --local-user "${GPG_KEY}" \
    --armor \
    --detach-sign \
    --output "sbom/pg_tviews-${VERSION}.spdx.json.asc" \
    "sbom/pg_tviews-${VERSION}.spdx.json"

# Generate checksums
sha256sum pg_tviews-${VERSION}.tar.gz > pg_tviews-${VERSION}.sha256
sha512sum pg_tviews-${VERSION}.tar.gz > pg_tviews-${VERSION}.sha512

# Sign checksums
gpg --local-user "${GPG_KEY}" \
    --clearsign \
    --output "pg_tviews-${VERSION}.sha256.asc" \
    "pg_tviews-${VERSION}.sha256"

echo "✓ Signatures created:"
ls -lh pg_tviews-${VERSION}.*
```

**Acceptance Criteria**:
- [ ] GPG key generated and published
- [ ] Signing script works end-to-end
- [ ] Verification instructions tested
- [ ] Public key accessible

---

## Task 2.3: Sigstore/Cosign for CI

**Effort**: 3 hours

**File**: `.github/workflows/release.yml` (enhance existing)

```yaml
name: Release

on:
  push:
    tags:
      - 'v*'

jobs:
  build-and-sign:
    runs-on: ubuntu-latest
    permissions:
      contents: write
      id-token: write  # Required for Sigstore

    steps:
      - uses: actions/checkout@v3

      - name: Install cosign
        uses: sigstore/cosign-installer@v3

      - name: Build extension
        run: |
          cargo pgrx package --release
          tar czf pg_tviews-${{ github.ref_name }}.tar.gz target/release/

      - name: Sign with Sigstore (keyless)
        run: |
          cosign sign-blob \
            --bundle pg_tviews-${{ github.ref_name }}.tar.gz.sigstore \
            pg_tviews-${{ github.ref_name }}.tar.gz

      - name: Generate provenance
        uses: actions/attest-build-provenance@v1
        with:
          subject-path: pg_tviews-${{ github.ref_name }}.tar.gz

      - name: Upload to release
        uses: softprops/action-gh-release@v1
        with:
          files: |
            pg_tviews-${{ github.ref_name }}.tar.gz
            pg_tviews-${{ github.ref_name }}.tar.gz.sigstore
```

### Verification Instructions

**File**: `docs/security/verify-release.md`

```markdown
# Verify Release Signatures

## Sigstore Verification (Recommended)

```bash
# Install cosign
go install github.com/sigstore/cosign/cmd/cosign@latest

# Download release and signature bundle
wget https://github.com/your-org/pg_tviews/releases/download/v0.1.0/pg_tviews-v0.1.0.tar.gz
wget https://github.com/your-org/pg_tviews/releases/download/v0.1.0/pg_tviews-v0.1.0.tar.gz.sigstore

# Verify (keyless)
cosign verify-blob \
  --bundle pg_tviews-v0.1.0.tar.gz.sigstore \
  --certificate-identity-regexp "https://github.com/your-org/pg_tviews" \
  --certificate-oidc-issuer "https://token.actions.githubusercontent.com" \
  pg_tviews-v0.1.0.tar.gz
```

## GPG Verification

```bash
# Import maintainer key
gpg --keyserver keys.openpgp.org --recv-keys YOUR_KEY_FINGERPRINT

# Verify signature
gpg --verify pg_tviews-v0.1.0.tar.gz.asc pg_tviews-v0.1.0.tar.gz
```

## Checksum Verification

```bash
# Verify SHA256
sha256sum -c pg_tviews-v0.1.0.sha256

# Verify SHA512
sha512sum -c pg_tviews-v0.1.0.sha512
```
```

**Acceptance Criteria**:
- [ ] Cosign signing in CI works
- [ ] Sigstore bundle attached to releases
- [ ] Verification tested with cosign
- [ ] Documentation clear

---

## Task 2.4: Container Image Signing

**Effort**: 2 hours

**File**: `.github/workflows/docker.yml`

```yaml
name: Build and Sign Container

on:
  push:
    tags:
      - 'v*'

jobs:
  docker:
    runs-on: ubuntu-latest
    permissions:
      contents: read
      packages: write
      id-token: write

    steps:
      - uses: actions/checkout@v3

      - name: Set up Docker Buildx
        uses: docker/setup-buildx-action@v2

      - name: Log in to GitHub Container Registry
        uses: docker/login-action@v2
        with:
          registry: ghcr.io
          username: ${{ github.actor }}
          password: ${{ secrets.GITHUB_TOKEN }}

      - name: Install cosign
        uses: sigstore/cosign-installer@v3

      - name: Build and push
        id: build
        uses: docker/build-push-action@v4
        with:
          context: .
          push: true
          tags: |
            ghcr.io/${{ github.repository }}:${{ github.ref_name }}
            ghcr.io/${{ github.repository }}:latest

      - name: Sign container image
        run: |
          cosign sign --yes \
            ghcr.io/${{ github.repository }}@${{ steps.build.outputs.digest }}

      - name: Generate SBOM for container
        run: |
          syft ghcr.io/${{ github.repository }}:${{ github.ref_name }} \
            -o spdx-json \
            > container-sbom.spdx.json

      - name: Attach SBOM to image
        run: |
          cosign attach sbom --sbom container-sbom.spdx.json \
            ghcr.io/${{ github.repository }}@${{ steps.build.outputs.digest }}
```

**Acceptance Criteria**:
- [ ] Container images signed with cosign
- [ ] Container SBOM attached
- [ ] Signatures verifiable
- [ ] Workflow tested

---

## Task 2.5: Signing Documentation

**Effort**: 2 hours

**File**: `docs/security/signing.md`

```markdown
# Artifact Signing

pg_tviews signs all releases for authenticity and integrity verification.

## Signing Methods

| Artifact Type | Method | Tool | Verification |
|---------------|--------|------|--------------|
| Release tarballs | GPG + Sigstore | gpg, cosign | gpg --verify |
| Container images | Sigstore | cosign | cosign verify |
| SBOM files | GPG | gpg | gpg --verify |
| Git tags | GPG | git | git tag -v |

## Quick Verification

### Verify Tarball

```bash
# Sigstore (keyless, recommended)
cosign verify-blob --bundle pg_tviews-v0.1.0.tar.gz.sigstore pg_tviews-v0.1.0.tar.gz

# GPG (traditional)
gpg --verify pg_tviews-v0.1.0.tar.gz.asc pg_tviews-v0.1.0.tar.gz
```

### Verify Container

```bash
cosign verify \
  --certificate-identity-regexp "https://github.com/your-org/pg_tviews" \
  --certificate-oidc-issuer "https://token.actions.githubusercontent.com" \
  ghcr.io/your-org/pg_tviews:v0.1.0
```

## Trust Model

### Sigstore (CI-signed artifacts)
- Keyless signing via GitHub OIDC
- Signed by: GitHub Actions workflow
- Logged in: Rekor transparency log
- Verifiable: Against GitHub identity

### GPG (Maintainer-signed releases)
- Signed by: Lionel Hamayon
- Key fingerprint: YOUR_KEY_FINGERPRINT
- Published: keys.openpgp.org, GitHub

## Security Guarantees

✅ **Authenticity** - Artifacts built by official CI/maintainers
✅ **Integrity** - No tampering after signing
✅ **Non-repudiation** - Transparency log provides proof
✅ **Freshness** - Timestamps in signatures
```

**Acceptance Criteria**:
- [ ] All signing methods documented
- [ ] Verification examples tested
- [ ] Trust model explained
- [ ] Links to tools provided

---

## Phase 2 Deliverables

- [ ] GPG signing for releases
- [ ] Sigstore signing in CI
- [ ] Container image signing
- [ ] Verification documentation
- [ ] Public keys published
- [ ] All signatures on v0.1.0-beta.2 release

**Success Metrics**:
- 100% of release artifacts signed
- Verification takes <1 minute
- Both GPG and Sigstore verification work
- Documentation rated "easy" by beta users

---

# Phase 3: Dependency Security

**Goal**: Comprehensive dependency scanning and management
**Effort**: 8-10 hours
**Priority**: P1 (Important for security)

## Objectives

1. Automate vulnerability scanning (cargo-audit, Dependabot)
2. Implement dependency update policy
3. Pin critical dependencies
4. Monitor for supply chain attacks
5. Document security advisories

---

## Task 3.1: Automated Vulnerability Scanning

**Effort**: 2 hours

**File**: `.github/workflows/security-audit.yml`

```yaml
name: Security Audit

on:
  push:
    branches: [main, dev]
  pull_request:
  schedule:
    - cron: '0 0 * * *'  # Daily

jobs:
  audit:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v3

      - name: Install Rust
        uses: actions-rs/toolchain@v1
        with:
          toolchain: stable

      - name: Install cargo-audit
        run: cargo install cargo-audit

      - name: Run cargo audit
        run: cargo audit --json > audit-report.json
        continue-on-error: true

      - name: Check for vulnerabilities
        run: |
          if cargo audit | grep -q "error:"; then
            echo "⚠️ Vulnerabilities found!"
            cargo audit
            exit 1
          else
            echo "✅ No vulnerabilities detected"
          fi

      - name: Upload audit report
        uses: actions/upload-artifact@v3
        with:
          name: security-audit
          path: audit-report.json
```

**Acceptance Criteria**:
- [ ] cargo-audit runs daily
- [ ] Vulnerabilities fail CI
- [ ] Reports uploaded as artifacts
- [ ] Notifications configured

---

## Task 3.2: Dependabot Configuration

**Effort**: 2 hours

**File**: `.github/dependabot.yml`

```yaml
version: 2
updates:
  # Rust dependencies
  - package-ecosystem: "cargo"
    directory: "/"
    schedule:
      interval: "weekly"
      day: "monday"
    open-pull-requests-limit: 5
    reviewers:
      - "lionel-hamayon"
    labels:
      - "dependencies"
      - "security"
    commit-message:
      prefix: "chore(deps)"
    # Group minor updates
    groups:
      minor-updates:
        patterns:
          - "*"
        update-types:
          - "minor"
          - "patch"
    # Security updates always separate
    ignore:
      - dependency-name: "*"
        update-types: ["version-update:semver-major"]

  # GitHub Actions
  - package-ecosystem: "github-actions"
    directory: "/"
    schedule:
      interval: "weekly"
    reviewers:
      - "lionel-hamayon"
```

**Acceptance Criteria**:
- [ ] Dependabot configured
- [ ] Weekly dependency PRs
- [ ] Security updates immediate
- [ ] Reviewers assigned

---

## Task 3.3: Dependency Pinning Strategy

**Effort**: 2 hours

**File**: `docs/security/dependency-policy.md`

```markdown
# Dependency Management Policy

## Pinning Strategy

### Critical Dependencies (Exact Pin)

```toml
# Security-critical: exact version
pgrx = "=0.12.8"
pgrx-macros = "=0.12.8"
```

**Rationale**: pgrx is tightly coupled to PostgreSQL internals

### Standard Dependencies (Caret)

```toml
# Regular dependencies: compatible updates
serde = "1.0"
serde_json = "1.0"
regex = "1.0"
```

**Rationale**: Allow security patches within major version

### Development Dependencies (Flexible)

```toml
# Dev-only: more flexible
[dev-dependencies]
pgrx-tests = "0.12"
```

## Update Cadence

| Type | Frequency | Approval | Testing |
|------|-----------|----------|---------|
| **Security patches** | Immediate | Auto-merge | Full suite |
| **Minor updates** | Weekly | Maintainer review | Full suite |
| **Major updates** | Quarterly | Design review | Extensive |

## Vulnerability Response

### Severity Levels

**Critical** (CVSS 9.0-10.0):
- Response time: <24 hours
- Action: Immediate patch or workaround
- Notification: Security advisory

**High** (CVSS 7.0-8.9):
- Response time: <7 days
- Action: Update in next patch release
- Notification: Release notes

**Medium** (CVSS 4.0-6.9):
- Response time: <30 days
- Action: Include in next minor release
- Notification: Changelog

**Low** (CVSS 0.1-3.9):
- Response time: Next quarter
- Action: Regular update cycle
- Notification: Optional

## Blocked Dependencies

Dependencies will NOT be added if they:
- ❌ Have known unpatched critical vulnerabilities
- ❌ Are unmaintained (>1 year no updates)
- ❌ Have unclear licensing
- ❌ Require unsafe code without audit
- ❌ Have excessive transitive dependencies (>50)
```

**Acceptance Criteria**:
- [ ] Dependency policy documented
- [ ] Critical deps pinned in Cargo.toml
- [ ] Update cadence defined
- [ ] Vulnerability response SLA

---

## Task 3.4: Supply Chain Monitoring

**Effort**: 2 hours

### Cargo Vet Setup

```bash
# Install cargo-vet
cargo install cargo-vet

# Initialize
cargo vet init

# Import audits from Mozilla
cargo vet import mozilla
```

**File**: `supply-chain/config.toml`

```toml
[cargo-vet]
version = "0.9"

[imports.mozilla]
url = "https://hg.mozilla.org/mozilla-central/raw-file/tip/supply-chain/audits.toml"

[policy.pgrx]
criteria = "safe-to-deploy"
notes = "Core dependency, manually audited"

[policy]
audit-as-crates-io = true
```

**Acceptance Criteria**:
- [ ] cargo-vet configured
- [ ] Mozilla audits imported
- [ ] Critical crates audited
- [ ] Policy documented

---

## Task 3.5: Security Advisory Process

**Effort**: 2 hours

**File**: `SECURITY.md` (root)

```markdown
# Security Policy

## Supported Versions

| Version | Supported |
|---------|-----------|
| 0.1.x (beta) | ✅ Security fixes |
| 0.0.x (alpha) | ❌ Unsupported |

## Reporting a Vulnerability

**DO NOT** open public GitHub issues for security vulnerabilities.

### Private Reporting

1. **GitHub Security Advisories** (Preferred):
   - Go to: https://github.com/your-org/pg_tviews/security/advisories
   - Click "Report a vulnerability"
   - Provide details

2. **Email** (Alternative):
   - Send to: security@your-domain.com
   - Use PGP key: YOUR_KEY_FINGERPRINT
   - Include: Affected version, exploit details, suggested fix

### Response Timeline

- **Acknowledgment**: Within 48 hours
- **Initial assessment**: Within 7 days
- **Patch development**: Depends on severity
- **Public disclosure**: After patch released (coordinated)

### Disclosure Policy

We follow **coordinated disclosure**:
1. Vulnerability reported privately
2. Patch developed and tested
3. Security advisory published
4. Patch released
5. Public announcement (24 hours after release)

## Security Updates

Subscribe to:
- GitHub Watch → Custom → Security alerts
- Release notifications
- Security mailing list (coming soon)
```

**Acceptance Criteria**:
- [ ] SECURITY.md in root
- [ ] Private reporting enabled
- [ ] Response timeline defined
- [ ] Disclosure policy clear

---

## Phase 3 Deliverables

- [ ] `.github/workflows/security-audit.yml` - Daily scanning
- [ ] `.github/dependabot.yml` - Automated updates
- [ ] `docs/security/dependency-policy.md` - Management policy
- [ ] `supply-chain/config.toml` - cargo-vet config
- [ ] `SECURITY.md` - Vulnerability reporting

**Success Metrics**:
- Vulnerabilities detected within 24 hours
- Security patches released within SLA
- Zero high/critical unfixed vulnerabilities
- Dependency audit coverage >80%

---

# Phase 4: Build Provenance & Reproducibility

**Goal**: Verifiable, reproducible builds with SLSA compliance
**Effort**: 10-12 hours
**Priority**: P1 (Important for supply chain)

## Objectives

1. Implement reproducible builds
2. Generate build provenance (SLSA)
3. Publish provenance attestations
4. Document verification process
5. Achieve SLSA Level 3

---

## Task 4.1: Reproducible Builds

**Effort**: 4 hours

### Lock Build Environment

**File**: `Dockerfile.build`

```dockerfile
# Reproducible build environment
FROM rust:1.75.0-slim-bookworm

# Exact PostgreSQL version
RUN apt-get update && apt-get install -y \
    postgresql-15=15.5-1.pgdg110+1 \
    postgresql-server-dev-15=15.5-1.pgdg110+1 \
    && rm -rf /var/lib/apt/lists/*

# Exact pgrx version
RUN cargo install --locked --version 0.12.8 cargo-pgrx

# Set up reproducible environment
ENV SOURCE_DATE_EPOCH=1
ENV RUSTFLAGS="-C opt-level=3 -C debuginfo=0 -C strip=symbols"

WORKDIR /build
COPY . .

# Reproducible build command
CMD ["cargo", "pgrx", "package", "--release"]
```

### Build Script

**File**: `scripts/reproducible-build.sh`

```bash
#!/bin/bash
set -e

VERSION="${1:?Usage: $0 <version>}"

echo "Building pg_tviews v${VERSION} reproducibly..."

# Build in container
docker build -t pg_tviews-builder:${VERSION} -f Dockerfile.build .
docker run --rm -v $(pwd)/dist:/build/target pg_tviews-builder:${VERSION}

# Generate build info
cat > dist/build-info.json <<EOF
{
  "version": "${VERSION}",
  "timestamp": "$(date -u +"%Y-%m-%dT%H:%M:%SZ")",
  "builder": "docker",
  "rust_version": "1.75.0",
  "postgres_version": "15.5",
  "pgrx_version": "0.12.8",
  "commit": "$(git rev-parse HEAD)"
}
EOF

# Checksums
cd dist
sha256sum pg_tviews-${VERSION}.tar.gz > SHA256SUMS
sha512sum pg_tviews-${VERSION}.tar.gz > SHA512SUMS
```

**Acceptance Criteria**:
- [ ] Builds produce identical outputs
- [ ] Build environment locked
- [ ] Build metadata captured
- [ ] Verification tested

---

## Task 4.2: SLSA Provenance Generation

**Effort**: 3 hours

**File**: `.github/workflows/slsa-provenance.yml`

```yaml
name: SLSA Provenance

on:
  push:
    tags:
      - 'v*'

permissions:
  id-token: write
  contents: write

jobs:
  provenance:
    uses: slsa-framework/slsa-github-generator/.github/workflows/generator_generic_slsa3.yml@v1.9.0
    with:
      base64-subjects: "${{ needs.build.outputs.hashes }}"
      upload-assets: true

  build:
    runs-on: ubuntu-latest
    outputs:
      hashes: ${{ steps.hash.outputs.hashes }}
    steps:
      - uses: actions/checkout@v3

      - name: Build
        run: ./scripts/reproducible-build.sh ${{ github.ref_name }}

      - name: Generate hashes
        id: hash
        run: |
          cd dist
          echo "hashes=$(sha256sum * | base64 -w0)" >> $GITHUB_OUTPUT

      - name: Upload artifacts
        uses: actions/upload-artifact@v3
        with:
          name: artifacts
          path: dist/*
```

**Acceptance Criteria**:
- [ ] SLSA provenance generated
- [ ] Provenance attached to releases
- [ ] Level 3 compliance achieved
- [ ] Verification works

---

## Task 4.3: Provenance Verification

**Effort**: 2 hours

**File**: `docs/security/provenance.md`

```markdown
# Build Provenance

pg_tviews provides SLSA Level 3 build provenance for all releases.

## What is Provenance?

Build provenance is cryptographic proof of:
- ✅ What was built
- ✅ Who built it
- ✅ How it was built
- ✅ When it was built
- ✅ From what source

## Verify Provenance

### Install slsa-verifier

```bash
go install github.com/slsa-framework/slsa-verifier/v2/cli/slsa-verifier@latest
```

### Verify Release

```bash
# Download release and provenance
wget https://github.com/your-org/pg_tviews/releases/download/v0.1.0/pg_tviews-0.1.0.tar.gz
wget https://github.com/your-org/pg_tviews/releases/download/v0.1.0/pg_tviews-0.1.0.intoto.jsonl

# Verify
slsa-verifier verify-artifact \
  --provenance-path pg_tviews-0.1.0.intoto.jsonl \
  --source-uri github.com/your-org/pg_tviews \
  --source-tag v0.1.0 \
  pg_tviews-0.1.0.tar.gz
```

Expected output:
```
✓ Verified build using builder https://github.com/slsa-framework/slsa-github-generator/.github/workflows/generator_generic_slsa3.yml@refs/tags/v1.9.0 at commit abc123
PASSED: Verified SLSA provenance
```

## Provenance Contents

Provenance includes:
- **Source**: Git commit, branch, tag
- **Builder**: GitHub Actions workflow
- **Build steps**: Exact commands run
- **Environment**: OS, tools, dependencies
- **Materials**: All inputs (source, tools)
- **Outputs**: Checksums of artifacts

## SLSA Level 3

pg_tviews achieves SLSA Level 3:
- ✅ Source integrity
- ✅ Isolated build
- ✅ Provenance generated
- ✅ Non-falsifiable provenance
- ✅ Publicly verifiable
```

**Acceptance Criteria**:
- [ ] Verification instructions clear
- [ ] Examples tested
- [ ] SLSA level documented
- [ ] Benefits explained

---

## Task 4.4: Reproducibility Documentation

**Effort**: 2 hours

**File**: `docs/development/reproducible-builds.md`

```markdown
# Reproducible Builds

## Build Locally

```bash
# Clone at specific tag
git clone --depth 1 --branch v0.1.0 https://github.com/your-org/pg_tviews.git
cd pg_tviews

# Build reproducibly
./scripts/reproducible-build.sh 0.1.0

# Verify checksum matches official release
curl -fsSL https://github.com/your-org/pg_tviews/releases/download/v0.1.0/SHA256SUMS | sha256sum -c
```

## Verify Build is Reproducible

```bash
# Build twice
./scripts/reproducible-build.sh 0.1.0
mv dist dist1

./scripts/reproducible-build.sh 0.1.0
mv dist dist2

# Compare (should be identical)
diff -r dist1 dist2
```

## Factors Affecting Reproducibility

### Controlled
- ✅ Rust version (locked)
- ✅ PostgreSQL version (locked)
- ✅ pgrx version (locked)
- ✅ Source code (git commit)
- ✅ Build flags (documented)
- ✅ Timestamps (normalized)

### Not Controlled
- ⚠️ System time zone (use UTC)
- ⚠️ Locale (use C)
- ⚠️ File ordering (sorted)

## Build Environment

See `Dockerfile.build` for exact specifications:
- Rust 1.75.0
- Debian Bookworm (12)
- PostgreSQL 15.5
- pgrx 0.12.8
```

**Acceptance Criteria**:
- [ ] Local build instructions work
- [ ] Reproducibility verified
- [ ] Environment documented
- [ ] Troubleshooting included

---

## Task 4.5: Build Security Hardening

**Effort**: 1 hour

**File**: `.cargo/config.toml`

```toml
[build]
rustflags = [
  # Security hardening
  "-C", "relocation-model=pic",
  "-C", "link-arg=-Wl,-z,relro,-z,now",
  "-C", "link-arg=-Wl,-z,noexecstack",

  # Optimization
  "-C", "opt-level=3",
  "-C", "lto=fat",
  "-C", "codegen-units=1",

  # Reproducibility
  "-C", "embed-bitcode=no",
  "-C", "debuginfo=0",
]

[profile.release]
strip = true
panic = "abort"
```

**Acceptance Criteria**:
- [ ] Security flags enabled
- [ ] Position-independent code (PIC)
- [ ] Stack protection enabled
- [ ] No executable stack

---

## Phase 4 Deliverables

- [ ] `Dockerfile.build` - Reproducible environment
- [ ] `scripts/reproducible-build.sh` - Build script
- [ ] `.github/workflows/slsa-provenance.yml` - CI automation
- [ ] `docs/security/provenance.md` - User guide
- [ ] `docs/development/reproducible-builds.md` - Developer guide
- [ ] `.cargo/config.toml` - Security hardening

**Success Metrics**:
- 100% reproducible builds
- SLSA Level 3 achieved
- Verification takes <2 minutes
- Build time <10 minutes

---

# Phase 5: Security Policies & Compliance

**Goal**: Security governance and compliance documentation
**Effort**: 8-10 hours
**Priority**: P2 (Nice to have for 1.0)

## Objectives

1. Document security architecture
2. Create incident response plan
3. Establish security review process
4. Compliance documentation (GDPR, SOC2)
5. Security training materials

---

## Task 5.1: Security Architecture Documentation

**Effort**: 3 hours

**File**: `docs/security/architecture.md`

```markdown
# Security Architecture

## Threat Model

### Assets
1. **Extension Code** - Rust code, SQL functions
2. **User Data** - Data in TVIEWs
3. **Build Artifacts** - Releases, containers
4. **Signing Keys** - GPG, Sigstore

### Threats
1. **Supply Chain Attacks** - Compromised dependencies
2. **Code Injection** - Malicious SQL/Rust code
3. **Data Leakage** - Unauthorized data access
4. **Build Tampering** - Modified releases

### Mitigations
- ✅ SBOM + signing → Supply chain
- ✅ Code review + tests → Code injection
- ✅ Row-level security → Data leakage
- ✅ Provenance → Build tampering

## Security Boundaries

```
┌─────────────────────────────────────────┐
│ PostgreSQL Server (Trust Boundary)      │
│ ┌─────────────────────────────────────┐ │
│ │ pg_tviews Extension                 │ │
│ │ ┌─────────────────────────────────┐ │ │
│ │ │ User SQL Functions              │ │ │
│ │ │ - pg_tviews_create()            │ │ │
│ │ │ - pg_tviews_drop()              │ │ │
│ │ └─────────────────────────────────┘ │ │
│ │                                     │ │
│ │ ┌─────────────────────────────────┐ │ │
│ │ │ Internal Rust Code              │ │ │
│ │ │ - Trigger handlers (unsafe)     │ │ │
│ │ │ - DDL processing                │ │ │
│ │ └─────────────────────────────────┘ │ │
│ └─────────────────────────────────────┘ │
└─────────────────────────────────────────┘
```

## Security Controls

| Control | Implementation | Effectiveness |
|---------|----------------|---------------|
| **Input validation** | SQL parameter validation | High |
| **Output encoding** | PostgreSQL type system | High |
| **Authentication** | PostgreSQL RBAC | High |
| **Authorization** | GRANT/REVOKE on functions | High |
| **Audit logging** | pg_tview_audit_log table | Medium |
| **Encryption** | TLS (PostgreSQL config) | High |
| **Rate limiting** | None (PostgreSQL handles) | N/A |

## Security Assumptions

We assume:
- ✅ PostgreSQL server is trusted
- ✅ Database administrators are trusted
- ✅ TLS is configured for network access
- ✅ File system permissions are correct
- ⚠️ Application users may be untrusted

## Secure Coding Practices

### Rust Code
- ✅ No `unsafe` in core logic (only pgrx FFI)
- ✅ All `unsafe` blocks documented
- ✅ Clippy lints enabled
- ✅ Bounds checking enforced
- ✅ Memory safety guaranteed

### SQL Code
- ✅ Parameterized queries only
- ✅ No dynamic SQL construction
- ✅ Schema-qualified names
- ✅ Explicit type casts
```

**Acceptance Criteria**:
- [ ] Threat model documented
- [ ] Security boundaries defined
- [ ] Controls listed
- [ ] Assumptions stated

---

## Task 5.2: Incident Response Plan

**Effort**: 2 hours

**File**: `docs/security/incident-response.md`

```markdown
# Security Incident Response Plan

## Incident Types

### Type 1: Vulnerability Disclosed
**Scenario**: Security researcher reports vulnerability

**Response**:
1. **Acknowledge** (24h): Confirm receipt, assign severity
2. **Assess** (48h): Reproduce, analyze impact
3. **Develop** (7d): Create and test patch
4. **Release** (14d): Security advisory + patch
5. **Disclose** (30d): Public writeup

### Type 2: Exploit Observed
**Scenario**: Active exploitation detected

**Response**:
1. **Alert** (Immediate): Notify maintainers
2. **Mitigate** (1h): Document workaround
3. **Patch** (24h): Emergency release
4. **Communicate** (2h): Security advisory
5. **Post-mortem** (7d): Root cause analysis

### Type 3: Dependency Vulnerability
**Scenario**: Upstream dependency has CVE

**Response**:
1. **Assess** (24h): Check if pg_tviews affected
2. **Update** (48h): Bump dependency version
3. **Test** (24h): Full regression suite
4. **Release** (72h): Patch version
5. **Notify** (24h): Release notes

## Contact Tree

```
Security Report
       ↓
Lead Maintainer
       ↓
    ┌──┴──┐
Security Team   Core Team
    ↓              ↓
Patch Dev      Testing
    ↓              ↓
    └──────┬───────┘
           ↓
     Coordinated
      Disclosure
```

## Communication Templates

### Acknowledgment (to reporter)
```
Subject: [pg_tviews] Security Report Acknowledgment

Thank you for reporting a potential security issue in pg_tviews.

Report ID: SEC-2025-001
Received: 2025-12-11
Assigned: Lionel Hamayon

We will assess this report and respond within 7 days with:
- Severity classification
- Impact assessment
- Proposed timeline for fix

We follow coordinated disclosure and ask that you:
- Do not publish details until we release a fix
- Allow us reasonable time to develop a patch
- Coordinate disclosure timing with us

Thank you for helping keep pg_tviews secure.
```

### Security Advisory Template
```
# Security Advisory: [Title]

**ID**: GHSA-xxxx-xxxx-xxxx
**Severity**: High (CVSS 7.5)
**Published**: 2025-12-11
**Patched**: v0.1.1

## Summary
Brief description of vulnerability

## Impact
Who is affected and how

## Patches
Fixed in v0.1.1

## Workarounds
Temporary mitigation if any

## Credits
Reporter name (if they wish to be credited)
```
```

**Acceptance Criteria**:
- [ ] Response procedures defined
- [ ] Contact tree established
- [ ] Templates created
- [ ] Timeline SLAs set

---

## Task 5.3: Security Review Process

**Effort**: 2 hours

**File**: `docs/development/security-review.md`

```markdown
# Security Review Process

## When Required

Security review is REQUIRED for:
- ✅ All code touching `unsafe` blocks
- ✅ New external-facing functions
- ✅ Authentication/authorization changes
- ✅ Cryptographic operations
- ✅ Input parsing/validation
- ✅ Dependencies with CVEs

## Review Checklist

### Code Review
- [ ] No new `unsafe` without justification
- [ ] Input validation for all user-controlled data
- [ ] SQL injection prevention (parameterized queries)
- [ ] Integer overflow checks
- [ ] Array bounds checking
- [ ] Error handling doesn't leak sensitive info
- [ ] No hardcoded secrets/keys

### Testing
- [ ] Fuzzing for parsers
- [ ] Negative test cases (invalid input)
- [ ] Boundary conditions
- [ ] Concurrent access
- [ ] Resource exhaustion
- [ ] Privilege escalation attempts

### Documentation
- [ ] Security implications documented
- [ ] Threat model updated if needed
- [ ] User-facing security guidance

## Security Approval

Changes require approval from:
- **Normal PRs**: 1 maintainer
- **Security-critical**: 2 maintainers + security review
- **Unsafe code**: Explicit security sign-off

## Security Testing

```bash
# Run security tests
cargo test --features security-tests

# Fuzzing (if applicable)
cargo fuzz run parser

# Static analysis
cargo clippy -- -D warnings
cargo audit

# Dynamic analysis (Valgrind)
valgrind --leak-check=full ./target/debug/pg_tviews_test
```
```

**Acceptance Criteria**:
- [ ] Review triggers defined
- [ ] Checklist comprehensive
- [ ] Approval requirements clear
- [ ] Testing procedures documented

---

## Task 5.4: Compliance Documentation

**Effort**: 2 hours

**File**: `docs/security/compliance.md`

```markdown
# Compliance & Standards

## Standards Compliance

### SLSA (Supply Chain Levels for Software Artifacts)
- **Level**: 3 (Hardened Builds)
- **Provenance**: ✅ Generated
- **Build isolation**: ✅ GitHub Actions
- **Verification**: ✅ Public

### SBOM Standards
- **SPDX 2.3**: ✅ ISO/IEC 5962:2021
- **CycloneDX 1.5**: ✅ OWASP standard
- **NTIA Minimum Elements**: ✅ Compliant

### Executive Order 14028
- **SBOM requirement**: ✅ Provided
- **Supply chain security**: ✅ Implemented
- **Vulnerability disclosure**: ✅ Process defined

## Data Protection

### GDPR Compliance

pg_tviews does NOT:
- ❌ Collect personal data
- ❌ Store user information
- ❌ Phone home / telemetry

pg_tviews DOES:
- ✅ Process data in PostgreSQL (customer-controlled)
- ✅ Provide audit logging (optional)
- ✅ Support encryption (via PostgreSQL)

**Guidance for users**:
- Personal data in TVIEWs is your responsibility
- Use PostgreSQL RLS for access control
- Enable audit logging if required
- Encrypt data at rest (PostgreSQL config)

### SOC2 Considerations

For organizations using pg_tviews:

**Security (CC6)**:
- ✅ Access control via PostgreSQL RBAC
- ✅ Audit logging available
- ✅ Vulnerability management process

**Availability (CC7)**:
- ✅ Disaster recovery documented
- ✅ Backup procedures provided
- ⚠️ HA/failover (PostgreSQL responsibility)

**Processing Integrity (CC8)**:
- ✅ Data consistency guarantees
- ✅ Transaction safety
- ✅ Error handling

## Licensing Compliance

- **License**: MIT (permissive)
- **Dependencies**: All MIT/Apache-2.0
- **No copyleft**: ✅ Safe for commercial use
- **Patent grant**: ✅ Included in license

## Certifications

pg_tviews does NOT hold certifications (Common Criteria, FIPS, etc.)

For certified environments:
- Run on certified PostgreSQL
- Use certified OS (RHEL, etc.)
- Follow your organization's security policies
```

**Acceptance Criteria**:
- [ ] SLSA compliance documented
- [ ] GDPR guidance provided
- [ ] SOC2 considerations listed
- [ ] Licensing compliance clear

---

## Task 5.5: Security Training & Awareness

**Effort**: 1 hour

**File**: `docs/security/README.md`

```markdown
# Security Documentation

## For Users

- **[SBOM](sbom.md)** - Software Bill of Materials
- **[Verify Releases](verify-release.md)** - Check signatures
- **[Secure Deployment](../operations/security.md)** - Best practices

## For Contributors

- **[Security Review](../development/security-review.md)** - Code review process
- **[Incident Response](incident-response.md)** - Emergency procedures
- **[Reproducible Builds](../development/reproducible-builds.md)** - Build locally

## For Security Researchers

- **[SECURITY.md](../../SECURITY.md)** - Vulnerability reporting
- **[Threat Model](architecture.md#threat-model)** - Security boundaries
- **[Bug Bounty](https://github.com/your-org/pg_tviews/security/policy)** - Rewards

## Quick Links

| I want to... | See |
|--------------|-----|
| Report a vulnerability | [SECURITY.md](../../SECURITY.md) |
| Verify a release | [verify-release.md](verify-release.md) |
| Check dependencies | [SBOM](sbom.md) |
| Understand threats | [architecture.md](architecture.md) |
| Review security | [security-review.md](../development/security-review.md) |
```

**Acceptance Criteria**:
- [ ] Security docs organized
- [ ] Navigation clear
- [ ] Audience-specific
- [ ] Links working

---

## Phase 5 Deliverables

- [ ] `docs/security/architecture.md` - Threat model
- [ ] `docs/security/incident-response.md` - Response plan
- [ ] `docs/development/security-review.md` - Review process
- [ ] `docs/security/compliance.md` - Standards compliance
- [ ] `docs/security/README.md` - Security hub

**Success Metrics**:
- All security docs published
- Review process followed on 5+ PRs
- Incident response tested (tabletop)
- Compliance claims verified

---

# Overall Roadmap Summary

## Timeline

| Phase | Duration | Priority | Dependencies |
|-------|----------|----------|--------------|
| Phase 1: SBOM | 2 weeks | P0 | None |
| Phase 2: Signing | 2 weeks | P0 | Phase 1 |
| Phase 3: Dependencies | 1-2 weeks | P1 | None |
| Phase 4: Provenance | 2 weeks | P1 | Phase 2 |
| Phase 5: Policies | 1-2 weeks | P2 | Phase 1-4 |

**Total**: 8-10 weeks (part-time) or 4-6 weeks (full-time)

## Success Criteria

### Minimum (Required for 1.0.0)
- ✅ SBOM generated and published
- ✅ All releases signed (GPG + Sigstore)
- ✅ Vulnerability scanning automated
- ✅ Security policy documented

### Target (Best-in-class)
- ✅ SLSA Level 3 compliance
- ✅ Reproducible builds
- ✅ Dependency auditing (cargo-vet)
- ✅ Incident response tested

### Stretch (Industry-leading)
- ✅ Formal security audit
- ✅ Bug bounty program
- ✅ SOC2 compliance support
- ✅ FIPS-validated builds

## Dependencies & Risks

### External Dependencies
- **Sigstore infrastructure**: Relies on Rekor transparency log
- **GitHub Actions**: For automated signing
- **Keyservers**: For GPG key distribution

### Risks
- ⚠️ **Key management**: GPG key security critical
- ⚠️ **CI compromise**: Could sign malicious artifacts
- ⚠️ **Supply chain**: Upstream vulnerabilities

### Mitigations
- Multiple signing methods (GPG + Sigstore)
- Branch protection + required reviews
- Daily vulnerability scanning
- Reproducible builds for verification

---

## Next Steps

1. **Create GitHub issues** for each phase
2. **Schedule** phases based on priority
3. **Assign** security champion (if team)
4. **Set milestones** for 1.0.0 release
5. **Budget** for potential security audit

---

**Document Version**: 1.0
**Last Updated**: December 11, 2025
**Owner**: Lionel Hamayon
**Status**: Planning
