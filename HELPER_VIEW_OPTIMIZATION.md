# TVIEW Helper View Optimization Opportunities

**Date:** 2024-12-09
**Context:** Analysis of whether TVIEW can reduce helper view proliferation
**Finding:** Yes - TVIEW could eliminate 40-60% of helper views

---

## Executive Summary

PrintOptim uses **~70 helper views** to compose complex JSONB structures. Analysis reveals that many exist solely due to manual materialization limitations. **TVIEW's automatic composition could eliminate 30-40 helper views** (~50%), simplifying the schema significantly.

**Key Insight:** Helper views serve two purposes:
1. **Reusability** - Shared across multiple parent views (TVIEW still needs these)
2. **Convenience** - Work around manual SQL limitations (TVIEW can eliminate these)

---

## Helper View Categories

### Category 1: Simple Wrappers (Can be ELIMINATED)

**Pattern:** Just wraps a base table with `jsonb_build_object`

**Current approach:**
```sql
-- Helper view (exists only for convenience)
CREATE VIEW v_manufacturer AS
SELECT
  pk_manufacturer,
  id,
  jsonb_build_object(
    'id', id,
    'name', name,
    'abbreviation', abbreviation
  ) AS data
FROM tb_manufacturer
GROUP BY ...;  -- Unnecessary GROUP BY!

-- Used by:
CREATE VIEW v_model AS
SELECT
  m.*,
  jsonb_build_object(
    'manufacturer', v_manufacturer.data  -- Nests helper
  ) AS data
FROM tb_model m
JOIN v_manufacturer ON ...;
```

**With TVIEW (helper eliminated):**
```sql
-- No helper needed!
CREATE TVIEW tv_model AS
SELECT
  m.pk_model,
  m.id,
  m.fk_manufacturer,
  jsonb_build_object(
    'id', m.id,
    'name', m.name,
    'manufacturer', jsonb_build_object(  -- Inline!
      'id', mfr.id,
      'name', mfr.name,
      'abbreviation', mfr.abbreviation
    )
  ) AS data
FROM tb_model m
JOIN tb_manufacturer mfr ON m.fk_manufacturer = mfr.pk_manufacturer;

-- TVIEW automatically:
-- 1. Creates v_model (virtual) with this SQL
-- 2. Creates tv_model (materialized table)
-- 3. Installs triggers on tb_model AND tb_manufacturer
```

**Why helper exists today:** Developer convenience - easier to write `v_manufacturer.data` than inline the `jsonb_build_object` everywhere.

**Why TVIEW eliminates it:** TVIEW handles the complexity of tracking `tb_manufacturer` as a transitive dependency. Developer writes the inline SQL once, TVIEW manages updates.

**Estimated elimination:** 20-25 views (30-35%)

---

### Category 2: Aggregation Helpers (Can be REDUCED)

**Pattern:** `GROUP BY` with `jsonb_agg` to create nested arrays

**Current approach:**
```sql
-- Helper view for aggregation
CREATE VIEW v_machine_items AS
SELECT
  machine_id,
  jsonb_agg(v_machine_item.data) AS data
FROM tb_machine_item
JOIN v_machine_item ON ...
GROUP BY machine_id;

-- Used by:
CREATE VIEW v_machine AS
SELECT
  m.*,
  jsonb_build_object(
    'items', v_machine_items.data  -- Nested array
  ) AS data
FROM tb_machine m
LEFT JOIN v_machine_items ON ...;
```

**With TVIEW (helper partially eliminated):**

**Option A: Inline simple aggregations**
```sql
-- If v_machine_item is also just a wrapper, inline completely
CREATE TVIEW tv_machine AS
SELECT
  m.pk_machine,
  m.id,
  jsonb_build_object(
    'id', m.id,
    'items', (
      SELECT jsonb_agg(
        jsonb_build_object(
          'id', mi.id,
          'name', mi.name,
          'installed_at', mi.installed_at
        )
      )
      FROM tb_machine_item mi
      WHERE mi.fk_machine = m.pk_machine
    )
  ) AS data
FROM tb_machine m;

-- No helper views needed!
```

**Option B: Keep helper if complex or reused**
```sql
-- If v_machine_item is complex (joins 3+ tables), keep it
CREATE VIEW v_machine_item AS
SELECT ... FROM tb_machine_item
  JOIN tb_generic_accessory
  JOIN tb_manufacturer
  JOIN tb_contract_item ...;  -- Complex!

-- Then aggregation helper is still useful
CREATE VIEW v_machine_items AS
SELECT machine_id, jsonb_agg(v_machine_item.data) AS data
FROM ... GROUP BY machine_id;

CREATE TVIEW tv_machine AS
SELECT ... FROM tb_machine JOIN v_machine_items ...;
```

**Decision rule:**
- **Inline if:** Aggregation sources from 1-2 tables, simple logic
- **Keep helper if:** Aggregation logic is complex OR used by 2+ parent views

**Estimated elimination:** 10-15 views (15-20%)

---

### Category 3: Shared Helpers (MUST KEEP)

**Pattern:** Helper used by multiple parent views

**Current approach:**
```sql
-- Helper used by MULTIPLE parents
CREATE VIEW v_model AS
SELECT ... FROM tb_model JOIN v_manufacturer ...;

-- Parent 1
CREATE TABLE tv_machine AS
SELECT ... FROM tb_machine JOIN v_model ...;

-- Parent 2
CREATE VIEW v_contract_price_tree AS
SELECT ... FROM tb_contract JOIN v_model ...;

-- Parent 3
CREATE VIEW v_order_item AS
SELECT ... FROM tb_order_item JOIN v_model ...;
```

**With TVIEW (helper STILL NEEDED):**
```sql
-- Keep helper (used by 3 parents)
CREATE VIEW v_model AS
SELECT ... FROM tb_model JOIN v_manufacturer ...;

-- All parents use it
CREATE TVIEW tv_machine AS SELECT ... FROM v_model ...;
CREATE TVIEW tv_contract_price_tree AS SELECT ... FROM v_model ...;
CREATE TVIEW tv_order_item AS SELECT ... FROM v_model ...;
```

**Why helper must stay:** Avoids duplicating the v_model definition 3 times. DRY principle.

**TVIEW benefit:** Still huge! TVIEW automatically tracks that all 3 TVIEWs depend on `tb_model`, installs triggers, cascades refreshes.

**Estimated kept:** 25-30 views (35-40%)

---

## Optimization Scenarios

### Scenario 1: Single-Use Simple Helper

**Before TVIEW:**
```sql
-- 3 objects to maintain
CREATE VIEW v_manufacturer AS SELECT ..., jsonb_build_object(...) FROM tb_manufacturer;
CREATE VIEW v_model AS SELECT ..., v_manufacturer.data FROM tb_model JOIN v_manufacturer;
CREATE TABLE tv_machine AS ...;
CREATE FUNCTION refresh_tv_machine() AS $$ ... $$;
```

**After TVIEW:**
```sql
-- 1 object to maintain
CREATE TVIEW tv_machine AS
SELECT
  m.*,
  jsonb_build_object(
    'manufacturer', jsonb_build_object(  -- Inlined v_manufacturer
      'id', mfr.id,
      'name', mfr.name
    )
  ) AS data
FROM tb_machine
JOIN tb_model m ON ...
JOIN tb_manufacturer mfr ON ...;

-- TVIEW auto-generates everything else
```

**Reduction:** 3 objects → 1 object (67% reduction)

---

### Scenario 2: Shared Complex Helper

**Before TVIEW:**
```sql
-- Keep helper (used by 3+ views)
CREATE VIEW v_contract AS
SELECT ...,
  jsonb_build_object(
    'financing', v_financing_condition.data,
    'prices', v_contract_prices.data,
    'items', v_contract_items.data
  ) AS data
FROM tb_contract
JOIN v_financing_condition ...
JOIN v_contract_prices ...
JOIN v_contract_items ...;

-- Parents
CREATE TABLE tv_machine AS ...;  -- Uses v_contract
CREATE FUNCTION refresh_tv_machine() AS $$ ... $$;

CREATE TABLE tv_contract_summary AS ...;  -- Uses v_contract
CREATE FUNCTION refresh_tv_contract_summary() AS $$ ... $$;

CREATE VIEW v_order AS ...;  -- Uses v_contract
```

**After TVIEW:**
```sql
-- Still keep helper (DRY principle)
CREATE VIEW v_contract AS
SELECT ... FROM tb_contract
JOIN v_financing_condition ...;

-- But parents become simpler
CREATE TVIEW tv_machine AS SELECT ... FROM v_contract ...;
CREATE TVIEW tv_contract_summary AS SELECT ... FROM v_contract ...;
CREATE VIEW v_order AS SELECT ... FROM v_contract ...;

-- TVIEW auto-generates refresh functions, triggers
```

**Reduction:** Helper stays, but 2 manual refresh functions eliminated

---

### Scenario 3: Aggregation Chain

**Before TVIEW:**
```sql
-- Level 1: Wrap item
CREATE VIEW v_machine_item AS
  SELECT ..., jsonb_build_object(...) FROM tb_machine_item;

-- Level 2: Aggregate items
CREATE VIEW v_machine_items AS
  SELECT machine_id, jsonb_agg(v_machine_item.data) AS data
  FROM v_machine_item GROUP BY machine_id;

-- Level 3: Use aggregation
CREATE VIEW v_machine AS
  SELECT ..., v_machine_items.data FROM tb_machine JOIN v_machine_items;

-- Level 4: Materialize
CREATE TABLE tv_machine AS ...;
CREATE FUNCTION refresh_tv_machine() AS $$ ... $$;
```

**After TVIEW (aggressive inlining):**
```sql
-- All inlined into one TVIEW
CREATE TVIEW tv_machine AS
SELECT
  m.pk_machine,
  m.id,
  jsonb_build_object(
    'id', m.id,
    'items', (
      SELECT jsonb_agg(
        jsonb_build_object('id', mi.id, 'name', mi.name)
      )
      FROM tb_machine_item mi
      WHERE mi.fk_machine = m.pk_machine
    )
  ) AS data
FROM tb_machine m;

-- TVIEW detects dependency on tb_machine_item, installs triggers
```

**Reduction:** 4 levels → 1 object (75% reduction)

**Trade-off:** SQL is longer but self-contained. No helper reuse.

**After TVIEW (conservative - if v_machine_item is complex):**
```sql
-- Keep only complex helper
CREATE VIEW v_machine_item AS
  SELECT ... FROM tb_machine_item
  JOIN tb_generic_accessory
  JOIN tb_manufacturer ...;  -- Complex, maybe used elsewhere

-- Inline simple aggregation
CREATE TVIEW tv_machine AS
SELECT
  m.pk_machine,
  m.id,
  jsonb_build_object(
    'id', m.id,
    'items', (
      SELECT jsonb_agg(v_machine_item.data)
      FROM v_machine_item
      WHERE v_machine_item.machine_id = m.pk_machine
    )
  ) AS data
FROM tb_machine m;
```

**Reduction:** 4 levels → 2 objects (50% reduction)

---

## Quantitative Analysis: PrintOptim Case Study

### Current State (Manual tv_*)

| Category | Count | Purpose | Could Eliminate? |
|----------|-------|---------|------------------|
| Simple wrappers | ~25 | Convenience (jsonb_build_object) | ✅ Yes (inline) |
| Single-use aggregators | ~15 | Convenience (jsonb_agg) | ✅ Yes (inline) |
| Shared helpers (2+ parents) | ~20 | DRY principle | ❌ No (keep) |
| Complex helpers (3+ joins) | ~10 | Complexity management | ⚠️ Maybe (case-by-case) |
| **Total helpers** | **70** | | |

### With TVIEW (Optimized)

| Category | Before | After | Reduction |
|----------|--------|-------|-----------|
| Simple wrappers | 25 | 0 | -25 (100%) |
| Single-use aggregators | 15 | 5 | -10 (67%) |
| Shared helpers | 20 | 20 | 0 (0%) |
| Complex helpers | 10 | 8 | -2 (20%) |
| **Total helpers** | **70** | **33** | **-37 (53%)** |

**Estimated reduction: 50-55% fewer helper views**

---

## Developer Experience Comparison

### Before TVIEW (Manual)

**To add a nested manufacturer field to tv_machine:**

```sql
-- Step 1: Create helper view
CREATE VIEW v_manufacturer AS
  SELECT pk, id, jsonb_build_object(...) AS data
  FROM tb_manufacturer;

-- Step 2: Update v_model to use it
CREATE OR REPLACE VIEW v_model AS
  SELECT m.*, v_manufacturer.data AS manufacturer_data
  FROM tb_model m
  JOIN v_manufacturer ON ...;

-- Step 3: Update v_machine to use v_model
CREATE OR REPLACE VIEW v_machine AS
  SELECT ..., v_model.data FROM tb_machine JOIN v_model;

-- Step 4: Update refresh function
CREATE OR REPLACE FUNCTION refresh_tv_machine() AS $$
  INSERT INTO tv_machine SELECT * FROM v_machine WHERE id = $1;
$$;

-- Step 5: Manually add trigger on tb_manufacturer
CREATE TRIGGER trg_manufacturer_update
AFTER UPDATE ON tb_manufacturer
FOR EACH ROW EXECUTE refresh_affected_machines();

-- Step 6: Write cascade logic to find affected machines
CREATE FUNCTION refresh_affected_machines() AS $$
  -- Find machines with this manufacturer via model
  SELECT refresh_tv_machine(m.id)
  FROM tb_machine m
  JOIN tb_model ON m.fk_model = tb_model.pk
  WHERE tb_model.fk_manufacturer = NEW.pk_manufacturer;
$$;
```

**Total:** 6 steps, 4 objects to maintain, manual cascade logic

---

### After TVIEW (Automated)

**To add a nested manufacturer field to tv_machine:**

```sql
-- Step 1: Update TVIEW definition (inline manufacturer)
CREATE OR REPLACE TVIEW tv_machine AS
SELECT
  m.pk_machine,
  m.id,
  jsonb_build_object(
    'id', m.id,
    'model', jsonb_build_object(
      'id', model.id,
      'name', model.name,
      'manufacturer', jsonb_build_object(  -- NEW: Inline manufacturer
        'id', mfr.id,
        'name', mfr.name
      )
    )
  ) AS data
FROM tb_machine m
JOIN tb_model model ON m.fk_model = model.pk_model
JOIN tb_manufacturer mfr ON model.fk_manufacturer = mfr.pk_manufacturer;

-- TVIEW automatically:
-- ✅ Detects new dependency on tb_manufacturer
-- ✅ Installs trigger on tb_manufacturer
-- ✅ Updates cascade logic
-- ✅ Rebuilds tv_machine
```

**Total:** 1 step, TVIEW handles everything

**Reduction:** 6 steps → 1 step (83% less work)

---

## TVIEW Design Implications

### 1. Inlining Support

TVIEW should **encourage inlining** for simple cases:

```sql
-- Good: Inline simple nesting
CREATE TVIEW tv_machine AS
SELECT
  m.*,
  jsonb_build_object(
    'manufacturer', jsonb_build_object(...)  -- Inline
  ) AS data
FROM tb_machine m JOIN tb_manufacturer ...;
```

**Why:** Reduces helper view proliferation, simpler schema.

### 2. Helper Reuse Detection

TVIEW should **detect shared dependencies**:

```sql
-- Create helper explicitly
CREATE VIEW v_model AS SELECT ... FROM tb_model ...;

-- Multiple TVIEWs use it
CREATE TVIEW tv_machine AS SELECT ... FROM v_model ...;
CREATE TVIEW tv_contract AS SELECT ... FROM v_model ...;

-- TVIEW detects:
-- - v_model is used by 2 TVIEWs → keep as helper
-- - Both TVIEWs depend on tb_model transitively
-- - Install triggers on tb_model that cascade to both
```

### 3. Complexity Threshold

TVIEW documentation should provide guidelines:

**Inline when:**
- ✅ 1-2 table joins
- ✅ Simple `jsonb_build_object`
- ✅ No aggregations
- ✅ Used by only 1 parent

**Create helper when:**
- ⚠️ 3+ table joins
- ⚠️ Complex aggregations (GROUP BY with FILTER, multiple CTEs)
- ⚠️ Used by 2+ parents
- ⚠️ Logic is likely to change frequently (easier to update one helper)

---

## Migration Strategy

### Phase 1: Identify Eliminable Helpers

```sql
-- Find single-use helpers
SELECT
  h.helper_name,
  array_length(h.used_by, 1) AS usage_count,
  h.complexity_score
FROM pg_tview_helpers h
WHERE array_length(h.used_by, 1) = 1
  AND h.complexity_score < 10  -- Simple
ORDER BY complexity_score;

-- Candidates for inlining:
-- v_manufacturer (score: 2, used by: 1)
-- v_contract_item (score: 3, used by: 1)
-- v_financing_condition (score: 4, used by: 1)
```

### Phase 2: Inline Simple Helpers

```sql
-- Before: 3 objects
CREATE VIEW v_manufacturer AS ...;
CREATE VIEW v_model AS ... JOIN v_manufacturer;
CREATE TVIEW tv_machine AS ... JOIN v_model;

-- After: 2 objects (inline v_manufacturer into v_model)
CREATE VIEW v_model AS
  SELECT ...
    jsonb_build_object(
      'manufacturer', jsonb_build_object(...)  -- Inlined!
    )
  FROM tb_model
  JOIN tb_manufacturer ...;  -- Direct join

CREATE TVIEW tv_machine AS ... JOIN v_model;

-- v_manufacturer eliminated!
```

### Phase 3: Keep Shared Helpers

```sql
-- Keep if used by multiple parents
CREATE VIEW v_model AS ...;  -- Used by 3 TVIEWs

CREATE TVIEW tv_machine AS ... JOIN v_model;
CREATE TVIEW tv_contract AS ... JOIN v_model;
CREATE TVIEW tv_order AS ... JOIN v_model;

-- TVIEW manages transitive dependencies automatically
```

---

## Benefits Summary

### Schema Simplification

| Metric | Before TVIEW | After TVIEW | Improvement |
|--------|--------------|-------------|-------------|
| Helper views | 70 | 33 | **-53%** |
| Objects per entity | 3-6 | 1-2 | **-50-75%** |
| Lines of SQL | ~5,000 | ~2,500 | **-50%** |

### Developer Productivity

| Task | Before | After | Improvement |
|------|--------|-------|-------------|
| Add nested field | 6 steps | 1 step | **-83%** |
| Update helper | 4 objects | 1-2 objects | **-50-75%** |
| Understand dependencies | Manual trace | `tview_dependency_graph()` | **Automated** |

### Maintenance Burden

| Aspect | Before | After | Improvement |
|--------|--------|-------|-------------|
| Refresh functions | Manual | Auto-generated | **100% reduction** |
| Trigger installation | Manual | Auto-generated | **100% reduction** |
| Cascade logic | Manual | Auto-generated | **100% reduction** |
| Dependency tracking | Mental model | Metadata table | **Automated** |

---

## Recommendations

### 1. TVIEW Should Support Both Patterns

**Allow helpers:**
```sql
CREATE VIEW v_model AS ...;  -- Helper
CREATE TVIEW tv_machine AS ... FROM v_model;  -- Uses helper
```

**Allow inlining:**
```sql
CREATE TVIEW tv_machine AS
  SELECT ... FROM tb_machine
  JOIN tb_model ...
  JOIN tb_manufacturer ...;  -- No helper
```

**Let developer choose** based on complexity and reuse.

### 2. Provide Inlining Tools

```sql
-- TVIEW command to inline a helper
SELECT tview_inline_helper('tv_machine', 'v_model');

-- Expands v_model definition into tv_machine, drops v_model if unused
```

### 3. Optimize for Common Case

**Common case:** Single-use simple helpers (~40% of helpers)

**TVIEW should make inlining easy:**
- Detect single-use helpers
- Suggest inlining during `CREATE TVIEW`
- Provide auto-inline option

```sql
-- Option 1: Explicit inline
CREATE TVIEW tv_machine AS
  SELECT ... FROM tb_machine
  JOIN INLINE(v_model);  -- Expands v_model definition

-- Option 2: Auto-inline flag
CREATE TVIEW tv_machine WITH (auto_inline = true) AS
  SELECT ... FROM v_model;
-- TVIEW detects v_model is single-use, inlines automatically
```

---

## Conclusion

**Yes, TVIEW can significantly reduce helper view proliferation.**

**Key findings:**
1. **50-55% reduction** in helper views achievable (70 → 33)
2. **Simple wrappers** can be eliminated entirely (25 views)
3. **Single-use aggregators** can mostly be inlined (10-15 views)
4. **Shared helpers** must be kept (20-30 views)
5. **Developer experience** dramatically improved (6 steps → 1 step)

**TVIEW value proposition:**
- ✅ Fewer objects to maintain
- ✅ Simpler schema
- ✅ Automatic dependency management
- ✅ Auto-generated refresh/trigger logic
- ✅ Better DX (developer experience)

**Critical insight:** Helper views exist partly due to manual materialization complexity. TVIEW's automation removes the need for many "convenience helpers" while preserving genuinely useful shared helpers.

**Next step:** Update TVIEW PRD to include helper inlining strategies and auto-inline detection.
