# Reproducible Builds

`scripts/reproducible-build.sh` builds the extension in a pinned container and packs it
in the release layout as a deterministic tarball: two runs from the same commit give
the same bytes.

## Pinned environment

`docker/dockerfile-build`:

| | |
|---|---|
| Base image | `rust:1.98-slim-bookworm` (the channel of `rust-toolchain.toml`) |
| PostgreSQL | 18 (`postgresql-server-dev-18` from apt.postgresql.org) |
| pgrx | `cargo-pgrx` 0.17.0, `--locked`; the crate pins `pgrx = "=0.17.0"` |
| Dependencies | `Cargo.lock` |
| Flags | `SOURCE_DATE_EPOCH=1`; `RUSTFLAGS` with `opt-level=3`, no debug info, stripped symbols, PIC, RELRO + `BIND_NOW`, non-executable stack |

The container runs `cargo pgrx package --no-default-features --features pg18` into
`/out`. pg_tviews supports PostgreSQL 16, 17 and 18; to build for 16 or 17, change the
`postgresql-*` packages, the `cargo pgrx init` line and the `pg18` feature in the
Dockerfile.

## Build

```bash
./scripts/reproducible-build.sh 0.1.0-beta.27
ls dist/
# pg_tviews-0.1.0-beta.27.tar.gz  build-info.json  SHA256SUMS  SHA512SUMS
```

The tarball has the layout of a release tarball:

```
lib/pg_tviews.so           -> $(pg_config --pkglibdir)
extension/pg_tviews.control, pg_tviews--*.sql
                           -> $(pg_config --sharedir)/extension
```

It is written with `tar --sort=name --mtime=@1 --owner=0 --group=0 --numeric-owner`
and `gzip -n`, so file order, times, owners and the gzip header carry nothing from the
build machine. `build-info.json` records the version, commit, toolchain, pgrx and
PostgreSQL versions (its `timestamp` is the only field that changes between runs; it
is not inside the tarball).

## Check reproducibility

```bash
./scripts/reproducible-build.sh 0.1.0-beta.27 && mv dist dist1
./scripts/reproducible-build.sh 0.1.0-beta.27 && mv dist dist2
cmp dist1/pg_tviews-0.1.0-beta.27.tar.gz dist2/pg_tviews-0.1.0-beta.27.tar.gz
diff dist1/SHA256SUMS dist2/SHA256SUMS
```

Both commands print nothing when the builds match.

## Relation to the official release

The release tarball is built by `.github/workflows/release.yml` on a GitHub runner
(PostgreSQL 18, `cargo pgrx package`), not by this script. Its integrity comes from:

- a Sigstore keyless signature (`cosign sign-blob`) of the tarball and of each SBOM;
- a build provenance attestation of the tarball, made by
  `actions/attest-build-provenance` in the same job, verifiable with
  `gh attestation verify pg_tviews-v<version>.tar.gz --repo fraiseql/pg_tviews`.

See [../security/provenance.md](../security/provenance.md) and
[../security/verify-release.md](../security/verify-release.md). The release job and this
script use different environments and archive flags, so their tarballs are not expected
to be byte-identical: use the script to check that a build from source is
deterministic, and the signature and attestation to check the release.

## Troubleshooting

- **`cargo pgrx package` fails in the container**: rebuild the image without cache
  (`docker build --no-cache -f docker/dockerfile-build .`) and check that
  `rust-toolchain.toml` and the Dockerfile's base image name the same Rust version.
- **Two builds differ**: compare `dist*/build-info.json` (commit, toolchain), then
  unpack both tarballs and `cmp` the files; a difference in `pg_tviews.so` points to a
  toolchain or dependency drift, one in `extension/` to an uncommitted SQL change.
- **Out of memory**: give Docker more memory, or set `CARGO_BUILD_JOBS=1`.
