# Artifact Signing

**Document Version:** 1.0
**Last Updated:** 2025-12-11
**Classification:** Public
**Applicable Standards:** Sigstore, GPG, SLSA, ISO 27001

## Executive Summary

pg_tviews implements comprehensive cryptographic signing for all release artifacts using both traditional GPG signatures and modern Sigstore keyless signing. This provides multiple layers of verification for authenticity, integrity, and supply chain security.

## Signing Methods

pg_tviews signs all release artifacts using complementary approaches:

| Artifact Type | GPG Signature | Sigstore | Checksums |
|---------------|---------------|----------|-----------|
| **Release tarballs** | ✅ Detached (.asc) | ✅ Bundle (.sigstore) | ✅ SHA256/SHA512 |
| **SBOM (SPDX)** | ✅ Detached (.asc) | ✅ Bundle (.sigstore) | ✅ SHA256 |
| **SBOM (CycloneDX)** | ✅ Detached (.asc) | ✅ Bundle (.sigstore) | ✅ SHA256 |
| **Checksum files** | ✅ Clearsign (.asc) | ❌ N/A | ✅ Self-verifying |

## Quick Verification

### Sigstore (Recommended)

```bash
# Verify release tarball
cosign verify-blob --bundle pg_tviews-v0.1.0.tar.gz.sigstore pg_tviews-v0.1.0.tar.gz

# Verify SBOM
cosign verify-blob --bundle sbom/pg_tviews-v0.1.0.spdx.json.sigstore sbom/pg_tviews-v0.1.0.spdx.json
```

### GPG (Traditional)

```bash
# Import maintainer key
gpg --keyserver keys.openpgp.org --recv-keys 9E57E2899574FA24DB1F1651C8FCB4AB8FDB6DB4

# Verify release
gpg --verify pg_tviews-v0.1.0.tar.gz.asc pg_tviews-v0.1.0.tar.gz

# Verify SBOM
gpg --verify sbom/pg_tviews-v0.1.0.spdx.json.asc sbom/pg_tviews-v0.1.0.spdx.json
```

## Trust Model

### Sigstore (CI-Signed Artifacts)

**Keyless signing** via GitHub Actions OIDC:

- **Signer**: GitHub Actions workflow (automated)
- **Identity**: Verified via OIDC token from `github.com/fraiseql/pg_tviews`
- **Audit Trail**: Immutable transparency log (Rekor)
- **Verification**: Against GitHub repository identity
- **Security**: No long-term secrets, ephemeral keys

**Benefits:**
- ✅ No key management burden
- ✅ Automated signing in CI/CD
- ✅ Transparency log provides non-repudiation
- ✅ Works with container registries and package managers

### GPG (Maintainer-Signed Releases)

**Personal signing** by project lead:

- **Signer**: Lionel Hamayon (project lead)
- **Key Algorithm**: Ed25519 (modern, secure)
- **Key Fingerprint**: 9E57E2899574FA24DB1F1651C8FCB4AB8FDB6DB4
- **Key Publication**: keys.openpgp.org, GitHub releases
- **Verification**: Traditional PGP web of trust

**Benefits:**
- ✅ Personal attestation from maintainer
- ✅ Long-term verifiability (survives key rotation)
- ✅ Compatible with existing PGP tooling
- ✅ Industry standard for software releases

## Security Guarantees

### Authenticity
- **Sigstore**: Proves artifact built by official CI/CD pipeline
- **GPG**: Proves artifact signed by legitimate maintainer
- **Combined**: Provides both automated and personal verification

### Integrity
- **Sigstore**: Cryptographic proof of no tampering since signing
- **GPG**: Cryptographic proof of no tampering since signing
- **Checksums**: Fast verification of file corruption

### Non-Repudiation
- **Sigstore**: Transparency log prevents denial of signing
- **GPG**: Digital signature prevents denial of signing
- **Timestamps**: Both methods include signing timestamps

### Freshness
- **Sigstore**: Includes certificate validity periods
- **GPG**: Includes signature timestamps
- **Combined**: Multiple independent timestamp sources

## Implementation Details

### CI/CD Signing Process

```yaml
# .github/workflows/release.yml
- name: Sign with Sigstore (keyless)
  run: |
    cosign sign-blob \
      --bundle pg_tviews-${{ github.ref_name }}.tar.gz.sigstore \
      pg_tviews-${{ github.ref_name }}.tar.gz

- name: Generate provenance
  uses: actions/attest-build-provenance@v1
  with:
    subject-path: pg_tviews-${{ github.ref_name }}.tar.gz
```

### Manual Signing Process

```bash
# scripts/sign-release.sh
# 1. Sign tarball with GPG
gpg --local-user "${GPG_KEY}" --armor --detach-sign --output "pg_tviews-${VERSION}.tar.gz.asc" "pg_tviews-${VERSION}.tar.gz"

# 2. Sign SBOM files
gpg --local-user "${GPG_KEY}" --armor --detach-sign --output "sbom/pg_tviews-${VERSION}.spdx.json.asc" "sbom/pg_tviews-${VERSION}.spdx.json"

# 3. Generate and sign checksums
sha256sum pg_tviews-${VERSION}.tar.gz > pg_tviews-${VERSION}.sha256
gpg --local-user "${GPG_KEY}" --clearsign --output "pg_tviews-${VERSION}.sha256.asc" "pg_tviews-${VERSION}.sha256"
```

## Release Artifact Structure

```
pg_tviews-v0.1.0/
├── pg_tviews-v0.1.0.tar.gz                    # Release tarball
├── pg_tviews-v0.1.0.tar.gz.sigstore          # Sigstore bundle
├── pg_tviews-v0.1.0.sha256                   # SHA256 checksum
├── pg_tviews-v0.1.0.sha256.asc               # GPG-signed checksum
├── sbom/
│   ├── pg_tviews-v0.1.0.spdx.json           # SPDX SBOM
│   ├── pg_tviews-v0.1.0.spdx.json.sigstore  # Sigstore bundle
│   ├── pg_tviews-v0.1.0.spdx.json.asc       # GPG signature
│   ├── pg_tviews-v0.1.0.cyclonedx.json      # CycloneDX SBOM
│   ├── pg_tviews-v0.1.0.cyclonedx.json.sigstore  # Sigstore bundle
│   ├── pg_tviews-v0.1.0.cyclonedx.json.asc   # GPG signature
│   └── pg_tviews-v0.1.0.sbom.txt            # Human-readable summary
└── keys/
    └── lionel.asc                            # GPG public key
```

## Compliance Standards

### International Standards
- **ISO 27001**: Information security management
- **SLSA Level 3**: Supply chain Levels for Software Artifacts
- **Sigstore**: Keyless signing specification

### Regulatory Compliance
- **US EO 14028**: Federal software supply chain security
- **EU Cyber Resilience Act**: Software transparency requirements
- **PCI-DSS 4.0**: Payment card industry security standards

### Industry Standards
- **OpenSSF**: Open Source Security Foundation best practices
- **CISA**: Cybersecurity and Infrastructure Security Agency guidance
- **OWASP**: CycloneDX SBOM standard

## For Different User Types

### For Developers
```bash
# Quick verification during development
cosign verify-blob --bundle pg_tviews-v0.1.0.tar.gz.sigstore pg_tviews-v0.1.0.tar.gz
```

### For DevOps/Security Teams
```bash
# Comprehensive verification
#!/bin/bash
VERSION="v0.1.0"

# Verify Sigstore signatures
cosign verify-blob --bundle pg_tviews-${VERSION}.tar.gz.sigstore pg_tviews-${VERSION}.tar.gz
cosign verify-blob --bundle sbom/pg_tviews-${VERSION}.spdx.json.sigstore sbom/pg_tviews-${VERSION}.spdx.json

# Verify GPG signatures
gpg --verify pg_tviews-${VERSION}.tar.gz.asc pg_tviews-${VERSION}.tar.gz
gpg --verify sbom/pg_tviews-${VERSION}.spdx.json.asc sbom/pg_tviews-${VERSION}.spdx.json

# Verify checksums
sha256sum -c pg_tviews-${VERSION}.sha256
```

### For Procurement Officers
- **Sigstore**: Provides automated, auditable build provenance
- **GPG**: Provides personal maintainer attestation
- **SBOM**: Provides complete software component inventory
- **Standards**: Meets all major regulatory requirements

## Troubleshooting

### Sigstore Issues

**Issue**: "no matching signatures"
```
cosign verify-blob --bundle file.sigstore file
```

**Solutions**:
- Ensure correct bundle file (`.sigstore` extension)
- Check certificate identity regex matches repository
- Verify OIDC issuer is correct

### GPG Issues

**Issue**: "gpg: Can't check signature: No public key"
```bash
# Import key from keyserver
gpg --keyserver keys.openpgp.org --recv-keys 9E57E2899574FA24DB1F1651C8FCB4AB8FDB6DB4
```

**Issue**: "gpg: BAD signature"
- File may be corrupted - re-download
- Wrong signature file - check filename matches

### General Issues

**Issue**: Verification fails for older releases
- Older releases may not have all signature types
- Check release notes for supported verification methods
- Use SHA256 checksums as fallback

## Future Enhancements

### Planned Improvements
- [ ] Container image signing (Docker)
- [ ] Git tag signing
- [ ] Additional SBOM formats (SPDX XML)
- [ ] Hardware Security Module (HSM) integration
- [ ] FIPS-compliant signing

### Roadmap
- **Phase 3**: Dependency security scanning
- **Phase 4**: Build provenance (SLSA)
- **Phase 5**: Security policies and compliance

## References

- [Sigstore Documentation](https://www.sigstore.dev/)
- [GPG Manual](https://www.gnupg.org/documentation/)
- [SLSA Framework](https://slsa.dev/)
- [pg_tviews Verification Guide](./verify-release.md)
- [pg_tviews SBOM Guide](./sbom.md)
- [Maintainer Keys](./maintainer-keys.md)

---

**Document Control:**
- **Author**: Lionel Hamayon
- **Reviewers**: Project Contributors
- **Review Cycle**: Annual
- **Distribution**: Public