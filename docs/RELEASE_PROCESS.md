# Release process

`main` always carries the next release: its version is in `Cargo.toml`, its changes
under `## [Unreleased]` in `CHANGELOG.md`, and its upgrade script in
`sql/pg_tviews--<previous>--<next>.sql`. How the extension SQL is versioned, and what a
pull request that changes it must do, is in
[development/extension-versioning.md](development/extension-versioning.md).

## Before tagging

- [ ] CI is green on the commit to tag: every SQL suite on PostgreSQL 16, 17 and 18
      (`ci.yml`), lints and unit tests (`clippy.yml`), the upgrade matrix
      (`upgrade.yml`), the nightly assertion build (`nightly.yml`).
- [ ] `CONFITURE_REF` in `.github/workflows/confiture.yml` pins the confiture commit
      that matches this release.
- [ ] A change that could cost time has been compared against a build of the previous
      release on one machine (`test/sql/real_benchmark/README.md`).
- [ ] `CHANGELOG.md`: `## [Unreleased]` becomes `## [<version>] - <date>`, and the
      compare link at the bottom is added.

## Tagging

Push the tag `v<version>`. `release.yml` then:

1. runs the CI, the lints and confiture's TVIEW suites on the tagged commit;
2. checks that the tag matches `Cargo.toml` and that `CHANGELOG.md` has the stamped
   heading;
3. builds `pg_tviews-v<version>.tar.gz` (`lib/`, `extension/`), its SBOMs, signs them
   with Sigstore, attests the tarball's build provenance, and creates the GitHub
   release;
4. publishes the crate to crates.io.

Nothing is built or published when a step before it fails.

## After the release

Open the next one:

```console
$ scripts/bump-version.sh <next-version>
```

It sets `Cargo.toml` to the next version, creates the empty
`sql/pg_tviews--<released>--<next>.sql`, and points the README's version badge and
"Current Version" line at the release just tagged. Review the diff and commit it.
