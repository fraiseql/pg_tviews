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
| Privilege escalation in the database | A role making a TVIEW owner's code run with its privileges, or the reverse; planting a function on a `search_path` the flush uses | Owner execution, fixed `search_path`, ownership checks (below) |
| Data leakage | A role reading a TVIEW or its backing view without a grant | Backing view privileges follow the table's |
| SQL injection | Identifiers and definitions passed to pg_tviews functions | Identifiers quoted with `quote_identifier`, values bound as parameters or quoted with `quote_literal`; a definition must be exactly one SELECT (42601) |
| Denial of service | Unbounded queues or recursion | `pg_tviews.max_queue_size` (54000), `max_propagation_depth` and `max_dependency_depth` (54001), stack-depth checks in the query-tree walkers |
| Supply chain | Compromised crates or CI | `cargo deny check` (`deny.toml`) and `cargo audit` on every pull request and daily (`security-audit.yml`), pinned toolchain and pgrx |
| Build tampering | Modified release tarball | Sigstore keyless signatures, GitHub build provenance, reproducible build script |
| Memory corruption | `unsafe` FFI into PostgreSQL | `unsafe` confined to pgrx FFI, audited in [UNSAFE_AUDIT.md](UNSAFE_AUDIT.md) |

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
  with `search_path = pg_catalog, pg_temp` and fixed render settings (`src/owner.rs`),
  as `REFRESH MATERIALIZED VIEW` does. A writer to a base table needs no privilege on
  the TVIEW, its backing view, or the tables the view reads.
- **No `SECURITY DEFINER` functions.** Every function checks as the caller; the
  catalog and audit-log writes then run as the extension's owner inside the library.
- **Ownership checks before any lock.** Every function acting on one TVIEW requires
  owning it or the extension (42501).
- **Maintenance functions acting on every TVIEW are revoked from `PUBLIC`**
  (`pg_tviews_refresh_all()`, `pg_tviews_rebuild_all()`, `pg_tviews_reregister_all()`
  and the others listed in the ADR); an operator role is granted them
  ([docs/user-guides/operators.md](../user-guides/operators.md)).
  `regress_security_surface.sql` fails when a new function is not classified.
- **Catalog.** `pg_tview_meta` is writable only by the extension's owner; `PUBLIC` can
  read it and the `tviews.registry` view. A restored catalog row whose plan does not
  resolve fails the insert.
- **Backing views** in `tviews` get the `SELECT` grants of their `tv_*` table and its
  owner, after every `GRANT`, `REVOKE` and `ALTER … OWNER`.
- **jsonb_delta.** Patches call only functions in jsonb_delta's own schema, never a
  same-named function elsewhere on a `search_path`.
- **Row-level security.** Refreshes read base tables as the TVIEW's owner, so a policy
  on a base table applies as it applies to that owner. A TVIEW does not carry its base
  tables' policies: put policies on `tv_*` itself to restrict who reads which rows.

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

## Reporting

See [incident-response.md](incident-response.md) and the repository's `SECURITY.md`.
