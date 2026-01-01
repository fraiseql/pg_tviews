#!/bin/bash
# Phase 4 Development Environment Setup Script

set -e

echo "=========================================="
echo "Phase 4 Development Environment Setup"
echo "=========================================="
echo ""

# Colors for output
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m' # No Color

# Check if we're in the right directory
if [ ! -f "Cargo.toml" ]; then
    echo -e "${RED}Error: Must run from pg_tviews root directory${NC}"
    exit 1
fi

echo -e "${YELLOW}Step 1: Checking Rust toolchain${NC}"
if ! command -v rustc &> /dev/null; then
    echo -e "${RED}Rust not found. Please install Rust first.${NC}"
    exit 1
fi
echo -e "${GREEN}✓ Rust toolchain found: $(rustc --version)${NC}"
echo ""

echo -e "${YELLOW}Step 2: Checking cargo-pgrx${NC}"
if ! command -v cargo-pgrx &> /dev/null; then
    echo -e "${YELLOW}Installing cargo-pgrx...${NC}"
    cargo install --locked cargo-pgrx
fi
echo -e "${GREEN}✓ cargo-pgrx installed${NC}"
echo ""

echo -e "${YELLOW}Step 3: Checking pgrx initialization${NC}"
if [ ! -d "$HOME/.pgrx" ]; then
    echo -e "${YELLOW}Initializing pgrx (this may take a while)...${NC}"
    cargo pgrx init
else
    echo -e "${GREEN}✓ pgrx already initialized${NC}"
fi
echo ""

echo -e "${YELLOW}Step 4: Creating Phase 4 directory structure${NC}"
mkdir -p test/sql
mkdir -p test/expected
mkdir -p docs
mkdir -p scripts
echo -e "${GREEN}✓ Directory structure created${NC}"
echo ""

echo -e "${YELLOW}Step 5: Checking dependencies${NC}"
echo "Checking for required extensions..."

# Check PostgreSQL version
PG_VERSION=$(cargo pgrx status 2>/dev/null | grep "is available" | head -1 | grep -oP 'pg\d+' || echo "")
if [ -z "$PG_VERSION" ]; then
    echo -e "${RED}Warning: Could not detect PostgreSQL version from pgrx${NC}"
    echo -e "${YELLOW}You may need to run: cargo pgrx init${NC}"
fi
echo ""

echo -e "${YELLOW}Step 6: Building pg_tviews${NC}"
if cargo build 2>&1 | grep -q "Finished"; then
    echo -e "${GREEN}✓ pg_tviews builds successfully${NC}"
else
    echo -e "${RED}Build errors detected. Please fix before continuing.${NC}"
    exit 1
fi
echo ""

echo -e "${YELLOW}Step 7: Checking for jsonb_delta extension${NC}"
echo -e "${YELLOW}Note: jsonb_delta is a dependency for Phase 4${NC}"
echo ""
echo "To install jsonb_delta:"
echo "  1. git clone https://github.com/fraiseql/jsonb_delta.git"
echo "  2. cd jsonb_delta"
echo "  3. cargo pgrx install --release"
echo ""

# Create .phase4-ready marker file
echo -e "${YELLOW}Step 8: Creating development markers${NC}"
cat > .phase4-ready <<EOF
# Phase 4 Development Environment Ready
# Generated: $(date)

Environment checks passed:
- Rust toolchain: OK
- cargo-pgrx: OK
- Directory structure: OK
- pg_tviews builds: OK

Next steps:
1. Install jsonb_delta extension (if not already installed)
2. Review test files in test/sql/40-44
3. Run: cargo pgrx test
4. Start implementing Phase 4 tasks

Test files created:
- test/sql/40_refresh_trigger_dynamic_pk.sql
- test/sql/41_refresh_single_row.sql
- test/sql/42_cascade_fk_lineage.sql
- test/sql/43_cascade_depth_limit.sql
- test/sql/44_trigger_cascade_integration.sql

Documentation:
- test/sql/README_PHASE4.md
- docs/CONCURRENCY.md (to be created)
EOF
echo -e "${GREEN}✓ Created .phase4-ready marker${NC}"
echo ""

echo -e "${GREEN}=========================================="
echo "Phase 4 Environment Setup Complete!"
echo "==========================================${NC}"
echo ""
echo "Next steps:"
echo "  1. Review PHASE_4_PLAN.md"
echo "  2. Run tests: cargo pgrx test"
echo "  3. Start implementing tasks in order (1-6)"
echo ""
echo "Quick commands:"
echo "  - Build: cargo build"
echo "  - Test: cargo pgrx test"
echo "  - Run specific test: cargo pgrx test <test_name>"
echo "  - Install: cargo pgrx install"
echo ""
