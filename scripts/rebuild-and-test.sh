#!/bin/bash
set -e

export PATH="$HOME/.pgrx/17.7/pgrx-install/bin:$PATH"

echo "Building..."
cargo pgrx install --release

echo "Restarting PostgreSQL..."
cargo pgrx stop pg17
cargo pgrx start pg17

echo "Running test..."
psql -h localhost -p 28817 -U lionel -d postgres -f test_task3_cascade.sql 2>&1 | /usr/bin/tail -40
