# TVIEW Creation Syntax Guide

> **Trinity Pattern Reference**: See complete patterns in [.phases/excellence-roadmap/00-TRINITY-PATTERN-REFERENCE.md](../../.phases/excellence-roadmap/00-TRINITY-PATTERN-REFERENCE.md)

pg_tviews supports three equivalent ways to create TVIEWs:

## 1. DDL Syntax (Recommended for Interactive Use)

```sql
CREATE TABLE tv_post AS
SELECT
  tb_post.pk_post,          -- INTEGER primary key
  tb_post.id,               -- UUID for GraphQL
  tb_post.fk_user,          -- INTEGER foreign key
  jsonb_build_object(
    'id', tb_post.id,       -- UUID in camelCase
    'title', tb_post.title,
    'userId', tb_user.id    -- Related UUID (from JOIN)
  ) as data
FROM tb_post
INNER JOIN tb_user ON tb_post.fk_user = tb_user.pk_user;
```

**Pros**: Natural SQL syntax, familiar to DBAs
**Cons**: Less explicit about TVIEW creation
**Use when**: Working in psql, migrations, manual DDL

## 2. CREATE TVIEW Syntax (Planned - Not Yet Implemented)

> **Note**: `CREATE TVIEW` syntax is planned for a future release but not currently implemented. PostgreSQL's parser cannot be extended to support custom DDL syntax through hooks.

For now, use either DDL method (1) or function method (3) below.

## 3. Function Syntax (Programmatic)

```sql
SELECT pg_tviews_create('tv_post', $$
  SELECT
    tb_post.pk_post,
    tb_post.id,
    tb_post.fk_user,
    jsonb_build_object(
      'id', tb_post.id,
      'title', tb_post.title,
      'userId', tb_user.id
    ) as data
  FROM tb_post
  INNER JOIN tb_user ON tb_post.fk_user = tb_user.pk_user
$$);
```

**Pros**: Best for dynamic SQL, scripting
**Cons**: More verbose
**Use when**: Application code, scripts, dynamic creation

## Trinity Pattern Notes

- `pk_post` and `fk_user` are integers (SERIAL/INTEGER)
- `id` is UUID (for external API)
- All columns qualified with table name
- JSONB keys use camelCase

All three methods produce identical results.