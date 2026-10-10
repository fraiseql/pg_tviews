# Development Guide

This guide covers setting up the development environment, running tests, and contributing to pg_tviews.

## Prerequisites

- **Rust**: 1.70+ with rustup
- **PostgreSQL**: 15, 16, or 17
- **pgrx**: 0.12.8+ for PostgreSQL extension development
- **jsonb_delta**: Required extension for JSONB operations

## Environment Setup

### 1. Install Rust

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env
```

The toolchain is pinned in `rust-toolchain.toml` (rustup installs it on first
`cargo` call). Local runs and CI use the same compiler; bump the pin in a
dedicated PR.

### 2. Install PostgreSQL

**Ubuntu/Debian:**
```bash
sudo apt-get update
sudo apt-get install postgresql-17 postgresql-server-dev-17
```

**macOS (Homebrew):**
```bash
brew install postgresql@17
```

**Arch Linux:**
```bash
sudo pacman -S postgresql
```

### 3. Install pgrx

```bash
cargo install --locked cargo-pgrx
```

### 4. Initialize pgrx

```bash
# Initialize with your PostgreSQL version
cargo pgrx init

# Or specify a specific version
cargo pgrx init --pg17 /usr/lib/postgresql/17/bin/pg_config
```

### 5. Install jsonb_delta

```bash
# Clone and build jsonb_delta
git clone https://github.com/evoludigit/jsonb_delta.git
cd jsonb_delta
make && sudo make install
```

### 6. Install SBOM Tools (Optional)

For generating Software Bill of Materials (SBOM) in compliance with international standards:

```bash
# SBOM generation for Rust (SPDX format)
cargo install cargo-sbom

# CycloneDX generator (CycloneDX format)
cargo install cargo-cyclonedx

# Optional: Validation and scanning tools
npm install -g @cyclonedx/cyclonedx-cli  # CycloneDX validation
pip install spdx-tools                    # SPDX validation

# Container and filesystem vulnerability scanning
# Trivy is used in CI/CD workflows for automated scanning
```

**SBOM Standards Compliance:**
- **SPDX 2.3**: ISO/IEC 5962:2021 (International standard)
- **CycloneDX 1.5**: OWASP security-focused format
- **NTIA Minimum Elements**: US Federal requirements
- **EU Cyber Resilience Act**: European requirements
- **PCI-DSS 4.0**: Payment card industry requirements

### 7. Install Signing Tools (For Releases)

For cryptographic signing of release artifacts:

```bash
# Sigstore Cosign (keyless signing)
# macOS
brew install cosign

# Linux
wget "https://github.com/sigstore/cosign/releases/download/v2.2.2/cosign-linux-amd64"
sudo mv cosign-linux-amd64 /usr/local/bin/cosign
sudo chmod +x /usr/local/bin/cosign

# GPG (traditional signing)
# Usually pre-installed on Linux/macOS
gpg --version

# GitHub CLI (for attestations)
# macOS
brew install gh

# Ubuntu/Debian
curl -fsSL https://cli.github.com/packages/githubcli-archive-keyring.gpg | sudo dd of=/usr/share/keyrings/githubcli-archive-keyring.gpg
echo "deb [arch=$(dpkg --print-architecture) signed-by=/usr/share/keyrings/githubcli-archive-keyring.gpg] https://cli.github.com/packages stable main" | sudo tee /etc/apt/sources.list.d/github-cli.list > /dev/null
sudo apt update && sudo apt install gh
```

**Signing Standards Compliance:**
- **Sigstore**: Keyless signing with transparency logs
- **GPG**: OpenPGP standard for maintainer signatures
- **SLSA Level 3**: Supply chain provenance
- **ISO 27001**: Cryptographic signing requirements

## Building

### Development Build

```bash
# Build the extension
cargo pgrx install

# Build with release optimizations
cargo pgrx install --release
```

### Testing Build

```bash
# Run all tests for PostgreSQL 17
cargo pgrx test pg17

# Run tests for every supported version (16, 17, 18)
cargo pgrx test pg16
cargo pgrx test pg17
cargo pgrx test pg18
```

## Testing

[docs/development/testing.md](development/testing.md) describes the suites, how
to run them and how to write a test.

## Debugging

### Logging

Use pgrx logging macros:

```rust
use pgrx::prelude::*;

info!("Info message: {}", value);
debug!("Debug message: {:?}", data);
warning!("Warning message");
error!("Error message: {}", err);
```

### PostgreSQL Logs

Check PostgreSQL logs for extension errors:

```bash
# View PostgreSQL logs
tail -f /var/log/postgresql/postgresql-17-main.log

# Or check systemd logs
journalctl -u postgresql -f
```

### SPI Debugging

Debug SPI queries:

```rust
// Log the query before execution
info!("Executing query: {}", query);

// Execute and check result
let result = Spi::get_one::<String>(&query);
info!("Query result: {:?}", result);
```

## Code Organization

```
src/
├── lib.rs              # Extension entry point
├── install_sql.rs      # Install SQL: catalog tables, views, triggers
├── catalog/            # pg_tview_meta rows and their propagation plan (plan.rs)
├── lineage/            # Analysis of a backing view's query tree (walk/ reads the nodes)
├── ddl/                # pg_tviews_create / create_or_replace / drop / rename
├── trigger.rs, delta.rs  # Row and statement triggers: writes to TVIEW keys
├── queue/              # The transaction's pending refresh work
├── flush/              # Applies it: dependency order, patches, propagation
├── refresh/            # Row and bulk refreshes, direct and fan-out patches
├── hooks/              # ProcessUtility hook: tv_* DDL, COMMIT flush, follow-ups
├── cache/              # Per-backend caches and their invalidation
└── utils/              # SPI helpers and shared utilities

test/
├── sql/                # Regression and integration suites
└── upgrade/            # Upgrade-path check

.github/
└── workflows/          # CI/CD configuration
```

## Development Workflow

### 1. Write Tests First (RED)

```bash
# Create failing tests
cargo test --lib  # Should fail initially
```

### 3. Implement Code (GREEN)

```bash
# Implement minimal code to pass tests
cargo test --lib  # Should pass now
```

### 4. Refactor (REFACTOR)

```bash
# Improve code quality while maintaining tests
cargo test --lib
cargo pgrx test pg17
```

### 5. Integration Test (QA)

```bash
# Run full test suite
cargo pgrx test pg17
psql -d test_db -f test/sql/*.sql
```

## Contributing

### Commit Messages

Follow conventional commit format:

```bash
feat(error): add TViewError enum with SQLSTATE mapping
fix(deps): correct pg_depend query direction
test(refresh): add cascade propagation tests
docs(readme): update installation instructions
```

### Pull Requests

1. Create a feature branch from `develop`
2. Implement changes with tests
3. Ensure CI passes
4. Update documentation if needed
5. Request review

### Code Style

- Use `rustfmt` for formatting: `cargo fmt`
- Use `clippy` for linting: `cargo clippy`
- Follow Rust naming conventions
- Add documentation comments to public APIs
- Use `TViewResult<T>` for all fallible operations

## Troubleshooting

### Common Issues

**pgrx init fails:**
```bash
# Check PostgreSQL is installed and running
pg_config --version
sudo systemctl status postgresql

# Try specifying pg_config path explicitly
cargo pgrx init --pg17 /usr/lib/postgresql/17/bin/pg_config
```

**Extension fails to load:**
```bash
# Check PostgreSQL logs
tail -f /var/log/postgresql/postgresql-17-main.log

# Verify jsonb_delta is installed
psql -c "SELECT * FROM pg_extension WHERE extname = 'jsonb_delta';"
```

**Tests fail:**
```bash
# Clean and rebuild
cargo clean
cargo pgrx install --release

# Check test database setup
psql -d pg_tviews_test -c "SELECT version();"
```

### Getting Help

- Check existing issues on GitHub
- Look at pgrx documentation: https://github.com/pgcentralfoundation/pgrx
- PostgreSQL extension development: https://www.postgresql.org/docs/17/extend.html