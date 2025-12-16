# ADR 2025-001: Breaking Changes for pg_tviews v2.0

## Status

**Accepted**

## Context

pg_tviews v1.0 (planned April 2026) will commit to long-term API stability. Any significant improvements requiring breaking changes must be planned for v2.0. This ADR documents the evaluation and decision process for breaking changes in v2.0.

### Current State (December 2025)

- **Version**: 0.1.0-beta.1
- **API Status**: Experimental, no stability guarantees
- **User Base**: Early adopters, limited production use
- **Timeline**: v1.0 in ~4 months, v2.0 in ~28 months

### Problems Identified

1. **API Clarity Issues**:
   - `refresh_pk()` uses opaque Oid parameters
   - Multiple refresh functions with overlapping functionality
   - Error types have too many variants (>15)

2. **Maintenance Burden**:
   - Complex parameter mappings (Oid ↔ string)
   - Duplicate error handling code
   - Multiple similar functions to maintain

3. **Future-Proofing**:
   - Schema organization needs improvement
   - Metadata structure could support more features
   - Refresh behavior could be more configurable

### Constraints

- **Stability Commitment**: v1.0 cannot have breaking changes
- **Timeline**: 24 months notice required for breaking changes
- **Migration Cost**: Must justify breaking changes with significant benefits
- **Backward Compatibility**: Where possible, maintain compatibility

## Decision

**Implement breaking changes in v2.0** to improve API clarity, reduce maintenance burden, and future-proof the codebase.

### Breaking Changes Approved

| Change | Rationale | Impact | Effort |
|--------|-----------|--------|--------|
| Entity naming simplification | Clearer API, better developer experience | HIGH | LOW |
| Error handling unification | Simpler error patterns, fewer variants | HIGH | MEDIUM |
| Refresh function consolidation | Single API surface, easier to maintain | MEDIUM | LOW |
| Debug function removal | Cleanup experimental features | LOW | VERY LOW |
| Schema reorganization | Better organization, future features | NONE | LOW |
| Metadata enhancement | Support future capabilities | NONE | LOW |
| Refresh policy configuration | Better control, opt-in feature | MEDIUM | MEDIUM |

### Timeline

- **Announcement**: December 2025 (this ADR and breaking changes document)
- **Deprecation Period**: April 2026 - April 2028 (24 months)
- **v2.0 Release**: April 2028
- **v1.x End of Life**: April 2029 (12 months after v2.0)

## Rationale

### Why Breaking Changes Are Necessary

1. **API Maturity**: v1.0 represents the first stable release. Breaking changes in v1.x would undermine stability guarantees.

2. **Design Improvements**: Current API has fundamental clarity issues that are expensive to maintain long-term.

3. **User Experience**: Breaking changes significantly improve developer experience and reduce cognitive load.

4. **Maintenance Cost**: Consolidating similar functionality reduces long-term maintenance burden.

### Why v2.0 Timeline

1. **24-Month Notice**: Provides ample time for enterprise users with change control processes.

2. **v1.0 Stability**: Allows v1.0 to establish stability reputation.

3. **Technology Evolution**: Allows time for PostgreSQL and ecosystem changes.

### Feasibility Assessment

#### Technical Feasibility

**✅ IMPLEMENTABLE**
- All changes build on existing functionality
- Migration paths are clear and automatable
- Testing can validate backward compatibility
- Rollback procedures are straightforward

#### User Migration Feasibility

**✅ MANAGEABLE**
- Migration guides provide clear step-by-step instructions
- Most changes are mechanical (find/replace)
- Error handling changes require careful review but are logical
- Enterprise support available for complex migrations

#### Business Feasibility

**✅ VIABLE**
- 24-month timeline accommodates enterprise planning
- Benefits justify migration cost
- Community communication plan in place
- Support structure available

## Alternatives Considered

### Alternative 1: No Breaking Changes

**Pros**:
- No user migration required
- Maintains compatibility
- Faster v1.0 release

**Cons**:
- Carries forward API design issues indefinitely
- Higher long-term maintenance cost
- Misses opportunity to improve user experience
- Technical debt accumulation

**Decision**: Rejected - benefits of breaking changes outweigh costs

### Alternative 2: Breaking Changes in v1.5

**Pros**:
- Earlier improvements
- Shorter migration timeline

**Cons**:
- Undermines v1.0 stability commitment
- Creates uncertainty about API stability
- Forces users to migrate twice (1.0 → 1.5 → 2.0)

**Decision**: Rejected - stability commitment is more important

### Alternative 3: Gradual Breaking Changes

**Pros**:
- Smaller migration steps
- Less disruptive per release

**Cons**:
- Prolonged migration period
- More complex version management
- Users still need to migrate eventually

**Decision**: Rejected - single migration is clearer and simpler

## Implementation Plan

### Phase 1: Planning & Communication (December 2025)
- [x] Create this ADR
- [x] Create breaking changes catalog
- [x] Create migration guide template
- [x] Announce plan to community

### Phase 2: Deprecation Warnings (v1.5, October 2026)
- Add deprecation warnings to affected functions
- Update documentation
- Provide migration tooling

### Phase 3: Migration Support (2027)
- Enhanced migration guides
- Community support
- Enterprise migration assistance

### Phase 4: v2.0 Release (April 2028)
- Implement breaking changes
- Comprehensive testing
- Release with migration support

## Impact Assessment

### User Impact

| User Type | Impact | Migration Effort | Timeline Fit |
|-----------|--------|------------------|--------------|
| Early Adopters | HIGH | MEDIUM | 24 months sufficient |
| Enterprise Users | HIGH | HIGH | Need enterprise support |
| New Users | NONE | NONE | Start with v2.0 |

### Business Impact

| Area | Impact | Mitigation |
|------|--------|------------|
| Development | Increased maintenance cost during transition | Clear timeline, tooling |
| Support | Higher support load during migration | Enhanced documentation, enterprise support |
| Reputation | API stability concerns | Clear communication, long timeline |

### Technical Impact

| Area | Impact | Mitigation |
|------|--------|------------|
| Code Complexity | Reduced (consolidated APIs) | Comprehensive testing |
| Testing | Increased (backward compatibility) | Automated testing, CI/CD |
| Documentation | Increased (migration guides) | Template-based approach |

## Risks & Mitigation

### Risk: Migration Failures
**Impact**: User downtime, data issues
**Probability**: Medium
**Mitigation**:
- Comprehensive testing requirements
- Rollback procedures documented
- Enterprise support for complex cases

### Risk: Timeline Extension
**Impact**: User uncertainty
**Probability**: Low
**Mitigation**:
- Conservative timeline (24 months)
- Regular progress updates
- Clear milestones

### Risk: Community Resistance
**Impact**: Adoption delays
**Probability**: Medium
**Mitigation**:
- Transparent decision process
- Community consultation
- Clear benefit communication

## Success Metrics

### Technical Metrics
- [ ] All breaking changes implemented by April 2028
- [ ] Migration tooling working for all scenarios
- [ ] Comprehensive test coverage maintained
- [ ] Performance regression < 5%

### User Metrics
- [ ] Migration guide completion rate > 90%
- [ ] Support ticket volume manageable
- [ ] Community feedback positive
- [ ] Enterprise adoption timeline met

### Business Metrics
- [ ] v2.0 adoption rate > 70% within 12 months
- [ ] Development velocity improved post-migration
- [ ] Maintenance cost reduced

## Monitoring & Adjustment

### Regular Reviews
- Monthly progress reviews
- Community feedback assessment
- Migration success tracking

### Adjustment Triggers
- Significant community resistance → Reconsider scope
- Technical blockers → Adjust timeline
- Migration complexity higher than expected → Enhance tooling

## Conclusion

The decision to implement breaking changes in v2.0 balances the need for API improvements with stability commitments. The 24-month timeline provides sufficient notice while allowing necessary improvements to the codebase.

**Approved by**: Architecture Review Board
**Date**: December 2025
**Review Date**: December 2026 (annual review)</content>
<parameter name="filePath">docs/adr/2025-v2-breaking-changes.md