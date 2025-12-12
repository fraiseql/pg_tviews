# Build Provenance

**Document Version:** 1.0
**Last Updated:** 2025-12-11
**Classification:** Public
**Applicable Standards:** SLSA Level 3, ISO 27001

## Executive Summary

pg_tviews implements SLSA (Supply chain Levels for Software Artifacts) Level 3 build provenance, providing cryptographic proof of build integrity and supply chain security. All releases include verifiable provenance attestations that prove artifacts were built from trusted sources using controlled processes.

## What is Build Provenance?

Build provenance is cryptographic evidence that proves:

- ✅ **What was built**: Exact source code and build inputs
- ✅ **Who built it**: Authorized CI/CD systems (GitHub Actions)
- ✅ **How it was built**: Reproducible build process with locked dependencies
- ✅ **When it was built**: Timestamped build execution
- ✅ **From what source**: Git commit hash and repository verification

## SLSA Framework

pg_tviews achieves **SLSA Level 3** compliance:

### SLSA Level 3 Requirements

| Requirement | Implementation | Status |
|-------------|----------------|--------|
| **Source Integrity** | GitHub branch protection + commit verification | ✅ |
| **Build Isolation** | GitHub Actions with controlled environment | ✅ |
| **Provenance Generated** | SLSA framework provenance attestations | ✅ |
| **Non-falsifiable** | Cryptographically signed provenance | ✅ |
| **Publicly Verifiable** | Public keys and transparency logs | ✅ |

### Build Process Security

```
┌─────────────────┐    ┌──────────────────┐    ┌─────────────────┐
│   Source Code   │ -> │  Build Process   │ -> │   Artifacts     │
│   (GitHub)      │    │  (Reproducible)  │    │   (Signed)      │
│                 │    │                  │    │                 │
│ • Commit hash   │    │ • Docker env     │    │ • Checksums     │
│ • Branch/tag    │    │ • Locked deps    │    │ • Provenance    │
│ • PR reviews    │    │ • Security flags │    │ • Signatures    │
└─────────────────┘    └──────────────────┘    └─────────────────┘
```

## Verify Build Provenance

### Prerequisites

```bash
# Install SLSA verifier
go install github.com/slsa-framework/slsa-verifier/v2/cli/slsa-verifier@latest

# Or download binary
wget https://github.com/slsa-framework/slsa-verifier/releases/download/v2.2.0/slsa-verifier-linux-amd64
sudo mv slsa-verifier-linux-amd64 /usr/local/bin/slsa-verifier
sudo chmod +x /usr/local/bin/slsa-verifier
```

### Verify Release Provenance

```bash
# Download release and provenance
wget https://github.com/fraiseql/pg_tviews/releases/download/v0.1.0/pg_tviews-0.1.0.tar.gz
wget https://github.com/fraiseql/pg_tviews/releases/download/v0.1.0/pg_tviews-0.1.0.intoto.jsonl

# Verify provenance
slsa-verifier verify-artifact \
  --provenance-path pg_tviews-0.1.0.intoto.jsonl \
  --source-uri github.com/fraiseql/pg_tviews \
  --source-tag v0.1.0 \
  pg_tviews-0.1.0.tar.gz
```

**Expected Output:**
```
✓ Verified build using builder https://github.com/slsa-framework/slsa-github-generator/.github/workflows/generator_generic_slsa3.yml@refs/tags/v1.9.0 at commit abc123...
PASSED: Verified SLSA provenance
```

### Verify GitHub Attestations

```bash
# Alternative: Use GitHub CLI for attestations
gh attestation verify pg_tviews-0.1.0.tar.gz \
  --owner fraiseql \
  --repo pg_tviews
```

## Provenance Contents

SLSA provenance includes comprehensive build metadata:

### Source Information
```json
{
  "materials": [
    {
      "uri": "git+https://github.com/fraiseql/pg_tviews@refs/tags/v0.1.0",
      "digest": {
        "sha1": "abc123..."
      }
    }
  ]
}
```

### Build Environment
```json
{
  "builder": {
    "id": "https://github.com/slsa-framework/slsa-github-generator/.github/workflows/generator_generic_slsa3.yml@v1.9.0"
  },
  "metadata": {
    "buildStartedOn": "2025-12-11T10:00:00Z",
    "completeness": {
      "parameters": true,
      "environment": true,
      "materials": true
    }
  }
}
```

### Build Parameters
```json
{
  "buildType": "https://github.com/slsa-framework/slsa-github-generator/generic@v1",
  "parameters": {
    "github": {
      "event_name": "push",
      "repository": "fraiseql/pg_tviews",
      "workflow_ref": "refs/tags/v0.1.0"
    }
  }
}
```

### Output Artifacts
```json
{
  "subjects": [
    {
      "name": "pg_tviews-0.1.0.tar.gz",
      "digest": {
        "sha256": "def456..."
      }
    }
  ]
}
```

## Reproducible Builds

pg_tviews builds are fully reproducible using Docker containers:

### Build Environment
- **Base Image**: `rust:1.91.1-slim-bookworm`
- **PostgreSQL**: Version 17 (locked)
- **pgrx**: Version 0.12.8 (locked)
- **Rust**: Version 1.91.1 (locked)

### Build Script
```bash
# Reproducible build
./scripts/reproducible-build.sh 0.1.0

# Verify against official checksums
curl -fsSL https://github.com/fraiseql/pg_tviews/releases/download/v0.1.0/SHA256SUMS | sha256sum -c
```

### Build Verification
```bash
# Build twice and compare
./scripts/reproducible-build.sh 0.1.0
mv dist dist1

./scripts/reproducible-build.sh 0.1.0
mv dist dist2

# Should be identical
diff -r dist1 dist2
```

## Security Benefits

### Supply Chain Protection

1. **Source Verification**: Git commits are cryptographically verified
2. **Build Isolation**: Builds run in controlled Docker environments
3. **Dependency Locking**: All dependencies are pinned to exact versions
4. **Provenance Tracking**: Complete audit trail from source to artifact

### Attack Mitigation

- **Poisoned Builds**: Provenance proves build integrity
- **Dependency Confusion**: Locked dependencies prevent substitution
- **Malicious Commits**: Git verification prevents unauthorized changes
- **Build Tampering**: Cryptographic signatures detect modifications

### Compliance Benefits

- **SLSA Level 3**: Highest supply chain security level
- **Executive Order 14028**: US federal software security requirements
- **EU Cyber Resilience Act**: European software transparency requirements
- **ISO 27001**: Information security management standards

## For Procurement Teams

### Verification Checklist

When evaluating pg_tviews for enterprise use:

- [x] **SLSA Level 3**: Provenance attestations provided
- [x] **Reproducible Builds**: Docker-based build environment
- [x] **Source Verification**: Git commit verification enabled
- [x] **Build Isolation**: Controlled CI/CD environment
- [x] **Dependency Security**: Automated vulnerability scanning
- [x] **Cryptographic Signing**: Multiple signature methods

### Procurement Assurance

pg_tviews provides **enterprise-grade supply chain security**:

> pg_tviews implements SLSA Level 3 build provenance with reproducible builds in locked Docker environments. All releases include cryptographic provenance attestations that prove artifacts were built from verified source code using controlled processes. Build environments are fully specified and dependencies are locked to prevent supply chain attacks.

> **Signed**: Lionel Hamayon, Project Lead
> **Date**: 2025-12-11
> **Effective**: pg_tviews v0.1.0-beta.1 and later

## Troubleshooting

### Provenance Verification Fails

**Issue**: "Verification failed"
```
slsa-verifier verify-artifact --provenance-path file.intoto.jsonl file.tar.gz
```

**Solutions**:
- Ensure correct provenance file (`.intoto.jsonl` extension)
- Check source URI matches repository exactly
- Verify tag/commit hash is correct
- Ensure artifact checksum matches provenance

### Build Not Reproducible

**Issue**: Local builds don't match official checksums

**Solutions**:
- Use exact Docker environment: `Dockerfile.build`
- Ensure all dependencies are locked
- Check SOURCE_DATE_EPOCH environment variable
- Verify Rust and PostgreSQL versions match

### Missing Provenance

**Issue**: No provenance file in release

**Solutions**:
- Provenance is generated for tagged releases only (`v*` tags)
- Check that SLSA workflow completed successfully
- Verify GitHub Actions permissions include `id-token: write`

## Continuous Improvement

### Roadmap

- [ ] Enhanced build environment locking
- [ ] Binary attestation integration
- [ ] Third-party reproducible build verification
- [ ] Automated provenance validation in CI/CD

### Metrics

Track provenance effectiveness:
- Successful verification rate (>99%)
- Build reproducibility rate (100%)
- Mean time to detect build issues (<5 minutes)
- Audit coverage completeness (100%)

## References

- [SLSA Framework](https://slsa.dev/)
- [SLSA Verifier Documentation](https://github.com/slsa-framework/slsa-verifier)
- [GitHub Attestations](https://docs.github.com/en/actions/security-guides/using-artifact-attestations-to-establish-provenance-for-builds)
- [pg_tviews Reproducible Builds](./reproducible-builds.md)

---

**Document Control:**
- **Author**: Lionel Hamayon
- **Reviewers**: Project Contributors
- **Review Cycle**: Annual
- **Distribution**: Public