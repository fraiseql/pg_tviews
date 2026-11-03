# Extension versioning and the update script

How pg_tviews versions its extension SQL, and what a pull request that changes it must
do. The design is in [ADR 0136](../adr/0136-tool-facing-surface.md), Decision 3.

## The version is the release

`pg_tviews.control` says `default_version = '@CARGO_VERSION@'`: the extension version is
the crate version, and `pg_extension.extversion` says which release a database runs.

`main` always carries the **next** release. Right after a release is tagged:

1. bump `version` in `Cargo.toml` to the next release;
2. replace the update script with `sql/pg_tviews--<released>--<next>.sql`, holding a
   header comment and the re-derivation that ends every update script (below): the
   repository keeps one update script, from the previous release;
3. point `revision::PREVIOUS_REVISION` and the release named in `revision::RECREATE`
   (`src/revision.rs`) at the release just tagged.

Tagging a release is then only stamping the CHANGELOG (`## [Unreleased]` becomes
`## [<version>] - <date>`) and tagging `v<version>`. The release workflow first runs
confiture's TVIEW suites (`.github/workflows/confiture.yml`) at the confiture commit
pinned in `CONFITURE_REF`, and refuses to publish when they fail, skip a test, or
nothing is pinned: move the pin to the confiture commit that matches the release before tagging. The version check accepts
`## [Unreleased]` while no tag `v<version>` exists; the release workflow refuses a tag
whose version has no stamped CHANGELOG heading.

## A pull request that changes the extension SQL

The extension SQL is every `extension_sql!` block and the name, signature or attributes
of every `#[pg_extern]` / `#[pg_trigger]`. A PR that changes any of it also:

- **adds the matching statements to the pending update script**, so that an install of
  the previous release, updated with `ALTER EXTENSION pg_tviews UPDATE`, has exactly the
  catalog of a fresh install. CI checks this (`upgrade-path`);
- **bumps the catalog revision**: `revision::CATALOG_REVISION` in `src/revision.rs`, and
  in the update script
  `CREATE OR REPLACE FUNCTION @extschema@.pg_tviews_catalog_revision() … AS 'SELECT <n>'`
  (the install script's definition in `sql/install/02-catalog.sql` says the same number). Bump it
  once per release: if the pending script already redefines the function, keep that
  number.

The update script ends by re-deriving every TVIEW:

```sql
UPDATE @extschema@.pg_tview_meta SET needs_reregister = true;
SELECT pg_catalog.count(*) FROM @extschema@.pg_tviews_reregister_all(strict => true);
```

`pg_tviews_reregister_all()` runs each registration as the TVIEW's owner, with the new
release's analysis, and clears `needs_reregister`; with `strict` a TVIEW that no longer
analyses fails the whole update, named with its error, and nothing is left
half-migrated. After a successful update no TVIEW has `needs_reregister`.

## Rules for scripts

- One update script, from the previous release to the next. An install of an older
  release, `0.1.0` included, has no update: it is dropped and recreated
  ([Upgrading pg_tviews](../operations/upgrade.md#from-an-older-release)).
- A released script is never edited. A mistake is fixed in the next one.
- A release with no SQL change still ships a script (a comment and the re-derivation),
  so the previous release always has an update.
- Plans of another version are never read. The stored plan carries `PLAN_VERSION`
  (`src/lineage/plan.rs`); the library reads only the current one, and a change to what
  the plan holds bumps it. The re-derivation at the end of the update script rewrites
  every plan, so no reader of an older shape is kept.
- Internal tables only gain columns, added with a default, never renamed or dropped:
  `pg_tview_meta` is a configuration table that `pg_dump` dumps, and a dump of one
  release must restore into the next. A column that is no longer needed stays, ignored.
  A dump restores into the release that made it; update or recreate from there.
- A `#[pg_extern]` whose SQL signature changes gets a new Rust name (so a new C symbol)
  while its SQL name stays, so a catalog that still declares the old signature never
  calls the new code with arguments of the wrong type.
- A SQL function renamed keeps its Rust name, so its C symbol, and takes the new SQL
  name from `#[pg_extern(name = …)]`.
- Every C symbol the previous catalog binds stays exported. The symbol of a removed
  function, or of an old signature, moves to `src/api/removed.rs`, where it raises the
  revision error (SQLSTATE `55000`, hint `run ALTER EXTENSION pg_tviews UPDATE`), never
  PostgreSQL's `could not find function "…" in file`. CI checks this (`upgrade-path`).
- The **install** script never uses `CREATE OR REPLACE`, `IF NOT EXISTS` or
  `ADD COLUMN IF NOT EXISTS`: a name collision in the fixed schema `tviews` must be an
  error, not a silent reuse. The update script may use `CREATE OR REPLACE` to redefine a
  function it changes.

## The library/catalog guard

Before pg_tviews does real work in a backend (the row trigger, a non-empty flush, a
`pg_tviews_*` function, a `CREATE TABLE tv_* AS` conversion) it compares
`CATALOG_REVISION` with `tviews.pg_tviews_catalog_revision()`. On a mismatch it raises
`pg_tviews library catalog revision <n> does not match the installed extension (<m>)`
with the hint `run ALTER EXTENSION pg_tviews UPDATE`. A catalog older than the previous
release, or a `0.1.0` catalog, which has no revision function, gets `revision::RECREATE`
as the hint instead: `this release updates only from <previous>: DROP EXTENSION
pg_tviews CASCADE, CREATE EXTENSION pg_tviews and create the TVIEWs again`. Only a match is
cached, so the first call after the update passes. The check is skipped while the
extension's own install or update script runs. `pg_tviews_check_health()` reports the
comparison without raising.

## CI

- **`upgrade-path`** installs the previous release on PostgreSQL 18 and 16 (its release
  tarball on 18, its tag built on 16), creates the fixture TVIEWs
  (`test/upgrade/fixtures.sql`), installs the commit under test, checks that every C
  symbol the previous catalog binds is exported and that the window refuses row,
  statement, partition and `TRUNCATE` writes, runs `ALTER EXTENSION pg_tviews UPDATE`,
  compares `test/upgrade/catalog_snapshot.sql` with a fresh install, and writes to the
  base tables (`test/upgrade/verify.sql`). It fails when the versions differ and the
  update script from the previous release is missing.
- The same job from `v0.1.0-beta.25`, on PostgreSQL 18, drops and recreates instead of
  updating.

Locally, with the previous release installed:

```bash
test/upgrade/upgrade_check.sh before
# stop PostgreSQL, install the commit under test, start it
test/upgrade/upgrade_check.sh after update      # or, from an older release: after recreate
```

## Release tarball

`pg_tviews-v<version>.tar.gz` holds:

```
lib/pg_tviews.so              -> $(pg_config --pkglibdir)/
extension/pg_tviews.control   -> $(pg_config --sharedir)/extension/
extension/pg_tviews--*.sql    -> $(pg_config --sharedir)/extension/
```
