#!/usr/bin/env bash
# The extension reaches SPI through src/utils/spi.rs only: no other file calls
# pgrx's Spi:: functions or PostgreSQL's SPI_* directly. Comments do not count.
set -u
cd "$(dirname "$0")/.."
found=$(grep -rnE '\bSpi::|\bpg_sys::SPI_' src --include='*.rs' \
  | grep -v '^src/utils/spi.rs:' \
  | grep -vE '^[^:]+:[0-9]+:[[:space:]]*//')
if [[ -n "$found" ]]; then
  echo "SPI used outside src/utils/spi.rs:"
  printf '%s\n' "$found"
  exit 1
fi
echo "SPI is reached through src/utils/spi.rs only"
