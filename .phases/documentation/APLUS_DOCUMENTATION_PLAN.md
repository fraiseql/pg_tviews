# A+ Documentation Quality Plan for pg_tviews

**Version**: 0.1.0-beta.1 → 1.0.0
**Plan Created**: 2025-12-11
**Estimated Timeline**: 6-8 weeks (part-time) or 2-3 weeks (full-time)
**Total Effort**: 80-120 hours

---

## Executive Summary

This plan transforms pg_tviews documentation from "good beta quality" to **A+ production-grade** documentation that:

1. **Eliminates all critical gaps** identified in the architect review
2. **Fixes all inconsistencies** between code and documentation
3. **Adds missing operational guidance** for production deployment
4. **Creates comprehensive learning paths** for all user personas
5. **Establishes documentation maintenance** processes for long-term quality

### What Makes This "A+" Quality?

**A+ documentation means**:
- ✅ **Zero blockers**: Every feature is documented before users need it
- ✅ **Zero inconsistencies**: Docs match code 100%, verified by tests
- ✅ **Zero assumptions**: New users can succeed without prior knowledge
- ✅ **Self-service**: 90%+ of questions answered without support
- ✅ **Production-ready**: Complete operational runbooks and disaster recovery
- ✅ **Maintainable**: Automated checks keep docs in sync with code

### Current State Assessment

**Strengths** (Keep):
- Good high-level README with clear value proposition
- Comprehensive changelog and release notes
- Strong benchmarking methodology and results
- Solid architecture documentation foundation
- Good quick start guide

**Critical Issues** (Fix First):
- Inconsistency: `CREATE TVIEW` syntax vs `pg_tviews_create()` function
- Missing: Migration guide from traditional materialized views
- Missing: Disaster recovery procedures
- Missing: jsonb_ivm dependency clarity
- Unclear: Production deployment checklist
- Missing: Security model documentation

**Enhancement Opportunities** (Add Later):
- Interactive examples and tutorials
- Video walkthroughs
- Visual architecture diagrams
- Community contribution templates
- Performance tuning decision trees

---

## Phase Structure Overview

This plan consists of **5 major phases** broken into **20 sub-phases**:

### Phase A: Foundation & Consistency (Weeks 1-2)
Fix critical inconsistencies and establish documentation infrastructure.

### Phase B: Comprehensive Reference (Weeks 2-4)
Complete all API, DDL, SQL, and error documentation.

### Phase C: Operational Excellence (Weeks 4-5)
Add production deployment, migration, and disaster recovery guides.

### Phase D: Learning & Onboarding (Weeks 5-6)
Create tutorials, examples, and learning paths for all personas.

### Phase E: Maintenance & Quality (Weeks 6-8)
Establish processes to keep documentation at A+ quality forever.

---

## Phase A: Foundation & Consistency (16-24 hours)

### A1: Documentation Audit & Inventory (4 hours)

**Objective**: Create source-of-truth inventory of all features vs documentation status.

**Tasks**:
1. **Code Inventory**:
   - List all public functions from `src/lib.rs`
   - List all SQL functions from `sql/*.sql`
   - List all views from monitoring SQL
   - List all error types from `src/error/mod.rs`
   - List all configuration options

2. **Documentation Inventory**:
   - Map each code item to documentation location
   - Identify gaps (feature exists, no docs)
   - Identify orphans (docs exist, no feature)
   - Identify conflicts (docs contradict code)

3. **Create Tracking Matrix**:
   ```markdown
   | Feature | Code Location | Doc Location | Status | Priority |
   |---------|--------------|--------------|--------|----------|
   | CREATE TVIEW | src/ddl/create.rs | docs/reference/ddl.md | ✅ Documented | High |
   | pg_tviews_create() | src/lib.rs | ❌ Missing | High |
   ```

**Deliverables**:
- `DOCUMENTATION_INVENTORY.md`: Complete feature-to-doc mapping
- `DOCUMENTATION_ISSUES.md`: List of all inconsistencies
- GitHub issues for each critical gap

**Acceptance Criteria**:
- [ ] 100% of public API catalogued
- [ ] 100% of SQL objects catalogued
- [ ] All inconsistencies documented
- [ ] Prioritization matrix complete

---

### A2: Resolve DDL Syntax Inconsistency (6 hours)

**Objective**: Fix the CREATE TVIEW vs pg_tviews_create() confusion once and for all.

**Current Confusion**:
- Quick Start shows: `CREATE TVIEW tv_name AS SELECT...`
- API Reference shows: `SELECT pg_tviews_create('tv_name', 'SELECT...')`
- Users don't know which to use or why both exist

**Investigation Required**:
1. Check `src/ddl/create.rs` - is there a ProcessUtility hook?
2. Check `src/lib.rs` - is `pg_tviews_create()` just an alternative API?
3. Test both approaches - do they produce identical results?
4. Determine the "blessed" approach vs "alternative" approach

**Resolution Options**:

**Option 1**: Both are valid (document both clearly)
```markdown
## Two Ways to Create TVIEWs

### Method 1: SQL DDL (Recommended)
Most users should use standard SQL DDL syntax:
```sql
CREATE TVIEW tv_posts AS SELECT ...;
```

### Method 2: Function API (Programmatic)
Use this for dynamic TVIEW creation in stored procedures:
```sql
SELECT pg_tviews_create('tv_posts', 'SELECT ...');
```
```

**Option 2**: One is deprecated (mark clearly)
```markdown
## Creating TVIEWs

Use the SQL DDL syntax:
```sql
CREATE TVIEW tv_posts AS SELECT ...;
```

⚠️ **Deprecated**: The `pg_tviews_create()` function exists for
backward compatibility but will be removed in 2.0.0.
```

**Tasks**:
1. Investigate implementation (2 hours)
2. Decide on official guidance (1 hour)
3. Update all docs to reflect decision (2 hours)
4. Add prominent note in README (30 min)
5. Update Quick Start if needed (30 min)

**Deliverables**:
- Decision documented in `docs/reference/ddl.md`
- README.md updated with clear guidance
- All examples use consistent approach
- Migration note if one method is deprecated

**Acceptance Criteria**:
- [ ] Investigation complete, decision documented
- [ ] All docs show consistent approach
- [ ] README has clear "Creating TVIEWs" section
- [ ] No conflicting examples remain

---

### A3: Clarify jsonb_ivm Dependency (4 hours)

**Objective**: Make dependency status crystal clear in all documentation.

**Current Confusion**:
- Some docs say it's "required"
- Code shows it's optional (feature detection)
- Users don't know when they need it

**Resolution Strategy**:

1. **Create Dependency Matrix**:
```markdown
## Dependencies

### Required (Cannot Use Without)
- PostgreSQL 15+
- Rust 1.70+ (build time only)
- pgrx 0.12.8+ (build time only)

### Optional (Enhances Performance)
- jsonb_ivm extension: 1.5-3× faster JSONB updates
  - Without: Uses native jsonb_set (slower)
  - With: Uses surgical patching (faster)
  - Performance impact: 2.03× improvement validated
```

2. **Add Decision Guide**:
```markdown
## Do I Need jsonb_ivm?

**Use jsonb_ivm if**:
- ✅ You have large JSONB objects (>100 fields)
- ✅ You update frequently (>100 ops/sec)
- ✅ You can install additional extensions

**Skip jsonb_ivm if**:
- ❌ Your JSONB objects are small (<20 fields)
- ❌ Updates are infrequent (<10 ops/sec)
- ❌ You cannot install additional extensions
- ❌ You prefer minimal dependencies

**Performance Comparison**:
| Workload | Without jsonb_ivm | With jsonb_ivm | Improvement |
|----------|------------------|----------------|-------------|
| Small (1K rows) | 0.591 ms | 0.364 ms | 1.6× faster |
| Medium (100K rows) | 1.255 ms | 0.591 ms | 2.1× faster |
```

**Tasks**:
1. Review code for actual dependency behavior (1 hour)
2. Test both modes thoroughly (1 hour)
3. Update README with dependency matrix (1 hour)
4. Update installation docs (30 min)
5. Add FAQ entry (30 min)

**Deliverables**:
- Updated `docs/getting-started/installation.md`
- Dependency matrix in README
- Performance comparison table
- FAQ entry: "Do I need jsonb_ivm?"

**Acceptance Criteria**:
- [ ] Every mention of jsonb_ivm is consistent
- [ ] Installation shows both paths (with/without)
- [ ] Performance impact quantified
- [ ] Decision guide clear

---

### A4: Version & Status Consistency (2 hours)

**Objective**: Fix version labeling confusion and update status tracking.

**Issues**:
- Marked "0.1.0-beta.1" but claims "production-ready"
- Status tables in docs/README.md are outdated
- No clear roadmap to 1.0.0

**Tasks**:

1. **Define Version Strategy**:
```markdown
## Version Roadmap

### Current: 0.1.0-beta.1 (December 2025)
- **Status**: Public Beta
- **Stability**: Feature-complete, API may change
- **Production Use**: Suitable for evaluation, not mission-critical
- **Support**: Community support, no SLA

### Target: 1.0.0 (Q1 2026)
- **Status**: Stable Release
- **Stability**: API stable, semantic versioning
- **Production Use**: Fully supported for production
- **Support**: Issue tracking, SLA for critical bugs

### Criteria for 1.0.0
- [ ] All A+ documentation complete
- [ ] 100+ production hours logged by beta testers
- [ ] Zero critical bugs outstanding
- [ ] Performance validated at 1M+ row scale
- [ ] Migration guide from beta versions complete
- [ ] Security audit complete
```

2. **Update Status Tables**:
   - Audit all status indicators in documentation
   - Update completion status
   - Remove outdated "Week 2", "Week 3" references
   - Replace with actual completion dates or "Planned for 1.0.0"

3. **Add Stability Badges**:
```markdown
## API Stability

| Component | Stability | Notes |
|-----------|-----------|-------|
| CREATE TVIEW syntax | 🟢 Stable | No breaking changes planned |
| pg_tviews_*() functions | 🟡 Beta | Minor changes possible |
| Monitoring views | 🟢 Stable | Structure locked |
| Internal Rust API | 🔴 Unstable | Not for external use |
```

**Deliverables**:
- Updated version roadmap in README
- All status tables synchronized
- API stability matrix
- Clear beta → 1.0.0 criteria

**Acceptance Criteria**:
- [ ] No conflicting status indicators
- [ ] Version roadmap clear
- [ ] 1.0.0 criteria defined
- [ ] Beta warning prominent in README

---

### A5: Establish Documentation Standards (4 hours)

**Objective**: Create style guide and templates for consistent documentation.

**Deliverables**:

1. **Style Guide** (`docs/style-guide.md`):
```markdown
# pg_tviews Documentation Style Guide

## Formatting Standards

### Code Blocks
- SQL: Use ```sql with proper indentation
- Bash: Use ```bash with $ prompt for user commands
- Output: Use ```text for command output
- Rust: Use ```rust for extension code examples

### Examples
- Always include expected output
- Use realistic data (e.g., blog posts, not foo/bar)
- Show both success and error cases
- Test all examples before committing

### Terminology
- "TVIEW" (capitalized): The feature/technology
- "tv_posts" (code format): Specific table name
- "materialized view": Traditional PostgreSQL feature
- "incremental refresh": Our approach

### Forbidden
- ❌ "Simply..." or "Just..." (condescending)
- ❌ "Obviously..." (not obvious to everyone)
- ❌ TODO comments in published docs
- ❌ Broken internal links
- ❌ Untested code examples
```

2. **Function Documentation Template**:
```markdown
### function_name()

**Signature**:
```sql
function_name(param1 TYPE, param2 TYPE) RETURNS TYPE
```

**Description**:
One-sentence summary of what this function does.

**Parameters**:
- `param1` (TYPE): Description of first parameter
- `param2` (TYPE, optional): Description with default value

**Returns**:
- `TYPE`: Description of return value

**Example**:
```sql
SELECT function_name('value1', 'value2');
```
Returns:
```text
expected output here
```

**Notes**:
- Additional context
- Performance considerations
- Common pitfalls

**See Also**:
- [Related Function](#related-function)
- [Concept Guide](../guides/concept.md)
```

3. **Document Header Template**:
```markdown
# Document Title

Brief one-paragraph description of what this document covers.

**Version**: 0.1.0-beta.1 • **Last Updated**: YYYY-MM-DD

## Table of Contents
- [Section 1](#section-1)
- [Section 2](#section-2)

## Section 1
...
```

4. **Review Checklist Template**:
```markdown
## Documentation Review Checklist

### Content
- [ ] Technically accurate (verified against code)
- [ ] All examples tested and work
- [ ] All links valid
- [ ] No typos (spell check passed)
- [ ] Appropriate level of detail

### Structure
- [ ] Header with version/date
- [ ] Table of contents (if >500 lines)
- [ ] Proper heading hierarchy (no skipped levels)
- [ ] Code blocks properly formatted
- [ ] Consistent terminology

### Quality
- [ ] Clear and concise
- [ ] Appropriate examples
- [ ] Cross-references added
- [ ] Follows style guide
- [ ] Accessibility considered
```

**Tasks**:
1. Write style guide (2 hours)
2. Create templates (1 hour)
3. Set up documentation CI checks (1 hour)

**Acceptance Criteria**:
- [ ] Style guide covers all common cases
- [ ] Templates exist for all doc types
- [ ] Review checklist ready to use
- [ ] CI validates markdown quality

---

## Phase B: Comprehensive Reference (32-48 hours)

### B1: Complete API Reference (8 hours)

**Objective**: Document all public PostgreSQL functions with examples.

**Already Done** (per inventory):
- ✅ `docs/reference/api.md` exists and covers 12 functions
- Status: Review and enhance

**Tasks**:

1. **Audit Existing API Docs** (2 hours):
   - Verify each function signature matches code
   - Test all examples
   - Check return types are accurate
   - Ensure consistent format

2. **Add Missing Details** (4 hours):
   - Performance characteristics for each function
   - Common use cases section
   - Error conditions and handling
   - Related functions cross-references
   - Real-world examples from benchmarks

3. **Create API Reference Index** (1 hour):
```markdown
## API Quick Reference

### Extension Management
- [pg_tviews_version()](#pg_tviews_version) - Get version
- [pg_tviews_check_jsonb_ivm()](#pg_tviews_check_jsonb_ivm) - Check dependencies

### Queue Management
- [pg_tviews_queue_stats()](#pg_tviews_queue_stats) - Queue statistics
- [pg_tviews_debug_queue()](#pg_tviews_debug_queue) - Debug queue

### Schema Analysis
- [pg_tviews_analyze_select()](#pg_tviews_analyze_select) - Analyze SQL
- [pg_tviews_infer_types()](#pg_tviews_infer_types) - Infer types

### Manual Operations
- [pg_tviews_cascade()](#pg_tviews_cascade) - Force cascade
- [pg_tviews_insert()](#pg_tviews_insert) - Manual insert
- [pg_tviews_delete()](#pg_tviews_delete) - Manual delete

### Two-Phase Commit
- [pg_tviews_commit_prepared()](#pg_tviews_commit_prepared) - Commit 2PC
- [pg_tviews_rollback_prepared()](#pg_tviews_rollback_prepared) - Rollback 2PC
- [pg_tviews_recover_prepared_transactions()](#pg_tviews_recover_prepared_transactions) - Recover
```

4. **Add Usage Patterns Section** (1 hour):
```markdown
## Common Usage Patterns

### Check Extension Health
```sql
-- Verify installation
SELECT pg_tviews_version();

-- Check optional features
SELECT pg_tviews_check_jsonb_ivm();

-- Full health check
SELECT * FROM pg_tviews_health_check();
```

### Monitor Performance
```sql
-- Current queue status
SELECT pg_tviews_queue_stats();

-- Cache efficiency
SELECT * FROM pg_tviews_cache_stats;

-- Historical metrics
SELECT * FROM pg_tviews_performance_summary
WHERE hour > now() - interval '24 hours';
```

### Debug Issues
```sql
-- See what's queued
SELECT pg_tviews_debug_queue();

-- Force manual refresh
SELECT pg_tviews_cascade('tb_post'::regclass::oid, 123);
```
```

**Deliverables**:
- Enhanced `docs/reference/api.md`
- All examples tested and verified
- Usage patterns guide
- Performance notes for each function

**Acceptance Criteria**:
- [ ] All 12 functions fully documented
- [ ] Every example tested and works
- [ ] Performance characteristics noted
- [ ] Common patterns section complete
- [ ] No TODOs or placeholders

---

### B2: Complete DDL Reference (6 hours)

**Objective**: Comprehensive CREATE/DROP/ALTER TVIEW documentation.

**Already Done**:
- ✅ `docs/reference/ddl.md` exists
- Status: Needs enhancement with edge cases

**Tasks**:

1. **CREATE TVIEW Comprehensive Docs** (3 hours):

```markdown
## CREATE TVIEW

### Basic Syntax
```sql
CREATE TVIEW tv_entity_name AS
SELECT ...;
```

### Full Syntax
```sql
CREATE TVIEW [ IF NOT EXISTS ] tv_entity_name AS
    SELECT
        pk_column,
        id_column,
        fk_columns...,
        jsonb_build_object(...) AS data
    FROM source_tables
    WHERE conditions;
```

### Required Elements

#### Table Name Convention
- **MUST** start with `tv_` prefix
- ✅ Valid: `tv_posts`, `tv_user_profiles`
- ❌ Invalid: `posts`, `tview_posts`, `v_posts`

#### Column Requirements
1. **Primary Key Column** (required):
   - Name pattern: `pk_{entity}`
   - Type: `BIGINT` or `INTEGER`
   - Must match source table PK

2. **UUID Column** (required):
   - Name: `id`
   - Type: `UUID`
   - Used for GraphQL queries

3. **Foreign Key Columns** (required for cascades):
   - Name pattern: `fk_{parent_entity}`
   - Type: `BIGINT` or `INTEGER`
   - Used for dependency tracking

4. **Data Column** (required):
   - Name: `data`
   - Type: `JSONB`
   - Contains complete read model

### Optional Elements

#### SEO-Friendly Identifier
```sql
identifier TEXT UNIQUE  -- Optional but recommended
```

#### Filtering UUID Foreign Keys
```sql
user_id UUID  -- For efficient GraphQL filtering
```

### Supported SQL Features

✅ **Supported**:
- JOINs (INNER, LEFT, RIGHT, FULL)
- WHERE clauses
- Subqueries in SELECT
- Aggregate functions (jsonb_agg, array_agg)
- Window functions
- CTEs (WITH clauses)
- CASE expressions

❌ **Not Supported**:
- UNION/INTERSECT/EXCEPT
- Recursive CTEs
- Volatile functions in SELECT
- DISTINCT ON
- GROUP BY without aggregation in data

### Examples

#### Simple TVIEW
```sql
CREATE TVIEW tv_user AS
SELECT
    pk_user,
    id,
    identifier,
    jsonb_build_object(
        'id', id,
        'name', name,
        'email', email
    ) AS data
FROM tb_user;
```

#### TVIEW with JOINs
```sql
CREATE TVIEW tv_post AS
SELECT
    p.pk_post,
    p.id,
    p.identifier,
    p.fk_user,
    u.id AS user_id,
    jsonb_build_object(
        'id', p.id,
        'title', p.title,
        'author', jsonb_build_object(
            'id', u.id,
            'name', u.name
        )
    ) AS data
FROM tb_post p
JOIN tb_user u ON p.fk_user = u.pk_user;
```

#### TVIEW with Aggregations
```sql
CREATE TVIEW tv_category AS
SELECT
    c.pk_category,
    c.id,
    c.identifier,
    jsonb_build_object(
        'id', c.id,
        'name', c.name,
        'postCount', COUNT(p.pk_post),
        'posts', COALESCE(jsonb_agg(
            jsonb_build_object('id', p.id, 'title', p.title)
        ) FILTER (WHERE p.pk_post IS NOT NULL), '[]'::jsonb)
    ) AS data
FROM tb_category c
LEFT JOIN tb_post p ON p.fk_category = c.pk_category
GROUP BY c.pk_category, c.id, c.identifier;
```

### Error Messages

| Error | Cause | Solution |
|-------|-------|----------|
| "TVIEW name must start with tv_" | Invalid naming | Rename to tv_* pattern |
| "Missing required pk_ column" | No primary key | Add pk_{entity} column |
| "Missing data column" | No JSONB column | Add data JSONB column |
| "Invalid SELECT statement" | Unsupported SQL | Check supported features |
```

2. **DROP TVIEW Documentation** (1 hour):

```markdown
## DROP TVIEW

### Syntax
```sql
DROP TVIEW [ IF EXISTS ] tv_entity_name [ CASCADE ];
```

### Behavior

When you drop a TVIEW, the following occurs:

1. **Triggers Removed**: All triggers on source tables are deleted
2. **Metadata Cleaned**: Entry removed from `pg_tview_meta`
3. **Table Dropped**: The `tv_*` table is deleted
4. **Views Preserved**: Source `v_*` view (if any) is **not** affected

### CASCADE Behavior

Currently `CASCADE` is **not implemented**. Attempting to drop a TVIEW that other TVIEWs depend on will fail.

**Workaround**: Drop dependent TVIEWs first, then drop the parent.

### Examples

#### Simple Drop
```sql
DROP TVIEW tv_posts;
```

#### Safe Drop (No Error if Missing)
```sql
DROP TVIEW IF EXISTS tv_posts;
```

#### Drop with Dependencies (Future)
```sql
-- Not yet supported, will error if dependencies exist
DROP TVIEW tv_user CASCADE;
```

### Recovery

If you accidentally drop a TVIEW:

1. **Recreate with original SQL**:
   ```sql
   CREATE TVIEW tv_posts AS SELECT ...;
   ```

2. **Data will be regenerated** on next write to source table

3. **No data loss** (rebuilt from source tables)
```

3. **Limitations and Workarounds** (2 hours):

```markdown
## Limitations

### SQL Feature Limitations

#### 1. DISTINCT ON Not Supported

❌ **Does not work**:
```sql
CREATE TVIEW tv_latest_post AS
SELECT DISTINCT ON (fk_user)
    pk_post, id, fk_user, ...
FROM tb_post
ORDER BY fk_user, created_at DESC;
```

✅ **Workaround** (use window functions):
```sql
CREATE TVIEW tv_latest_post AS
SELECT pk_post, id, fk_user, data
FROM (
    SELECT
        pk_post, id, fk_user,
        jsonb_build_object(...) AS data,
        ROW_NUMBER() OVER (PARTITION BY fk_user ORDER BY created_at DESC) AS rn
    FROM tb_post
) sub
WHERE rn = 1;
```

#### 2. UNION Not Supported

❌ **Does not work**:
```sql
CREATE TVIEW tv_all_content AS
SELECT pk_post AS pk, 'post' AS type, ... FROM tb_post
UNION ALL
SELECT pk_page AS pk, 'page' AS type, ... FROM tb_page;
```

✅ **Workaround** (use separate TVIEWs):
```sql
-- Create separate TVIEWs, query both from application
CREATE TVIEW tv_post AS SELECT ... FROM tb_post;
CREATE TVIEW tv_page AS SELECT ... FROM tb_page;
```

### Schema Limitations

#### 1. Trinity Pattern Required

pg_tviews **requires** the trinity identifier pattern:
- Primary key: `pk_{entity}` (BIGINT)
- UUID: `id` (UUID)
- Foreign keys: `fk_{parent}` (BIGINT)

If your existing schema doesn't follow this, you have two options:

**Option A**: Add trinity columns to existing tables
```sql
ALTER TABLE posts ADD COLUMN pk_post BIGSERIAL PRIMARY KEY;
ALTER TABLE posts ADD COLUMN id UUID DEFAULT gen_random_uuid();
```

**Option B**: Create mapping views
```sql
CREATE VIEW tb_post AS
SELECT
    post_id AS pk_post,
    uuid AS id,
    user_id AS fk_user,
    -- other columns
FROM legacy_posts;

-- Then create TVIEW on top of the view
CREATE TVIEW tv_post AS SELECT ... FROM tb_post;
```

#### 2. JSONB Data Column Required

TVIEWs must have a `data` column of type `JSONB`. This is non-negotiable.

If you need non-JSONB columns, you can:
1. Store them alongside the `data` column
2. Use a traditional materialized view instead

### Performance Limitations

#### 1. Very Deep Cascade Chains

Cascade depth >5 levels may cause performance issues.

**Example problematic hierarchy**:
```
company → division → department → team → user → post → comment
```

**Solution**: Flatten intermediate levels or batch updates.

#### 2. Massive Bulk Operations

Updates affecting >100,000 rows in one transaction may cause memory pressure.

**Solution**: Use statement-level triggers:
```sql
SELECT pg_tviews_install_stmt_triggers();
```

### Known Issues

Track known issues at: [GitHub Issues](https://github.com/your-org/pg_tviews/issues)

#### Issue #1: Example Issue Title
- **Description**: Brief description
- **Workaround**: Temporary solution
- **Status**: Planned for v1.1.0
```

**Deliverables**:
- Complete DDL syntax reference
- DROP TVIEW documentation
- Comprehensive limitations guide
- 10+ worked examples

**Acceptance Criteria**:
- [ ] Every DDL command documented
- [ ] All limitations documented with workarounds
- [ ] 10+ real-world examples
- [ ] Error messages explained
- [ ] No hidden "gotchas"

---

### B3: SQL Monitoring Reference (8 hours)

**Objective**: Complete documentation for all monitoring views and functions.

**Scope**:
- 4 monitoring views
- 3 health check functions
- 2 statement-level trigger functions

**Tasks**:

1. **Document Monitoring Views** (4 hours):

For each view, document:
- Purpose and use case
- Column descriptions
- Example queries
- Performance impact
- Typical values vs. warning signs

Example:
```markdown
### pg_tviews_queue_realtime

**Purpose**: Real-time view of the current transaction's refresh queue.

**Columns**:
| Column | Type | Description |
|--------|------|-------------|
| entity | TEXT | Entity name (e.g., "post", "user") |
| pk | BIGINT | Primary key value queued for refresh |
| depth | INTEGER | Cascade depth (0 = direct update) |
| queued_at | TIMESTAMPTZ | When this item was queued |

**Example Queries**:

```sql
-- View current queue
SELECT * FROM pg_tviews_queue_realtime;

-- Count by entity
SELECT entity, COUNT(*)
FROM pg_tviews_queue_realtime
GROUP BY entity;

-- Find deep cascades
SELECT * FROM pg_tviews_queue_realtime
WHERE depth > 3;
```

**Normal Values**:
- Queue size: 0-50 items typical
- Depth: 0-3 levels typical
- Age: Should clear within seconds

**Warning Signs**:
- 🟡 Queue size >100: May indicate bulk operation
- 🟠 Queue size >1000: Performance degradation likely
- 🔴 Queue age >5 seconds: Possible deadlock or error
- 🔴 Depth >5: Cascade chain too deep

**Performance Impact**: Negligible (reads thread-local state)
```

2. **Document Health Check Functions** (2 hours):

```markdown
### pg_tviews_health_check()

**Purpose**: Comprehensive system health validation.

**Returns**: TABLE with health check results

**Columns**:
| Column | Type | Description |
|--------|------|-------------|
| check_name | TEXT | Name of health check |
| status | TEXT | "OK", "WARNING", "ERROR" |
| message | TEXT | Details or error message |
| details | JSONB | Additional diagnostic info |

**Checks Performed**:
1. Extension loaded correctly
2. Metadata tables exist
3. No orphaned triggers
4. No corrupted metadata
5. Cache health
6. Queue not stuck
7. 2PC recovery needed
8. Statement triggers installed

**Example**:
```sql
SELECT * FROM pg_tviews_health_check();
```

Returns:
```text
 check_name          | status  | message                    | details
--------------------+---------+----------------------------+----------
 extension_loaded   | OK      | Extension version 0.1.0    | {...}
 metadata_tables    | OK      | All tables exist           | {...}
 orphaned_triggers  | OK      | No orphaned triggers       | {...}
 cache_health       | WARNING | Graph cache hit rate 65%   | {...}
```

**Interpreting Results**:

| Status | Meaning | Action |
|--------|---------|--------|
| OK | All checks passed | No action needed |
| WARNING | Non-critical issue | Monitor, investigate if persistent |
| ERROR | Critical issue | Immediate action required |

**Common Warnings**:
- Low cache hit rate: Normal after restart, investigate if <80% after 1 hour
- Old 2PC transactions: Run recovery function
- Queue backlog: Check for long-running transactions

**Common Errors**:
- Missing metadata table: Reinstall extension
- Corrupted metadata: Contact support, check logs
- Stuck queue: Rollback transaction, investigate deadlock
```

3. **Create Monitoring Guide** (2 hours):

```markdown
## Production Monitoring Guide

### Essential Metrics

Monitor these metrics in production:

#### 1. Queue Size (Critical)
```sql
SELECT COUNT(*) FROM pg_tviews_queue_realtime;
```
- **Target**: <50
- **Warning**: >100
- **Critical**: >1000

#### 2. Cache Hit Rate (Important)
```sql
SELECT
    (cache_hits::float / NULLIF(cache_hits + cache_misses, 0) * 100)::numeric(5,2) AS hit_rate
FROM pg_tviews_cache_stats;
```
- **Target**: >90%
- **Warning**: <80%
- **Critical**: <60%

#### 3. Refresh Latency (Important)
```sql
SELECT
    AVG(refresh_duration_ms) AS avg_ms,
    MAX(refresh_duration_ms) AS max_ms
FROM pg_tviews_performance_summary
WHERE hour > now() - interval '1 hour';
```
- **Target**: <5ms
- **Warning**: >50ms
- **Critical**: >500ms

### Alerting Thresholds

Configure alerts for:

```yaml
# Prometheus AlertManager example
alerts:
  - alert: TViewQueueBacklog
    expr: pg_tviews_queue_size > 100
    for: 5m
    severity: warning

  - alert: TViewQueueCritical
    expr: pg_tviews_queue_size > 1000
    for: 1m
    severity: critical

  - alert: TViewCachePoorPerformance
    expr: pg_tviews_cache_hit_rate < 60
    for: 10m
    severity: warning

  - alert: TViewRefreshSlow
    expr: pg_tviews_avg_refresh_ms > 100
    for: 5m
    severity: warning
```

### Grafana Dashboard

Example dashboard queries:

```sql
-- Queue size over time
SELECT
    hour,
    AVG(queue_size) AS avg_queue,
    MAX(queue_size) AS max_queue
FROM pg_tviews_performance_summary
WHERE hour > now() - interval '24 hours'
GROUP BY hour
ORDER BY hour;

-- Refresh operations per minute
SELECT
    date_trunc('minute', recorded_at) AS minute,
    COUNT(*) AS operations
FROM pg_tviews_metrics
WHERE recorded_at > now() - interval '1 hour'
GROUP BY minute
ORDER BY minute;

-- Cache hit rate trend
SELECT
    hour,
    (SUM(cache_hits)::float / NULLIF(SUM(cache_hits + cache_misses), 0) * 100) AS hit_rate_pct
FROM pg_tviews_performance_summary
WHERE hour > now() - interval '24 hours'
GROUP BY hour
ORDER BY hour;
```

### Troubleshooting Runbook

#### High Queue Size

**Symptoms**: pg_tviews_queue_size >100

**Diagnosis**:
```sql
-- Check what's in queue
SELECT entity, COUNT(*)
FROM pg_tviews_queue_realtime
GROUP BY entity
ORDER BY count DESC;

-- Check for long-running transactions
SELECT pid, now() - xact_start AS duration, query
FROM pg_stat_activity
WHERE state = 'active'
AND xact_start < now() - interval '1 minute';
```

**Solutions**:
1. If bulk operation: Normal, will clear on commit
2. If stuck transaction: Investigate query, consider canceling
3. If repeated: Enable statement-level triggers

#### Low Cache Hit Rate

**Symptoms**: Cache hit rate <80%

**Diagnosis**:
```sql
-- Check cache stats
SELECT * FROM pg_tviews_cache_stats;
```

**Solutions**:
1. After restart: Normal, wait 10-15 minutes for warmup
2. Schema changes: Normal, cache will rebuild
3. Persistent low rate: Check for schema volatility or bugs
```

**Deliverables**:
- Complete monitoring views documentation
- All functions documented
- Production monitoring guide
- Grafana/Prometheus examples
- Troubleshooting runbook

**Acceptance Criteria**:
- [ ] All 4 views documented
- [ ] All 3 functions documented
- [ ] Monitoring guide complete
- [ ] Alert thresholds defined
- [ ] Troubleshooting runbook complete

---

### B4: Error Reference (6 hours)

**Objective**: Document all error types with causes and solutions.

**Tasks**:

1. **Catalog All Errors** (2 hours):
   - Extract all error types from `src/error/mod.rs`
   - Test each error condition
   - Capture actual error messages
   - Document error codes (SQLSTATE)

2. **Create Error Reference** (3 hours):

```markdown
# Error Reference

Complete guide to pg_tviews errors, causes, and solutions.

## Error Code Format

pg_tviews uses PostgreSQL's SQLSTATE system:
- `TV000` - `TV099`: General errors
- `TV100` - `TV199`: Schema/DDL errors
- `TV200` - `TV299`: Refresh errors
- `TV300` - `TV399`: Dependency errors
- `TV400` - `TV499`: Configuration errors
- `TV500` - `TV599`: Internal errors

## Error Reference

### TV001: MetadataNotFound

**Error Message**:
```
ERROR: TVIEW metadata not found for entity 'posts'
SQLSTATE: TV001
```

**Cause**:
TVIEW has not been created or metadata was corrupted.

**Common Scenarios**:
1. Attempting to drop non-existent TVIEW
2. Metadata table was manually modified
3. Extension was reinstalled without recreating TVIEWs

**Solution**:

Check if TVIEW exists:
```sql
SELECT * FROM pg_tview_meta WHERE entity = 'posts';
```

If missing, recreate:
```sql
CREATE TVIEW tv_posts AS SELECT ...;
```

If metadata corrupted:
```sql
-- WARNING: This will drop all TVIEWs
DROP EXTENSION pg_tviews CASCADE;
CREATE EXTENSION pg_tviews;
-- Recreate all TVIEWs
```

**Prevention**:
- Never manually modify `pg_tview_meta`
- Use DDL commands only (CREATE/DROP TVIEW)

---

### TV102: InvalidSelectStatement

**Error Message**:
```
ERROR: Invalid SELECT statement for TVIEW: UNION not supported
SQLSTATE: TV102
```

**Cause**:
SELECT statement uses unsupported SQL features.

**Common Scenarios**:
1. Using UNION/INTERSECT/EXCEPT
2. Using recursive CTEs
3. Missing required columns (pk_, id, data)

**Solution**:

Check supported features:
```sql
SELECT pg_tviews_analyze_select('your SELECT statement');
```

Common fixes:
```sql
-- ❌ UNION not supported
CREATE TVIEW tv_content AS
SELECT ... FROM tb_post
UNION ALL
SELECT ... FROM tb_page;

-- ✅ Use separate TVIEWs instead
CREATE TVIEW tv_post AS SELECT ... FROM tb_post;
CREATE TVIEW tv_page AS SELECT ... FROM tb_page;

-- ❌ Missing pk_ column
CREATE TVIEW tv_post AS
SELECT id, data FROM tb_post;

-- ✅ Add required pk_ column
CREATE TVIEW tv_post AS
SELECT pk_post, id, data FROM tb_post;
```

**Prevention**:
- Review DDL reference before creating TVIEWs
- Use pg_tviews_analyze_select() to validate SQL
- Follow trinity pattern strictly

---

### TV301: DependencyCycle

**Error Message**:
```
ERROR: Dependency cycle detected: tv_user -> tv_post -> tv_user
SQLSTATE: TV301
```

**Cause**:
Circular dependency in TVIEW definitions.

**Common Scenarios**:
1. Bidirectional relationships in read models
2. Self-referencing TVIEWs
3. Complex multi-level dependencies

**Solution**:

Identify the cycle:
```sql
-- Check dependencies
SELECT entity, depends_on
FROM pg_tview_dependencies
WHERE entity IN ('user', 'post');
```

Break the cycle:
```sql
-- ❌ Creates cycle
CREATE TVIEW tv_user AS
SELECT
    pk_user, id,
    jsonb_build_object(
        'posts', (SELECT data FROM tv_post WHERE fk_user = pk_user)
    ) AS data
FROM tb_user;

CREATE TVIEW tv_post AS
SELECT
    pk_post, id,
    jsonb_build_object(
        'author', (SELECT data FROM tv_user WHERE pk_user = fk_user)
    ) AS data
FROM tb_post;

-- ✅ Use base tables instead
CREATE TVIEW tv_post AS
SELECT
    pk_post, id, fk_user,
    jsonb_build_object(
        'author', (
            SELECT jsonb_build_object('id', id, 'name', name)
            FROM tb_user WHERE pk_user = fk_user
        )
    ) AS data
FROM tb_post;
```

**Prevention**:
- Query base tables (tb_*), not other TVIEWs
- Keep dependency graph acyclic
- Draw dependency diagram before creating TVIEWs

---

[... document all 14 error types similarly ...]
```

3. **Create Quick Reference Table** (1 hour):

```markdown
## Error Quick Reference

| Code | Error | Severity | Common Cause | Quick Fix |
|------|-------|----------|--------------|-----------|
| TV001 | MetadataNotFound | Error | TVIEW not created | CREATE TVIEW |
| TV102 | InvalidSelectStatement | Error | Unsupported SQL | Review DDL docs |
| TV103 | MissingRequiredColumn | Error | No pk_/id/data | Add required columns |
| TV201 | RefreshFailed | Error | Query error | Check source tables |
| TV202 | CascadeFailed | Error | Dependency issue | Check dependencies |
| TV301 | DependencyCycle | Error | Circular deps | Break cycle |
| TV401 | ConfigError | Warning | Bad configuration | Check settings |
| TV501 | InternalError | Critical | Extension bug | Report issue |
```

**Deliverables**:
- Complete error reference document
- Quick reference table
- Troubleshooting flowchart
- Prevention checklist

**Acceptance Criteria**:
- [ ] All 14 error types documented
- [ ] Each error has cause/solution/prevention
- [ ] Quick reference table complete
- [ ] Real error messages captured
- [ ] Code examples for each scenario

---

### B5: Configuration Reference (4 hours)

**Objective**: Document all configuration options and tuning parameters.

**Investigation** (1 hour):
- Review `src/config/mod.rs` for configuration options
- Check for GUC (Grand Unified Configuration) parameters
- Test each configuration option

**Documentation** (3 hours):

```markdown
# Configuration Reference

## Configuration Options

pg_tviews currently has minimal configuration requirements. Most behavior is automatic.

### Extension Settings

#### postgresql.conf Settings

```ini
# Required: Enable shared library preloading
shared_preload_libraries = 'pg_tviews'  # Not required for pg_tviews

# Optional: Statement-level trigger threshold
# pg_tviews.stmt_trigger_threshold = 10  # Enable stmt triggers for bulk ops >10 rows

# Optional: Cache sizes
# pg_tviews.graph_cache_size = 100  # Number of dependency graphs to cache
# pg_tviews.table_cache_size = 1000  # Number of table OID mappings to cache

# Optional: Performance limits
# pg_tviews.max_cascade_depth = 10  # Prevent infinite recursion
# pg_tviews.max_queue_size = 10000  # Prevent memory exhaustion
```

### Runtime Configuration

#### Statement-Level Triggers

Enable for bulk operations:
```sql
SELECT pg_tviews_install_stmt_triggers();
```

Disable if causing issues:
```sql
SELECT pg_tviews_uninstall_stmt_triggers();
```

**When to use**:
- Bulk inserts/updates (>10 rows)
- Batch processing jobs
- Data migrations
- ETL pipelines

**When to avoid**:
- Single-row operations
- High-frequency small updates
- Interactive applications

### Monitoring Configuration

#### Metrics Retention

```sql
-- Default: Keep metrics for 7 days
SELECT pg_tviews_cleanup_metrics(7);

-- Longer retention for analysis
SELECT pg_tviews_cleanup_metrics(30);

-- Shorter retention to save space
SELECT pg_tviews_cleanup_metrics(1);
```

#### 2PC Queue Cleanup

```sql
-- Clean expired prepared transactions (default: 1 hour old)
SELECT pg_tviews_cleanup_expired_queues();
```

### Performance Tuning

#### Connection Pooler Settings

**PgBouncer**:
```ini
# pgbouncer.ini
[databases]
mydb = host=localhost dbname=mydb

[pgbouncer]
pool_mode = transaction
server_reset_query = DISCARD ALL  # Required for pg_tviews
```

**pgpool-II**:
```ini
# pgpool.conf
reset_query_list = 'DISCARD ALL'  # Required for pg_tviews
```

## Environment Variables

pg_tviews does not use environment variables.

## Best Practices

### Small Deployments (<10K rows)
```ini
# Minimal configuration
shared_preload_libraries = ''  # pg_tviews works without preload
# Use default settings
```

### Medium Deployments (10K-1M rows)
```ini
# Enable caching optimizations
pg_tviews.graph_cache_size = 200
pg_tviews.table_cache_size = 2000

# Install statement-level triggers
```

### Large Deployments (>1M rows)
```ini
# Maximize cache sizes
pg_tviews.graph_cache_size = 500
pg_tviews.table_cache_size = 5000

# Strict limits to prevent runaway queries
pg_tviews.max_cascade_depth = 5
pg_tviews.max_queue_size = 5000

# Enable statement-level triggers
# Monitor metrics closely
```

## Troubleshooting Configuration

### Check Current Settings

```sql
-- Check if extension is loaded
SELECT * FROM pg_extension WHERE extname = 'pg_tviews';

-- Check version
SELECT pg_tviews_version();

-- Check optional dependencies
SELECT pg_tviews_check_jsonb_ivm();

-- Check statement-level triggers status
SELECT COUNT(*) AS stmt_triggers
FROM pg_trigger
WHERE tgname LIKE '%_stmt_trig';
```

### Common Configuration Issues

#### Issue: Extension not loading

**Symptom**:
```
ERROR: could not load library "pg_tviews"
```

**Solution**:
1. Check installation:
   ```bash
   cargo pgrx install --release
   ```

2. Verify library exists:
   ```bash
   ls /usr/lib/postgresql/*/lib/pg_tviews.so
   ```

3. Check PostgreSQL version compatibility

#### Issue: Poor performance

**Symptom**: Slow refreshes, high CPU usage

**Diagnosis**:
```sql
SELECT * FROM pg_tviews_performance_summary
WHERE hour > now() - interval '1 hour';
```

**Solutions**:
1. Enable statement-level triggers
2. Increase cache sizes
3. Review cascade depth
4. Check for missing indexes on source tables
```

**Deliverables**:
- Configuration reference document
- postgresql.conf examples
- Best practices by deployment size
- Troubleshooting guide

**Acceptance Criteria**:
- [ ] All configuration options documented
- [ ] Default values specified
- [ ] Performance impact explained
- [ ] Examples for each deployment size
- [ ] Troubleshooting guide complete

---

### B6: Create Missing Reference Docs (6 hours)

**Objective**: Fill any remaining reference documentation gaps.

**Tasks**:

1. **Security Reference** (2 hours):

```markdown
# Security Reference

## Permission Requirements

### Minimum Required Permissions

To create TVIEWs, user needs:
```sql
-- Permission to create objects
GRANT CREATE ON SCHEMA public TO app_user;

-- Permission on source tables
GRANT SELECT, INSERT, UPDATE, DELETE ON tb_user TO app_user;
GRANT SELECT, INSERT, UPDATE, DELETE ON tb_post TO app_user;

-- Permission to create functions (for triggers)
GRANT CREATE ON SCHEMA public TO app_user;
```

### Recommended Role Setup

```sql
-- Create TVIEW admin role
CREATE ROLE tview_admin;
GRANT CREATE ON SCHEMA public TO tview_admin;
GRANT SELECT ON ALL TABLES IN SCHEMA public TO tview_admin;

-- Create TVIEW user role (read-only)
CREATE ROLE tview_user;
GRANT SELECT ON ALL TABLES IN SCHEMA public TO tview_user;

-- Grant to application user
GRANT tview_admin TO app_admin;
GRANT tview_user TO app_readonly;
```

## Security Best Practices

### 1. Least Privilege

Grant only necessary permissions:
```sql
-- ❌ Too permissive
GRANT ALL PRIVILEGES ON DATABASE mydb TO app_user;

-- ✅ Minimal necessary
GRANT USAGE ON SCHEMA public TO app_user;
GRANT SELECT ON tv_posts, tv_users TO app_user;
```

### 2. Separate Admin from Users

```sql
-- Admin can create/modify TVIEWs
CREATE ROLE tview_admin;

-- Applications only read from TVIEWs
CREATE ROLE tview_reader;
GRANT SELECT ON ALL TABLES IN SCHEMA public TO tview_reader;
```

### 3. Row-Level Security

TVIEWs respect RLS policies:
```sql
-- Enable RLS on source table
ALTER TABLE tb_post ENABLE ROW LEVEL SECURITY;

-- Create policy
CREATE POLICY post_isolation ON tb_post
    USING (tenant_id = current_setting('app.tenant_id')::uuid);

-- TVIEW automatically enforces RLS when refreshing
CREATE TVIEW tv_post AS SELECT ... FROM tb_post;
```

## SQL Injection Protection

### Safe Practices

✅ **Always use parameterized queries**:
```javascript
// Node.js example
const result = await client.query(
    'SELECT data FROM tv_post WHERE id = $1',
    [postId]
);
```

❌ **Never interpolate user input**:
```javascript
// VULNERABLE
const result = await client.query(
    `SELECT data FROM tv_post WHERE id = '${postId}'`
);
```

### Dynamic TVIEW Creation

If you must create TVIEWs dynamically, validate inputs:

```sql
CREATE OR REPLACE FUNCTION create_tview_safely(
    entity_name TEXT,
    base_table TEXT
) RETURNS VOID AS $$
BEGIN
    -- Validate entity name (alphanumeric + underscore only)
    IF entity_name !~ '^[a-z_][a-z0-9_]*$' THEN
        RAISE EXCEPTION 'Invalid entity name';
    END IF;

    -- Validate table exists
    IF NOT EXISTS (
        SELECT 1 FROM information_schema.tables
        WHERE table_name = base_table
    ) THEN
        RAISE EXCEPTION 'Table does not exist';
    END IF;

    -- Safe to proceed
    EXECUTE format(
        'CREATE TVIEW tv_%I AS SELECT * FROM %I',
        entity_name, base_table
    );
END;
$$ LANGUAGE plpgsql SECURITY DEFINER;
```

## Audit Logging

### Track TVIEW Operations

```sql
-- Create audit log table
CREATE TABLE tview_audit_log (
    id BIGSERIAL PRIMARY KEY,
    operation TEXT NOT NULL,  -- 'CREATE', 'DROP', 'REFRESH'
    entity TEXT NOT NULL,
    performed_by TEXT NOT NULL DEFAULT current_user,
    performed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    details JSONB
);

-- Create audit trigger
CREATE OR REPLACE FUNCTION log_tview_operation()
RETURNS event_trigger AS $$
DECLARE
    obj record;
BEGIN
    FOR obj IN SELECT * FROM pg_event_trigger_ddl_commands()
    LOOP
        IF obj.command_tag LIKE '%TVIEW%' THEN
            INSERT INTO tview_audit_log (operation, entity, details)
            VALUES (
                obj.command_tag,
                obj.object_identity,
                jsonb_build_object(
                    'schema', obj.schema_name,
                    'type', obj.object_type
                )
            );
        END IF;
    END LOOP;
END;
$$ LANGUAGE plpgsql;

CREATE EVENT TRIGGER tview_audit
    ON ddl_command_end
    EXECUTE FUNCTION log_tview_operation();
```

## Common Security Mistakes

### 1. Trusting JSONB Data

❌ **Dangerous**:
```javascript
// Directly interpolating JSONB into HTML
const html = `<div>${post.data.title}</div>`;
```

✅ **Safe**:
```javascript
// Escape untrusted data
const html = `<div>${escapeHtml(post.data.title)}</div>`;
```

### 2. Exposing Sensitive Data

❌ **Over-sharing**:
```sql
CREATE TVIEW tv_user AS
SELECT
    pk_user, id,
    jsonb_build_object(
        'id', id,
        'email', email,  -- Sensitive!
        'password_hash', password_hash  -- NEVER!
    ) AS data
FROM tb_user;
```

✅ **Minimal exposure**:
```sql
CREATE TVIEW tv_user AS
SELECT
    pk_user, id,
    jsonb_build_object(
        'id', id,
        'name', name,
        'avatar_url', avatar_url
    ) AS data
FROM tb_user;

-- Create separate TVIEW for admin access
CREATE TVIEW tv_user_admin AS
SELECT
    pk_user, id,
    jsonb_build_object(
        'id', id,
        'email', email,  -- OK in admin view
        'created_at', created_at
    ) AS data
FROM tb_user;

-- Restrict access
GRANT SELECT ON tv_user TO public;
GRANT SELECT ON tv_user_admin TO admin_role;
```

### 3. Missing Encryption at Rest

If your data is sensitive:
```sql
-- Enable transparent data encryption (TDE)
-- Or use pgcrypto for column-level encryption

CREATE EXTENSION pgcrypto;

CREATE TABLE tb_user (
    pk_user BIGSERIAL PRIMARY KEY,
    id UUID NOT NULL,
    email TEXT,
    ssn_encrypted BYTEA  -- Encrypted with pgcrypto
);

-- TVIEW can expose decrypted view (with proper permissions)
CREATE TVIEW tv_user_admin AS
SELECT
    pk_user, id,
    jsonb_build_object(
        'id', id,
        'email', email,
        'ssn', pgp_sym_decrypt(ssn_encrypted, current_setting('app.encryption_key'))
    ) AS data
FROM tb_user;
```

## Compliance Considerations

### GDPR "Right to be Forgotten"

When deleting user data:
```sql
-- Deletion cascades to TVIEW automatically
DELETE FROM tb_user WHERE id = 'user-uuid';

-- TVIEW tv_user automatically updated
-- No manual intervention needed
```

### Data Retention Policies

```sql
-- Clean old metrics data
SELECT pg_tviews_cleanup_metrics(7);  -- Keep 7 days

-- Or automate with cron
CREATE EXTENSION pg_cron;

SELECT cron.schedule(
    'cleanup-tview-metrics',
    '0 2 * * *',  -- Daily at 2 AM
    $$SELECT pg_tviews_cleanup_metrics(7)$$
);
```

## Vulnerability Reporting

If you discover a security vulnerability:
1. **Do NOT** open a public GitHub issue
2. Email: security@your-domain.com
3. Include: version, description, steps to reproduce
4. Allow 90 days for patch before disclosure
```

2. **PostgreSQL Version Compatibility** (2 hours):

```markdown
# PostgreSQL Version Compatibility

## Supported Versions

| PostgreSQL Version | Support Status | Notes |
|-------------------|----------------|-------|
| 17 | ✅ Fully Supported | Recommended |
| 16 | ✅ Fully Supported | Tested in CI |
| 15 | ✅ Fully Supported | Minimum version |
| 14 | ❌ Not Supported | Missing required features |
| 13 and earlier | ❌ Not Supported | Too old |

## Version-Specific Features

### PostgreSQL 17
All features available.

### PostgreSQL 16
All features available.
Minor performance difference in query planning (<5%).

### PostgreSQL 15
All features available.
Marginally slower statement-level triggers (~10%).

## Testing Matrix

```bash
# Run tests against all supported versions
cargo pgrx test pg15
cargo pgrx test pg16
cargo pgrx test pg17
```

## Upgrading PostgreSQL

### From PG 15/16 to 17

pg_tviews is compatible across versions. Follow standard PostgreSQL upgrade process:

```bash
# 1. Dump database
pg_dumpall > backup.sql

# 2. Install PostgreSQL 17
# (distribution-specific)

# 3. Rebuild extension for PG 17
cargo pgrx install --release --pg17

# 4. Restore database
psql -f backup.sql

# 5. Recreate extension
psql -c "CREATE EXTENSION pg_tviews;"

# 6. Verify
psql -c "SELECT pg_tviews_version();"
```

### Zero-Downtime Upgrade

Use logical replication:

1. Set up PG 17 replica
2. Rebuild pg_tviews on PG 17
3. Let replication catch up
4. Failover to PG 17

## Known Issues by Version

### PostgreSQL 15

**Issue**: Slower transition table access in statement-level triggers
**Impact**: ~10% slower bulk operations
**Workaround**: None, upgrade to PG 16/17 for best performance

### PostgreSQL 16

No known issues.

### PostgreSQL 17

No known issues.

## Extension Compatibility

### Compatible Extensions

✅ **Works well with**:
- jsonb_ivm (optional, recommended)
- pg_stat_statements (for monitoring)
- pg_cron (for scheduled maintenance)
- PostGIS (no conflicts)
- TimescaleDB (no conflicts)

⚠️ **Partial compatibility**:
- Citus (distributed PostgreSQL): Not tested, may have issues with 2PC

❌ **Incompatible**:
- None known

## Future PostgreSQL Versions

pg_tviews aims to support new PostgreSQL versions within 30 days of stable release.

Track compatibility at: [GitHub Compatibility Matrix](link)
```

3. **Glossary** (2 hours):

```markdown
# Glossary

## Core Concepts

### TVIEW
**Transactional View**: A materialized view that automatically refreshes incrementally when source data changes, maintaining transactional consistency.

**Example**: `tv_posts` is a TVIEW backed by `tb_posts`.

### Trinity Pattern
**Identity scheme** using three types of identifiers:
- `pk_entity` (BIGINT): Internal primary key for joins
- `id` (UUID): Public identifier for APIs
- `fk_parent` (BIGINT): Foreign key for cascades

**Rationale**: Combines UUID ergonomics with integer performance.

### Incremental Refresh
**Update strategy** that modifies only changed rows instead of rebuilding entire materialized views.

**Comparison**:
- Traditional: `REFRESH MATERIALIZED VIEW` (full rebuild)
- pg_tviews: Surgical row-level updates

### Cascade Propagation
**Automatic update** of dependent TVIEWs when parent data changes.

**Example**:
```
tb_user changes → tv_user updates → tv_post updates (has user data)
```

## Technical Terms

### Lineage Tracking
**Dependency tracing** using integer foreign keys to determine which TVIEWs need updates.

### JSONB Read Model
**Denormalized data structure** stored as JSONB containing complete entity representation for fast queries.

### Statement-Level Trigger
**Bulk-optimized trigger** that fires once per SQL statement instead of once per row.

**Performance**: 100-500× faster for bulk operations.

### Two-Phase Commit (2PC)
**Distributed transaction protocol** allowing TVIEW refreshes to participate in cross-database transactions.

### Queue Deduplication
**Optimization** that removes redundant refresh operations for the same entity within a transaction.

### Dependency Graph
**Directed acyclic graph (DAG)** representing TVIEW dependencies, used for topological sort during cascade.

## Database Schema

### tb_* Tables
**Source tables** containing normalized write models.
Convention: `tb_user`, `tb_post`, `tb_comment`

### tv_* Tables
**TVIEW tables** containing materialized read models.
Convention: `tv_user`, `tv_post`, `tv_comment`

### v_* Views
**PostgreSQL views** that can serve as source for TVIEWs (optional pattern).

### pk_* Columns
**Primary key columns** following trinity pattern.
Example: `pk_user`, `pk_post`

### fk_* Columns
**Foreign key columns** following trinity pattern.
Example: `fk_user`, `fk_category`

## Operations

### Surgical Update
**Precise modification** of specific JSONB paths instead of replacing entire object.

**Tool**: jsonb_ivm extension

### Cache Hit
**Successful retrieval** from in-memory cache (graph cache, table cache, or plan cache).

### Health Check
**Diagnostic function** that validates system integrity.

**Function**: `pg_tviews_health_check()`

## Performance

### Hit Rate
**Percentage of cache hits** vs. total cache accesses.

**Formula**: `hits / (hits + misses) × 100%`

### Cascade Depth
**Number of levels** in dependency chain.

**Example**: `user → post → comment` = depth 3

### Refresh Latency
**Time taken** to complete incremental refresh operation.

**Target**: <5ms for typical operations

## Monitoring

### Queue Size
**Number of pending refresh operations** in current transaction.

### Refresh Duration
**Elapsed time** for refresh operation in milliseconds.

### Cache Stats
**Metrics** about cache performance (hits, misses, evictions).

## Advanced

### ProcessUtility Hook
**PostgreSQL hook** that intercepts DDL commands like CREATE TVIEW.

### SPI (Server Programming Interface)
**PostgreSQL C API** for executing SQL from extensions.

### SQLSTATE
**Five-character error code** following SQL standard.

**pg_tviews codes**: TV000-TV599

### Thread-Local Storage
**Per-connection memory** for queue state and metrics.

### Optimistic Locking
**Concurrency control** using version numbers to detect conflicts without blocking.

## Acronyms

- **2PC**: Two-Phase Commit
- **CQRS**: Command Query Responsibility Segregation
- **DAG**: Directed Acyclic Graph
- **DDL**: Data Definition Language
- **FK**: Foreign Key
- **GUC**: Grand Unified Configuration
- **IMVM**: Incremental Materialized View Maintenance
- **JSONB**: JSON Binary (PostgreSQL data type)
- **MV**: Materialized View
- **OID**: Object Identifier
- **PK**: Primary Key
- **RLS**: Row-Level Security
- **SPI**: Server Programming Interface
- **UUID**: Universally Unique Identifier

## See Also

- [Trinity Pattern Documentation](trinity-pattern.md)
- [Architecture Deep Dive](architecture-deep-dive.md)
- [API Reference](api-reference.md)
```

**Deliverables**:
- Security reference document
- PostgreSQL version compatibility matrix
- Comprehensive glossary

**Acceptance Criteria**:
- [ ] Security best practices documented
- [ ] All PG versions tested and documented
- [ ] Glossary has 50+ terms
- [ ] All acronyms defined

---

## Phase C: Operational Excellence (24-32 hours)

### C1: Migration Guide from Traditional MVs (8 hours)

**Objective**: Help users migrate from `REFRESH MATERIALIZED VIEW` to pg_tviews.

**Target Audience**:
- DBAs with existing materialized views
- Developers considering pg_tviews
- Teams evaluating migration effort

**Content Structure**:

```markdown
# Migration Guide: Traditional MVs to pg_tviews

## Should You Migrate?

### When to Migrate

✅ **Migrate if**:
- Full refresh takes >1 second
- Data must be "always fresh"
- You have complex JOINs (>2 tables)
- Updates are frequent (>10/minute)
- JSONB is your target format

❌ **Don't migrate if**:
- Refresh <100ms and infrequent
- Append-only data (logs, events)
- Simple aggregations without JOINs
- Can tolerate stale data

### Migration Effort Estimation

| Current Setup | Migration Effort | Expected Benefit |
|--------------|-----------------|------------------|
| 1-5 MVs, simple schema | 4-8 hours | 100-1000× speedup |
| 5-20 MVs, normalized schema | 16-40 hours | 1000-5000× speedup |
| 20+ MVs, complex dependencies | 40-80 hours | 5000-10000× speedup |

## Pre-Migration Checklist

- [ ] Audit all materialized views
- [ ] Document refresh schedules
- [ ] Measure current refresh times
- [ ] Identify dependencies between MVs
- [ ] Review schema compatibility with trinity pattern
- [ ] Test pg_tviews in development
- [ ] Plan rollback strategy

## Migration Process

### Phase 1: Assessment (1-2 hours per MV)

**1. Inventory Current MVs**:
```sql
-- List all materialized views
SELECT schemaname, matviewname, definition
FROM pg_matviews
ORDER BY schemaname, matviewname;
```

**2. Measure Current Performance**:
```sql
-- Benchmark current refresh time
\timing on
REFRESH MATERIALIZED VIEW my_view;
-- Note the time
```

**3. Analyze Dependencies**:
```sql
-- Check view dependencies
SELECT
    dependent_ns.nspname AS dependent_schema,
    dependent_view.relname AS dependent_view,
    source_ns.nspname AS source_schema,
    source_table.relname AS source_table
FROM pg_depend
JOIN pg_rewrite ON pg_depend.objid = pg_rewrite.oid
JOIN pg_class AS dependent_view ON pg_rewrite.ev_class = dependent_view.oid
JOIN pg_class AS source_table ON pg_depend.refobjid = source_table.oid
JOIN pg_namespace dependent_ns ON dependent_ns.oid = dependent_view.relnamespace
JOIN pg_namespace source_ns ON source_ns.oid = source_table.relnamespace
WHERE dependent_view.relname = 'my_view';
```

**4. Check Schema Compatibility**:
```sql
-- Review MV definition
\d+ my_view

-- Check if compatible with trinity pattern:
-- - Has integer PK?
-- - Has UUID id?
-- - Uses JSONB for nested data?
```

### Phase 2: Schema Adaptation (2-4 hours per table)

**Scenario A: Schema Already Compatible**

If your tables already have:
- Integer primary keys
- UUID columns
- Proper foreign keys

You can proceed directly to Phase 3.

**Scenario B: Add Trinity Columns**

```sql
-- Example: Migrating legacy schema

-- Before
CREATE TABLE posts (
    id SERIAL PRIMARY KEY,
    title TEXT,
    user_id INTEGER REFERENCES users(id)
);

-- Add trinity pattern
ALTER TABLE posts
    RENAME COLUMN id TO pk_post;  -- Rename PK

ALTER TABLE posts
    ADD COLUMN id UUID DEFAULT gen_random_uuid() UNIQUE,
    RENAME COLUMN user_id TO fk_user;

-- Create index on UUID for lookups
CREATE INDEX idx_posts_id ON posts(id);
```

**Scenario C: Create Mapping Views**

If you cannot modify existing tables:

```sql
-- Create view layer implementing trinity pattern
CREATE VIEW tb_post AS
SELECT
    post_id AS pk_post,
    post_uuid AS id,
    author_id AS fk_user,
    -- other columns
FROM legacy_posts;

-- Now create TVIEW on top
CREATE TVIEW tv_post AS
SELECT ... FROM tb_post;
```

### Phase 3: TVIEW Creation (1-2 hours per MV)

**Step 1: Convert MV Definition to TVIEW**

```sql
-- Original Materialized View
CREATE MATERIALIZED VIEW mv_user_posts AS
SELECT
    u.id AS user_id,
    u.name AS user_name,
    u.email AS user_email,
    p.id AS post_id,
    p.title AS post_title,
    p.content AS post_content,
    p.created_at AS post_created_at
FROM users u
JOIN posts p ON u.id = p.user_id;

-- Converted to TVIEW
CREATE TVIEW tv_user_posts AS
SELECT
    p.pk_post,                    -- Required: PK
    p.id,                         -- Required: UUID
    p.fk_user,                    -- Required: FK for cascades
    u.id AS user_id,              -- Optional: For filtering
    jsonb_build_object(           -- Required: JSONB data
        'userId', u.id,
        'userName', u.name,
        'userEmail', u.email,
        'postId', p.id,
        'postTitle', p.title,
        'postContent', p.content,
        'postCreatedAt', p.created_at
    ) AS data
FROM tb_post p
JOIN tb_user u ON p.fk_user = u.pk_user;
```

**Step 2: Verify TVIEW Structure**

```sql
-- Check TVIEW was created
SELECT * FROM pg_tview_meta WHERE entity = 'user_posts';

-- Check columns
\d tv_user_posts

-- Test query
SELECT * FROM tv_user_posts LIMIT 5;
```

**Step 3: Migrate Application Queries**

```sql
-- Old query (flat columns)
SELECT user_name, post_title
FROM mv_user_posts
WHERE user_id = 123;

-- New query (JSONB)
SELECT
    data->>'userName' AS user_name,
    data->>'postTitle' AS post_title
FROM tv_user_posts
WHERE user_id = '123-uuid-here';

-- Or return entire JSONB to application
SELECT data FROM tv_user_posts WHERE user_id = '123-uuid-here';
```

### Phase 4: Performance Validation (1 hour per MV)

**Benchmark pg_tviews Performance**:

```sql
-- Test single-row update
BEGIN;
\timing on
UPDATE tb_post SET title = 'Updated' WHERE pk_post = 1;
COMMIT;
-- Note TVIEW refresh time (included in transaction)

-- Test bulk update
BEGIN;
\timing on
UPDATE tb_post SET title = 'Bulk Update' WHERE pk_post <= 100;
COMMIT;
-- Note time for 100-row update
```

**Compare Results**:

| Operation | Old (REFRESH) | New (pg_tviews) | Improvement |
|-----------|---------------|-----------------|-------------|
| Single update | 2,500 ms | 0.5 ms | 5,000× |
| 100-row bulk | 2,500 ms | 50 ms | 50× |

### Phase 5: Cutover (Variable, plan carefully)

**Option A: Blue-Green Deployment**

1. Deploy pg_tviews alongside existing MVs
2. Dual-write to both systems
3. Verify data consistency
4. Switch read traffic to pg_tviews
5. Monitor for 24-48 hours
6. Remove old MVs

**Option B: Phased Migration**

1. Migrate least-critical MV first
2. Monitor for 1 week
3. Migrate next MV
4. Repeat until all migrated

**Option C: Big Bang (Not Recommended)**

1. Schedule maintenance window
2. Migrate all MVs at once
3. Hope nothing breaks 😬

## Application Code Changes

### Query Adjustments

**Before** (Traditional MV):
```javascript
// Node.js example
const result = await db.query(`
    SELECT user_name, post_title, post_content
    FROM mv_user_posts
    WHERE user_id = $1
`, [userId]);

const posts = result.rows.map(row => ({
    userName: row.user_name,
    postTitle: row.post_title,
    postContent: row.post_content
}));
```

**After** (pg_tviews):
```javascript
// Query returns JSONB directly
const result = await db.query(`
    SELECT data
    FROM tv_user_posts
    WHERE user_id = $1
`, [userId]);

// Data already structured
const posts = result.rows.map(row => row.data);
```

### Index Migration

```sql
-- Old indexes on MV
CREATE INDEX idx_mv_user_posts_user_id ON mv_user_posts(user_id);
CREATE INDEX idx_mv_user_posts_created ON mv_user_posts(post_created_at);

-- New indexes on TVIEW
CREATE INDEX idx_tv_user_posts_user_id ON tv_user_posts(user_id);
CREATE INDEX idx_tv_user_posts_created ON tv_user_posts USING gin((data->'postCreatedAt'));
```

### Refresh Schedule Changes

**Before**:
```sql
-- Cron job to refresh MV every 5 minutes
*/5 * * * * psql -c "REFRESH MATERIALIZED VIEW mv_user_posts"
```

**After**:
```sql
-- No refresh needed! TVIEWs update automatically.
-- Delete the cron job.
```

## Rollback Plan

If migration fails, you can rollback:

### Immediate Rollback (During Cutover)

```sql
-- Switch application back to old MV
-- DROP TVIEW (keeps source data intact)
DROP TVIEW tv_user_posts;

-- Original MV still exists, just refresh it
REFRESH MATERIALIZED VIEW mv_user_posts;
```

### Long-Term Rollback

```sql
-- Remove pg_tviews completely
DROP EXTENSION pg_tviews CASCADE;

-- Recreate all traditional MVs
CREATE MATERIALIZED VIEW ... AS SELECT ...;
```

## Common Migration Issues

### Issue: Schema Doesn't Match Trinity Pattern

**Problem**: Existing tables don't have pk_/fk_ naming.

**Solution**: Create view layer or rename columns.

### Issue: UNION Queries Not Supported

**Problem**: MV uses UNION to combine data.

**Solution**: Create separate TVIEWs, query both.

### Issue: Performance Regression

**Problem**: pg_tviews slower than expected.

**Solution**:
1. Enable statement-level triggers
2. Add JSONB indexes
3. Check cascade depth
4. Verify jsonb_ivm is installed

### Issue: Application Breaks

**Problem**: Queries assume flat columns, not JSONB.

**Solution**: Create compatibility view:
```sql
CREATE VIEW mv_user_posts AS
SELECT
    data->>'userId' AS user_id,
    data->>'userName' AS user_name,
    data->>'postTitle' AS post_title
FROM tv_user_posts;

-- Application can still query "mv_user_posts"
```

## Success Criteria

Migration is successful when:

- [ ] All materialized views converted to TVIEWs
- [ ] Application queries return correct data
- [ ] Performance metrics meet targets
- [ ] No data inconsistencies
- [ ] Monitoring shows healthy metrics
- [ ] Team trained on new system
- [ ] Documentation updated
- [ ] Old cron jobs removed

## Post-Migration Optimization

After migration, optimize:

1. **Add Indexes**:
   ```sql
   CREATE INDEX idx_tv_post_created
   ON tv_post USING gin((data->'createdAt'));
   ```

2. **Enable Statement Triggers**:
   ```sql
   SELECT pg_tviews_install_stmt_triggers();
   ```

3. **Set Up Monitoring**:
   ```sql
   SELECT * FROM pg_tviews_health_check();
   ```

4. **Benchmark New Performance**:
   Document improvement metrics.

## Case Studies

### Case Study 1: E-commerce Product Catalog

**Before**:
- 50 materialized views
- Refresh every 5 minutes via cron
- 45-second full refresh
- Stale data between refreshes

**Migration**:
- 2 weeks effort
- Converted to 50 TVIEWs
- Added jsonb_ivm

**After**:
- Always-fresh data
- <1ms refresh latency
- Removed 50 cron jobs
- 5,000× performance improvement

### Case Study 2: Analytics Dashboard

**Before**:
- 15 complex MVs with JOINs
- Overnight batch refresh (2 hours)
- Dashboard showed yesterday's data

**Migration**:
- 1 week effort
- Converted to 15 TVIEWs
- Trinity pattern already in use

**After**:
- Real-time dashboard
- 10ms refresh latency
- 12,000× performance improvement
- Batch jobs eliminated

## Getting Help

If you encounter migration issues:

1. Check [Troubleshooting Guide](troubleshooting.md)
2. Review [Error Reference](errors.md)
3. Ask in [GitHub Discussions](link)
4. File issue with "migration" label

## Next Steps

After successful migration:

- [ ] Read [Operations Guide](operations.md)
- [ ] Set up [Monitoring](monitoring.md)
- [ ] Review [Performance Tuning](performance-tuning.md)
- [ ] Train team on TVIEW maintenance
```

**Deliverables**:
- Complete migration guide document
- Step-by-step checklists
- Before/after code examples
- Rollback procedures
- 2+ real-world case studies

**Acceptance Criteria**:
- [ ] All migration scenarios covered
- [ ] Rollback plan documented
- [ ] Common issues with solutions
- [ ] Real case studies included
- [ ] Effort estimation guide

---

### C2: Disaster Recovery Guide (6 hours)

**Objective**: Document procedures for recovering from failures.

**Content**:

```markdown
# Disaster Recovery Guide

## Disaster Scenarios

### 1. Corrupted TVIEW Data

**Symptoms**:
- Query returns incorrect data
- Data inconsistent with source tables
- JSONB structure malformed

**Recovery**:

```sql
-- 1. Drop corrupted TVIEW
DROP TVIEW tv_posts;

-- 2. Recreate from original definition
CREATE TVIEW tv_posts AS
SELECT
    pk_post, id, fk_user,
    jsonb_build_object(...) AS data
FROM tb_post p
JOIN tb_user u ON p.fk_user = u.pk_user;

-- 3. Data automatically regenerates on next write
-- Or force regeneration:
UPDATE tb_post SET updated_at = now();
```

**Prevention**:
- Store TVIEW definitions in version control
- Regular backups of pg_tview_meta
- Monitor data consistency

---

### 2. Lost TVIEW Metadata

**Symptoms**:
```
ERROR: TVIEW metadata not found
```

**Recovery**:

```sql
-- 1. Check if metadata table exists
SELECT * FROM pg_tview_meta;

-- 2. If missing, reinstall extension
DROP EXTENSION pg_tviews CASCADE;
CREATE EXTENSION pg_tviews;

-- 3. Recreate all TVIEWs from backups
\i tview_definitions.sql

-- 4. Verify
SELECT entity FROM pg_tview_meta;
```

**Prevention**:
- Backup `pg_tview_meta` regularly
- Store DDL in version control
- Use pg_dump with --extension

---

### 3. Extension Won't Load

**Symptoms**:
```
ERROR: could not load library "pg_tviews"
```

**Recovery**:

```bash
# 1. Reinstall extension
cd pg_tviews
cargo pgrx install --release

# 2. Restart PostgreSQL
sudo systemctl restart postgresql

# 3. Verify installation
psql -c "SELECT pg_tviews_version();"
```

**Rollback** (if reinstall fails):
```sql
-- 1. Drop extension
DROP EXTENSION pg_tviews CASCADE;

-- 2. Use traditional MVs temporarily
CREATE MATERIALIZED VIEW mv_posts AS SELECT ...;

-- 3. Fix pg_tviews installation
-- 4. Re-migrate when ready
```

---

### 4. Infinite Cascade Loop

**Symptoms**:
- Transaction hangs
- High CPU usage
- Log shows:
  ```
  ERROR: Maximum cascade depth exceeded
  ```

**Recovery**:

```sql
-- 1. Cancel stuck transaction
SELECT pg_cancel_backend(pid)
FROM pg_stat_activity
WHERE state = 'active'
  AND query LIKE '%pg_tviews%';

-- 2. Identify cycle
-- Check TVIEW dependencies
SELECT * FROM pg_tview_dependencies;

-- 3. Break cycle
-- Drop one TVIEW in the cycle
DROP TVIEW tv_problematic;

-- 4. Fix TVIEW definition to avoid cycle
-- Then recreate
```

**Prevention**:
- Draw dependency graph before creating TVIEWs
- Set max_cascade_depth configuration
- Use acyclic design patterns

---

### 5. Database Crash During Refresh

**Symptoms**:
- PostgreSQL crashed
- Data may be inconsistent

**Recovery**:

```bash
# 1. PostgreSQL handles WAL recovery automatically
sudo systemctl start postgresql

# 2. Check pg_tviews health
psql -c "SELECT * FROM pg_tviews_health_check();"

# 3. Check for stuck 2PC transactions
psql -c "SELECT * FROM pg_prepared_xacts;"

# 4. Recover if needed
psql -c "SELECT pg_tviews_recover_prepared_transactions();"
```

**Data Consistency**:
TVIEWs maintain ACID compliance. If transaction aborted, TVIEWs automatically rollback to consistent state.

**No manual intervention needed** unless health check shows errors.

---

### 6. Out of Disk Space

**Symptoms**:
```
ERROR: could not extend file: No space left on device
```

**Immediate Recovery**:

```bash
# 1. Free up space
df -h  # Check usage
du -sh /var/lib/postgresql/  # Find large directories

# 2. Clean up if safe
SELECT pg_tviews_cleanup_metrics(1);  # Keep only 1 day
VACUUM FULL pg_tviews_metrics;  # Reclaim space

# 3. Drop non-critical TVIEWs temporarily
DROP TVIEW tv_optional;
```

**Long-Term Fix**:
- Increase disk size
- Set up metrics retention policy
- Monitor disk usage

---

### 7. Accidental DROP TVIEW

**Symptoms**:
- TVIEW gone, applications failing

**Recovery**:

```bash
# 1. Check version control for definition
git log -- tview_definitions.sql

# 2. Recreate from backup
psql -f tview_definitions.sql

# 3. Or recreate manually
psql <<EOF
CREATE TVIEW tv_posts AS SELECT ...;
EOF

# 4. Verify
psql -c "SELECT * FROM tv_posts LIMIT 5;"
```

**Prevention**:
- Require `IF EXISTS` in DROP scripts
- Enable query logging
- Use read-only roles for applications

---

## Backup Strategies

### Full Backup (Recommended)

```bash
# Backup entire database including pg_tviews
pg_dump --format=custom --file=backup.dump mydb

# Restore
pg_restore --dbname=mydb_restored backup.dump
```

### TVIEW-Only Backup

```bash
# Export TVIEW definitions
pg_dump --schema-only --table='tv_*' mydb > tviews.sql

# Export metadata
pg_dump --data-only --table=pg_tview_meta mydb > tview_metadata.sql

# Restore
psql mydb < tviews.sql
psql mydb < tview_metadata.sql
```

### Version Control Backup

```bash
# Store all TVIEW definitions
cat > tview_definitions.sql <<'EOF'
-- Auto-generated TVIEW definitions
-- Last updated: 2025-12-11

CREATE TVIEW tv_user AS SELECT ...;
CREATE TVIEW tv_post AS SELECT ...;
CREATE TVIEW tv_comment AS SELECT ...;
EOF

# Commit to git
git add tview_definitions.sql
git commit -m "Update TVIEW definitions"
```

### Automated Backup Schedule

```bash
# Cron job: Daily TVIEW backup
0 2 * * * pg_dump --table='tv_*' --table=pg_tview_meta mydb | gzip > /backups/tviews_$(date +\%Y\%m\%d).sql.gz

# Retention: Keep 30 days
find /backups -name "tviews_*.sql.gz" -mtime +30 -delete
```

---

## Recovery Testing

### Quarterly DR Drill

Run these tests every 3 months:

```bash
# 1. Create test database
createdb test_dr

# 2. Restore from backup
pg_restore --dbname=test_dr latest_backup.dump

# 3. Verify TVIEWs work
psql test_dr -c "SELECT COUNT(*) FROM tv_posts;"
psql test_dr -c "SELECT * FROM pg_tviews_health_check();"

# 4. Test update
psql test_dr <<EOF
BEGIN;
UPDATE tb_post SET title = 'Test' WHERE pk_post = 1;
SELECT * FROM tv_post WHERE pk_post = 1;
ROLLBACK;
EOF

# 5. Cleanup
dropdb test_dr
```

### Chaos Testing (Optional)

```bash
# Randomly kill PostgreSQL during transaction
while true; do
    psql -c "UPDATE tb_post SET title = 'Chaos' WHERE pk_post = random() * 1000;" &
    sleep 0.1
    sudo systemctl kill -s SIGKILL postgresql
    sleep 2
    sudo systemctl start postgresql
    sleep 5
    # Check consistency
    psql -c "SELECT * FROM pg_tviews_health_check();"
done
```

---

## High Availability Setup

### Primary-Replica Configuration

```ini
# postgresql.conf on primary
wal_level = replica
max_wal_senders = 5
wal_keep_segments = 64

# On replica
hot_standby = on
```

**pg_tviews on Replicas**:
- Extension must be installed on all replicas
- TVIEWs replicate automatically via WAL
- Triggers don't fire on replicas (read-only)

### Failover Procedure

```bash
# 1. Promote replica
pg_ctl promote

# 2. Verify pg_tviews works
psql -c "SELECT pg_tviews_version();"
psql -c "SELECT * FROM pg_tviews_health_check();"

# 3. Test write
psql -c "UPDATE tb_post SET title = 'Failover Test' WHERE pk_post = 1;"

# 4. Update application connection string
# Point to new primary
```

---

## Monitoring for Disasters

### Proactive Alerts

```yaml
# Alert if health check fails
alerts:
  - alert: TViewHealthCheckFailed
    expr: pg_tviews_health_status != 'OK'
    for: 5m
    severity: critical

  - alert: TViewMetadataCorrupted
    expr: pg_tviews_metadata_count == 0
    for: 1m
    severity: critical

  - alert: TViewQueueStuck
    expr: pg_tviews_oldest_queue_age_seconds > 300
    for: 5m
    severity: critical
```

### Health Check Script

```bash
#!/bin/bash
# check_tview_health.sh

ERRORS=$(psql -qtAX -c "
    SELECT COUNT(*)
    FROM pg_tviews_health_check()
    WHERE status = 'ERROR';
")

if [ "$ERRORS" -gt 0 ]; then
    echo "CRITICAL: $ERRORS pg_tviews health check errors"
    psql -c "SELECT * FROM pg_tviews_health_check() WHERE status = 'ERROR';"
    exit 2
fi

echo "OK: pg_tviews health checks passed"
exit 0
```

---

## Contact & Escalation

### Self-Service
1. Check this disaster recovery guide
2. Review [Error Reference](errors.md)
3. Search [GitHub Issues](link)

### Community Support
1. Post in [GitHub Discussions](link)
2. Tag issue with "disaster-recovery"

### Emergency Escalation
1. Critical production outage: emergency@your-domain.com
2. Include: symptoms, error messages, steps attempted

---

## Appendix: Recovery Checklists

### Checklist: Corrupted TVIEW

- [ ] Identify corrupted TVIEW(s)
- [ ] Retrieve TVIEW definition from version control
- [ ] Drop corrupted TVIEW
- [ ] Recreate from definition
- [ ] Verify data consistency
- [ ] Update monitoring
- [ ] Document root cause

### Checklist: Database Restore

- [ ] Stop application writes
- [ ] Create restore point
- [ ] Restore from backup
- [ ] Verify pg_tviews extension loaded
- [ ] Run health check
- [ ] Test TVIEW refresh
- [ ] Verify data consistency
- [ ] Resume application writes
- [ ] Monitor for issues

### Checklist: Failover to Replica

- [ ] Promote replica to primary
- [ ] Verify pg_tviews functional
- [ ] Test write operations
- [ ] Update application config
- [ ] Restart application
- [ ] Monitor metrics
- [ ] Begin replica rebuild
```

**Deliverables**:
- Complete disaster recovery guide
- 7+ disaster scenarios with solutions
- Backup strategy documentation
- Recovery testing procedures
- HA setup guide

**Acceptance Criteria**:
- [ ] All critical scenarios covered
- [ ] Step-by-step recovery procedures
- [ ] Backup strategies documented
- [ ] Testing procedures included
- [ ] Escalation paths defined

---

(Continued in next message due to length...)
