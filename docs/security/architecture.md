# Security Architecture

**Document Version:** 1.0
**Last Updated:** 2025-12-11
**Classification:** Public
**Applicable Standards:** ISO 27001, OWASP Threat Modeling

## Executive Summary

pg_tviews implements a comprehensive security architecture designed to protect against supply chain attacks, data breaches, and unauthorized access. This document outlines the threat model, security boundaries, controls, and assumptions that form the foundation of pg_tviews security.

## Threat Model

### Assets

1. **Extension Code** - Rust code, SQL functions, and compiled binaries
2. **User Data** - Data stored in TVIEWs and materialized views
3. **Build Artifacts** - Releases, containers, and distribution packages
4. **Signing Keys** - GPG and Sigstore keys for artifact verification
5. **SBOM Data** - Software Bill of Materials and dependency information
6. **Build Provenance** - SLSA attestations and build metadata

### Threats

#### 1. Supply Chain Attacks
- **Description**: Compromised dependencies, malicious code in upstream crates
- **Impact**: Code execution, data theft, system compromise
- **Likelihood**: Medium (common in open-source ecosystem)
- **Risk Level**: High

#### 2. Code Injection
- **Description**: Malicious SQL/Rust code execution through extension functions
- **Impact**: Database compromise, data manipulation, privilege escalation
- **Likelihood**: Low (requires valid PostgreSQL access)
- **Risk Level**: Medium

#### 3. Data Leakage
- **Description**: Unauthorized access to TVIEW data through misconfiguration
- **Impact**: Privacy violations, data breaches, compliance failures
- **Likelihood**: Medium (depends on configuration)
- **Risk Level**: High

#### 4. Build Tampering
- **Description**: Modified releases or compromised build process
- **Impact**: Malicious code distribution, trust erosion
- **Likelihood**: Low (protected by provenance)
- **Risk Level**: Medium

#### 5. Dependency Confusion
- **Description**: Malicious packages with similar names to legitimate dependencies
- **Impact**: Code execution during build or runtime
- **Likelihood**: Low (protected by cargo-vet)
- **Risk Level**: Medium

#### 6. Side-Channel Attacks
- **Description**: Timing attacks, resource exhaustion, or information leakage
- **Impact**: Information disclosure, denial of service
- **Likelihood**: Low (Rust memory safety)
- **Risk Level**: Low

### Attack Vectors

#### External Attack Vectors
- **Malicious Dependencies**: Compromised crates.io packages
- **Build Infrastructure**: Compromised CI/CD pipelines
- **Distribution Channels**: Tampered release artifacts
- **Social Engineering**: Developer credential theft

#### Internal Attack Vectors
- **Misconfiguration**: Incorrect PostgreSQL security settings
- **Privilege Escalation**: Database user permission abuse
- **SQL Injection**: Through TVIEW creation parameters
- **Memory Corruption**: Rust unsafe code exploitation

## Security Boundaries

```
┌─────────────────────────────────────────────────────────────────┐
│                    TRUSTED COMPUTING BASE                        │
│  ┌─────────────────────────────────────────────────────────────┐ │
│  │                PostgreSQL Server (Trust Boundary)           │ │
│  │  ┌─────────────────────────────────────────────────────────┐ │ │
│  │  │                                                         │ │ │
│  │  │                pg_tviews Extension                      │ │ │
│  │  │  ┌─────────────────────────────────────────────────────┐ │ │ │
│  │  │  │                                                     │ │ │ │
│  │  │  │          User-Facing SQL Functions                  │ │ │ │
│  │  │  │                                                     │ │ │ │
│  │  │  │  • pg_tviews_create() - TVIEW creation              │ │ │ │
│  │  │  │  • pg_tviews_drop() - TVIEW removal                 │ │ │ │
│  │  │  │  • pg_tviews_refresh() - Manual refresh             │ │ │ │
│  │  │  │                                                     │ │ │ │
│  │  │  └─────────────────────────────────────────────────────┘ │ │ │
│  │  │                                                         │ │ │
│  │  │  ┌─────────────────────────────────────────────────────┐ │ │ │
│  │  │  │                                                     │ │ │ │
│  │  │  │           Internal Rust Implementation               │ │ │ │
│  │  │  │                                                     │ │ │ │
│  │  │  │  • Trigger handlers (unsafe FFI)                    │ │ │ │
│  │  │  │  • DDL processing and validation                    │ │ │ │
│  │  │  │  • Incremental refresh logic                        │ │ │ │
│  │  │  │  • Cache management                                 │ │ │ │
│  │  │  │                                                     │ │ │ │
│  │  │  └─────────────────────────────────────────────────────┘ │ │ │
│  │  └─────────────────────────────────────────────────────────┘ │ │
│  └─────────────────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────────────────┘
```

### Trust Levels

#### Level 1: Fully Trusted
- **PostgreSQL Server**: Assumed secure and properly configured
- **Database Administrators**: Full access by design
- **Extension Installation**: Requires superuser privileges

#### Level 2: High Trust
- **Extension Code**: Audited Rust code with memory safety guarantees
- **Build Process**: Reproducible builds with provenance
- **Release Artifacts**: Cryptographically signed and verified

#### Level 3: Medium Trust
- **Application Users**: Database users with appropriate permissions
- **TVIEW Data**: Protected by PostgreSQL Row Level Security (RLS)
- **Configuration**: User-provided settings validated

#### Level 4: Low Trust
- **External Dependencies**: Audited via cargo-vet and cargo-audit
- **Network Communications**: TLS-protected where applicable
- **User Input**: Parameterized and validated

## Security Controls

### Preventive Controls

| Control Category | Implementation | Effectiveness |
|------------------|----------------|----------------|
| **Input Validation** | SQL parameter binding, type checking | High |
| **Access Control** | PostgreSQL RBAC, function permissions | High |
| **Code Security** | Rust memory safety, clippy lints | High |
| **Build Security** | Reproducible builds, provenance | High |
| **Dependency Security** | cargo-vet audits, cargo-audit scans | High |

### Detective Controls

| Control Category | Implementation | Effectiveness |
|------------------|----------------|----------------|
| **Audit Logging** | pg_tview_audit_log table | Medium |
| **Vulnerability Scanning** | Daily automated scans | High |
| **Integrity Checking** | SHA256/SHA512 checksums | High |
| **Provenance Verification** | SLSA Level 3 attestations | High |

### Corrective Controls

| Control Category | Implementation | Effectiveness |
|------------------|----------------|----------------|
| **Patch Management** | Automated dependency updates | High |
| **Incident Response** | Defined procedures and timelines | Medium |
| **Backup Recovery** | PostgreSQL native capabilities | High |
| **Configuration Management** | Version-controlled settings | High |

### Security Control Mapping

#### ISO 27001 Controls
- **A.9 Access Control**: PostgreSQL RBAC implementation
- **A.12 Operations Security**: Secure development practices
- **A.13 Communications Security**: TLS-protected connections
- **A.14 System Acquisition**: Secure supply chain practices

#### NIST Cybersecurity Framework
- **Identify**: Asset management and risk assessment
- **Protect**: Access control and data security
- **Detect**: Audit logging and vulnerability scanning
- **Respond**: Incident response procedures
- **Recover**: Backup and disaster recovery

## Security Assumptions

### Trusted Components

We assume the following components are trustworthy and properly secured:

- ✅ **PostgreSQL Server**: Secure installation and configuration
- ✅ **Database Administrators**: Authorized and trained personnel
- ✅ **Operating System**: Secure host environment
- ✅ **Network Infrastructure**: TLS-enabled communications
- ✅ **File System**: Proper permissions and access controls

### Untrusted Components

We assume the following may be untrusted or hostile:

- ⚠️ **Application Users**: May attempt privilege escalation or data theft
- ⚠️ **External Dependencies**: May contain vulnerabilities or backdoors
- ⚠️ **Network Traffic**: May be intercepted or modified
- ⚠️ **Configuration Files**: May be tampered with
- ⚠️ **Build Artifacts**: May be compromised during distribution

## Secure Coding Practices

### Rust Code Standards

#### Memory Safety
- ✅ **No unsafe code** in core logic (only pgrx FFI bindings)
- ✅ **All unsafe blocks** are documented and audited
- ✅ **Bounds checking** enforced by Rust compiler
- ✅ **No raw pointers** or manual memory management

#### Code Quality
- ✅ **Clippy lints** enabled with strict settings
- ✅ **Rust 2021 edition** with modern safety features
- ✅ **Comprehensive testing** including edge cases
- ✅ **Error handling** without sensitive data leakage

#### Security Features
- ✅ **Position Independent Code** (PIC) for ASLR
- ✅ **RELRO and BIND_NOW** for GOT protection
- ✅ **No executable stack** linker hardening
- ✅ **Stack canaries** and buffer overflow protection

### SQL Code Standards

#### Injection Prevention
- ✅ **Parameterized queries** only (no string concatenation)
- ✅ **Schema-qualified names** to prevent confusion attacks
- ✅ **Explicit type casting** and validation
- ✅ **Input sanitization** for all user-provided data

#### Access Control
- ✅ **Function permissions** via GRANT/REVOKE
- ✅ **Row Level Security** (RLS) support
- ✅ **Least privilege** principle implementation
- ✅ **Audit logging** for sensitive operations

### Development Practices

#### Code Review
- ✅ **Security review** required for all changes
- ✅ **Automated testing** including security test cases
- ✅ **Dependency updates** reviewed for security implications
- ✅ **Unsafe code** requires explicit security approval

#### Build Security
- ✅ **Reproducible builds** with locked environments
- ✅ **Dependency auditing** via cargo-vet
- ✅ **Vulnerability scanning** via cargo-audit
- ✅ **Cryptographic signing** of all artifacts

## Threat Mitigation Strategies

### Supply Chain Protection

1. **Dependency Auditing**: cargo-vet audits all transitive dependencies
2. **Vulnerability Scanning**: Daily cargo-audit checks for CVEs
3. **Reproducible Builds**: Docker-based builds prevent tampering
4. **Provenance Tracking**: SLSA Level 3 build attestations

### Runtime Protection

1. **Memory Safety**: Rust prevents buffer overflows and use-after-free
2. **Type Safety**: Strong typing prevents type confusion attacks
3. **Access Control**: PostgreSQL RBAC prevents unauthorized access
4. **Input Validation**: Parameterized queries prevent SQL injection

### Operational Security

1. **Audit Logging**: Comprehensive logging of all operations
2. **Configuration Management**: Secure configuration practices
3. **Patch Management**: Automated security updates
4. **Incident Response**: Defined procedures for security events

## Risk Assessment

### High Risk Items

| Risk | Probability | Impact | Mitigation | Residual Risk |
|------|-------------|--------|------------|----------------|
| Supply Chain Attack | Medium | High | cargo-vet, provenance | Low |
| SQL Injection | Low | High | Parameterized queries | Very Low |
| Data Leakage | Medium | High | RLS, access control | Low |
| Build Tampering | Low | High | SLSA, reproducible builds | Very Low |

### Medium Risk Items

| Risk | Probability | Impact | Mitigation | Residual Risk |
|------|-------------|--------|------------|----------------|
| Dependency Vulnerability | Medium | Medium | cargo-audit, updates | Low |
| Misconfiguration | High | Medium | Documentation, validation | Low |
| Privilege Escalation | Low | Medium | RBAC, least privilege | Very Low |

### Monitoring and Metrics

#### Security Metrics
- **Vulnerability Scan Results**: Daily pass/fail status
- **Audit Coverage**: Percentage of dependencies audited
- **Build Reproducibility**: Success rate of reproducible builds
- **Incident Response Time**: Average time to patch vulnerabilities

#### Key Performance Indicators
- **Zero Critical Vulnerabilities**: Target for all releases
- **100% Audit Coverage**: All dependencies vetted
- **<24 hours**: Time to patch critical vulnerabilities
- **100%**: Build reproducibility rate

## Compliance Alignment

### Regulatory Frameworks

#### GDPR (Data Protection)
- **Data Minimization**: No personal data collection
- **Access Control**: PostgreSQL RBAC implementation
- **Audit Logging**: Comprehensive operation logging
- **Data Encryption**: TLS and PostgreSQL encryption support

#### SOC 2 (Trust Services)
- **Security**: Access controls and vulnerability management
- **Availability**: PostgreSQL HA and backup capabilities
- **Processing Integrity**: Transaction safety and consistency
- **Confidentiality**: Data protection and access controls

#### ISO 27001 (Information Security)
- **Information Security Policies**: Comprehensive security documentation
- **Access Control**: RBAC and permission management
- **Cryptography**: TLS and signing key management
- **Operations Security**: Secure development and deployment

## Future Enhancements

### Planned Security Improvements

- [ ] **Hardware Security Modules** (HSM) for key management
- [ ] **Runtime Application Self-Protection** (RASP)
- [ ] **Advanced Threat Detection** and behavioral analysis
- [ ] **Zero Trust Architecture** implementation
- [ ] **Formal Security Verification** of critical components

### Research Areas

- **Post-Quantum Cryptography** for future-proofing
- **Confidential Computing** for data protection
- **AI/ML Security** for threat detection
- **Blockchain-based Provenance** for enhanced trust

## References

- [OWASP Threat Modeling](https://owasp.org/www-community/Threat_Modeling)
- [ISO 27001 Information Security](https://www.iso.org/standard/54534.html)
- [NIST Cybersecurity Framework](https://www.nist.gov/cyberframework)
- [Rust Security Guidelines](https://www.rust-lang.org/static/pdfs/Rust-security.pdf)
- [PostgreSQL Security](https://www.postgresql.org/docs/current/security.html)

---

**Document Control:**
- **Author**: Lionel Hamayon
- **Reviewers**: Project Contributors
- **Review Cycle**: Annual
- **Distribution**: Public