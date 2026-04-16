# Phase 3: Second Drain Pass in flush_refresh_queue()

## Objective
Catch items enqueued by triggers firing during the flush (e.g. pg_treekey cascades).

## Success Criteria
- [ ] Outer drain loop wraps existing while-loop
- [ ] `processed` set carries across drain passes
- [ ] `max_propagation_depth` guard covers total iterations
- [ ] clippy clean

## TDD Cycles

### Cycle 1: Outer drain loop
- **GREEN**: Wrap existing loop, add second take_queue_snapshot() after inner loop
- **REFACTOR**: Ensure iteration counter is shared across passes
- **CLEANUP**: clippy clean

## Status
[ ] Not Started
