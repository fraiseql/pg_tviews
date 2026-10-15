# ADR 0232: Where pg_tviews runs before 1.0

- Status: Accepted
- Related: #232, [ADR 0220](0220-settings.md), [ADR 0231](0231-publish-changes.md),
  [ADR 0233](0233-fanout-cost-and-guard.md)

## Context

pg_tviews is a compiled extension (Rust, pgrx) loaded with
`shared_preload_libraries = 'pg_tviews'`, next to a second compiled extension,
`jsonb_delta`. Managed PostgreSQL services install extensions from their own allow-lists
and let customers preload only the libraries on them. A FraiseQL user on such a service
cannot use pg_tviews today, and FraiseQL's `v_*` mode (plain views, computed at read time)
is what they run.

#232 lists four ways forward:

1. **Self-hosted and Kubernetes first**: say so in the install docs, package for the
   operators, keep FraiseQL's `v_*` mode as the managed-service path.
2. **Ask the providers** to allow-list pg_tviews and jsonb_delta.
3. **A portable SQL edition**: generate PL/pgSQL statement triggers and functions instead
   of running a library, installable anywhere SQL is.
4. **pg_tle**: package the extension as a Trusted Language Extension.

## What the managed services allow (checked 2026-10-13)

Pages read on 2026-10-13; "unverified" where no current document says it.

| Service | A third-party compiled extension | `shared_preload_libraries` | pg_tle | Getting an extension added |
|---|---|---|---|---|
| Amazon RDS for PostgreSQL | no: the published list only; `rds.allowed_extensions` narrows it, never widens it [1][2] | parameter group, available modules only [1] | yes (1.5.2) [3] | no public program found |
| Amazon Aurora PostgreSQL | no: the same catalog model [5] | parameter group (unverified for Aurora) | yes (1.5.2) [5] | no public program found |
| Google Cloud SQL | no: "You cannot create your own extensions in Cloud SQL" [6] | supported flags only [7] | not listed | public issue tracker [6] |
| Google AlloyDB | no [8] | no: "you can't configure the `shared_preload_libraries`" [8] | not listed | unverified |
| Azure Database for PostgreSQL (Flexible) | no: `azure.extensions` allow-list [9] | server parameter, supported libraries [10] | unverified | feedback forum [11] |
| Supabase | SQL-only extensions [12] | not documented as configurable | yes; database.dev is built on it [13][14] | GitHub Discussions [12] |
| Neon | not in the current docs; a 2024 changelog offered custom builds to paid plans by ticket [16] | from Neon's list, `preloaded_libraries` [15] | unverified | support, by demand [15] |
| Crunchy Bridge | no published policy | `ALTER SYSTEM` + restart [17] | not listed | support [17] |
| CloudNativePG 1.27+ (Kubernetes) | **yes**: extensions as OCI image volumes, PostgreSQL 18 (`extension_control_path`) [18][19] | yes (a preloaded module is added by hand) [18] | not needed | build the image to the spec [18] |
| Crunchy PGO (Kubernetes) | **yes**: a custom image; image volumes in 6.0 [20][21] | yes (Patroni configuration) | not needed | custom image |

pg_tle cannot host pg_tviews: it runs trusted languages only ("JavaScript, Perl, Tcl,
PL/pgSQL, and SQL" [3]; "no extensions that are written in C can be TLEs" [14]), and the
hooks it exposes are password checks and client authentication [22], not `ProcessUtility`,
transaction callbacks or background workers. PL/Rust does not help either: RDS dropped it
from PostgreSQL 18 on [4], and it only defines SQL-callable functions.

Sources:
[1] https://docs.aws.amazon.com/AmazonRDS/latest/UserGuide/Appendix.PostgreSQL.CommonDBATasks.Extensions.html
[2] https://docs.aws.amazon.com/AmazonRDS/latest/UserGuide/PostgreSQL.Concepts.General.FeatureSupport.Extensions.html
[3] https://docs.aws.amazon.com/AmazonRDS/latest/UserGuide/PostgreSQL_trusted_language_extension.html
[4] https://docs.aws.amazon.com/AmazonRDS/latest/UserGuide/PostgreSQL.Concepts.General.Using.PL_Rust.html
[5] https://docs.aws.amazon.com/AmazonRDS/latest/AuroraPostgreSQLReleaseNotes/AuroraPostgreSQL.Extensions.html
[6] https://docs.cloud.google.com/sql/docs/postgres/extensions
[7] https://docs.cloud.google.com/sql/docs/postgres/flags
[8] https://docs.cloud.google.com/alloydb/docs/reference/extensions
[9] https://learn.microsoft.com/en-us/azure/postgresql/extensions/concepts-extensions
[10] https://learn.microsoft.com/en-us/azure/postgresql/extensions/how-to-load-libraries
[11] https://learn.microsoft.com/en-us/answers/questions/1665580/why-plpgsql-check-extension-is-not-supported-in-az
[12] https://supabase.com/docs/guides/database/extensions
[13] https://supabase.com/blog/dbdev
[14] https://supabase.com/blog/pg-tle
[15] https://neon.com/docs/extensions/pg-extensions
[16] https://neon.com/docs/changelog/2024-01-19
[17] https://docs.crunchybridge.com/extensions-and-languages
[18] https://cloudnative-pg.io/docs/devel/imagevolume_extensions
[19] https://cloudnative-pg.io/docs/1.27/release_notes/v1.27
[20] https://access.crunchydata.com/documentation/postgres-operator/latest/releases/6.0.x
[21] https://access.crunchydata.com/documentation/postgres-operator/v5/guides/extension-management/
[22] https://docs.aws.amazon.com/AmazonRDS/latest/UserGuide/PostgreSQL_trusted_language_extension-hooks-reference.html

## What a SQL-only edition would keep and lose

What pg_tviews does through each server interface it uses, and what generated SQL could do
instead:

| Mechanism | Used for | In SQL only |
|---|---|---|
| Row and statement triggers (`pg_tview_trigger_handler`, flush trigger) | capturing writes, refreshing at the end of each statement | **kept**: PL/pgSQL statement triggers with transition tables, generated per base table |
| `ProcessUtility` hook | `CREATE TABLE tv_* AS` makes a TVIEW; `DROP TABLE tv_*` drops one; renames, partition DDL, column DDL refused, index names reserved, `SET [UN]LOGGED` filling, `REFRESH MATERIALIZED VIEW` follow-up, `COMMIT`/`PREPARE` flush, `DROP EXTENSION` cleanup | **mostly lost**: event triggers (`ddl_command_end`, `sql_drop`) can follow drops and renames after the fact, but not refuse column DDL before it runs nor turn a CTAS into a TVIEW; TVIEWs would be created by function call only |
| `ExecutorRun`/`ExecutorFinish` hooks | one flush per outermost writing statement (nested writes from triggers) | **degraded**: each nested statement flushes, which is correct but repeats work for trigger cascades |
| Transaction callbacks | refusing to commit queued work, publishing at commit (ADR 0231), resetting backend state, refusing PREPARE | **degraded**: `pg_notify` from the statement trigger is transactional and savepoint-aware, but per statement, not folded per transaction and not capped; no PRE_COMMIT check |
| Shared memory (`tviews.stats`) | per-TVIEW counters readable from any session | **lost** or a table (one more write per refresh) |
| Background worker | refilling reset UNLOGGED TVIEWs after a crash or promotion | **not needed**: UNLOGGED is an opt-in (ADR 0220); a SQL edition would offer LOGGED only |
| Backend caches + relcache callback | catalog rows, graphs and plans kept per backend | **kept by PL/pgSQL's plan cache**; the rest is generated into the functions |
| Value locks (advisory lock tags, ADR 0207) | concurrent refreshes of the same rows | **kept**: `pg_advisory_xact_lock` on the same keys |
| Lineage from the query tree (`pg_analyze_and_rewrite`) | deriving what each write refreshes | **moved to a generator** outside the database (FraiseQL already has the schema), or into SQL over `pg_get_viewdef` (fragile) |
| `jsonb_delta` (C) | `jsonb_smart_patch_scalar` for direct patches | **replaced** by `jsonb_set` (pg_tviews already uses it for fan-out) |

### What a generated trigger costs

`test/bench/write_path/portable/bio_trigger.sql` keeps the write_path `bio` write fresh
with a PL/pgSQL statement trigger written as a generator would emit it: two `UPDATE …
SET data = jsonb_set(…)` from the transition tables, one for `tv_user`, one for the ten
`tv_post` rows embedding the author. Stock PostgreSQL 18.4, pgbench, one client, 10 s, a
fresh database per run, three rounds, ms per write:

| | round 1 | round 2 | round 3 | refresh cost (over plain) |
|---|---:|---:|---:|---:|
| plain write (triggers off) | 0.482 | 0.486 | 0.485 | — |
| pg_tviews 0.1.0-beta.29 | 0.571 | 0.556 | 0.565 | 0.080 ms |
| PL/pgSQL statement trigger | 0.581 | 0.569 | 0.597 | 0.098 ms |

For this shape, a generated trigger costs about a fifth more than the extension, and
3 % more per write. It does less: no value locks (ADR 0207), no journal for the report
or the change feed, no recompute fallback, no DDL tracking. Those would be generated too,
or given up.

## Decision

**Option 1, now; option 3 stays open as research.**

- pg_tviews targets self-hosted PostgreSQL and Kubernetes before 1.0. The install
  documentation says so up front. On Kubernetes the extension ships as an image for the
  operator to mount (CloudNativePG's image-volume extensions, with PostgreSQL 18's
  `extension_control_path`; a custom operand image for PostgreSQL 16 and 17, and for
  Crunchy PGO), next to the release tarball.
- On a managed service, FraiseQL's `v_*` mode is the documented path: the same queries
  against plain views, without materialization.
- Option 2 is not pursued before 1.0: allow-lists favour stable, widely used extensions,
  and pg_tviews' surface still changes between betas (break freely, pre-1.0).
- Option 4 is closed: pg_tle hosts SQL and trusted procedural languages, not compiled
  libraries. pg_tviews needs hooks and shared memory a TLE cannot install.
- Option 3 is kept open. The prototype above shows what a generated trigger costs; the
  inventory shows what such an edition would refuse or degrade. Building it is a product
  decision after 1.0, when the extension's behaviour is stable enough to generate from.

## Consequences

- Install docs (README, getting started) state the supported deployments and point
  managed-service users to `v_*` mode.
- No code changes in pg_tviews for distribution before 1.0; packaging work goes to the
  release tarball and the operator image.
- FraiseQL keeps `v_*` mode as a first-class path, not a fallback to deprecate.
