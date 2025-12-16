# Known Breaking Changes (pg_tviews)

## Current Version: 0.1.0-beta.1

### No Breaking Changes (Beta Period)

This is the initial beta release. No previous versions to break from.

---

## Planned Breaking Changes for v2.0+

See [Phase 4.3: Breaking Changes Roadmap](../phases/phase-4.3-breaking-changes.md)

---

## Deprecation Policy

### Timeline
1. **Current Release**: New deprecation announced in release notes
2. **Next Minor**: Deprecation warnings in code (if applicable)
3. **+6 months**: Minimum before removal in patch release
4. **+12 months**: Preferred before removal in minor version
5. **Major version**: Can remove without notice if properly deprecated

### Removal Example
- v0.2.0: Feature X deprecated (Aug 2025)
- v0.3.0: Deprecation warning added (Sep 2025)
- v0.5.0 (Feb 2026): Can be removed (>6 months)
- v1.0.0 (Apr 2026): Should be removed (>12 months recommended)

### User Communication
- Release notes prominently feature deprecations
- Documentation updated with alternatives
- Error messages point to migration guide
- Forum/discussions alerted to changes