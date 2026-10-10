# ADR 0220: Options describe TVIEWs; settings tune sessions

- Status: Accepted
- Related: [ADR 0136](0136-tool-facing-surface.md) (tool-facing surface),
  [ADR 0211](0211-api-surface.md) (API and read contract v2),
  [ADR 0216](0216-union-keys.md), #215

## Context

What a TVIEW is could come from three places:

- its definition;
- its options, declared with `pg_tviews_create_or_replace(…, options)`;
- `pg_tviews.*` settings, read when it was created or when it was written.

Settings read at creation made the same statement produce different TVIEWs in different
sessions:

- `unlogged_by_default`, `fillfactor` and `data_gin_index` set the storage;
- `uncascaded_policy` and `time_refresh` set what a write does about tables no cascade reaches.

`data_gin_index`'s own description said "Enable per TVIEW (SET LOCAL)". A session could also
`SET pg_tviews.uncascaded_policy = 'warn'` to get past a refusal at creation.

Settings read at write time made the same TVIEW behave differently per session:

- `union_duplicate_policy` decided what a TVIEW contains ([ADR 0216](0216-union-keys.md));
- `suspend_triggers` dropped refreshes for the session and recorded nothing;
- `max_propagation_depth`, `max_dependency_depth`, `max_queue_size` and
  `lock_escalation_threshold` decided whether a write or a creation succeeds, and any role
  could change them;
- `audit_enabled` was an audit any session could turn off.

Tools paid for this. confiture hard-codes `fillfactor = 85` and the uncascaded policy,
because a database cannot tell it what a session's defaults were. Its guide warns that a
session `SET` "is no pin".

TVIEWs were UNLOGGED by default. A crash or a promotion empties them. Making that safe took
#214 (claim and refill), and it still left #215 (PREPARE, raw `SET LOGGED`, a refill paid by
the first writer). A standby cannot read them at all.

## Decision

### 1. A TVIEW is its definition and its options

Every option has one fixed default, written here and in `docs/reference/ddl.md`. No setting
changes them.

| Option | Default | Meaning |
|---|---|---|
| `logged` | `true` | the table is LOGGED (crash-safe, replicated); `false` for UNLOGGED |
| `fillfactor` | `85` | heap fillfactor (room for HOT updates) |
| `data_gin_index` | `false` | pg_tviews' GIN index on `data` |
| `group_keys` | none | aggregate TVIEW keys |
| `uncascaded_policy` | `error` | what a write to a table no cascade reaches does |
| `uncascaded_tables` | `{}` | a policy per table |
| `function_reads` | `{}` | tables a non-immutable function reads |
| `time_refresh` | none | `external` for a definition that reads the current time |
| `typename` | `PascalCase(entity)` | the GraphQL type name in `pg_tviews_flush_and_report()` |

- `pg_tviews_create(tview, query, options jsonb DEFAULT '{}')` takes the same options as
  `pg_tviews_create_or_replace`.
- `CREATE TABLE tv_x AS SELECT …` creates the TVIEW with the defaults.
- `typename` replaces `pg_tviews_set_typename()`.
- `registry.options` publishes every key, defaults included ([ADR 0211](0211-api-surface.md)):
  `pg_tviews_create_or_replace(r.schema || '.' || r.name, r.query, r.options)` recreates a
  TVIEW on an empty database, and returns `unchanged` on its own.

### 2. Removed settings

| Setting | Instead |
|---|---|
| `unlogged_by_default` | option `logged` (default `true`) |
| `fillfactor` | option `fillfactor` |
| `data_gin_index` | option `data_gin_index` |
| `uncascaded_policy` | option `uncascaded_policy` (default `error`) |
| `time_refresh` | option `time_refresh` |
| `union_duplicate_policy` | none: always an error ([ADR 0216](0216-union-keys.md)) |
| `suspend_triggers` | `pg_tviews_suspend_triggers()` / `pg_tviews_resume_triggers()`: one transaction, every change recorded and refreshed at resume or commit |
| `log_level` | `client_min_messages = debug1` / `log_min_messages` (diagnostics are `DEBUG1`) |

The `pg_tviews.` prefix is reserved: setting a removed name fails. A `postgresql.conf` or
`ALTER ROLE … SET` that carries one fails loudly.

### 3. What is left, and who may set it

| Setting | Context | Why |
|---|---|---|
| `max_propagation_depth`, `max_dependency_depth`, `max_queue_size`, `lock_escalation_threshold` | superuser | decide whether a write or a creation succeeds: the same for every session |
| `audit_enabled` | superuser | an audit the audited cannot turn off |
| `graph_cache_enabled`, `table_cache_enabled`, `direct_patch_enabled`, `test_skip_ctas_intercept` | superuser, hidden from `SHOW ALL` | diagnostics; results are the same either way |
| `batch_size`, `cache_size` | user | performance only |
| `report_max_tracked` | user | the session's own report |
| `auto_rebuild_databases` | postmaster | `*` (default): every database with pg_tviews; a list: only those; empty: none |

Rule for any future setting: if two sessions with different values can produce different
rows, or one of them succeeds where the other fails, it is an option or a superuser
setting, never a user one.

### 4. LOGGED by default; UNLOGGED is an opt-in that pays for itself

- The `logged` default is `true`.
- A TVIEW declared `logged: false` keeps everything #214 built: `tviews.pg_tview_valid`, claim
  and refill.
- PREPARE TRANSACTION with a claimed refill is refused (#215).
- A raw `ALTER TABLE … SET LOGGED` on a reset TVIEW fills it first, like the `logged` option
  (#215).
- After a crash restart or a promotion, a launcher worker refills the reset TVIEWs of every
  database with pg_tviews, with no configuration (`auto_rebuild_databases = '*'`). The
  per-database workers exit when done.

## Consequences

- The same statement creates the same TVIEW in every session. CTAS, `pg_tviews_create` and
  `pg_tviews_create_or_replace` agree.
- confiture's constants become pg_tviews' documented defaults. Its drift check compares
  `registry.options` whole.
- Upgrading (0.1.0-beta.28):
  - existing TVIEWs keep their tables as they are;
  - `registry.options` reports what they are;
  - a TVIEW that relied on a session's `uncascaded_policy` already stores its own (it always
    did).
- Writes cost a little more on a LOGGED TVIEW (WAL). A TVIEW whose rows can be recomputed and
  needs the speed declares `logged: false`.
