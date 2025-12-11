#!/bin/bash
# Extract system dependencies for SBOM

echo "System Dependencies:"
echo "===================="

# PostgreSQL version
PG_VERSION=$(pg_config --version 2>/dev/null || echo "PostgreSQL (runtime)")
echo "- ${PG_VERSION}"

# pgrx version
PGRX_VERSION=$(cargo pgrx --version 2>/dev/null || echo "pgrx 0.12.8")
echo "- ${PGRX_VERSION}"

# System libraries (from ldd on compiled .so)
if [ -f "target/release/libpg_tviews.so" ]; then
    echo ""
    echo "Linked System Libraries:"
    ldd target/release/libpg_tviews.so | grep -E "(libc|libpq|libssl)" | awk '{print "- " $1 " " $3}'
fi

# OS information
echo ""
echo "Build Environment:"
echo "- OS: $(uname -s) $(uname -r)"
echo "- Arch: $(uname -m)"
echo "- Rust: $(rustc --version)"