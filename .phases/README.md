# Phase Plans for pg_tviews Development

This directory contains detailed implementation plans for each phase of pg_tviews development, following TDD methodology (RED → GREEN → REFACTOR).

## Overview

Each phase plan includes:
- **Objective** - What to achieve
- **RED Phase** - Write failing tests first
- **GREEN Phase** - Minimal implementation to pass tests
- **REFACTOR Phase** - Improve code quality
- **Verification** - How to confirm success
- **Acceptance Criteria** - Checklist before moving to next phase

## Phase Structure

```
Phase 0: Foundation
Phase 1: Schema Inference
Phase 2: DDL & Tables
Phase 3: Dependency & Triggers
Phase 4: Refresh & Cascade ✅ COMPLETE
Phase 5: jsonb_ivm Integration ⏳ IN PROGRESS
  ├─ Task 1: Dependency Setup ✅ Ready to implement
  ├─ Task 2: Metadata Enhancement (coming next)
  ├─ Task 3: Dependency Detection
  ├─ Task 4: Smart apply_patch()
  ├─ Task 5: Context Passing
  ├─ Task 6: Performance Benchmarking
  └─ Task 7: Array Handling (optional)
```

## Available Phase Plans

### Completed Phases
- **Phase 0-3**: See git history and docs/archive/
- **Phase 4**: See docs/archive/PHASE_4_PLAN.md

### Current Phase (Phase 5)
- **[phase-5-jsonb-ivm-integration.md](./phase-5-jsonb-ivm-integration.md)** - Overall Phase 5 strategy
- **[phase-5-task-1-dependency-setup.md](./phase-5-task-1-dependency-setup.md)** - Runtime detection & documentation

### Upcoming Tasks
- Task 2: Metadata Enhancement (will create after Task 1 complete)
- Task 3: Dependency Type Detection
- Task 4: Smart apply_patch() Implementation
- Task 5: Context Passing
- Task 6: Performance Benchmarking
- Task 7: Array Handling (optional)

## How to Use These Plans

### With opencode (Recommended)
```bash
# Run a phase plan with opencode
opencode run .phases/phase-5-task-1-dependency-setup.md

# opencode will:
# 1. Read the plan
# 2. Implement RED phase (write tests)
# 3. Implement GREEN phase (make tests pass)
# 4. Implement REFACTOR phase (improve code)
# 5. Run verification commands
```

### Manual Implementation
1. Read the phase plan
2. Follow RED → GREEN → REFACTOR sequence
3. Run verification commands
4. Check acceptance criteria
5. Commit with phase tag: `[Phase 5 Task 1]`

## Development Workflow

```
1. Claude writes detailed phase plan
   ↓
2. User runs: opencode run .phases/phase-X-task-Y.md
   ↓
3. opencode implements the plan
   ↓
4. Claude verifies the implementation
   ↓
5. Fix issues if needed, or move to next task
   ↓
6. Commit with descriptive message
```

## Commit Message Format

```bash
# Phase completion
git commit -m "feat(phase5): complete Task 1 - jsonb_ivm dependency setup [Phase 5 Task 1]"

# Individual steps
git commit -m "test(phase5): add RED tests for jsonb_ivm detection [Phase 5 Task 1 RED]"
git commit -m "feat(phase5): implement jsonb_ivm runtime check [Phase 5 Task 1 GREEN]"
git commit -m "refactor(phase5): cache jsonb_ivm detection result [Phase 5 Task 1 REFACTOR]"
```

## Phase Plan Template

See any existing phase plan for the standard structure:
- Objective & Success Criteria
- Context & Background
- RED Phase (failing tests)
- GREEN Phase (minimal implementation)
- REFACTOR Phase (code quality)
- Verification Commands
- Acceptance Criteria Checklist
- Files Modified
- Rollback Plan
- DO NOT list (anti-patterns)

## Notes

- **One task at a time**: Complete each task fully before moving to next
- **Test-first**: Always write tests in RED phase before implementation
- **Incremental**: Small, verifiable steps
- **Reversible**: Each task can be rolled back independently
- **Documented**: Every decision explained in phase plan
