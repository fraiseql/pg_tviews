# Verify Release Signatures

**Document Version:** 1.0
**Last Updated:** 2025-12-11
**Classification:** Public
**Applicable Standards:** Sigstore, GPG, SLSA

## Executive Summary

pg_tviews implements comprehensive cryptographic signing using both traditional GPG signatures and modern Sigstore keyless signing. All release artifacts can be independently verified for authenticity and integrity.

## Verification Methods

pg_tviews provides **three layers** of verification:

1. **Sigstore (Keyless)** - Recommended for automated verification
2. **GPG Signatures** - Traditional maintainer-signed artifacts
3. **SHA256/512 Checksums** - Basic integrity verification

## Prerequisites

### Install Required Tools

```bash
# Sigstore Cosign (recommended)
# macOS
brew install cosign

# Linux
wget "https://github.com/sigstore/cosign/releases/download/v2.2.2/cosign-linux-amd64"
sudo mv cosign-linux-amd64 /usr/local/bin/cosign
sudo chmod +x /usr/local/bin/cosign

# GPG (traditional)
# Usually pre-installed on Linux/macOS
gpg --version

# GitHub CLI (for attestations)
# macOS
brew install gh

# Ubuntu/Debian
curl -fsSL https://cli.github.com/packages/githubcli-archive-keyring.gpg | sudo dd of=/usr/share/keyrings/githubcli-archive-keyring.gpg
echo "deb [arch=$(dpkg --print-architecture) signed-by=/usr/share/keyrings/githubcli-archive-keyring.gpg] https://cli.github.com/packages stable main" | sudo tee /etc/apt/sources.list.d/github-cli.list > /dev/null
sudo apt update && sudo apt install gh
```

## 1. Sigstore Verification (Recommended)

**Keyless signing** - No key management required, strongest security guarantees.

### What It Verifies
- ✅ Artifact built by official GitHub Actions CI/CD
- ✅ No tampering since build
- ✅ Build provenance (SLSA Level 3)
- ✅ Transparency log provides immutable audit trail

### Download and Verify Release

```bash
# Download release and signature bundle
wget https://github.com/fraiseql/pg_tviews/releases/download/v0.1.0/pg_tviews-0.1.0.tar.gz
wget https://github.com/fraiseql/pg_tviews/releases/download/v0.1.0/pg_tviews-0.1.0.tar.gz.sigstore

# Verify (keyless)
cosign verify-blob \
  --bundle pg_tviews-0.1.0.tar.gz.sigstore \
  --certificate-identity-regexp "https://github.com/fraiseql/pg_tviews" \
  --certificate-oidc-issuer "https://token.actions.githubusercontent.com" \
  pg_tviews-0.1.0.tar.gz
```

**Expected Output:**
```
Verified OK
```

### Verify SBOM Signatures

```bash
# Download SBOM and signature
wget https://github.com/fraiseql/pg_tviews/releases/download/v0.1.0/sbom/pg_tviews-0.1.0.spdx.json
wget https://github.com/fraiseql/pg_tviews/releases/download/v0.1.0/sbom/pg_tviews-0.1.0.spdx.json.sigstore

# Verify SBOM
cosign verify-blob \
  --bundle pg_tviews-0.1.0.spdx.json.sigstore \
  --certificate-identity-regexp "https://github.com/fraiseql/pg_tviews" \
  --certificate-oidc-issuer "https://token.actions.githubusercontent.com" \
  sbom/pg_tviews-0.1.0.spdx.json
```

## 2. GPG Verification

**Maintainer-signed** - Traditional PGP signatures from project lead.

### What It Verifies
- ✅ Artifact signed by Lionel Hamayon (project lead)
- ✅ No tampering since signing
- ✅ Personal attestation from maintainer
- ✅ Long-term verifiability

### Import Maintainer Key

```bash
# Import from keyserver
gpg --keyserver keys.openpgp.org --recv-keys 9E57E2899574FA24DB1F1651C8FCB4AB8FDB6DB4

# Or import from file
curl -fsSL https://github.com/fraiseql/pg_tviews/releases/download/keys/lionel.asc | gpg --import

# Verify import
gpg --list-keys 9E57E2899574FA24DB1F1651C8FCB4AB8FDB6DB4
```

### Verify Release Tarball

```bash
# Download release and signature
wget https://github.com/fraiseql/pg_tviews/releases/download/v0.1.0/pg_tviews-0.1.0.tar.gz
wget https://github.com/fraiseql/pg_tviews/releases/download/v0.1.0/pg_tviews-0.1.0.tar.gz.asc

# Verify signature
gpg --verify pg_tviews-0.1.0.tar.gz.asc pg_tviews-0.1.0.tar.gz
```

**Expected Output:**
```
gpg: Signature made Thu Dec 11 2025 10:00:00 AM UTC
gpg:                using EDDSA key 9E57E2899574FA24DB1F1651C8FCB4AB8FDB6DB4
gpg: Good signature from "Lionel Hamayon (président, Évolution digitale) <lionel.hamayon@evolution-digitale.fr>"
```

### Verify SBOM Files

```bash
# Download SBOM and signature
wget https://github.com/fraiseql/pg_tviews/releases/download/v0.1.0/sbom/pg_tviews-0.1.0.spdx.json
wget https://github.com/fraiseql/pg_tviews/releases/download/v0.1.0/sbom/pg_tviews-0.1.0.spdx.json.asc

# Verify SBOM signature
gpg --verify sbom/pg_tviews-0.1.0.spdx.json.asc sbom/pg_tviews-0.1.0.spdx.json
```

## 3. Checksum Verification

**Basic integrity** - Always works, but provides weakest guarantees.

### What It Verifies
- ✅ File not corrupted during download
- ✅ File matches what was published
- ⚠️ Does NOT verify who signed it

### Verify SHA256 Checksum

```bash
# Download artifact and checksum
wget https://github.com/fraiseql/pg_tviews/releases/download/v0.1.0/pg_tviews-0.1.0.tar.gz
wget https://github.com/fraiseql/pg_tviews/releases/download/v0.1.0/pg_tviews-0.1.0.sha256

# Verify checksum
sha256sum -c pg_tviews-0.1.0.sha256
```

**Expected Output:**
```
pg_tviews-0.1.0.tar.gz: OK
```

### Verify SHA512 Checksum

```bash
# Download SHA512 checksum
wget https://github.com/fraiseql/pg_tviews/releases/download/v0.1.0/pg_tviews-0.1.0.sha512

# Verify
sha512sum -c pg_tviews-0.1.0.sha512
```

## Verification Matrix

| Artifact Type | Sigstore | GPG | SHA256 | SHA512 |
|---------------|----------|-----|--------|--------|
| **Release tarballs** | ✅ Recommended | ✅ Yes | ✅ Yes | ✅ Yes |
| **SBOM (SPDX)** | ✅ Recommended | ✅ Yes | ✅ Yes | ❌ No |
| **SBOM (CycloneDX)** | ✅ Recommended | ✅ Yes | ✅ Yes | ❌ No |
| **Checksum files** | ❌ No | ✅ Signed | ✅ Self-verifying | ✅ Self-verifying |

## For Procurement & Security Teams

### Verification Requirements Checklist

When evaluating pg_tviews for use in your organization:

- [x] **Build Provenance**: SLSA Level 3 attestations via Sigstore
- [x] **Supply Chain Transparency**: All builds in public CI/CD with full logs
- [x] **Keyless Signing**: No secret key management (Sigstore)
- [x] **Maintainer Signing**: GPG signatures from project lead
- [x] **Standards Compliance**: Sigstore, GPG, SLSA, ISO 27001
- [x] **Independent Verification**: All signatures verifiable without vendor tools

### Automated Verification (CI/CD)

```yaml
# .github/workflows/verify-pg_tviews.yml
name: Verify pg_tviews Dependencies

on: [pull_request]

jobs:
  verify:
    runs-on: ubuntu-latest
    steps:
      - name: Verify pg_tviews Release
        run: |
          # Download latest release
          VERSION=$(curl -s https://api.github.com/repos/fraiseql/pg_tviews/releases/latest | jq -r .tag_name)
          wget "https://github.com/fraiseql/pg_tviews/releases/download/${VERSION}/pg_tviews-${VERSION}.tar.gz"
          wget "https://github.com/fraiseql/pg_tviews/releases/download/${VERSION}/pg_tviews-${VERSION}.tar.gz.sigstore"

          # Verify with Sigstore
          cosign verify-blob \
            --bundle pg_tviews-${VERSION}.tar.gz.sigstore \
            --certificate-identity-regexp "https://github.com/fraiseql/pg_tviews" \
            --certificate-oidc-issuer "https://token.actions.githubusercontent.com" \
            pg_tviews-${VERSION}.tar.gz
```

## Troubleshooting

### Issue: "Verification failed with certificate identity mismatch"

**Cause**: Wrong repository or branch specified.

**Solution**: Ensure you're using:
- Identity regex: `https://github.com/fraiseql/pg_tviews`
- OIDC issuer: `https://token.actions.githubusercontent.com`

### Issue: "gpg: Can't check signature: No public key"

**Cause**: GPG key not imported.

**Solution**: Import the maintainer key first:
```bash
gpg --keyserver keys.openpgp.org --recv-keys 9E57E2899574FA24DB1F1651C8FCB4AB8FDB6DB4
```

### Issue: "cosign: no matching signatures"

**Cause**: Sigstore bundle corrupted or wrong file.

**Solution**: Ensure you're using the correct bundle file (`.sigstore` extension).

### Issue: Checksum verification fails

**Cause**: File corrupted during download.

**Solution**: Re-download the file and checksum, or use a different mirror.

## Trust Model

### Sigstore (CI-signed artifacts)
- **Signer**: GitHub Actions workflow (automated)
- **Identity**: Verified via OIDC token
- **Audit**: Transparency log (Rekor)
- **Verification**: Against GitHub repository identity

### GPG (Maintainer-signed releases)
- **Signer**: Lionel Hamayon (project lead)
- **Key**: Ed25519, fingerprint: 9E57E2899574FA24DB1F1651C8FCB4AB8FDB6DB4
- **Publication**: keys.openpgp.org, GitHub releases
- **Verification**: Traditional PGP web of trust

## Continuous Verification

### Scheduled Verification

```yaml
# Verify pg_tviews releases weekly
name: Verify pg_tviews Releases

on:
  schedule:
    - cron: '0 0 * * 0'  # Every Sunday

jobs:
  verify-latest:
    runs-on: ubuntu-latest
    steps:
      - name: Verify Latest Release
        run: |
          # Get latest release info
          RELEASE=$(curl -s https://api.github.com/repos/fraiseql/pg_tviews/releases/latest)
          VERSION=$(echo $RELEASE | jq -r .tag_name)

          # Download and verify
          wget "https://github.com/fraiseql/pg_tviews/releases/download/${VERSION}/pg_tviews-${VERSION}.tar.gz"
          wget "https://github.com/fraiseql/pg_tviews/releases/download/${VERSION}/pg_tviews-${VERSION}.tar.gz.sigstore"

          cosign verify-blob \
            --bundle pg_tviews-${VERSION}.tar.gz.sigstore \
            --certificate-identity-regexp "https://github.com/fraiseql/pg_tviews" \
            --certificate-oidc-issuer "https://token.actions.githubusercontent.com" \
            pg_tviews-${VERSION}.tar.gz
```

## References

- [Sigstore Documentation](https://www.sigstore.dev/)
- [GPG Manual](https://www.gnupg.org/documentation/)
- [SLSA Framework](https://slsa.dev/)
- [pg_tviews Signing Documentation](./signing.md)
- [pg_tviews SBOM Documentation](./sbom.md)

---

**Document Control:**
- **Author**: Lionel Hamayon
- **Reviewers**: Project Contributors
- **Review Cycle**: Annual
- **Distribution**: Public