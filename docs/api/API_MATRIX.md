# pg_tviews API Stability Matrix

## Quick Reference

| Function | Type | Stability | Since | Maturity | Recommendation |
|----------|------|-----------|-------|----------|-----------------|
| pg_tviews_convert_existing_table | SQL | STABLE | 0.1.0-beta.1 | Production | ✅ Safe |
| pg_tviews_version | SQL | STABLE | 0.1.0-beta.1 | Production | ✅ Safe |
| pg_tviews_metadata | SQL | STABLE | 0.1.0-beta.1 | Production | ✅ Safe |
| pg_tviews_health_check | SQL | STABLE | 0.1.0-beta.1 | Production | ✅ Safe |
| pg_tviews_debug_queue | SQL | EVOLVING | 0.1.0-beta.1 | Debug only | ⚠️ May change |
| pg_tviews_queue_stats | SQL | EVOLVING | 0.1.0-beta.1 | Debug only | ⚠️ May change |
| pg_tviews_clear_queue | SQL | EXPERIMENTAL | 0.1.0-beta.1 | Advanced | ❌ Experts only |
| pg_tviews_performance_stats | SQL | EXPERIMENTAL | 0.1.0-beta.1 | Advanced | ❌ Experts only |
| pg_tviews_create | SQL | EXPERIMENTAL | 0.1.0-beta.1 | Testing | ⚠️ Use DDL instead |
| pg_tviews_drop | SQL | EXPERIMENTAL | 0.1.0-beta.1 | Testing | ⚠️ Use DDL instead |
| pg_tviews_refresh | SQL | EXPERIMENTAL | 0.1.0-beta.1 | Testing | ⚠️ Benchmarking only |
| pg_tviews_commit_prepared | SQL | EXPERIMENTAL | 0.1.0-beta.1 | Advanced | ❌ 2PC experts only |
| pg_tviews_rollback_prepared | SQL | EXPERIMENTAL | 0.1.0-beta.1 | Advanced | ❌ 2PC experts only |
| refresh_pk | Rust | STABLE | 0.1.0-beta.1 | Production | ✅ Safe |
| refresh_batch | Rust | STABLE | 0.1.0-beta.1 | Production | ✅ Safe |
| find_base_tables | Rust | STABLE | 0.1.0-beta.1 | Production | ✅ Safe |
| ViewRow | Rust | STABLE | 0.1.0-beta.1 | Production | ✅ Safe |
| TViewError | Rust | STABLE | 0.1.0-beta.1 | Production | ✅ Safe |
| RefreshKey | Rust | EVOLVING | 0.1.0-beta.1 | Internal | ⚠️ May change |

## Stability Guarantees by Version

### 0.1.x - Beta Period
- All STABLE functions guaranteed compatible
- EVOLVING functions may change
- EXPERIMENTAL functions may disappear

### 1.0.x - Production Release
- All STABLE functions guaranteed compatible
- EVOLVING functions may change in 1.1+
- EXPERIMENTAL functions stabilize or deprecate

### 2.0.x - Next Major Release
- Breaking changes allowed for all APIs
- Clear migration path required for each change
- 12+ month deprecation notice

## Using Stable APIs in Production

✅ **Recommended**: Use STABLE functions in production
- Safe to upgrade minor versions (0.1 → 0.2 → 1.0)
- Breaking changes only in major versions
- 12+ month deprecation notice for any removals

⚠️ **Caution**: EVOLVING APIs in production
- May change in minor versions
- Monitor release notes carefully
- Consider pinning to specific version

❌ **Not Recommended**: EXPERIMENTAL APIs in production
- No compatibility guarantee
- Use only for debugging/testing
- Do not rely in automation

---

## Future Stability Targets (v1.0+)

| Current Status | Target Status | Target Version | Notes |
|---|---|---|---|
| EVOLVING | STABLE | 1.1 | pg_tviews_debug_queue output schema |
| EVOLVING | STABLE | 1.1 | pg_tviews_queue_stats format |
| EXPERIMENTAL | Deprecated | 1.0 | pg_tviews_clear_queue (needs safer alternative) |
| EXPERIMENTAL | STABLE | 1.0 | Advanced refresh tuning APIs |
| EXPERIMENTAL | STABLE | 1.0 | pg_tviews_create/pg_tviews_drop (improve validation) |

---

## Migration Examples

### From EXPERIMENTAL to STABLE

```sql
-- v0.1 (EXPERIMENTAL - may disappear)
SELECT pg_tviews_create('my_view', 'SELECT * FROM users');

-- v1.0 (STABLE - guaranteed to work)
-- Use DDL syntax instead:
CREATE TVIEW my_view AS SELECT * FROM users;
```

### Handling EVOLVING API Changes

```sql
-- Monitor for changes in release notes
-- Current (v0.1):
SELECT * FROM pg_tviews_debug_queue();

-- Future (v1.1) - may change:
SELECT queue_id, entity, priority FROM pg_tviews_refresh_queue();
```

---

## API Governance

### Adding New APIs
1. **Start as EXPERIMENTAL** in beta releases
2. **Promote to EVOLVING** after feedback and testing
3. **Promote to STABLE** in major releases with long-term commitment

### Deprecating APIs
1. **Mark as DEPRECATED** with replacement guidance
2. **Provide migration path** in documentation
3. **Remove after deprecation period** (6+ months)

### Breaking Changes
1. **Only in major versions** for STABLE APIs
2. **12+ month notice** for planned breaking changes
3. **Migration tools** provided where possible