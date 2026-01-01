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
git clone https://github.com/fraiseql/jsonb_delta.git
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

# Run tests for multiple versions
cargo pgrx test pg15
cargo pgrx test pg16
cargo pgrx test pg17
```

## Testing

### Test Types

1. **Rust Unit Tests**: Test individual functions without PostgreSQL
2. **pgrx Integration Tests**: Test with PostgreSQL using `#[pg_test]`
3. **SQL Integration Tests**: Test complete workflows with SQL files

### Running Tests

```bash
# Run only Rust unit tests (no PostgreSQL required)
cargo test --lib

# Run pgrx integration tests (requires PostgreSQL)
cargo pgrx test pg17

# Run specific test
cargo pgrx test pg17 -- --test test_metadata_tables_creation

# Run SQL integration tests manually
psql -d test_db -f test/sql/00_extension_loading.sql
```

### Writing Tests

#### Rust Unit Tests

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_my_function() {
        assert_eq!(my_function(2), 4);
    }
}
```

#### pgrx Integration Tests

```rust
#[cfg(any(test, feature = "pg_test"))]
#[pg_schema]
mod tests {
    use pgrx::prelude::*;

    #[pg_test]
    fn test_with_postgres() {
        let result = Spi::get_one::<i32>("SELECT 1 + 1");
        assert_eq!(result, Ok(Some(2)));
    }
}
```

#### SQL Integration Tests

Create files in `test/sql/` with SQL commands and expected output.

#### Test Feature Configuration

**Important**: All `#[pg_test]` functions must be conditional on the `pg_test` feature:

```rust
#[cfg(any(test, feature = "pg_test"))]
#[pg_test]
fn test_my_function() {
    // Test code here
}
```

This allows the code to compile with `--no-default-features` for CI/CD pipelines that don't need PostgreSQL integration tests.

```bash
# Compile without PostgreSQL integration tests
cargo check --no-default-features --features pg17

# Run with full test suite
cargo pgrx test pg17
```

### Test Database Setup

```bash
# Create test database
createdb pg_tviews_test

# Enable required extensions
psql -d pg_tviews_test -c "CREATE EXTENSION jsonb_delta;"

# Install pg_tviews
cargo pgrx install --release
psql -d pg_tviews_test -c "CREATE EXTENSION pg_tviews;"
```

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
├── lib.rs              # Extension entry point and exports
├── error/              # Error types and testing utilities
├── metadata.rs         # Metadata table management
├── catalog.rs          # PostgreSQL catalog queries
├── trigger.rs          # Trigger installation logic
├── refresh.rs          # Incremental refresh implementation
├── propagate.rs        # Cascade propagation logic
└── utils.rs            # Shared utility functions

test/
└── sql/                # SQL integration tests

.github/
└── workflows/          # CI/CD configuration
```

## Development Workflow

### 1. Choose a Phase

Follow the implementation plan in `.phases/implementation/`.

### 2. Write Tests First (RED)

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
- Review the implementation plan in `.phases/implementation/`
- Look at pgrx documentation: https://github.com/pgcentralfoundation/pgrx
- PostgreSQL extension development: https://www.postgresql.org/docs/17/extend.html