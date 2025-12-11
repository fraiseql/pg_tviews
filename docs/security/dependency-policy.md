# Dependency Management Policy

**Document Version:** 1.0
**Last Updated:** 2025-12-11
**Classification:** Public
**Applicable Standards:** ISO 27001, SLSA, PCI-DSS 4.0

## Executive Summary

pg_tviews implements a comprehensive dependency management policy to ensure supply chain security, license compliance, and timely security updates. This policy governs how dependencies are selected, pinned, updated, and monitored throughout the project lifecycle.

## Dependency Selection Criteria

### Inclusion Requirements

Dependencies will be added if they meet ALL of the following criteria:

- ✅ **License Compatibility**: MIT, Apache-2.0, BSD, or compatible open-source licenses
- ✅ **Active Maintenance**: Regular updates within the last 12 months
- ✅ **Security Audit**: No known critical vulnerabilities (CVSS 7.0+)
- ✅ **Reasonable Size**: <50 transitive dependencies
- ✅ **Rust Ecosystem**: Available on crates.io with proper metadata
- ✅ **No Unsafe Code**: Or unsafe code is properly audited and documented

### Exclusion Criteria

Dependencies will NOT be added if they have ANY of the following issues:

- ❌ **Copyleft Licenses**: GPL, AGPL, LGPL (restrictive licensing)
- ❌ **Unmaintained**: No updates for >12 months
- ❌ **Critical Vulnerabilities**: Known unpatched CVSS 7.0+ vulnerabilities
- ❌ **Excessive Dependencies**: >50 transitive dependencies
- ❌ **Unsafe Code**: Unaudited unsafe Rust code
- ❌ **Unclear Licensing**: Ambiguous or missing license information

## Pinning Strategy

### Critical Dependencies (Exact Pin)

Security-critical and tightly-coupled dependencies are pinned to exact versions:

```toml
# Security-critical: exact version pinning
pgrx = "=0.12.8"
pgrx-macros = "=0.12.8"
pgrx-tests = "=0.12.8"
```

**Rationale**: pgrx is tightly coupled to PostgreSQL internals and ABI compatibility is critical.

### Standard Dependencies (Caret)

Regular dependencies allow compatible updates within major versions:

```toml
# Regular dependencies: compatible updates allowed
serde = "1.0"
serde_json = "1.0"
regex = "1.0"
once_cell = "1.0"
chrono = "0.4"
flate2 = "1.0"
bincode = "1.3"
```

**Rationale**: Allows security patches and bug fixes while maintaining API compatibility.

### Development Dependencies (Flexible)

Development-only dependencies are more flexible:

```toml
# Dev-only: more flexible versioning
[dev-dependencies]
# Testing and development tools can be more permissive
```

## Update Cadence

| Dependency Type | Frequency | Process | Approval |
|----------------|-----------|---------|----------|
| **Security Patches** | Immediate | Auto-merge PR | Automated |
| **Bug Fixes (patch)** | Weekly | Dependabot PR | Automated |
| **Minor Updates** | Weekly | Dependabot PR | Manual review |
| **Major Updates** | Quarterly | Manual PR | Design review |

### Security Update Priority

- **Critical (CVSS 9.0-10.0)**: Update within 24 hours
- **High (CVSS 7.0-8.9)**: Update within 7 days
- **Medium (CVSS 4.0-6.9)**: Update within 30 days
- **Low (CVSS 0.1-3.9)**: Update in next quarterly cycle

## Vulnerability Response

### Severity Classification

**Critical** (CVSS 9.0-10.0):
- Response time: <24 hours
- Action: Immediate patch or temporary mitigation
- Notification: Security advisory and release notes
- Communication: Direct contact with affected users

**High** (CVSS 7.0-8.9):
- Response time: <7 days
- Action: Update in next patch release
- Notification: Release notes and changelog
- Communication: Release announcement

**Medium** (CVSS 4.0-6.9):
- Response time: <30 days
- Action: Include in next minor release
- Notification: Changelog only
- Communication: Standard release process

**Low** (CVSS 0.1-3.9):
- Response time: Next quarterly cycle
- Action: Regular update process
- Notification: Optional, in changelog
- Communication: Standard release process

### Response Process

1. **Detection**: Automated scanning or manual report
2. **Assessment**: Evaluate impact and exploitability
3. **Mitigation**: Implement fix or temporary workaround
4. **Testing**: Full regression testing
5. **Release**: Patch release with security notes
6. **Communication**: Notify users via appropriate channels

## Supply Chain Monitoring

### Cargo Vet Integration

pg_tviews uses `cargo-vet` for supply chain security audits:

```toml
# supply-chain/config.toml
[cargo-vet]
version = "0.9"

[imports.mozilla]
url = "https://hg.mozilla.org/mozilla-central/raw-file/tip/supply-chain/audits.toml"

[policy.pgrx]
criteria = "safe-to-deploy"
notes = "Core dependency, manually audited"

[policy]
audit-as-crates-io = true
```

### Audit Criteria

- **safe-to-deploy**: Reviewed for security and correctness
- **safe-to-run**: Basic functionality verified
- **safe-to-use**: API stability confirmed

### Critical Dependencies Audit

| Dependency | Audit Status | Last Reviewed | Notes |
|------------|--------------|---------------|-------|
| **pgrx** | ✅ Audited | 2025-12-11 | Core PostgreSQL extension framework |
| **serde** | ✅ Mozilla | 2025-12-11 | Standard serialization library |
| **regex** | ✅ Mozilla | 2025-12-11 | Regular expression engine |

## Automated Dependency Management

### Dependabot Configuration

```yaml
# .github/dependabot.yml
version: 2
updates:
  - package-ecosystem: "cargo"
    directory: "/"
    schedule:
      interval: "weekly"
      day: "monday"
    groups:
      minor-updates:
        patterns: ["*"]
        update-types: ["minor", "patch"]
```

### PR Review Process

When Dependabot creates dependency update PRs:

1. **Automated Checks**:
   - CI pipeline runs full test suite
   - Security audit scans execute
   - SBOM regeneration occurs

2. **Review Criteria**:
   - **Security updates**: Auto-merge if tests pass
   - **Minor updates**: Manual review for API changes
   - **Major updates**: Full design review required

3. **Merge Process**:
   - Security patches: Merge within 24 hours
   - Regular updates: Merge within 1 week
   - Breaking changes: Extended testing period

## License Compliance

### Dependency License Inventory

All dependencies must have compatible licenses:

- ✅ **Permissive**: MIT, Apache-2.0, BSD-2-Clause, BSD-3-Clause
- ✅ **Compatible**: ISC, CC0, Unlicense
- ⚠️ **Review Required**: EPL, MPL (case-by-case)
- ❌ **Blocked**: GPL, AGPL, LGPL, CDDL

### License Verification

```bash
# Check license compatibility
cargo tree --format "{p} {l}" | grep -E "(GPL|AGPL|LGPL)"
# Should return no results for compliant dependencies
```

### Attribution Requirements

For dependencies requiring attribution:
- Notices included in LICENSE file
- Attribution in documentation
- Proper credit in release notes

## Dependency Health Metrics

### Monitoring Dashboard

Track dependency health using automated metrics:

- **Vulnerability Score**: Number of CVSS 7.0+ vulnerabilities
- **Update Frequency**: Average days between updates
- **License Compliance**: Percentage of compliant dependencies
- **Audit Coverage**: Percentage of dependencies audited

### Health Thresholds

- **Vulnerabilities**: 0 critical/high (CVSS 7.0+)
- **Outdated**: <10% of dependencies >6 months old
- **License Compliance**: 100% compatible licenses
- **Audit Coverage**: >80% of critical dependencies audited

## Emergency Procedures

### Critical Vulnerability Response

1. **Immediate Assessment** (within 1 hour):
   - Evaluate exploitability in pg_tviews context
   - Determine if pg_tviews is affected
   - Assess user impact

2. **Mitigation** (within 24 hours):
   - Implement temporary workaround if available
   - Prepare emergency patch release
   - Notify security contacts

3. **Resolution** (within 7 days):
   - Release patched version
   - Update SBOM and signatures
   - Publish security advisory

### Supply Chain Attack Response

1. **Detection**: Monitor for unusual dependency behavior
2. **Isolation**: Temporarily pin affected dependencies
3. **Investigation**: Audit dependency change history
4. **Recovery**: Update to secure versions
5. **Prevention**: Implement additional audit controls

## Compliance Standards

### International Standards
- **ISO 27001**: Information security management (Control 5.21)
- **SLSA Level 3**: Supply chain Levels for Software Artifacts
- **PCI-DSS 4.0**: Software component inventory (Requirement 6.3.2)

### Regulatory Compliance
- **EU Cyber Resilience Act**: Software transparency requirements
- **US EO 14028**: Federal software supply chain security
- **UK NCSC**: Supply chain security principles

### Industry Standards
- **OWASP**: Dependency management best practices
- **CISA**: Cybersecurity and Infrastructure Security Agency guidance
- **Mozilla**: Supply chain audit standards

## Continuous Improvement

### Policy Review

This policy is reviewed quarterly or when:
- New regulatory requirements emerge
- Security incidents occur
- Major dependency changes happen
- Audit findings require updates

### Metrics and Reporting

Quarterly dependency health reports include:
- Vulnerability remediation time
- License compliance status
- Audit coverage progress
- Update frequency analysis

### Future Enhancements

- [ ] Automated license scanning integration
- [ ] Dependency usage analysis
- [ ] Security scorecard integration
- [ ] Automated audit report generation

## References

- [Cargo Vet Documentation](https://mozilla.github.io/cargo-vet/)
- [Dependabot Configuration](https://docs.github.com/en/code-security/dependabot)
- [Rust Security Advisory Database](https://github.com/RustSec/advisory-db)
- [ISO 27001 Controls](https://www.iso.org/standard/54534.html)
- [SLSA Framework](https://slsa.dev/)

---

**Document Control:**
- **Author**: Lionel Hamayon
- **Reviewers**: Project Contributors
- **Review Cycle**: Annual
- **Distribution**: Public