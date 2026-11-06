# Security Documentation

Welcome to pg_tviews security documentation. This hub provides comprehensive information about our security practices, compliance posture, and operational security measures.

## 🏗️ Security Architecture

### [Security Architecture Overview](architecture.md)
Complete threat model, security boundaries, controls, and assumptions that form the foundation of pg_tviews security.

**Key Topics:**
- Threat modeling and risk assessment
- Security boundaries and trust levels
- Security controls and countermeasures
- Secure coding practices and standards

### [Compliance & Standards](compliance.md)
Our compliance posture across international standards and regulatory requirements.

**Standards Covered:**
- SLSA Level 3, SBOM (SPDX/CycloneDX)
- EU Cyber Resilience Act, NIS2 Directive
- US EO 14028, NIST Cybersecurity Framework
- ISO 27001, PCI-DSS 4.0, SOC 2

## 🔐 Supply Chain Security

### [Software Bill of Materials (SBOM)](sbom.md)
Comprehensive SBOM generation and verification for complete software transparency.

**Features:**
- SPDX 2.3 and CycloneDX 1.5 formats
- Automated generation in CI/CD
- International compliance (NTIA, EU CRA)
- Verification instructions and tools

### [Build Provenance](provenance.md)
SLSA Level 3 build provenance ensuring build integrity and supply chain security.

**Capabilities:**
- Cryptographic build attestation
- Reproducible build verification
- Supply chain attack prevention
- Automated provenance generation

### [Dependency Management](dependency-policy.md)
Comprehensive dependency security policy and automated management.

**Security Measures:**
- cargo-audit vulnerability scanning
- cargo-deny license, source and advisory checks
- Automated dependency updates
- Security patch prioritization

## 🚨 Incident Response & Security Operations

### [Security Policy](../../SECURITY.md)
Vulnerability reporting process and security commitments.

**What's Covered:**
- Supported versions and maintenance
- Private vulnerability reporting
- Response timelines and coordination
- Security update procedures

### [Incident Response Plan](incident-response.md)
Comprehensive procedures for handling security incidents.

**Response Framework:**
- Incident classification and severity levels
- Response team structure and roles
- Communication protocols and templates
- Recovery procedures and lessons learned

## 🛠️ Development & Review Processes

### [Security Review Process](../development/security-review.md)
Requirements and procedures for security code reviews.

**Review Requirements:**
- When security review is mandatory
- Comprehensive security checklist
- Approval workflows and escalation
- Testing and documentation requirements

### [Reproducible Builds](../development/reproducible-builds.md)
Guide for building pg_tviews from source with verification.

**Build Security:**
- Docker-based reproducible environments
- Build metadata and checksums
- Verification against official releases
- Troubleshooting common issues

## 🔑 Cryptographic Security

### [Artifact Signing](signing.md)
Cryptographic signing of all release artifacts.

**Signing Methods:**
- GPG maintainer signatures
- Sigstore keyless signing
- SHA256/SHA512 checksums
- Multiple verification methods

### [Release Verification](verify-release.md)
Step-by-step guide for verifying release integrity.

**Verification Options:**
- Sigstore keyless verification
- GPG signature verification
- Checksum validation
- Automated verification scripts

### [Maintainer Keys](maintainer-keys.md)
GPG key management and verification procedures.

**Key Information:**
- Current maintainer keys
- Key verification procedures
- Key rotation policies
- Trust model documentation

## 📊 Security Monitoring & Compliance

### Automated Security Scanning
- **Daily Vulnerability Scans**: cargo-audit runs automatically
- **Dependency Audits**: cargo-deny license, source and advisory checks
- **Container Scanning**: Trivy scans container images for vulnerabilities
- **Filesystem Scanning**: Trivy scans codebase for security issues
- **SBOM Vulnerability Analysis**: Trivy analyzes SBOMs for known CVEs
- **CI/CD Security Gates**: Automated security checks on all PRs
- **Release Security**: All artifacts cryptographically signed

### Compliance Monitoring
- **Regulatory Compliance**: Continuous monitoring of requirements
- **Standards Adherence**: Automated compliance validation
- **Audit Preparation**: Documentation and evidence collection
- **Security Metrics**: Key performance indicators tracking

## 🎯 Quick Start for Different Roles

### For Users & Operators
1. **[Verify Releases](verify-release.md)** - Ensure download integrity
2. **[SBOM Access](sbom.md)** - Review software components
3. **[Security Updates](../../SECURITY.md)** - Stay informed of vulnerabilities

### For Developers & Contributors
1. **[Security Review Process](../development/security-review.md)** - Code contribution requirements
2. **[Reproducible Builds](../development/reproducible-builds.md)** - Build verification
3. **[Architecture Overview](architecture.md)** - Security design principles

### For Security Researchers
1. **[Security Policy](../../SECURITY.md)** - Vulnerability reporting
2. **[Incident Response](incident-response.md)** - Response procedures
3. **[Architecture](architecture.md)** - Technical security details

### For Procurement & Compliance Teams
1. **[Compliance Overview](compliance.md)** - Standards and regulations
2. **[SBOM Documentation](sbom.md)** - Software transparency
3. **[Provenance](provenance.md)** - Build integrity assurance

## 📞 Contact & Support

### Security Issues
- **Vulnerability Reports**: [GitHub Security Advisories](https://github.com/fraiseql/pg_tviews/security/advisories)
- **Private Contact**: security@your-domain.com
- **PGP Key**: [Maintainer Keys](maintainer-keys.md)

### General Support
- **Issues**: [GitHub Issues](https://github.com/fraiseql/pg_tviews/issues)
- **Discussions**: [GitHub Discussions](https://github.com/fraiseql/pg_tviews/discussions)
- **Documentation**: [Main Documentation](../../README.md)

## Reviews

### 0.1.0-beta.34 review (2026-10)

A review of every path by which one role's SQL can run with another role's privileges,
against 0.1.0-beta.34 before its release. All findings are fixed:

| Finding | Fix | Since |
|---|---|---|
| The write path read the catalog under the writer's `search_path`: a function or operator the writer planted could run as a TVIEW's owner | Every entry point pins `search_path = pg_catalog, pg_temp` (`owner::CatalogPath`); names the caller wrote resolve under the caller's path, before the pin (`owner::CallerPath`) | 0.1.0-beta.34 |
| The rebuild worker ran under a database's `search_path` setting | The worker pins `pg_catalog, pg_temp` | 0.1.0-beta.34 |
| Catalog SQL compared values (an `oid` with a `regclass`) for which an exact operator in any schema on the path wins over pg_catalog's | Covered by the pin above; `scripts/check-name-resolution.sh` checks that every entry point pins it; `regress_caller_search_path.sql`, `regress_hostile_search_path.sql` | 0.1.0-beta.34, 0.1.0-beta.36 |
| The change feed's parent ids were read as the writer | Read as the TVIEW's owner | 0.1.0-beta.34 |
| Any role could attach pg_tviews' trigger functions to its own table and make them work on TVIEWs (#269) | The triggers take no argument and find their TVIEWs from the firing table: on a table no TVIEW reads they do nothing | 0.1.0-beta.35 |
| The change report listed ids of TVIEWs the caller may not read, and failed 42501 with data (#270) | Rows of an unreadable TVIEW are left out and its type listed in `omitted_types` | 0.1.0-beta.36 |
| Hardening (#271): the name check missed forms of a name resolved to an object; workflow actions pinned by tag; two audit documents out of date | The check reads whole SQL literals and flags every form (fixture in CI); actions pinned by commit, tools by version; the stale documents removed | 0.1.0-beta.36 |

## 🔄 Continuous Improvement

### Security Program Evolution
- **Annual Reviews**: Complete security assessment and updates
- **Threat Intelligence**: Monitoring emerging security threats
- **Technology Updates**: Adoption of new security tools and practices
- **Training Programs**: Ongoing security awareness and training

### Future Enhancements
- Enhanced SBOM metadata and formats
- Advanced threat detection capabilities
- Post-quantum cryptography preparation
- Enhanced supply chain monitoring

## 📚 Additional Resources

### External References
- [SLSA Framework](https://slsa.dev/) - Supply chain security
- [OWASP](https://owasp.org/) - Web application security
- [Rust Security](https://www.rust-lang.org/static/pdfs/Rust-security.pdf) - Language security
- [PostgreSQL Security](https://www.postgresql.org/docs/current/security.html) - Database security

### Internal Documentation
- [Main README](../../README.md) - Project overview
- [Development Guide](../development/) - Contributor information
- [Operations Guide](../operations/) - Deployment and maintenance

---

**Document Control:**
- **Author**: Lionel Hamayon
- **Reviewers**: Project Contributors
- **Review Cycle**: Annual
- **Distribution**: Public