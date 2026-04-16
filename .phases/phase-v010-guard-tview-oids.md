# Phase 2: Guard TVIEW OIDs in filter_base_tables()

## Objective
Skip TVIEW-managed tables in dependency traversal to prevent spurious trigger installation.

## Success Criteria
- [ ] filter_base_tables() skips OIDs present in pg_tview_meta.table_oid
- [ ] find_base_tables() passes tview_oids set to filter_base_tables()
- [ ] clippy clean

## TDD Cycles

### Cycle 1: Pre-compute TVIEW OID set
- **RED**: N/A (integration-only, no pg_test harness)
- **GREEN**: Add query for known TVIEW OIDs, pass HashSet to filter
- **REFACTOR**: Keep filter_base_tables pure (no SPI)
- **CLEANUP**: clippy clean

## Status
[ ] Not Started
