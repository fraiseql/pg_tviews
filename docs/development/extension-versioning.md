# Extension versioning and upgrade scripts

How pg_tviews versions its extension SQL, and what a pull request that changes it must
do. The design is in [ADR 0136](../adr/0136-tool-facing-surface.md), Decision 3.

## The version is the release

`pg_tviews.control` says `default_version = '@CARGO_VERSION@'`: the extension version is
the crate version, and `pg_extension.extversion` says which release a database runs.

`main` always carries the **next** release. Right after a release is tagged:

1. bump `version` in `Cargo.toml` to the next release;
2. create `sql/pg_tviews--<released>--<next>.sql` holding only a header comment.

Tagging a release is then only stamping the CHANGELOG (`## [Unreleased]` becomes
`## [<version>] - <date>`) and tagging `v<version>`. The version check accepts
`## [Unreleased]` while no tag `v<version>` exists; the release workflow refuses a tag
whose version has no stamped CHANGELOG heading.

## A pull request that changes the extension SQL

The extension SQL is every `extension_sql!` block and the name, signature or attributes
of every `#[pg_extern]` / `#[pg_trigger]`. A PR that changes any of it also:

- **adds the matching statements to the pending upgrade script**, so that an install of
  the previous release, updated with `ALTER EXTENSION pg_tviews UPDATE`, has exactly the
  catalog of a fresh install. CI checks this (`upgrade-path`);
- **bumps the catalog revision**: `revision::CATALOG_REVISION` in `src/revision.rs`, and
  in the upgrade script
  `CREATE OR REPLACE FUNCTION @extschema@.pg_tviews_catalog_revision() … AS 'SELECT <n>'`
  (the install script's definition in `src/metadata.rs` says the same number). Bump it
  once per release: if the pending script already redefines the function, keep that
  number;
- **ends the script with** `UPDATE @extschema@.pg_tview_meta SET needs_reregister = true;`
  when it changes what registration derives (cascade paths, fan-out patches, direct
  maps, aggregate embeds…). Upgrade scripts never re-derive metadata themselves: that
  would run the new analysis inside `ALTER EXTENSION`, lock every base table, and let
  one failing TVIEW fail the whole update. `pg_tviews_reregister_all()` does it after.

## Rules for scripts

- One script per pair of consecutive releases; PostgreSQL chains them. No skip scripts.
- A released script is never edited. A mistake is fixed in the next one.
- A release with no SQL change still ships a script (a comment), so the chain has no gap.
- Internal tables only gain columns, added with a default, never renamed or dropped:
  `pg_tview_meta` is a configuration table that `pg_dump` dumps, and a dump of one
  release must restore into the next. A column that is no longer needed stays, ignored.
- A `#[pg_extern]` whose SQL signature changes gets a new Rust name (so a new C symbol)
  while its SQL name stays. A database whose catalog still declares the old signature
  then fails with `could not find function "…" in file` instead of calling the new
  code with arguments of the wrong type.
- The **install** script never uses `CREATE OR REPLACE`, `IF NOT EXISTS` or
  `ADD COLUMN IF NOT EXISTS`: a name collision in the fixed schema `tviews` must be an
  error, not a silent reuse. Upgrade scripts may use `CREATE OR REPLACE` to redefine a
  function they change.

## The library/catalog guard

Before pg_tviews does real work in a backend (the row trigger, a non-empty flush, a
`pg_tviews_*` function, a `CREATE TABLE tv_* AS` conversion) it compares
`CATALOG_REVISION` with `tviews.pg_tviews_catalog_revision()`. On a mismatch it raises
`pg_tviews library catalog revision <n> does not match the installed extension (<m>)`
with the hint `run ALTER EXTENSION pg_tviews UPDATE`; a `0.1.0` catalog, which has no
revision function, gets the migration script as the hint instead. Only a match is
cached, so the first call after the update passes. The check is skipped while the
extension's own install or upgrade script runs. `pg_tviews_health_check()` reports the
comparison without raising.

## CI

- **`upgrade-path`** installs the previous release from its release tarball, creates the
  fixture TVIEWs (`test/upgrade/fixtures.sql`), installs the commit under test, runs
  `ALTER EXTENSION pg_tviews UPDATE` and `pg_tviews_reregister_all(strict => true)`,
  compares `test/upgrade/catalog_snapshot.sql` with a fresh install, and writes to the
  base tables (`test/upgrade/verify.sql`). It fails when the versions differ and no
  upgrade script exists, and skips while the previous release predates this policy.
- **`migrate-from-0.1.0`** does the same from `v0.1.0-beta.19` with
  `scripts/migrate-from-0.1.0.sql` instead of `ALTER EXTENSION`. It is removed once no
  supported release predates the policy.

Locally, with the previous release installed:

```bash
test/upgrade/upgrade_check.sh before
# stop PostgreSQL, install the commit under test, start it
test/upgrade/upgrade_check.sh after update      # or: after migrate
```

## Release tarball

`pg_tviews-v<version>.tar.gz` holds:

```
lib/pg_tviews.so              -> $(pg_config --pkglibdir)/
extension/pg_tviews.control   -> $(pg_config --sharedir)/extension/
extension/pg_tviews--*.sql    -> $(pg_config --sharedir)/extension/
```
