# Phase 2.4: Security Audit

**Objective**: Comprehensive security review of all unsafe code, SQL injection vectors, and privilege escalation risks

**Priority**: HIGH
**Estimated Time**: 2-3 days
**Blockers**: Phase 2.1-2.3 complete

---

## Context

**Current State**: 74 unsafe blocks, SQL string construction, FFI boundaries

**Why This Matters**:
- PostgreSQL extensions run with superuser privileges
- SQL injection can lead to complete database compromise
- Unsafe Rust code can cause memory corruption
- FFI boundaries must be bulletproof

**Deliverable**: Security audit report with all findings addressed

---

## Audit Scope

### 1. Unsafe Rust Code (74 blocks)
- All `unsafe` blocks reviewed for soundness
- Memory safety verification
- Null pointer dereferencing checks
- FFI boundary validation

### 2. SQL Injection Vectors
- Dynamic SQL construction
- User input validation
- Parameter binding vs string interpolation
- Identifier escaping

### 3. Privilege Escalation
- Function permission analysis
- SECURITY DEFINER vs SECURITY INVOKER
- Row-level security bypass risks
- Superuser-only operations

### 4. Input Validation
- Entity name validation (SQL injection)
- Primary key validation
- JSONB data validation
- Dependency graph validation

---

## Implementation Steps

### Step 1: Unsafe Code Audit

**Install auditing tools**:
```bash
cargo install cargo-geiger
cargo install cargo-audit
cargo install cargo-deny
```

**Run automated scans**:
```bash
# Unsafe code statistics
cargo geiger --output-format=GitHubMarkdown > unsafe-audit.md

# Known vulnerabilities
cargo audit

# License and security policy compliance
cargo deny check
```

**Manual review checklist** for each unsafe block:

**Create**: `docs/security/UNSAFE_AUDIT.md`

```markdown
# Unsafe Code Audit

## Summary

Total unsafe blocks: 74
- Audited: 74
- Safe: 71
- Needs fix: 3

## Audit Criteria

For each unsafe block, verify:

1. **Memory Safety**
   - [ ] No use-after-free
   - [ ] No double-free
   - [ ] No null pointer dereference
   - [ ] Proper lifetime management

2. **FFI Safety**
   - [ ] Correct calling convention (extern "C-unwind")
   - [ ] NULL checks on pointers from PostgreSQL
   - [ ] Proper `pg_guard` usage
   - [ ] Error handling via PG_TRY/PG_CATCH

3. **Data Race Freedom**
   - [ ] No shared mutable state without synchronization
   - [ ] Thread-local storage used correctly
   - [ ] Atomic operations where needed

## Per-File Audit

### src/lib.rs (12 unsafe blocks)

#### Block 1: `pg_module_magic!()`
```rust
unsafe impl pgrx::PgSharedMemoryInitialization for TViewsSharedMemory {
    fn pg_init(&'static self) {
        // SAFETY: Called once by PostgreSQL during startup
        // No concurrent access possible at this point
    }
}
```

**Status**: ✅ SAFE
**Justification**: Single-threaded initialization, guaranteed by PostgreSQL
**Last reviewed**: 2025-12-13

---

### src/refresh/main.rs (18 unsafe blocks)

#### Block 1: SPI query execution
```rust
unsafe {
    Spi::connect(|client| {
        client.select(query, None, None)
    })
}
```

**Status**: ✅ SAFE
**Justification**:
- `Spi::connect` properly handles PostgreSQL context
- Query is validated before execution
- Error handling via Result<T, TViewError>

**Last reviewed**: 2025-12-13

#### Block 2: Datum extraction
```rust
unsafe {
    let ptr = PG_GETARG_POINTER(0);
    if ptr.is_null() {
        return None;
    }
    Some(&*ptr)
}
```

**Status**: ⚠️  NEEDS FIX
**Issue**: No validation that pointer is actually valid
**Fix**: Add bounds checking or use pgrx safe wrappers
**Assigned to**: [TBD]

---

[Continue for all 74 blocks...]

## High-Risk Areas

1. **FFI boundaries** (28 blocks) - PRIORITY 1
2. **Pointer arithmetic** (8 blocks) - PRIORITY 2
3. **Type transmutation** (2 blocks) - PRIORITY 3
4. **Thread-local storage** (6 blocks) - PRIORITY 2

## Action Items

- [ ] Fix Block src/refresh/main.rs:234 (null pointer check)
- [ ] Fix Block src/dependency/graph.rs:89 (unchecked transmute)
- [ ] Add fuzzing tests for FFI boundaries
- [ ] Document safety invariants in code comments
```

### Step 2: SQL Injection Audit

**Create**: `test/security/test-sql-injection.sh`

```bash
#!/bin/bash
set -euo pipefail

echo "Testing SQL injection vulnerabilities..."

# Setup
psql <<EOF
CREATE TABLE tb_inject_test (pk_test SERIAL PRIMARY KEY, data TEXT);
CREATE TABLE tv_inject_test AS SELECT pk_test, data FROM tb_inject_test;
EOF

echo "Test 1: Entity name injection"

# Try to inject SQL via entity name
set +e
psql <<EOF 2>&1 | tee /tmp/inject-test.txt
SELECT pg_tviews_convert_existing_table('tv_inject_test; DROP TABLE tb_inject_test; --');
EOF
RESULT=$?
set -e

# Should fail safely
if [ $RESULT -ne 0 ]; then
    echo "✅ PASS: SQL injection blocked"
    grep -qi "invalid" /tmp/inject-test.txt && echo "✅ Proper error message"
else
    # Check if tb_inject_test still exists
    if psql -c "SELECT 1 FROM tb_inject_test LIMIT 1;" &>/dev/null; then
        echo "✅ PASS: Table not dropped, injection failed"
    else
        echo "❌ CRITICAL: SQL injection succeeded - table dropped!"
        exit 1
    fi
fi

echo "Test 2: Column name injection"

# Try to inject via JSONB field
set +e
psql <<EOF
INSERT INTO tb_inject_test (data) VALUES ('test');
-- Try to inject malicious field name
SELECT pg_tviews_refresh('tv_inject_test', jsonb_fields => ARRAY['data); DROP TABLE tb_inject_test; --']);
EOF
RESULT=$?
set -e

if [ $RESULT -ne 0 ]; then
    echo "✅ PASS: Column injection blocked"
fi

echo "Test 3: Batch SQL injection"

# Test if batch operations properly escape
psql <<EOF
-- Create legitimate TVIEW
CREATE TABLE tv_batch_test AS SELECT 1 as id;
SELECT pg_tviews_convert_existing_table('tv_batch_test');

-- Try batch refresh with malicious entity name
SELECT pg_tviews_refresh_batch(ARRAY['tv_batch_test', 'evil''; DROP TABLE tb_inject_test; --']);
EOF

# Verify table still exists
if psql -c "SELECT 1 FROM tb_inject_test LIMIT 1;" &>/dev/null; then
    echo "✅ PASS: Batch injection blocked"
else
    echo "❌ CRITICAL: Batch SQL injection succeeded!"
    exit 1
fi

echo "✅ SQL injection tests passed"
```

**Review all SQL construction**:

```bash
# Find all format!() and string concatenation in SQL context
rg 'format!\(.*SELECT|INSERT|UPDATE|DELETE' src/ --type rust

# Find all execute() calls
rg 'execute\(' src/ --type rust -A 2 -B 2
```

**For each SQL construction, verify**:
1. Uses parameter binding (`$1, $2`) not string interpolation
2. Identifiers escaped with `quote_ident()`
3. Literals escaped with `quote_literal()`
4. No direct user input in SQL

### Step 3: Privilege Escalation Audit

**Create**: `test/security/test-privileges.sh`

```bash
#!/bin/bash
set -euo pipefail

echo "Testing privilege escalation vectors..."

# Create non-superuser
psql <<EOF
DROP ROLE IF EXISTS tview_test_user;
CREATE ROLE tview_test_user LOGIN PASSWORD 'test';
GRANT CREATE ON DATABASE postgres TO tview_test_user;
EOF

echo "Test 1: Non-superuser cannot bypass RLS"

psql <<EOF
-- Create table with RLS
CREATE TABLE tb_rls_test (pk_test INT PRIMARY KEY, data TEXT, owner TEXT);
ALTER TABLE tb_rls_test ENABLE ROW LEVEL SECURITY;

CREATE POLICY rls_policy ON tb_rls_test
    USING (owner = current_user);

CREATE TABLE tv_rls_test AS SELECT pk_test, data FROM tb_rls_test;
SELECT pg_tviews_convert_existing_table('tv_rls_test');

-- Insert data as superuser
INSERT INTO tb_rls_test VALUES (1, 'secret', 'postgres');
INSERT INTO tb_rls_test VALUES (2, 'public', 'tview_test_user');
EOF

# Connect as test user
PGUSER=tview_test_user psql <<EOF
-- Should only see own data
SELECT * FROM tv_rls_test;
EOF

ROW_COUNT=$(PGUSER=tview_test_user psql -tAc "SELECT COUNT(*) FROM tv_rls_test;")

if [ "$ROW_COUNT" -eq 1 ]; then
    echo "✅ PASS: RLS enforced on TVIEW"
else
    echo "❌ FAIL: RLS bypassed (saw $ROW_COUNT rows, expected 1)"
    exit 1
fi

echo "Test 2: Non-superuser cannot modify metadata"

set +e
PGUSER=tview_test_user psql <<EOF
INSERT INTO pg_tviews_metadata (entity_name, backing_view, pk_column)
VALUES ('evil_view', 'pg_authid', 'oid');
EOF
RESULT=$?
set -e

if [ $RESULT -ne 0 ]; then
    echo "✅ PASS: Metadata table protected"
else
    echo "❌ FAIL: Non-superuser modified metadata"
    exit 1
fi

# Cleanup
psql -c "DROP ROLE tview_test_user;"

echo "✅ Privilege tests passed"
```

### Step 4: Input Validation Audit

**Create test for all input validation**:

**File**: `test/security/test-input-validation.sh`

```bash
#!/bin/bash
set -euo pipefail

echo "Testing input validation..."

echo "Test 1: Invalid entity names"

INVALID_NAMES=(
    ""                      # Empty
    "a"                     # Too short
    "$(printf 'x%.0s' {1..256})"  # Too long
    "123_invalid"           # Starts with number
    "invalid-dash"          # Contains dash
    "invalid space"         # Contains space
    "invalid;drop"          # Contains semicolon
)

for name in "${INVALID_NAMES[@]}"; do
    set +e
    psql -c "SELECT pg_tviews_convert_existing_table('$name');" 2>/dev/null
    RESULT=$?
    set -e

    if [ $RESULT -ne 0 ]; then
        echo "✅ Rejected: '$name'"
    else
        echo "❌ FAIL: Accepted invalid name: '$name'"
        exit 1
    fi
done

echo "Test 2: Validate dependency depth limit"

# Create deep dependency chain
psql <<EOF
CREATE TABLE tb_depth_0 (pk INT PRIMARY KEY);
CREATE TABLE tv_depth_0 AS SELECT pk FROM tb_depth_0;
SELECT pg_tviews_convert_existing_table('tv_depth_0');
EOF

# Try to create 11 levels (exceeds limit of 10)
for i in {1..11}; do
    PREV=$((i-1))
    psql <<EOF
CREATE TABLE tb_depth_$i (pk INT PRIMARY KEY, fk INT);
CREATE TABLE tv_depth_$i AS
    SELECT d$i.pk, d$PREV.pk as prev_pk
    FROM tb_depth_$i d$i
    LEFT JOIN tv_depth_$PREV d$PREV ON d$i.fk = d$PREV.pk;
EOF

    set +e
    psql -c "SELECT pg_tviews_convert_existing_table('tv_depth_$i');" 2>&1 | tee /tmp/depth-test.txt
    RESULT=$?
    set -e

    if [ $i -gt 10 ] && [ $RESULT -ne 0 ]; then
        echo "✅ PASS: Dependency depth limit enforced at level $i"
        grep -qi "dependency depth" /tmp/depth-test.txt && echo "✅ Clear error message"
        break
    fi
done

echo "✅ Input validation tests passed"
```

### Step 5: Fuzzing Tests

**Create**: `test/security/fuzz-entity-names.sh`

```bash
#!/bin/bash
set -euo pipefail

echo "Fuzzing entity name validation..."

# Generate 1000 random entity names
python3 <<EOF
import random
import string

for i in range(1000):
    # Random length 0-100
    length = random.randint(0, 100)

    # Random characters including special chars
    chars = string.ascii_letters + string.digits + "_;-'\"\\n\\t\0"
    name = ''.join(random.choice(chars) for _ in range(length))

    # Escape for shell
    name_escaped = name.replace("'", "'\\''")
    print(f"psql -c \"SELECT pg_tviews_convert_existing_table('{name_escaped}');\" 2>/dev/null || true")
EOF | bash

echo "✅ Fuzzing completed (no crashes)"
```

---

## Verification Commands

```bash
# Run security test suite
cd test/security
./test-sql-injection.sh
./test-privileges.sh
./test-input-validation.sh
./fuzz-entity-names.sh

# Run static analysis
cargo geiger
cargo audit
cargo clippy -- -W clippy::unwrap_used -W clippy::expect_used

# Check for common vulnerabilities
rg "SECURITY DEFINER" src/
rg "execute\(format!" src/
rg "unsafe" src/ | wc -l
```

---

## Acceptance Criteria

- [ ] All 74 unsafe blocks audited and documented
- [ ] No SQL injection vulnerabilities found
- [ ] No privilege escalation vectors
- [ ] All input validation comprehensive
- [ ] Fuzzing tests pass without crashes
- [ ] cargo audit shows no known vulnerabilities
- [ ] UNSAFE_AUDIT.md completed
- [ ] Security findings document created
- [ ] All high-severity issues fixed

---

## Security Checklist

### SQL Injection Prevention
- [ ] All SQL uses parameter binding (`$1, $2`)
- [ ] All identifiers use `quote_ident()`
- [ ] No string concatenation in SQL
- [ ] Entity names validated against `^[a-z_][a-z0-9_]{0,63}$`

### Memory Safety
- [ ] All `unsafe` blocks have SAFETY comments
- [ ] No null pointer dereferences without checks
- [ ] All FFI pointers validated
- [ ] No use-after-free possible

### Privilege Management
- [ ] No SECURITY DEFINER without validation
- [ ] RLS respected on TVIEWs
- [ ] Metadata table has proper permissions
- [ ] Superuser-only operations documented

---

## DO NOT

- ❌ Add unsafe blocks without SAFETY comments
- ❌ Use string interpolation for SQL
- ❌ Skip input validation "because it's internal"
- ❌ Assume PostgreSQL will prevent all attacks

---

## Security Disclosure

If vulnerabilities found:

1. **Do NOT commit fixes to public repo immediately**
2. Create private security advisory on GitHub
3. Develop and test fix privately
4. Coordinate disclosure with users
5. Release fix and advisory simultaneously

---

## Next Steps

After completion:
- Commit with message: `security: Complete comprehensive security audit [PHASE2.4]`
- Publish security audit results
- Address all high-severity findings
- Proceed to **Phase 3.1: Benchmark Validation** (already created)
