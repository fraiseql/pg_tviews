# Security Architecture

This page states what pg_tviews protects, where its trust boundaries are, and which
controls exist today. How the extension works is in
[architecture.md](../../architecture.md); the privilege decisions are in
[ADR 0136](../adr/0136-tool-facing-surface.md) (Decision 2 and its amendments).

## Assets

1. **Extension code**: the Rust library and the extension SQL.
2. **User data**: the rows of `tv_*` tables and the base tables their definitions read.
3. **Release artifacts**: the tarball, its Sigstore bundles, SBOMs and provenance.

## Threats

| Threat | Vector | Main controls |
|---|---|---|
| Privilege escalation in the database | A role making a TVIEW owner's code run with its privileges, or the reverse; planting a function, relation or type where the owner's code resolves names | The [owner frame](#owner-frame), ownership checks, `TRIGGER` to attach work to another role's table ([below](#effects-on-other-roles)) |
| Data leakage | A role reading a TVIEW or its backing view without a grant; a role learning row data from pg_tviews' functions, errors or locks | Backing views are their owner's alone; [caller visibility](#caller-visibility) for every row a function returns; errors from owner-run work reach other roles without values; value-lock tags keyed with a server secret |
| SQL injection | Identifiers and definitions passed to pg_tviews functions | Identifiers quoted with `quote_identifier`, values bound as parameters or quoted with `quote_literal`; a definition must be exactly one SELECT (42601) |
| Denial of service | Unbounded queues or recursion; a writer's locks blocking writers of tables it has no rights on | `pg_tviews.max_queue_size` on every enqueue path (54000), at most 1,000 entity batches per flush and 64 nested views (54001); a refresh by keys locks a whole table only for a role that may write it, otherwise one lock per linked value, at most 16 times `pg_tviews.lock_escalation_threshold` (54000); a full refresh locks every table its TVIEW reads whatever the caller's rights, and a write's own locks escalate on the TVIEW tables its flush changes (see [concurrency](../concurrency.md)); a private registration lock; `idle_in_transaction_session_timeout` for writer roles |
| Supply chain | Compromised crates or CI | `cargo deny check` (`deny.toml`) and `cargo audit` on every pull request and daily (`security-audit.yml`), pinned toolchain and pgrx, every workflow action pinned by commit and every installed tool by version |
| Build tampering | Modified release tarball | Sigstore keyless signatures, GitHub build provenance, reproducible build script |
| Memory corruption | `unsafe` FFI into PostgreSQL | `unsafe` only around pgrx FFI calls, each block with a `SAFETY:` comment; every PostgreSQL callback `#[pg_guard]`ed (`scripts/check-ffi-guards.sh`) |

## Trust boundaries

- **Trusted**: the PostgreSQL server, its superusers, the extension's owner, and the
  host. `pg_tviews.control` sets `superuser = false`: a role with `CREATE` on the
  database can install the extension and becomes its owner.
- **Partly trusted**: a TVIEW's owner. Its definition's code runs as itself, never as
  the role whose write triggered a refresh, nor as a superuser calling a maintenance
  function.
- **Untrusted**: any other role writing base tables, reading TVIEWs, or calling
  pg_tviews functions.

## Privilege model

- **Refreshes run as the TVIEW's owner.** Every read and write of a `tv_*` table during
  a flush or rebuild runs as that table's owner, in a security-restricted operation,
  with `search_path = pg_catalog, pg_temp` and fixed render settings ([owner frame](#owner-frame)),
  as `REFRESH MATERIALIZED VIEW` does. A writer to a base table needs no privilege on
  the TVIEW, its backing view, or the tables the view reads.
- **The caller's `search_path` has no say in pg_tviews' own SQL.** Every function, the
  triggers, the flush, the `ProcessUtility` hook and the rebuild worker run their catalog
  SQL under `search_path = pg_catalog, pg_temp` (`owner::CatalogPath`), and the
  extension's PL/pgSQL functions set it: an operator or function in another schema on
  the caller's path is never used, and what is read from the catalog (names, types,
  mapping templates) comes out qualified, so the SQL then run as an owner is the same
  whatever the caller's session holds. Only the names the caller writes resolve under
  its own path (`owner::CallerPath`): a definition, the tables and functions its options
  name, an unqualified TVIEW's schema, the relations a statement names.
  `scripts/check-name-resolution.sh` fails on an entry point that does not pin it.
- **No `SECURITY DEFINER` functions.** Every function checks as the caller; the
  catalog and audit-log writes then run as the extension's owner inside the library.
- **Ownership checks before any lock.** Every function acting on, or showing, one
  TVIEW requires owning it or the extension (42501) before it takes a lock, so a
  refused call holds none, even in a transaction left open. The lock serializing
  DDL on one entity is private to pg_tviews: no role can take it with
  `pg_advisory_lock()`.
- **Functions acting on every TVIEW end in `_all` and are revoked from `PUBLIC`**
  (`pg_tviews_refresh_all()`, `pg_tviews_rebuild_all()`, `pg_tviews_reregister_all()`,
  `pg_tviews_ensure_propagation_indexes_all()`, `pg_tviews_reset_stats_all()`, with the
  internal `pg_tviews_invalidate_caches()` and `pg_tviews_audit_write()`); an operator
  role is granted them
  ([docs/user-guides/operators.md](../user-guides/operators.md)).
  `regress_security_surface.sql` fails when a new function is not classified.
- **Catalog.** `pg_tview_meta` is writable only by the extension's owner; `PUBLIC` can
  read it and the `tviews.registry` view. A restored catalog row whose plan does not
  resolve fails the insert.
- **Backing views** in `tviews` are their owner's alone, the TVIEW's: a backing view
  reads the base tables with its owner's privileges, so a role granted it would read
  every row the definition yields, whatever row-level security hides on `tv_*`. No
  grant on the table reaches it, and every privilege on it held by another role, on
  the view or on one of its columns, is revoked when the view is created or rebuilt,
  when the TVIEW is re-derived, and after every `GRANT`, `REVOKE`, `ALTER … OWNER` and
  `REASSIGN OWNED` (`src/ddl/privileges.rs`). Readers read `tv_*`.
- **jsonb_delta.** Patches call only functions in jsonb_delta's own schema, never a
  same-named function elsewhere on a `search_path`.
- **Row-level security.** Refreshes read base tables as the TVIEW's owner, so a policy
  on a base table applies as it applies to that owner. A TVIEW does not carry its base
  tables' policies: put policies on `tv_*` itself to restrict who reads which rows.

## Owner frame

SQL derived from a TVIEW, planned (`EXPLAIN`) or run, runs only in an owner frame of the
TVIEW's owner: its creation (the creating role, its owner-to-be),
`pg_tviews_create_or_replace`, re-registration (`pg_tviews_reregister`,
`pg_tviews_reregister_all`, `ALTER EXTENSION pg_tviews UPDATE`), the follow-up of a
column rename, the health check's plans, `pg_tviews_resync`'s mappings, and every
refresh and rebuild. One function opens frames, `AsOwner::run` in `src/owner.rs`, and
hands its work a token nothing else can make. The analysis of a definition
(`lineage::analyze`, its `EXPLAIN`s), the recompute and rebuild statements of
`refresh/` and the mappings of `pg_tviews_resync` take that token and check that it
names the owner of the TVIEW's table, failing with an internal error on a mismatch;
the other statements of a refresh run inside the same frame without taking it. A frame
opened inside another (the extension's owner's, for a catalog write) makes the outer
token unusable until it closes, so no TVIEW's SQL runs as the inner frame's role.

A frame gives four guarantees:

- **Who.** The current user is the owner, inside a security-restricted operation, as
  in `REFRESH MATERIALIZED VIEW`. Privileges are decided for the caller, the current
  user when the outermost frame opened (`owner::caller()`), never for the current user
  inside a frame.
- **Where names resolve.** `search_path` is `pg_catalog, pg_temp`. The session's
  temporary schema is the writer's, so a temporary relation or type of the writer
  never stands in for a name the owner's code resolves: before a frame opens for a
  caller without the owner's privileges, a temporary relation or type named like one
  in another schema refuses the work (`42501`). Creating or replacing a TVIEW is
  refused (`42P13`) when a function its refresh runs (one its definition calls,
  directly or through an operator, aggregate or view, or a user trigger on its table)
  sets a `search_path` that does not end in `pg_temp`; the health check's
  `function_paths` component lists such functions of TVIEWs registered before the rule.
- **How values render.** These settings have fixed values in every frame, never the
  session's or the role's: `TimeZone` `UTC`, `DateStyle` `ISO, YMD`, `IntervalStyle`
  `postgres`, `extra_float_digits` `1`, `bytea_output` `hex`, `lc_monetary`,
  `lc_numeric` and `lc_time` `C`, `timezone_abbreviations` `Default`, `array_nulls`
  `on`, `quote_all_identifiers` `off`, `transform_null_equals` `off`,
  `standard_conforming_strings` `on`, `xmlbinary` `base64`, `xmloption` `content`,
  `gin_fuzzy_search_limit` `0`, `plan_cache_mode` `auto` and `jit` `off`.
  `default_text_search_config` is the TVIEW's own, its creator's. The constants a plan
  stores are rendered under the same settings, and a writer's trigger reads a row key
  that is not an integer under them too.
- **What an error reveals.** To a caller without the owner's privileges, an error the
  owner's work raises arrives with its SQLSTATE and a message naming its condition
  (`refreshing a TVIEW as its owner <role> failed: <condition>`), without the values it
  quotes; an error pg_tviews returns keeps its SQLSTATE and drops the rows and the
  PostgreSQL report it quotes. The server log has the whole error. The owner, its
  members and superusers get it unchanged.

Stored plans name relations by OID; the names they also hold are for messages. SQL is
rendered from those OIDs inside the frame, right before it runs, so a renamed relation
is still the one read and a relation taking its old name never is.
`scripts/check-name-resolution.sh` fails on SQL built from a stored name.

## Caller visibility

Whatever pg_tviews returns to a caller about rows is decided for the caller, outside any
owner frame, by one module, `src/visibility.rs`:

- a relation is readable with `SELECT` on it (table-level) and `USAGE` on its schema; a
  relation that is gone is unreadable, and so is every relation inside an owner frame,
  where the current user is not the caller;
- row-level security applies when `check_enable_rls` enables it for the caller.

| Function | What the caller sees |
|---|---|
| `pg_tviews_flush_and_report()` | the rows of readable TVIEWs only, the others' types in `omitted_types`; under row security, only the changed rows a read as the caller returns (deleted rows not listed, the type in `invalidated_types`); collections only when the child and parent TVIEWs are readable, and only parents a read as the caller returns |
| `pg_tviews_show_queue()` | queued keys of readable TVIEWs without row security; the others' types in `omitted_types`; `queue_size` counts every key |
| `pg_tviews_show_replication()` | `is_empty` read as the caller from the `tv_*` table, by OID; NULL unless readable without row security |
| `pg_tviews_resync(source, keys)` | reads `source`'s rows as the caller, once: `SELECT` and `UPDATE` on it, and no row security for the caller (`42501`); each TVIEW's owner maps the rows it was handed, and never reads `source`; a TVIEW's count of changed rows is NULL unless the caller reads it without row security |
| Change feed (`pg_tviews.publish_changes`, a superuser setting) | listeners on `tviews_changes` receive the ids of changed rows of every TVIEW, readable by them or not ([ADR 0231](../adr/0231-publish-changes.md) §7): a channel is not a relation, and the setting is off by default |

The health check plans or reads a TVIEW as its owner (`seq_scans`, `contents`) only for
a caller with the owner's privileges; the other TVIEWs are counted in the message.

## Effects on other roles

What a role makes happen to another role's objects needs the privilege PostgreSQL would
require for the same effect.

| Effect | Required |
|---|---|
| Attaching work to a table's writes: creating a TVIEW that reads it, another role's `tv_*` table included | `TRIGGER` on that table; a role's own TVIEWs pass, and a replace keeps reading a table it already read |
| Rebuilding a TVIEW whole (`TRUNCATE`, `ACCESS EXCLUSIVE`) from `pg_tviews_refresh`, which also reaches the TVIEWs reading it, or from a resume | the privileges of its owner; for any other caller its rows are brought to its view one by one, and no reader waits. The `_all` functions rebuild every TVIEW for the operator roles granted them |
| Locking a whole relation in place of its values (escalation) | `INSERT` or `UPDATE` on that relation, for the caller; otherwise one lock per value, at most 16 times `pg_tviews.lock_escalation_threshold` (`54000`) |
| Holding the registration lock of an entity | for an existing TVIEW, the right to change it, checked before the lock is taken and again under it; the lock is pg_tviews' own (`objsubid` 21624), which `pg_advisory_lock()` can neither take nor block |
| Queuing refresh work | `pg_tviews.max_queue_size` holds on every path, keys queued together and `pg_tviews_resync` included (`54000`) |

## Supply chain and releases

- Dependencies are checked by `cargo deny` (licenses, sources, RustSec advisories) and `cargo audit` on every pull request and daily.
- The toolchain is pinned (`rust-toolchain.toml`, Rust 1.98) and pgrx is `=0.17.0`.
- Release builds link with RELRO, `BIND_NOW` and a non-executable stack
  (`.cargo/config.toml`).
- `.github/workflows/release.yml` signs the tarball and SBOMs with Sigstore (keyless)
  and attests the tarball's provenance with `actions/attest-build-provenance`. See
  [signing.md](signing.md), [provenance.md](provenance.md),
  [verify-release.md](verify-release.md) and
  [../development/reproducible-builds.md](../development/reproducible-builds.md).
- Audit logging of create, refresh and drop goes to `tviews.pg_tview_audit_log` while
  `pg_tviews.audit_enabled` is on (off by default).

## Assumptions

- PostgreSQL is installed and configured securely, and `shared_preload_libraries` is
  controlled by an administrator.
- Superusers and the extension's owner are trusted.
- TVIEW owners are trusted with the code their definitions call; the guarantees above
  stop that code from running with another role's privileges.
- A role that may attach work to a table's writes holds `TRIGGER` on it: a TVIEW's
  creator holds it on every table its definition reads, base tables and other
  TVIEWs' tables alike.

## Reporting

See [incident-response.md](incident-response.md) and the repository's `SECURITY.md`.
