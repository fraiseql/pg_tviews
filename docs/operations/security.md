# Security Guide

**Version**: 0.1.0-beta.1
**Last Updated**: December 11, 2025

> **Trinity Pattern Reference**: All examples follow the pattern from [.phases/excellence-roadmap/00-TRINITY-PATTERN-REFERENCE.md](../../.phases/excellence-roadmap/00-TRINITY-PATTERN-REFERENCE.md)

## Overview

pg_tviews provides powerful SQL generation capabilities that require careful security considerations. This guide covers SQL injection prevention, access control, and secure usage patterns.

## SQL Injection Prevention

### Safe Parameter Handling

**✅ SAFE: Function parameters are escaped**
```sql
SELECT pg_tviews_create('tv_post', $$
  SELECT
    tb_post.pk_post,  -- INTEGER pk
    tb_post.id,       -- UUID
    jsonb_build_object(
      'id', tb_post.id,
      'title', tb_post.title,
      'userId', tb_user.id
    ) as data
  FROM tb_post
  INNER JOIN tb_user ON tb_post.fk_user = tb_user.pk_user
$$);
```

**❌ UNSAFE: Never concatenate user input**
```sql
-- DON'T DO THIS
SELECT pg_tviews_create(user_provided_name, user_provided_sql);
```

### Dynamic TVIEW Creation

When creating TVIEWs dynamically:

```sql
-- ✅ SAFE: Use format() with proper escaping
CREATE OR REPLACE FUNCTION create_user_posts_tview(user_uuid UUID)
RETURNS VOID AS $$
DECLARE
    tview_name TEXT;
BEGIN
    -- Safe name generation
    tview_name := format('tv_user_posts_%s', replace(user_uuid::TEXT, '-', '_'));

    -- Use parameterized queries
    EXECUTE format('SELECT pg_tviews_create(%L, %L)', tview_name, $$
        SELECT
          tb_post.pk_post,
          tb_post.id,
          jsonb_build_object('id', tb_post.id, 'title', tb_post.title) as data
        FROM tb_post WHERE tb_post.fk_user = (
          SELECT pk_user FROM tb_user WHERE id = $1
        )
    $$) USING user_uuid;
END;
$$ LANGUAGE plpgsql SECURITY DEFINER;
```

## Column-Level Security

### Excluding Sensitive Data

**❌ BAD: Including sensitive data**
```sql
CREATE TABLE tv_user AS
SELECT
  tb_user.pk_user,
  tb_user.id,
  jsonb_build_object(
    'id', tb_user.id,
    'username', tb_user.username,
    'passwordHash', tb_user.password_hash  -- DON'T EXPOSE!
  ) as data
FROM tb_user;
```

**✅ GOOD: Exclude sensitive columns**
```sql
CREATE TABLE tv_user AS
SELECT
  tb_user.pk_user,
  tb_user.id,
  jsonb_build_object(
    'id', tb_user.id,
    'username', tb_user.username,
    'email', tb_user.email,
    'createdAt', tb_user.created_at
  ) as data
FROM tb_user;
```

### Row-Level Security (RLS)

Implement RLS on TVIEWs for multi-tenant applications:

```sql
-- Enable RLS on TVIEW
ALTER TABLE tv_post ENABLE ROW LEVEL SECURITY;

-- Create security policy
CREATE POLICY tenant_posts ON tv_post
    FOR ALL
    USING (fk_user IN (
        SELECT pk_user FROM tb_user
        WHERE tenant_id = current_setting('app.tenant_id')::UUID
    ));

-- Create indexes to support RLS efficiently
CREATE INDEX idx_tv_post_fk_user_rls ON tv_post(fk_user);
```

## Access Control

### Granting Permissions

```sql
-- Grant read access to application user
GRANT SELECT ON tv_post TO app_user;
GRANT SELECT ON tv_user TO app_user;

-- Grant write access for data modifications
GRANT UPDATE ON tv_post TO app_admin;
GRANT INSERT, UPDATE, DELETE ON tb_post TO app_admin;

-- Grant TVIEW management permissions
GRANT EXECUTE ON FUNCTION pg_tviews_create(TEXT, TEXT) TO db_admin;
GRANT EXECUTE ON FUNCTION pg_tviews_drop(TEXT, BOOLEAN) TO db_admin;
```

### Role-Based Access

```sql
-- Create roles
CREATE ROLE readonly_user;
CREATE ROLE readwrite_user;
CREATE ROLE admin_user;

-- Grant appropriate permissions
GRANT SELECT ON ALL TABLES IN SCHEMA public TO readonly_user;
GRANT SELECT, INSERT, UPDATE, DELETE ON tb_post, tb_user TO readwrite_user;
GRANT ALL PRIVILEGES ON ALL TABLES IN SCHEMA public TO admin_user;
GRANT EXECUTE ON ALL FUNCTIONS IN SCHEMA public TO admin_user;
```

## Data Validation

### Input Sanitization

```sql
-- ✅ SAFE: Validate UUID inputs
CREATE OR REPLACE FUNCTION get_user_posts_safe(user_id_param TEXT)
RETURNS TABLE (
    pk_post BIGINT,
    id UUID,
    data JSONB
) AS $$
BEGIN
    -- Validate input is valid UUID
    IF user_id_param !~ '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$' THEN
        RAISE EXCEPTION 'Invalid UUID format';
    END IF;

    RETURN QUERY
    SELECT tv_post.pk_post, tv_post.id, tv_post.data
    FROM tv_post
    WHERE tv_post.fk_user = (
        SELECT pk_user FROM tb_user WHERE id = user_id_param::UUID
    );
END;
$$ LANGUAGE plpgsql SECURITY DEFINER;
```

### JSONB Data Validation

```sql
-- Validate JSONB structure before insertion
CREATE OR REPLACE FUNCTION validate_post_data()
RETURNS TRIGGER AS $$
BEGIN
    -- Check required fields exist
    IF NEW.data->>'id' IS NULL THEN
        RAISE EXCEPTION 'Post data must include id field';
    END IF;

    IF NEW.data->>'title' IS NULL OR trim(NEW.data->>'title') = '' THEN
        RAISE EXCEPTION 'Post data must include non-empty title field';
    END IF;

    -- Validate data types
    IF jsonb_typeof(NEW.data->'id') != 'string' THEN
        RAISE EXCEPTION 'Post id must be a string';
    END IF;

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

-- Attach trigger to base table
CREATE TRIGGER validate_post_data_trigger
    BEFORE INSERT OR UPDATE ON tb_post
    FOR EACH ROW EXECUTE FUNCTION validate_post_data();
```

## Secure Configuration

### Connection Security

```sql
-- Use SSL connections
ALTER SYSTEM SET ssl = 'on';

-- Require SSL for pg_tviews operations
ALTER SYSTEM SET ssl_min_protocol_version = 'TLSv1.2';

-- Set secure search_path
ALTER DATABASE your_db SET search_path = 'public';
```

### Extension Security

```sql
-- Grant extension privileges carefully
GRANT CREATE ON SCHEMA public TO pg_tviews_user;

-- Don't grant superuser privileges
-- GRANT SUPERUSER TO pg_tviews_user;  -- DON'T DO THIS

-- Use SECURITY DEFINER for controlled access
CREATE FUNCTION pg_tviews_create_secure(tview_name TEXT, sql_query TEXT)
RETURNS VOID AS $$
BEGIN
    -- Add security checks here
    IF current_user != 'pg_tviews_admin' THEN
        RAISE EXCEPTION 'Access denied';
    END IF;

    -- Validate inputs
    IF tview_name !~ '^tv_[a-z_]+$' THEN
        RAISE EXCEPTION 'Invalid TVIEW name format';
    END IF;

    -- Call actual function
    PERFORM pg_tviews_create(tview_name, sql_query);
END;
$$ LANGUAGE plpgsql SECURITY DEFINER;
```

## Audit Logging

### Enable Audit Trails

```sql
-- Create audit table
CREATE TABLE audit_log (
    id BIGSERIAL PRIMARY KEY,
    timestamp TIMESTAMPTZ DEFAULT NOW(),
    user_name TEXT,
    operation TEXT,
    table_name TEXT,
    old_values JSONB,
    new_values JSONB,
    client_ip INET
);

-- Create audit function
CREATE OR REPLACE FUNCTION audit_trigger_func()
RETURNS TRIGGER AS $$
BEGIN
    INSERT INTO audit_log (user_name, operation, table_name, old_values, new_values, client_ip)
    VALUES (
        current_user,
        TG_OP,
        TG_TABLE_NAME,
        CASE WHEN TG_OP != 'INSERT' THEN row_to_json(OLD) ELSE NULL END,
        CASE WHEN TG_OP != 'DELETE' THEN row_to_json(NEW) ELSE NULL END,
        inet_client_addr()
    );
    RETURN COALESCE(NEW, OLD);
END;
$$ LANGUAGE plpgsql;

-- Attach to TVIEW tables
CREATE TRIGGER audit_tv_post
    AFTER INSERT OR UPDATE OR DELETE ON tv_post
    FOR EACH ROW EXECUTE FUNCTION audit_trigger_func();
```

### Monitor Suspicious Activity

```sql
-- Alert on unusual patterns
CREATE OR REPLACE FUNCTION monitor_suspicious_activity()
RETURNS VOID AS $$
DECLARE
    suspicious_count INTEGER;
BEGIN
    -- Check for excessive TVIEW creations
    SELECT COUNT(*) INTO suspicious_count
    FROM audit_log
    WHERE operation = 'CREATE_TVIEW'
      AND timestamp > NOW() - INTERVAL '1 hour'
      AND user_name != 'pg_tviews_admin';

    IF suspicious_count > 10 THEN
        -- Send alert
        RAISE WARNING 'Suspicious TVIEW creation activity detected';
    END IF;
END;
$$ LANGUAGE plpgsql;
```

## Performance Security

### Prevent Resource Exhaustion

```sql
-- Set reasonable limits
ALTER SYSTEM SET work_mem = '64MB';
ALTER SYSTEM SET maintenance_work_mem = '256MB';
ALTER SYSTEM SET max_parallel_workers_per_gather = 2;

-- Limit TVIEW complexity
CREATE OR REPLACE FUNCTION validate_tview_complexity()
RETURNS TRIGGER AS $$
DECLARE
    join_count INTEGER;
    table_count INTEGER;
BEGIN
    -- Count JOINs in the SQL
    SELECT
        array_length(regexp_split_array(NEW.sql_definition, 'JOIN|FROM'), 1) - 1,
        array_length(regexp_split_array(NEW.sql_definition, 'FROM'), 1)
    INTO join_count, table_count;

    IF join_count > 5 THEN
        RAISE EXCEPTION 'TVIEW too complex: % JOINs (max 5)', join_count;
    END IF;

    IF table_count > 3 THEN
        RAISE EXCEPTION 'TVIEW too complex: % tables (max 3)', table_count;
    END IF;

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

-- Attach to metadata table
CREATE TRIGGER validate_tview_complexity_trigger
    BEFORE INSERT OR UPDATE ON pg_tview_meta
    FOR EACH ROW EXECUTE FUNCTION validate_tview_complexity();
```

## Incident Response

### Security Breach Procedures

1. **Immediate Response**
   ```sql
   -- Disconnect suspicious sessions
   SELECT pg_terminate_backend(pid)
   FROM pg_stat_activity
   WHERE usename = 'suspicious_user';

   -- Disable public access
   REVOKE ALL ON ALL TABLES IN SCHEMA public FROM PUBLIC;
   ```

2. **Investigation**
   ```sql
   -- Check audit logs
   SELECT * FROM audit_log
   WHERE timestamp > NOW() - INTERVAL '24 hours'
   ORDER BY timestamp DESC;

   -- Check for unauthorized TVIEWs
   SELECT * FROM pg_tview_meta
   WHERE created_at > NOW() - INTERVAL '24 hours';
   ```

3. **Recovery**
   ```sql
   -- Restore from clean backup
   -- Recreate TVIEWs from trusted definitions
   -- Update security policies
   ```

## Best Practices Summary

1. **Validate all inputs** before using in SQL
2. **Use parameterized queries** instead of string concatenation
3. **Exclude sensitive data** from TVIEWs
4. **Implement RLS** for multi-tenant applications
5. **Grant minimal permissions** required
6. **Enable audit logging** for critical operations
7. **Monitor resource usage** to prevent DoS
8. **Regular security reviews** of TVIEW definitions
9. **Keep backups secure** and test restoration
10. **Have incident response plan** ready

## See Also

- [API Reference](../reference/api.md) - Function permissions
- [Troubleshooting Guide](troubleshooting.md) - Security-related issues
- [Monitoring Guide](monitoring.md) - Production security monitoring