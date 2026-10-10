# Release notes

What changed in each release is in [CHANGELOG.md](../CHANGELOG.md): one section per
release, with the breaking changes and upgrade notes first. Each tagged release also
has a GitHub release page with the same text and the signed artifacts.

The current release line is `0.1.0-beta.*`. pg_tviews is in beta: a function, setting
or catalog column can be removed in the next release.
[DEPRECATION_WARNINGS.md](DEPRECATION_WARNINGS.md) lists every removal and its
replacement.

## Requirements

| | |
|---|---|
| PostgreSQL | 16, 17, 18 |
| pgrx | 0.17.0 |
| Rust | 1.98 (`rust-toolchain.toml`), to build from source |
| jsonb_delta | optional: without it rows are recomputed, never patched |

The library must be in `shared_preload_libraries`
([installation](getting-started/installation.md)).

## Release artifacts

A release (`.github/workflows/release.yml`, run on a `v*` tag) publishes:

- `pg_tviews-v<version>.tar.gz`, built for PostgreSQL 18: `lib/pg_tviews.so` goes to
  `$(pg_config --pkglibdir)`, `extension/pg_tviews*` (control file, install script and
  upgrade scripts) to `$(pg_config --sharedir)/extension`;
- a Sigstore bundle for the tarball and for each SBOM (SPDX, CycloneDX);
- a build provenance attestation for the tarball, from
  `actions/attest-build-provenance`.

[docs/security/verify-release.md](security/verify-release.md) says how to check them;
[docs/development/reproducible-builds.md](development/reproducible-builds.md) how to
rebuild the extension yourself. The crate is also published to crates.io.

## Upgrading

`ALTER EXTENSION pg_tviews UPDATE` is supported from 0.1.0-beta.20 on, one upgrade
script per release ([extension versioning](development/extension-versioning.md)).
Read the release's "Upgrade notes" in the CHANGELOG first: some releases ask for
`SELECT * FROM tviews.pg_tviews_reregister_all();` or a full refresh afterwards.

## Reporting issues

Open an issue at <https://github.com/fraiseql/pg_tviews/issues> with the PostgreSQL
version, `SELECT extversion FROM pg_extension WHERE extname = 'pg_tviews'`, a minimal
reproduction and the full error (SQLSTATE, DETAIL, HINT).
