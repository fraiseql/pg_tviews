# Documentation

Welcome to pg_tviews documentation! This guide will help you understand and use pg_tviews, the transactional materialized view extension for PostgreSQL that powers FraiseQL's GraphQL Cascade.

## 📖 Documentation Overview

pg_tviews is a PostgreSQL extension that provides automatic incremental refresh of materialized views. It's designed as core infrastructure for the FraiseQL framework, enabling real-time GraphQL Cascade with 5,000-12,000× performance improvements over traditional materialized views.

### 🗺️ User Journeys

Choose your path based on your role:

#### 👩‍💻 **I'm a FraiseQL Developer**
Want to integrate pg_tviews into your FraiseQL application?

1. **[Quick Start](getting-started/quickstart.md)** - Get running in 10 minutes
2. **[FraiseQL Integration](getting-started/fraiseql-integration.md)** - Framework patterns and best practices
3. **[Developer Guide](user-guides/developers.md)** - Application integration patterns
4. **[API Reference](reference/api.md)** - Function reference for development

#### 🏗️ **I'm a System Architect**
Need to design CQRS systems with pg_tviews?

1. **[Architect Guide](user-guides/architects.md)** - CQRS design patterns and decisions
2. **[Running Benchmarks](benchmarks/running-benchmarks.md)** - Execute performance tests
3. **[Performance Analysis](benchmarks/overview.md)** - Scaling characteristics and limits
4. **[Architecture](../architecture.md)** - Technical implementation details
5. **[Configuration Reference](../README.md#configuration)** - GUC settings

#### 🛠️ **I'm a Database Operator**
Responsible for production deployment and monitoring?

1. **[Installation](getting-started/installation.md)** - Production setup guide
2. **[Operator Guide](user-guides/operators.md)** - Production deployment and management
3. **[Monitoring](operations/monitoring.md)** - Health checks and metrics
4. **[Troubleshooting](operations/troubleshooting.md)** - Common issues and solutions

#### 🧪 **I'm a Developer/Contributor**
Want to contribute to pg_tviews development?

1. **[Development](development.md)** - Development setup
2. **[Testing](development/testing.md)** - Testing patterns and procedures
3. **[Architecture](../architecture.md)** - Code structure and design

## 📚 Documentation Sections

### Getting Started
Essential guides for new users:

- **[Quick Start](getting-started/quickstart.md)** - Step-by-step setup and first TVIEW
- **[Installation](getting-started/installation.md)** - Detailed installation for different environments
- **[FraiseQL Integration](getting-started/fraiseql-integration.md)** - Framework integration patterns

### User Guides
Role-specific guidance:

- **[For Developers](user-guides/developers.md)** - Application integration and API usage
- **[For Operators](user-guides/operators.md)** - Production deployment and operations
- **[For Architects](user-guides/architects.md)** - CQRS design patterns and architecture decisions

### Reference Documentation
Technical reference materials:

- **[API Reference](reference/api.md)** - Complete function reference with examples
- **[DDL Reference](reference/ddl.md)** - CREATE TABLE tv_* / DROP TABLE tv_* syntax and options
- **[Error Reference](error-reference.md)** - Error types, causes, and solutions
- **[Configuration](../README.md#configuration)** - GUC settings

### Operations
Production operations and maintenance:

- **[Monitoring](operations/monitoring.md)** - Health checks, metrics, and alerting
- **[Troubleshooting](operations/troubleshooting.md)** - Debugging procedures and common issues
- **[Performance Tuning](operations/performance-tuning.md)** - Optimization strategies and best practices

### Benchmarks
Performance testing and validation:

- **[Running Benchmarks](benchmarks/running-benchmarks.md)** - Step-by-step guide to execute the harness
- **[Overview](benchmarks/overview.md)** - Benchmark methodology and test scenarios
- **[Results](benchmarks/results.md)** - Measured performance data and analysis
- **[Results Interpretation](benchmarks/results-interpretation.md)** - Reading the numbers honestly
- **[jsonb_delta Integration](benchmarks/jsonb-ivm-integration.md)** - jsonb_delta's role and the parity finding

### Development
For contributors and advanced users:

- **[Development](development.md)** - Development setup and coding standards
- **[Testing](development/testing.md)** - Testing patterns, procedures, and quality assurance
- **[Architecture](../architecture.md)** - Technical architecture and implementation details

## 🔗 Quick Links

### External Resources
- **FraiseQL Framework**: [github.com/fraiseql/fraiseql](https://github.com/fraiseql/fraiseql)
- **PostgreSQL Documentation**: [postgresql.org/docs](https://www.postgresql.org/docs/)
- **pgrx Framework**: [github.com/pgcentralfoundation/pgrx](https://github.com/pgcentralfoundation/pgrx)

### Related Files
- **[CHANGELOG](https://github.com/fraiseql/pg_tviews/blob/main/CHANGELOG.md)** - Version history and release notes
- **[Architecture](../architecture.md)** - High-level system architecture
- **[Development](development.md)** - Development environment setup

## 📞 Support & Community

### Getting Help
- **Issues**: [GitHub Issues](https://github.com/fraiseql/pg_tviews/issues) for bug reports and feature requests
- **Discussions**: [GitHub Discussions](https://github.com/fraiseql/pg_tviews/discussions) for questions and community support
- **FraiseQL Community**: Connect with other FraiseQL users for integration questions

### Contributing
We welcome contributions! See [the development guide](development.md) to get started.

---

## 📋 Documentation Status

| Section | Status | Completion |
|---------|--------|------------|
| Getting Started | ✅ Complete | Installation, quickstart, fraiseql integration |
| User Guides | ✅ Complete | Architects, developers, operators guides |
| Reference | ✅ Complete | API, DDL, error references |
| Operations | 🟡 In Progress | Monitoring, troubleshooting, performance tuning |
| Benchmarks | ✅ Complete | Performance results, benchmarking methodology |
| Development | 🟡 In Progress | Architecture docs, contribution guidelines |

**Legend**: ✅ Complete 🟡 In Progress 🟠 Planned 🔴 Missing

**Current Focus**: A+ documentation quality improvement

---

*For release notes and upgrade steps, see the [CHANGELOG](../CHANGELOG.md).*"