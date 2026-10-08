# ADR 0136: A stable surface for tools

- Status: Accepted; amended for #181 (see [Amendment](#amendment-181-pg_tviews-objects-live-in-tviews))
  and for the maintenance functions (see [Amendment](#amendment-maintenance-functions-and-the-real-privilege-model))
- Issues: #136 (fixed schema, privileges), #139 (health check), #137 (upgrade path),
  #133 (read contract), #134 (create-or-replace)

## Context

confiture generates migrations for TVIEWs and reads back what a database registers. Today
it has to guess at four things that pg_tviews never promised:

1. **Where the extension lives.** `CREATE EXTENSION` installs into the first schema on
   `search_path`, so it is `public` in one database and `app` in another. A few objects
   ignore that and hardcode `public.` (`pg_tviews_performance_summary`, the audit-log
   index).
2. **Which version is installed.** The extension SQL has said `0.1.0` since the first
   beta. Two databases reporting `0.1.0` can hold different catalogs, and the only upgrade
   is to drop every TVIEW and start again.
3. **What a TVIEW is.** confiture reads `pg_tview_meta`, an internal table that changed
   shape in #103 and will change again.
4. **How to create one.** The CTAS interception guesses intent from a name prefix and a
   statement shape (#79, #80, #82, #95, #96). `pg_tviews_create()` is explicit but fails
   when the TVIEW exists, so a migration that uses it cannot be re-applied, and it takes
   no storage options.

A fifth problem surfaced while designing the first: **only the extension owner can use a
database with pg_tviews installed.** Everything pg_tviews does runs as the current role:
- An `INSERT` into a `tb_*` table fails with `permission denied for table pg_tview_meta`,
  because the trigger reads the catalog as the writing role.
- A `DROP … CASCADE` that takes a `v_*` with it runs `pg_tviews_drop` from the `sql_drop`
  event trigger, and then its fallback `DELETE FROM pg_tview_meta`, as the dropping role
  (`src/metadata.rs:183-201`). Both fail, and the user's `DROP` aborts.
- With `pg_tviews.audit_enabled`, the flush inserts into `pg_tview_audit_log` as the
  writing role.
- The refresh writes `tv_*` as the writing role, so every writer would need write access to
  every TVIEW its changes reach, and could then edit TVIEW rows directly.

This ADR fixes each of these; the delivery order is at the end.

## Decision 1: the extension lives in schema `tviews` (#136)

**Schema.** `pg_tviews.control` gets `schema = tviews`. `CREATE EXTENSION pg_tviews`
creates the schema when missing and puts every object there; `WITH SCHEMA other` is refused
by PostgreSQL. The name cannot be `pg_tviews`: the `pg_` prefix is reserved for system
schemas and `CREATE SCHEMA pg_tviews` fails.

- The extension SQL uses `@extschema@` everywhere, or no qualification inside the install
  script (where `search_path` is the target schema). No `public.` remains.
- The schema is fixed, so `utils::ext_schema()` returns the constant `tviews` and its
  per-backend cache (`EXT_SCHEMA_CACHE`, `src/utils.rs:342`) goes away. A cache filled
  before a migration could otherwise still say `public`.
- Every reference from outside the extension's own script is qualified: the base-table
  triggers (`tviews.pg_tview_trigger_handler()`, `tviews.pg_tview_flush_trigger()`), the
  event triggers, and every SQL string the Rust code builds.
- **Function names keep their `pg_tviews_` prefix.** Renaming them would break every caller
  for no gain in safety. The qualified form is `tviews.pg_tviews_create_or_replace(…)`;
  generated SQL uses it, and hand-written SQL can add `tviews` to `search_path` instead.
  The two new read objects of Decision 3 have no prefix (`tviews.registry`,
  `tviews.contract_version()`), because they only exist in the qualified form.
- Trigger names are built by one function that fits them into 63 **bytes** the way
  `index_name()` does (truncate on a character boundary, append a hash of the full name).
  Today `remove_entity_triggers` (`src/dependency/triggers.rs:130`) uses `left(…, 63)`,
  which counts characters while PostgreSQL truncates bytes, and the install path does not
  truncate at all. Trigger removal finds triggers by `tgfoid` (the pg_tviews trigger
  functions), not by recomputing a name.
- The test runners add `tviews` to the test databases' `search_path` (as the README will
  recommend), and one regression test runs with `search_path = ''` to prove that nothing
  needs it.

TVIEWs themselves (`tv_*`, `v_*`) stay in the schema the caller names or `current_schema()`,
as today. Only the extension's own objects move.

**Install-script hardening.** With a fixed schema name, a hostile role could create
`tviews` or objects in it before the extension is installed. Now that upgrade scripts carry
changes (Decision 2), the install script drops every `CREATE OR REPLACE`, `IF NOT EXISTS`
and `ADD COLUMN IF NOT EXISTS` (the CVE-2022-2625 pattern), so a name collision is an
error instead of a silent reuse. The script also raises if `tviews` already exists and is
owned by a role other than the one running `CREATE EXTENSION`. A schema created because the
control file names it is not an extension member, so `DROP EXTENSION` leaves `tviews`
behind, empty; the README says so, and the migration script and the CI comparison expect
it.

## Decision 2: privileges (#136)

The model is the one PostgreSQL uses for materialized views: **the owner of a TVIEW
maintains it, whoever triggered the change.**

**Refresh runs as the TVIEW owner.** The flush refreshes one entity at a time. For each,
it switches to the owner of that `tv_*` with
`SetUserIdAndSecContext(owner, SECURITY_LOCAL_USERID_CHANGE | SECURITY_RESTRICTED_OPERATION)`,
as `REFRESH MATERIALIZED VIEW` does, with `search_path` set to `pg_catalog, pg_temp`
(a writer cannot get the owner to run a function it planted on its own path), and
restores the previous user, context and path after that entity. PostgreSQL restores
them on abort as well.
- The writer then needs nothing on `tv_*`, `v_*` or the tables `v_*` reads. Those are
  checked as the owner, which already needs them to have created the TVIEW.
- Application roles can be given `SELECT` only on `tv_*`, so they cannot edit TVIEW rows.
- `SECURITY_RESTRICTED_OPERATION` forbids temporary objects and some session-state changes
  during the refresh. The refresh path must not create temp objects; `regress_issue_136_*`
  covers each refresh kind (full row, scalar patch, fan-out, aggregate, array) to prove it.
- The row-level trigger only reads the catalog and enqueues. It needs nothing beyond the
  grants below.

**Grants in the install script:**

| object | to `PUBLIC` | why |
|---|---|---|
| schema `tviews` | `USAGE` | reach the triggers, functions and views |
| `pg_tview_meta`, `pg_tview_helpers` | `SELECT` | the row trigger reads them as the writing role |
| `tviews.registry`, `tviews.contract_version()` | `SELECT`, `EXECUTE` | the read contract |
| `pg_tview_audit_log` | none | holds `performed_by` and `client_addr` |

The internal tables stay writable only by the extension owner. The metadata granted is not
secret: it is view definitions that `pg_views` already shows to everyone.

**Writes to internal tables run as the extension owner, inside the library.** No SQL
function is `SECURITY DEFINER`. The library checks its caller, then switches to the
extension owner (`SetUserIdAndSecContext`, as for the refresh) for the write itself:
- The `sql_drop` handler (`pg_tviews_handle_drop_event()`, PL/pgSQL) runs as the dropping
  role: PostgreSQL has already checked that the role may drop the objects. It calls
  `pg_tviews_handle_dropped(entity)`, which acts only when the current statement dropped
  that TVIEW's `v_*` or `tv_*` (`pg_event_trigger_dropped_objects()`), and removes its
  catalog row as the extension owner.
- The audit flush calls `pg_tviews_audit_write(jsonb)` as the extension owner. The
  function is revoked from `PUBLIC` and fills `performed_by` from `session_user`, not
  from the caller.
- Registration writes (Decision 5) require the caller to own the TVIEW, as `ALTER TABLE`
  would. A SQL `SECURITY DEFINER` function cannot see which role called it (inside it,
  `current_user` is the definer, and `session_user` is wrong under `SET ROLE`), so the
  library checks ownership as the caller and then writes the catalog as the extension
  owner. No SQL-callable function writes the catalog on a caller's behalf.

`regress_issue_136_*` runs as a role with only table privileges on `tb_*`:
- DML that cascades into two TVIEWs, with auditing off and then on;
- an unrelated `CREATE TABLE` and `DROP TABLE`;
- a `DROP TABLE tb_x CASCADE` that takes the `v_*` of a TVIEW another role owns;
- a read of `tviews.registry`;
- an attempted `UPDATE` of a `tv_*` row, which must fail.

## Decision 3: extension SQL versioned per release, with upgrade scripts (#137)

**Version.** The extension version is the crate version: `default_version =
'@CARGO_VERSION@'`. Every release has its own SQL version, and `pg_extension.extversion`
says which one a database runs. The first release under this policy is `0.1.0-beta.20`.

**main carries the next version.** Right after a release is tagged, main's `Cargo.toml` is
bumped to the next version and an upgrade script `sql/pg_tviews--<released>--<next>.sql`
is created, holding only a header. Every PR that changes the extension SQL (an
`extension_sql!` block, or the name, signature or attributes of a `#[pg_extern]`) adds the
matching statements to that script in the same PR. Tagging a release is then only
stamping the CHANGELOG and tagging. `version-check.yml` today requires a `## [<version>]`
heading for every version in `Cargo.toml`, which the post-release bump cannot have. It
changes to accept `## [Unreleased]` while no tag `v<version>` exists, and to require the
stamped heading only on the release PR.

**Scripts.**
- One script per consecutive pair of releases; PostgreSQL chains them. No skip scripts.
- A released script is never edited. A mistake is fixed in the next one.
- A release with no SQL change still ships a script (a comment), so the version chain has
  no gap.
- A script brings an install of the previous version to exactly the catalog of a fresh
  install (checked below).
- **A script never re-derives TVIEW metadata.** Re-deriving would run the new library's
  analysis and trigger installation inside `ALTER EXTENSION`, lock every base table, and let
  one failing TVIEW fail the whole update. Instead, when a release changes what registration
  derives (cascade paths, fan-out patches, direct maps, aggregate embeds…), its script
  ends with `UPDATE @extschema@.pg_tview_meta SET needs_reregister = true;`. Until
  `pg_tviews_reregister_all()` clears it, those TVIEWs keep refreshing with their old
  metadata, which is what the previous release did. `tviews.registry.needs_reregister` and
  `pg_tviews_health_check()` report them.
- **Internal tables only gain columns.** A column is added with a default and never renamed
  or dropped (`pg_tview_meta` is a config table that `pg_dump` dumps, and a dump of vN must
  restore into vN+1). A column that is no longer needed is left in place and ignored.
- A `#[pg_extern]` whose SQL signature changes gets a new Rust name, and so a new C symbol,
  while its SQL name stays. A database whose catalog still declares the old signature then
  gets `could not find function "…" in file` instead of calling the new code with
  arguments of the wrong type.
- The stale `sql/pg_tviews--0.1.0*.sql` files are removed. They are not installed: the
  installed `pg_tviews--0.1.0.sql` is pgrx-generated. One of them even holds a build log.
  From now on `sql/` holds only upgrade scripts, which the install copies next to the
  generated install script.

**Library/catalog guard.** The library and the catalog each carry a **catalog revision**,
an integer. The library has it as a constant; the catalog has it as
`tviews.pg_tviews_catalog_revision()`, a SQL function in the install script that each
upgrade script which changes SQL redefines. The revision is bumped only by PRs that change
the extension SQL, so a release with no SQL change needs no `ALTER EXTENSION` before
writes succeed.
- **Where it is checked:** the first time in a backend that pg_tviews does real work:
  - the row trigger;
  - the flush, only when the queue is non-empty;
  - a `pg_tviews_*` function;
  - the CTAS interception of a `tv_*` target.

  Not on every `COMMIT`, not in the `sql_drop` handler (plain SQL), and not in databases
  without the extension (#128).
- **Skipped while the extension's own script runs:** when `creating_extension` is true and
  `CurrentExtensionObject` is pg_tviews. Inside a chained `ALTER EXTENSION UPDATE`, the
  intermediate revisions legitimately differ, and the `needs_reregister` `UPDATE` fires
  `pg_tview_meta_changed`, which calls into the library.
- **On a mismatch:** it raises `pg_tviews library catalog revision <n> does not match the
  installed extension (<m>)`, with the hint `run ALTER EXTENSION pg_tviews UPDATE`. When the
  catalog has no revision function (a `0.1.0` install), the hint names
  `scripts/migrate-from-0.1.0.sql` instead, since `ALTER EXTENSION` cannot work there.
- **Caching:** only a match is cached per backend, so the first call after the update
  passes.
- **Rebuild worker:** on a mismatch it logs once and idles until its next start, instead of
  exiting into the 60-second restart loop (`src/rebuild_worker.rs:31-33`).

**Upgrade procedure** (README and CHANGELOG):
1. Install the new package and restart; the library is preloaded.
2. `ALTER EXTENSION pg_tviews UPDATE` in each database.
3. `SELECT * FROM tviews.pg_tviews_reregister_all()` when the release notes say so or
   `tviews.registry` shows `needs_reregister`.

Between 1 and 2, writes to base tables fail with the guard's error when the revision
changed. They are never served by a mismatched library.

**Re-register.** `pg_tviews_reregister(entity)` re-derives one TVIEW's metadata from its
stored definition with the current release's analysis, re-installs its base-table
triggers (removing those on tables the definition no longer reads), and clears
`needs_reregister`. It does not touch `tv_*` rows. `pg_tviews_reregister_all(strict
boolean DEFAULT false)` is a PL/pgSQL loop over entities, dependencies first. It calls the
per-entity function inside `BEGIN … EXCEPTION`, so each entity runs in its own
subtransaction without Rust managing any. It returns `(entity, status)`, `reregistered` or
the error text, and raises at the end only when `strict` is true. This is how TVIEWs
created before #120, #126 or #130 get fan-out patches, aggregate embeds and direct maps
without being dropped. Both require the extension owner or the TVIEW's owner (Decision 2).

**Baseline and the `0.1.0` migration.** Installs of `0.1.0` (every beta up to beta.19)
cannot be upgraded in place: `0.1.0` names many different catalogs, and PostgreSQL cannot
move a non-relocatable extension to another schema. `scripts/migrate-from-0.1.0.sql` keeps
the TVIEWs and their data. It runs after installing the new package and restarting, in one
transaction:
1. It aborts, listing them, if any object outside the extension depends on an extension
   member (a user view over `pg_tviews_queue_realtime`, a function calling a `pg_tviews_*`
   function…), because `DROP EXTENSION … CASCADE` would drop it silently. The base-table
   triggers are excluded from that check, by `tgfoid`: they depend on the trigger
   functions by design and are re-created in step 4.
2. It saves the registration rows, only the columns that exist in both the old and the new
   `pg_tview_meta`.
3. `DROP EXTENSION pg_tviews CASCADE` drops the extension's objects and the base-table
   triggers, not `tv_*` / `v_*`. Then `CREATE EXTENSION pg_tviews`.
4. It inserts the saved rows with `cascade_paths = '{}'` (their OIDs describe the old
   derivation) and `needs_reregister = true`, then runs
   `pg_tviews_reregister_all(strict => true)`.

Audit-log history is not carried over; the script says so and the CHANGELOG repeats it.

**CI.**
- `upgrade-path` job, for every release after `0.1.0-beta.20`:
  1. Install the previous release from its release tarball, into the paths the tarball's
     documented layout defines. `release.yml` currently packs all of `target/release`; this
     PR defines the layout: `lib/pg_tviews.so`, `extension/*.control` and `*.sql`.
  2. `CREATE EXTENSION` and create fixture TVIEWs: plain, cascading, aggregate, array, and
     one in an off-path schema.
  3. Stop PostgreSQL (overwriting a loaded `.so` crashes the backends using it), install the
     commit under test, start. Check that a base-table write fails with the guard's error.
     `ALTER EXTENSION pg_tviews UPDATE`; `pg_tviews_reregister_all(strict => true)`.
  4. In a second database, run a fresh `CREATE EXTENSION`.
  5. Compare a catalog snapshot of both databases:
     - the set of extension members (`pg_depend` deptype `e`);
     - functions: definitions (`pg_get_functiondef`), volatility, security, `proconfig`, ACLs;
     - views (`pg_get_viewdef`) and their ACLs;
     - tables: columns as a set of name, type, default and not-null, plus constraints,
       indexes, triggers and ACLs;
     - types and sequences; event triggers; comments;
     - `extconfig` / `extcondition`; the schema's ACL.

     Column **order** in internal tables is not compared: an upgrade appends columns.
  6. Write to the fixture base tables and check that the TVIEWs follow.

  It fails when the versions differ and no upgrade script exists. While the previous release
  is a `0.1.0` one, the job skips, with a notice.
- `migrate-from-0.1.0` job, which **does** gate this release. It runs the same fixtures and
  checks on the real `v0.1.0-beta.19` tarball, migrated with the script instead of
  `ALTER EXTENSION`, then compares against a fresh install. It is removed once no supported
  release predates the policy.
- The guard has a regression test that needs no second build: as superuser, in the test's
  own throwaway database, it redefines `tviews.pg_tviews_catalog_revision()` to return
  another number, and checks the error, the hint and the pass after restoring it.

## Decision 4: a versioned read contract (#133)

```sql
SELECT tviews.contract_version();   -- integer, 1
SELECT * FROM tviews.registry;      -- one row per registered TVIEW
```

| column | type | meaning |
|---|---|---|
| `schema` | text | schema of the TVIEW table |
| `name` | text | TVIEW table name, unquoted (`tv_post`) |
| `entity` | text | entity (`post`); unique across the database |
| `query` | text | the normalized definition (below) |
| `base_tables` | regclass[] | the tables the backing view reads (below) |
| `logged` | boolean | the table is LOGGED |
| `options` | jsonb | the effective options (Decision 5), every key present; `group_keys` is `null` for a plain TVIEW |
| `needs_reregister` | boolean | a release changed what registration derives since this TVIEW was last registered |
| `identity` | text[] | the column that names its rows and keys its table: `pk_<entity>`, or a `DISTINCT ON` key (appended in 0.1.0-beta.23, [ADR 0169](0169-tview-row-identity.md)) |

**`query`** is the definition as pg_tviews stores it: the author's text after the creation
pipeline, with `SELECT *` expanded and a raw SELECT rewritten to the `pk_<entity>, id,
data` shape, and after column-rename rewrites (#81). It is not the author's original
text. The pipeline leaves its own output unchanged, so passing `query` back to
`pg_tviews_create_or_replace()` with the same `options` returns `unchanged`.
`regress_issue_134_*` checks this round trip for a plain, a raw-SELECT, a `SELECT *` and
an aggregate TVIEW.

**`base_tables`** is every relation reached from the backing view `v_<entity>` through its
rewrite rule's dependencies (`pg_depend` on `pg_rewrite`) whose `relkind` is `r`, `p`, `f`
or `m`:
- views (`relkind = 'v'`) are recursed into; the walk stops at the four relkinds above;
- a `tv_*` table of another TVIEW is a table, so it is listed and its own sources are not;
- functions, sequences and types the view uses are not listed;
- the list is sorted by schema name, then relation name.

**The view is plain SQL** over the internal tables and the system catalogs, with a
recursive CTE for `base_tables`. It calls no C function, so it can be read without the
library, with a mismatched library, and on a standby.

Rules:
- `contract_version()` covers the view above, the `options` keys and their meaning, and the
  signatures, "same" rules and return values of the Decision 5 functions.
- **Additive changes do not bump it:** a new column (always appended), a new `options`
  key, a new function. Consumers select columns by name and ignore unknown keys.
- **Everything else bumps it:** removing or renaming a column, changing a type or a
  meaning, removing an `options` key, changing what counts as "same" or what the functions
  return. A bump is called out in the CHANGELOG's upgrade notes. While the extension is in
  beta, the previous contract is not kept alongside the new one.
- Values come from the system catalogs where they can (`logged`, the `options` storage
  keys, `base_tables`), not from pg_tviews' own bookkeeping, so the view reports the truth
  after a manual `ALTER TABLE`.
- `contract_version()` is a `STABLE` SQL function in the extension script, so its value
  always matches the installed catalog version.
- `pg_tview_meta` and the other `pg_tview_*` tables are documented as internal: they may
  change in any release, and tools must not read them.

## Decision 5: `pg_tviews_create_or_replace()` as the one way to create (#134)

```sql
SELECT tviews.pg_tviews_create_or_replace(
    'app.tv_post', $$SELECT …$$,
    options => '{"logged": true, "fillfactor": 85}');
-- returns 'created' | 'unchanged' | 'altered' | 'replaced' | 'rebuilt'
SELECT tviews.pg_tviews_drop('app.tv_post', if_exists => true);
```

**Who may call it.** The DDL itself runs as the caller, as `CREATE TABLE` and
`CREATE VIEW` would:
- `v_*` and `tv_*` are owned by the caller;
- creating them needs `CREATE` on the target schema;
- the view needs `SELECT` on what it reads;
- installing the base-table triggers needs `TRIGGER` on each base table.

Only the registration write runs as the extension owner (Decision 2), after the library
has checked that the caller owns the `tv_*` (or is a member of the owning role), and so
does removing pg_tviews' own base-table triggers, since `DROP TRIGGER` needs the base
table's owner where `CREATE TRIGGER` needs only `TRIGGER`.
Replacing or dropping an existing TVIEW therefore requires owning it, exactly as
`ALTER TABLE` / `DROP TABLE` would. A migration role that owns the application schema and
has `TRIGGER` on its tables needs no superuser.

**Name.** `tv_post`, `post` and `app.tv_post` all name the same TVIEW; an unqualified
name resolves to `current_schema()`, as today. The entity is unique across the database, so
`app.tv_post` when `post` is registered in another schema is an error, not a second TVIEW.
The name must match the definition's key: `tv_post` whose definition keys on `pk_article`
is an error. For a `DISTINCT ON` TVIEW, the key is its `pk_<entity>` output column; for an
aggregate TVIEW, it is the `pk_<entity>` column the group keys produce.

**Options.** A JSON object; an unknown key or a value of the wrong type is an error.

| key | type | on create, when omitted | notes |
|---|---|---|---|
| `logged` | boolean | `NOT pg_tviews.unlogged_by_default` | |
| `fillfactor` | integer 10–100 | `pg_tviews.fillfactor` | |
| `data_gin_index` | boolean | `pg_tviews.data_gin_index` | GIN index on `data` |
| `group_keys` | object or `null` | `null`: not an aggregate | makes it an aggregate TVIEW (#58) |

An omitted key takes its default when the TVIEW is created, and **keeps its current value**
when it exists. So a migration that only changes the query does not reset storage settings
someone tuned by hand, and a tool that wants a key pinned passes it. Passing
`"group_keys": null` turns an aggregate TVIEW into a plain one.
`pg_tviews_create_aggregate(name, sql, group_keys)` stays, as a wrapper passing
`group_keys`.

**Serialization.** Every call that registers, changes or drops a TVIEW takes
`pg_advisory_xact_lock(<pg_tviews class id>, hashtext(entity))`, the two-key form with a
fixed class id. That covers `create_or_replace`, `pg_tviews_create`, `pg_tviews_drop`, the
CTAS path and `pg_tviews_reregister`. Two calls for one entity therefore run one after the
other.

**What counts as the same.** For an existing TVIEW, under `ACCESS SHARE` on `tv_*` and
`v_*`, the call compares:
- **query**: the definition goes through the creation pipeline, is created as a temporary
  view, and is the same when `pg_get_viewdef` of that view equals `pg_get_viewdef` of
  `v_<entity>`. It ignores whitespace, comments and keyword case, and it sees a change of
  name resolution under a different `search_path`. Unlike the rename path's
  `pg_tviews_defines_view()` (#81), which turns any error into "different", this check
  **propagates errors**: a syntax error or an unknown column in the new definition is
  raised to the caller, not read as a change that then fails half-way through a rebuild.
- **options**: the keys passed, against the table's actual state (`relpersistence`, the
  `fillfactor` reloption, the presence of the GIN index) and the stored `group_keys`.

Then, by the smallest change that applies:
- **`unchanged`**: everything is the same. The TVIEW is not touched. The comparison itself
  creates and drops a temporary view, so the call writes to the system catalogs and cannot
  run on a standby or in a read-only transaction.
- **`altered`**: only `logged`, `fillfactor` or `data_gin_index` differ. The change is made
  in place (`ALTER TABLE … SET LOGGED/UNLOGGED`, `SET (fillfactor = n)`, create or drop the
  index), and the rows are kept.
- **`replaced`**: the query differs but produces the **same columns** (names and types, in
  order), and `group_keys` is the same.
  1. Take `EXCLUSIVE` on `tv_*`, which still admits readers but no concurrent refresh, and
     `SHARE` on the base tables of both the old and new definitions, which blocks writers
     for the duration.
  2. `CREATE OR REPLACE VIEW v_<entity>`, then re-register: metadata, triggers added and
     removed.
  3. Reconcile the rows in place with three statements that touch only rows that change:
     `UPDATE … FROM v_* WHERE (tv.*) IS DISTINCT FROM (v.*)`, `INSERT` the missing keys,
     `DELETE` the keys that are gone. They work on every supported PostgreSQL version,
     16–18; `MERGE … NOT MATCHED BY SOURCE` would need 17.

  Rows that do not change fire no triggers, so TVIEWs reading this one cascade only real
  changes. The table, its indexes, privileges, comment, foreign keys and dependents are
  untouched.
- **`rebuilt`**: the columns or `group_keys` differ.
  - **Refused, naming the object**, when an object depends on the table or the backing view
    (a view over `tv_post`, another TVIEW reading it), or the table has something a rebuild
    cannot carry: RLS policies, user triggers, rules, publication membership, a non-default
    replica identity, per-column statistics targets, security labels. It never cascades.
  - **Otherwise** the backing view, table and registration are dropped and recreated, and the
    rows computed.
  - **Carried over:** the table's owner, privileges and comment; `graphql_typename`; and the
    indexes the user added to `tv_*`. Those are saved with `pg_get_indexdef` and re-run; one
    that no longer applies, because its column is gone, fails the call and names the index.

A TVIEW registered by an older release is "the same" when its query and options are;
bringing its derived metadata up to date is `pg_tviews_reregister`'s job.

**Anywhere.** The function does all its work itself, synchronously: it does not depend on
the ProcessUtility hook, the event trigger or a deferred populate. It works inside any
transaction, a multi-statement batch, a `DO` block, the same batch as `CREATE EXTENSION`,
and in a session where the library is not preloaded.

**CTAS.** The ProcessUtility hook turns `CREATE TABLE tv_x AS SELECT …` into a call to the
same code before PostgreSQL creates anything, with **create-only** semantics, as a CTAS
has: an existing TVIEW is an error, `IF NOT EXISTS` on an existing TVIEW is a NOTICE and
nothing else.
- **No SPI inside `catch_unwind`.** The `catch_unwind` block only inspects the parse tree
  (no SPI) and returns a decision. The create runs after it, outside, the way the COMMIT
  flush does (`src/hooks.rs:134-184`). On error, `HOOK_IN_PROGRESS` is reset before
  raising.
- It intercepts only `CreateTableAsStmt` with `objtype = OBJECT_TABLE`; a
  `CREATE MATERIALIZED VIEW tv_x` is left to PostgreSQL.
- It fills the `QueryCompletion` as PostgreSQL would (`SELECT <rows>`), so clients and
  drivers see the row count.
- `CREATE UNLOGGED TABLE` sets `logged: false`; `WITH (fillfactor = n)` sets `fillfactor`.
- It refuses, with a hint pointing to `pg_tviews_create_or_replace()`: `SELECT … INTO tv_x`,
  `TEMP`/`TEMPORARY`, `AS EXECUTE`, a column-name list, `TABLESPACE`, `USING`, any other
  reloption, `WITH NO DATA`, and a query with parameters (`$1`).
- `EXPLAIN [ANALYZE] CREATE TABLE tv_x AS …` reaches `ProcessUtility` as an `ExplainStmt`
  and never fires the event trigger, so the #80 fallback would miss it. The hook inspects
  `ExplainStmt.query` and refuses a CTAS into a `tv_*` target.

The create-then-drop-then-rebuild sequence and the deferred populate go away. The event
trigger stays only to fail loudly when a CTAS was not intercepted (#80), with the same
hint. `pg_tviews_create(name, sql)` keeps its create-only behaviour, on the same code.

**Drop.** `pg_tviews_drop(tview_name, if_exists, cascade)` keeps its signature and
behaviour, and also accepts a schema-qualified name. It requires owning the TVIEW.

## Delivery

One PR per item, each branched from the previous one and opened against main (`ci.yml`
and coverage only run for PRs into main). Each starts with a failing
`test/sql/regress_issue_<n>_*.sql`. Landed bottom-up with squash merges as one release,
`0.1.0-beta.20`.

1. **#136 schema:** control file, `@extschema@` / qualification, `ext_schema()` constant,
   trigger naming by bytes and removal by `tgfoid`, install-script hardening, test runners
   and `search_path = ''` test.
2. **#136 privileges:** grants, drop handler and audit writer, refresh as the TVIEW
   owner, the ordinary-role regression test.
3. **#139 health check:** match the real `trg_tview_*` names (by `tgfoid`), resolve base
   tables from the catalog instead of `('tb_'||entity)::regclass`, and stop failing on an
   unrelated `tview_*` trigger. It lands before #137, which makes `health_check` report
   `needs_reregister`.
4. **#137 versioning:** `@CARGO_VERSION@`, upgrade-script policy, `version-check.yml`,
   release tarball layout, catalog revision and guard, `needs_reregister`,
   `pg_tviews_reregister(entity)` / `pg_tviews_reregister_all()`,
   `scripts/migrate-from-0.1.0.sql`, and the `upgrade-path` and `migrate-from-0.1.0` CI
   jobs.
5. **#133 read contract:** `contract_version()`, `tviews.registry`, internal-table docs.
6. **#134a create_or_replace:** `created` / `unchanged` / `altered` / `rebuilt`, options,
   name checks, serialization, ownership checks, qualified `pg_tviews_drop`.
7. **#134b replaced tier:** in-place reconcile with its locks.
8. **#134c CTAS:** interception on the shared code outside `catch_unwind`, refusals,
   `EXPLAIN`, removal of the deferred populate.
9. **#138 docs:** remove the undocumented-but-listed functions; CI check against `pg_proc`.
10. **#135 confiture CI:** runs confiture's TVIEW suites at a **pinned confiture ref**
    that already targets `tviews` and `create_or_replace`. That ref is prepared in
    confiture alongside this series. The job is required before tagging, so confiture's
    change is merged first and pg_tviews is tagged after.

## Consequences

- Breaking for existing installs: the extension moves to `tviews`, and unqualified calls
  need `tviews` on `search_path`. `scripts/migrate-from-0.1.0.sql` moves an install
  without rebuilding TVIEWs; the CHANGELOG upgrade notes give the steps.
- Ordinary roles can write to base tables and run DDL in a database with pg_tviews
  installed, and application roles no longer need write access to `tv_*`. A migration role
  that owns its schema can manage TVIEWs without superuser.
- From this release on, `ALTER EXTENSION pg_tviews UPDATE`, plus `reregister_all()` when
  flagged, is the upgrade. CI proves it matches a fresh install, and a library/catalog
  mismatch is a clear error instead of undefined behaviour.
- confiture reads `tviews.registry`, checks `contract_version()`, and generates
  `pg_tviews_create_or_replace` / `pg_tviews_drop` calls; its suites run in pg_tviews CI
  (#135) as the conformance tests of this contract.
- Every PR that changes extension SQL now also edits the pending upgrade script and, if
  the SQL changed, bumps the catalog revision.

## Resolved during review

1. Schema `tviews` (not `pg_tviews`, which PostgreSQL reserves); view `tviews.registry`.
2. Next version `0.1.0-beta.20`, set in the #137 PR; main carries it until tagged.
3. Function names keep the `pg_tviews_` prefix; no unprefixed aliases.
4. `options`: an omitted key takes the default on create and keeps the current value on
   replace.
5. A change that keeps the columns does not rebuild; a rebuild recreates user indexes and
   refuses what it cannot carry.
6. Upgrade scripts flag TVIEWs for re-registration; they never run it.
7. Refresh runs as the TVIEW owner.
8. TVIEW DDL runs as the caller, gated by ownership; no superuser needed.
9. Registration writes: the library checks ownership as the caller and switches to the
   extension owner for the catalog write, instead of a SQL `SECURITY DEFINER` function,
   which cannot see its caller (decided while implementing #134).

## Amendment (#181): pg_tviews' objects live in `tviews`

Decision 1 put the extension in `tviews`, but a TVIEW's backing view stayed in the
application's schema as `v_<entity>`. By fraiseql's naming convention (`tb_` command
table, `v_` application query view, `tv_` materialized) that is the application's own
query view, so a schema built from templates could not get the TVIEW (#181).

- **Backing views live in `tviews`**, named after the TVIEW's table:
  `tviews.<schema>__<tv table>`, fitted to 63 bytes like the other generated names. No
  option chooses the name: it follows from the convention, and `CREATE TABLE tv_x AS`
  has nowhere to pass one. `registry.view` reports it. A name already taken is refused
  at create; `ALTER TABLE tv_x RENAME` and `SET SCHEMA` rename the view with the table.
- **Ownership is unchanged**: the TVIEW's owner owns its backing view, which reads the
  base tables with that role's privileges. The role usually lacks CREATE on `tviews`
  (Decision 2), so pg_tviews grants it for the DDL, as the extension's owner, and
  revokes it in the same transaction. Owning the views by the extension's owner would
  read the base tables with its privileges; `security_invoker` views would need grants
  for every reader.
- **Privileges follow the table**: a grant on the application's schema (`GRANT SELECT ON
  ALL TABLES IN SCHEMA app`, default privileges on `app`) no longer reaches the backing
  view, so whoever can `SELECT` from `tv_<entity>` can `SELECT` from its backing view.
  The view's `SELECT` grants are made its table's when the view is created, rebuilt or
  moved by the upgrade, and again after every `GRANT` or `REVOKE` on tables (by name or
  `ALL TABLES IN SCHEMA`); `ALTER TABLE tv_x OWNER TO` and `REASSIGN OWNED` give the view
  the table's owner. Only `SELECT` is copied: the view reads the base tables with its
  owner's privileges, and so would a write through it. A grant on the backing view alone
  does not outlive the next of these. The grants are written as the view's owner, who
  may always grant on it, so the caller needs no privilege on the view, and the
  extension's owner need not be a superuser (`superuser = false`).
- **Found by OID**: after creation nothing builds or matches the name; aggregate embeds
  come from the query tree (ADR 0157). A definition that embeds another TVIEW reads its
  `tv_<entity>` table.
- **Dump and upgrade**: the views are not extension members; `pg_dump` dumps them after
  `CREATE EXTENSION` and the tables they read. The 0.1.0-beta.25 upgrade script moves
  every existing backing view (`ALTER VIEW … SET SCHEMA tviews`, then the derived name),
  keeping its OID.
  Not being members, they would outlive `DROP EXTENSION pg_tviews` (#199): the
  `ProcessUtility` hook reads them before the statement and drops them after it; the
  `tv_*` tables stay as plain tables. A session that never loaded the library cannot
  run the hook, so a view at a backing name that no TVIEW is registered with and
  nothing depends on is dropped, with a NOTICE, by the create that needs the name.
- **Refresh context** (#200): besides the owner and `search_path`, every computation of
  a TVIEW's rows pins the settings a value's text depends on (`TimeZone` `UTC`,
  `DateStyle` `ISO, YMD`, `IntervalStyle` `postgres`, `extra_float_digits` `1`,
  `bytea_output` `hex`), so the stored rows do not depend on the writer's session. Fixed
  values rather than options: one rendering per database, comparable across TVIEWs.
- **Options** (Decision 5) gain `uncascaded_policy`: a TVIEW declares what a write to a
  table no cascade reaches does (ADR 0157, amendment), instead of a file setting the
  session's `pg_tviews.uncascaded_policy` first. A different value is an `altered`
  change.
- **Options** gain `uncascaded_tables` (#195, a policy per table), `function_reads`
  (#193, the tables each non-immutable function reads) and `time_refresh` (#193,
  `external`), and `registry` gains `uncascaded_table_policies`, `function_reads`,
  `time_dependent` and `time_refresh`, appended: additive, `contract_version()` stays
  1. A change to any of the three options alone is an `altered` change. Functions are
  stored as `schema.name(argument types)` text: a `regprocedure` column in a dumped
  configuration table would block `pg_upgrade`.

## Amendment: maintenance functions and the real privilege model

Decision 2 described the drop handler and the audit writer as `SECURITY DEFINER`
functions. None was ever shipped: the model is the one Decision 2 now states, a check as
the caller and a write as the extension owner inside the library.

Decision 2 also left every maintenance function executable by `PUBLIC`. Most failed on a
missing table privilege, but a role could force rebuilds and take locks on TVIEWs it does
not own. Since 0.1.0-beta.27:
- The functions that act on every TVIEW are revoked from `PUBLIC`:
  `pg_tviews_refresh_all()`, `pg_tviews_refresh_all_entities()`,
  `pg_tviews_rebuild_all(boolean)`, `pg_tviews_reregister_all(boolean)`,
  `pg_tviews_set_logged(text, boolean)`, `pg_tviews_ensure_propagation_indexes(text,
  boolean)` and `pg_tviews_invalidate_caches(oid)`. An operator role is granted them
  (`docs/user-guides/operators.md`); the install and the upgrade script carry the same
  `REVOKE`, and the upgrade check compares the ACLs.
- Every function acting on one TVIEW requires owning it (or the extension), checked
  before any lock: `pg_tviews_refresh`, `pg_tviews_reregister`, `pg_tviews_set_logged`,
  `pg_tviews_recover_after_crash`, `pg_tviews_ensure_propagation_indexes(entity)`,
  `pg_tviews_set_typename`, `pg_tviews_create_or_replace`, `pg_tviews_drop`.
- Bulk rebuilds run each backing view as its TVIEW's owner, never as the caller.

`regress_security_surface.sql` runs each as a role that owns nothing, an operator and an
owner, and fails when a new function of the extension is not classified.
