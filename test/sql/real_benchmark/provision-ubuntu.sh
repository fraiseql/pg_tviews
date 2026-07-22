#!/usr/bin/env bash
# Provision a fresh Ubuntu 24.04 box (as root) to run the real benchmark:
# PostgreSQL 18 (PGDG) + pgrx 0.17.0 + pg_tviews + jsonb_delta 0.3.0.
#
# Expects the pg_tviews working tree already synced to /root/pg_tviews.
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive
PG_MAJOR=18
PGC="/usr/lib/postgresql/${PG_MAJOR}/bin/pg_config"

echo "== apt: base packages =="
apt-get update -qq
apt-get install -y -qq curl git build-essential pkg-config \
    libssl-dev libreadline-dev zlib1g-dev libclang-dev clang postgresql-common >/dev/null

echo "== apt: PostgreSQL ${PG_MAJOR} (PGDG) =="
/usr/share/postgresql-common/pgdg/apt.postgresql.org.sh -y >/dev/null
apt-get install -y -qq "postgresql-${PG_MAJOR}" "postgresql-server-dev-${PG_MAJOR}" >/dev/null

echo "== cluster: trust local + preload pg_tviews =="
HBA="/etc/postgresql/${PG_MAJOR}/main/pg_hba.conf"
sed -i '1i local all all trust\nhost all all 127.0.0.1/32 trust\nhost all all ::1/128 trust' "$HBA"
sudo -u postgres psql -c "ALTER SYSTEM SET shared_preload_libraries='pg_tviews';"

echo "== rust toolchain =="
curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal >/dev/null
# shellcheck disable=SC1091
source "$HOME/.cargo/env"

echo "== cargo-pgrx 0.17.0 =="
cargo install --locked cargo-pgrx --version 0.17.0 >/dev/null
cargo pgrx init "--pg${PG_MAJOR}" "$PGC"

echo "== build + install pg_tviews =="
cd /root/pg_tviews
cargo pgrx install --release --no-default-features --features "pg${PG_MAJOR}" --pg-config "$PGC" --sudo

echo "== build + install jsonb_delta 0.3.0 =="
rm -rf /root/jsonb_delta
git clone --depth 1 --branch v0.3.0 https://github.com/evoludigit/jsonb_delta.git /root/jsonb_delta
cd /root/jsonb_delta
cargo pgrx install --release --pg-config "$PGC" --sudo

echo "== restart cluster =="
pg_ctlcluster "${PG_MAJOR}" main restart || pg_ctlcluster "${PG_MAJOR}" main start
sleep=0

echo "== verify =="
sudo -u postgres psql -c "SHOW shared_preload_libraries;"
sudo -u postgres psql -tAc \
  "SELECT name||' '||default_version FROM pg_available_extensions WHERE name IN ('pg_tviews','jsonb_delta') ORDER BY 1;"
echo "== provision complete =="
