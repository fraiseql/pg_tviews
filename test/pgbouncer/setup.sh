#!/bin/bash
set -euo pipefail

echo "Setting up PgBouncer for testing..."

# Install PgBouncer if needed
if ! command -v pgbouncer &> /dev/null; then
    sudo apt-get install -y pgbouncer
fi

# Copy config
sudo cp pgbouncer.ini /etc/pgbouncer/
sudo chown postgres:postgres /etc/pgbouncer/pgbouncer.ini

# Create userlist
echo '"postgres" "trust"' | sudo tee /etc/pgbouncer/userlist.txt

# Start PgBouncer
sudo systemctl restart pgbouncer

# Verify
psql -h localhost -p 6432 -c "SELECT 1;" && echo "✅ PgBouncer running"