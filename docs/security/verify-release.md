# Verify a release

Each GitHub release of pg_tviews carries, for tag `v<version>`:

| Asset | What it is |
|---|---|
| `pg_tviews-v<version>.tar.gz` | The extension: `lib/pg_tviews.so` and `extension/pg_tviews*` (control file, install and upgrade scripts) |
| `pg_tviews-v<version>.tar.gz.sigstore` | Sigstore bundle: keyless signature of the tarball by the release workflow |
| `SHA256SUMS` | SHA-256 of the tarball |
| `pg_tviews-v<version>.spdx.json`, `.cyclonedx.json`, `.sbom.txt` | SBOMs, the first two with `.sigstore` bundles |

The tarball also has a GitHub build-provenance attestation, made by
`actions/attest-build-provenance` where the tarball is built
(`.github/workflows/release.yml`). There is no GPG signature.

## Tools

- [cosign](https://docs.sigstore.dev/cosign/system_config/installation/) for the Sigstore bundles
- [GitHub CLI](https://cli.github.com/) (`gh`) for the attestation

## Checksum

```console
$ VERSION=v0.1.0-beta.27
$ gh release download "$VERSION" -R fraiseql/pg_tviews -p "pg_tviews-$VERSION.tar.gz" -p SHA256SUMS
$ sha256sum -c SHA256SUMS
pg_tviews-v0.1.0-beta.27.tar.gz: OK
```

## Signature (Sigstore)

```console
$ gh release download "$VERSION" -R fraiseql/pg_tviews -p "pg_tviews-$VERSION.tar.gz.sigstore"
$ cosign verify-blob \
    --bundle "pg_tviews-$VERSION.tar.gz.sigstore" \
    --certificate-identity-regexp '^https://github.com/fraiseql/pg_tviews/\.github/workflows/release\.yml@' \
    --certificate-oidc-issuer https://token.actions.githubusercontent.com \
    "pg_tviews-$VERSION.tar.gz"
Verified OK
```

The SBOM files verify the same way with their own `.sigstore` bundles.

## Build provenance

```console
$ gh attestation verify "pg_tviews-$VERSION.tar.gz" -R fraiseql/pg_tviews
```

It checks that the tarball was built by this repository's release workflow from the
tagged commit.

## Reproducing the build

`scripts/reproducible-build.sh <version>` builds the same layout in a pinned
container ([reproducible-builds.md](../development/reproducible-builds.md)). Its
tarball is deterministic for a given toolchain; the released `.so` is built by the
release workflow, so compare the two by content, not by checksum, unless both were
built with the same image.
