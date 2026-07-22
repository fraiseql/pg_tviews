# Docker

This directory holds Docker tooling for pg_tviews.

## `dockerfile-build` — reproducible build image

A pinned image for building the extension reproducibly (fixed toolchain,
`SOURCE_DATE_EPOCH`, deterministic `RUSTFLAGS`). It produces a release package
via `cargo pgrx package`:

```bash
# from the repository root
docker build -f docker/dockerfile-build -t pg_tviews_build .
docker run --rm -v "$PWD/out:/build/target" pg_tviews_build
```

Use this when you need a byte-reproducible build artifact independent of the host
toolchain. It does not run PostgreSQL or any tests.

## Benchmarks are not run from Docker

There is **no Docker path for the benchmark**. The current benchmark harness is
[`test/sql/real_benchmark/`](../test/sql/real_benchmark/README.md); it runs
against a real PostgreSQL 18 cluster with `pg_tviews` and `jsonb_delta` installed.

Reproduce it one of two ways:

- **Local pgrx cluster** — see
  [docs/benchmarks/running-benchmarks.md](../docs/benchmarks/running-benchmarks.md).
- **Fresh Ubuntu box** — `test/sql/real_benchmark/provision-ubuntu.sh` provisions
  PostgreSQL 18 (PGDG) + pgrx 0.17.0 + both extensions, the exact stack behind
  [docs/benchmarks/results.md](../docs/benchmarks/results.md).

> The previous `dockerfile-benchmarks` image and its helper scripts were removed:
> they built the retired `test/sql/comprehensive_benchmarks/` harness, which
> targeted a `pg_tviews.enable_tview(...)` function the extension never exported
> and so never ran against the real extension.
