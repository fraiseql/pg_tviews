# Quick Start

Install pg_tviews, create a TVIEW, and watch it follow writes to its tables.

## Prerequisites

- PostgreSQL 16, 17 or 18 installed and running
- Rust toolchain (the version pinned in `rust-toolchain.toml`, for building the extension)
- A database for testing

## 1. Install pg_tviews

### Install Rust (if not already installed)

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env
```

### Install pgrx

```bash
cargo install --locked cargo-pgrx --version 0.17.0
cargo pgrx init
```

### Build and Install pg_tviews

```bash
git clone https://github.com/fraiseql/pg_tviews.git
cd pg_tviews
cargo pgrx install --release   # PostgreSQL 18; for 16 or 17 add
                               # --no-default-features --features pg16 (or pg17)
```

## 2. Enable the Extension

pg_tviews installs hooks when its library is loaded, so load it with the server:
add it to `shared_preload_libraries` in `postgresql.conf` and restart PostgreSQL.

```ini
shared_preload_libraries = 'pg_tviews'
```

Its objects go to the schema `tviews`. Put that schema on the database's
`search_path`, so its functions can be called unqualified (`your_database` is your
database's name):

```bash
psql -d your_database -c 'ALTER DATABASE your_database SET search_path = "$user", public, tviews;'
```

Then, connected to your database, create the extension:

```sql
CREATE EXTENSION IF NOT EXISTS pg_tviews;
-- The ALTER DATABASE above applies to new sessions; this one sets it now.
SET search_path = "$user", public, tviews;
```

Verify installation:

```sql
SELECT pg_tviews_version();
-- Returns the installed version
```

## 3. Create Your First TVIEW

A small blog with users and posts. The tables follow FraiseQL's naming (`tb_*`,
`pk_*`, `fk_*`, `id`), which fits pg_tviews but is not required: the only rule is
that the definition outputs a `pk_<entity>` column, here `pk_post` for `tv_post`.

### Create Base Tables

```sql
CREATE TABLE tb_user (
    pk_user BIGSERIAL PRIMARY KEY,
    id UUID NOT NULL DEFAULT gen_random_uuid(),
    identifier TEXT UNIQUE,
    name TEXT NOT NULL,
    email TEXT UNIQUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE tb_post (
    pk_post BIGSERIAL PRIMARY KEY,
    id UUID NOT NULL DEFAULT gen_random_uuid(),
    identifier TEXT UNIQUE,
    title TEXT NOT NULL,
    content TEXT,
    fk_user BIGINT NOT NULL REFERENCES tb_user(pk_user),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
```

### Insert Sample Data

```sql
-- Create a user
INSERT INTO tb_user (identifier, name, email)
VALUES ('alice', 'Alice Johnson', 'alice@example.com');

-- Create some posts
INSERT INTO tb_post (identifier, title, content, fk_user)
VALUES
    ('hello-world', 'Hello World', 'Welcome to my blog!', 1),
    ('getting-started', 'Getting Started with pg_tviews', 'This is amazing!', 1);
```

### Create a TVIEW

```sql
CREATE TABLE tv_post AS
SELECT
    p.pk_post,             -- required: names the rows (the table's primary key)
    p.id,                  -- public UUID
    p.identifier,          -- slug
    p.fk_user,             -- the author's key, for queries by author
    u.id as user_id,       -- the author's UUID, for filtering
    jsonb_build_object(
        'id', p.id,
        'identifier', p.identifier,
        'title', p.title,
        'content', p.content,
        'created_at', p.created_at,
        'author', jsonb_build_object(
            'id', u.id,
            'identifier', u.identifier,
            'name', u.name,
            'email', u.email
        )
    ) as data              -- the JSONB read model
FROM tb_post p
JOIN tb_user u ON p.fk_user = u.pk_user;
```

> **Alternative Syntax**: For programmatic creation, use `pg_tviews_create()`. See [Syntax Comparison](syntax-comparison.md) for details.

## 4. Test Automatic Updates

### Query Your TVIEW

```sql
-- See your data
SELECT pk_post, id, identifier, data FROM tv_post;
```

### Add New Data

```sql
-- Add a new user
INSERT INTO tb_user (identifier, name, email)
VALUES ('bob', 'Bob Smith', 'bob@example.com');

-- Add a post for the new user
INSERT INTO tb_post (identifier, title, content, fk_user)
VALUES ('bobs-first-post', 'Bob''s First Post', 'Hello from Bob!', 2);
```

Each statement refreshes the TVIEW rows it changed when it ends; in a transaction
block, the refreshes are part of the transaction. pg_tviews read the join
`p.fk_user = u.pk_user` from the definition, so renaming a user refreshes that user's
posts too:

```sql
UPDATE tb_user SET name = 'Alice J.' WHERE identifier = 'alice';
```

### Verify Automatic Update

```sql
-- Check that tv_post was automatically updated
SELECT pk_post, id, identifier, data->>'title' as title,
       data->'author'->>'name' as author_name
FROM tv_post
ORDER BY pk_post;
```

All three posts are there, Bob's included, and Alice's two carry her new name.

## 5. Health Check

Nothing needs enabling: the triggers were installed when the TVIEW was created.
To check them:

```sql
SELECT * FROM pg_tviews_health_check();
```

## 6. Query It

`tv_post` is an ordinary table, indexed on `id` and `user_id`:

```sql
SELECT data FROM tv_post
WHERE identifier = 'hello-world';

-- by public UUID
SELECT data FROM tv_post
WHERE id = '550e8400-e29b-41d4-a716-446655440000';

-- by the author's UUID
SELECT data FROM tv_post
WHERE user_id = '550e8400-e29b-41d4-a716-446655440001';
```

## Next Steps

- **[DDL Reference](../reference/ddl.md)** - What a definition may contain
- **[FraiseQL Integration Guide](fraiseql-integration.md)** - FraiseQL's conventions with pg_tviews
- **[Developer Guide](../user-guides/developers.md)** - Application integration patterns
- **[API Reference](../reference/api.md)** - Complete function reference

## Troubleshooting

### Extension Not Found
If you get "extension pg_tviews does not exist":

```sql
-- Check if extension is installed
\dx pg_tviews
```

Reinstall it if needed, then restart PostgreSQL:

```bash
cargo pgrx install --release
```

### Permission Issues

`CREATE EXTENSION pg_tviews` needs `CREATE` on the database. Creating a TVIEW needs `CREATE` on its
schema, `SELECT` on the tables it reads and `TRIGGER` on them; refreshing, replacing or
dropping one requires owning it (SQLSTATE `42501` otherwise). The functions that act on
every TVIEW (`pg_tviews_refresh_all()`, `pg_tviews_rebuild_all()`, …) are not
executable by `PUBLIC`: see the [Operator role](../user-guides/operators.md#operator-role).

### No Automatic Updates
If TVIEWs aren't updating:

```sql
-- Check triggers are installed
SELECT tgrelid::regclass, tgname FROM pg_trigger WHERE tgname LIKE 'trg\_tview%';

-- Check for errors
SELECT * FROM pg_tviews_health_check() WHERE status <> 'OK';
```

For more help, see the [troubleshooting guide](../operations/troubleshooting.md).