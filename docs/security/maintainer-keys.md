# Maintainer GPG Keys

## Lionel Hamayon (Project Lead)

```
pub   ed25519 2024-05-08 [C] [expires: 2026-03-29]
      9E57E2899574FA24DB1F1651C8FCB4AB8FDB6DB4
uid           [ultimate] Lionel Hamayon (président, Évolution digitale) <lionel.hamayon@evolution-digitale.fr>
uid           [ultimate] Lionel Hamayon (personal) <lionel.h@mayon.email>
uid           [ultimate] Lionel Hamayon (personal - master) <lionel.h@mayon.email>
sub   cv25519 2024-05-08 [E] [expires: 2026-03-29]
```

### Import Public Key

```bash
# From keyserver
gpg --keyserver keys.openpgp.org --recv-keys 9E57E2899574FA24DB1F1651C8FCB4AB8FDB6DB4

# From file
curl -fsSL https://github.com/your-org/pg_tviews/releases/download/keys/lionel.asc | gpg --import

# Verify import
gpg --list-keys 9E57E2899574FA24DB1F1651C8FCB4AB8FDB6DB4
```

### Key Details

- **Algorithm**: Ed25519 (modern, secure)
- **Key ID**: 9E57E2899574FA24DB1F1651C8FCB4AB8FDB6DB4
- **Created**: May 8, 2024
- **Expires**: March 29, 2026
- **Owner**: Lionel Hamayon (Project Lead)
- **Email**: lionel.hamayon@evolution-digitale.fr

### Key Usage

This key is used to sign:
- Release tarballs
- SBOM files
- Checksum files
- Git tags

### Verification Example

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
gpg:                using EDDSA key 9E57E2899574FA24DB1F1651C8FCB4AB8FDB6DB4
gpg: Good signature from "Lionel Hamayon (président, Évolution digitale) <lionel.hamayon@evolution-digitale.fr>"
```

## Key Management

### Key Rotation

Keys are rotated every 2 years or when:
- Key compromise is suspected
- Team membership changes
- Security best practices require rotation

### Backup

Master keys are backed up in encrypted form and stored securely.

### Revocation

If a key needs to be revoked:
1. Generate revocation certificate
2. Publish revocation to keyservers
3. Announce key rotation in release notes
4. Update documentation with new key

## Trust Model

This GPG key provides:
- **Authenticity**: Proof that artifacts come from the legitimate maintainer
- **Integrity**: Assurance that artifacts haven't been tampered with
- **Non-repudiation**: Maintainers cannot deny signing legitimate releases
- **Long-term verification**: Signatures remain valid even after key expiration