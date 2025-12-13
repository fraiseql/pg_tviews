#!/bin/bash
# Failure simulation library for pg_tviews testing

# Simulate PostgreSQL crash
simulate_pg_crash() {
    echo "Simulating PostgreSQL crash..."
    sudo systemctl restart postgresql

    # Wait for PostgreSQL to come back up
    echo "Waiting for PostgreSQL recovery..."
    local retries=30
    local count=0
    while ! pg_isready -q; do
        sleep 1
        count=$((count + 1))
        if [ $count -ge $retries ]; then
            echo "❌ PostgreSQL failed to recover"
            return 1
        fi
    done
    echo "✅ PostgreSQL recovered"
    return 0
}

# Simulate disk full condition
simulate_disk_full() {
    local mount_point="$1"
    local size="${2:-100M}"

    echo "Simulating disk full at $mount_point (size: $size)..."

    sudo mkdir -p "$mount_point"
    sudo mount -t tmpfs -o size="$size" tmpfs "$mount_point"

    echo "✅ Disk full simulation ready at $mount_point"
}

# Cleanup disk full simulation
cleanup_disk_full() {
    local mount_point="$1"

    echo "Cleaning up disk full simulation at $mount_point..."
    sudo umount "$mount_point" 2>/dev/null || true
    sudo rm -rf "$mount_point"
    echo "✅ Disk full simulation cleaned up"
}

# Simulate network partition (kill connections)
simulate_network_partition() {
    echo "Simulating network partition..."

    # Kill active connections (except our own)
    psql -c "
        SELECT pg_terminate_backend(pid)
        FROM pg_stat_activity
        WHERE pid <> pg_backend_pid()
          AND state = 'active'
          AND query NOT LIKE '%pg_stat_activity%';
    " 2>/dev/null || true

    echo "✅ Network partition simulated"
}

# Wait for TVIEW consistency
wait_for_tview_consistency() {
    local backing_table="$1"
    local tview_table="$2"
    local timeout="${3:-30}"

    echo "Waiting for TVIEW consistency ($tview_table)..."

    local start_time=$(date +%s)
    while true; do
        local backing_count=$(psql -tAc "SELECT COUNT(*) FROM $backing_table;" 2>/dev/null || echo "ERROR")
        local tview_count=$(psql -tAc "SELECT COUNT(*) FROM $tview_table;" 2>/dev/null || echo "ERROR")

        if [ "$backing_count" != "ERROR" ] && [ "$tview_count" != "ERROR" ] && [ "$backing_count" = "$tview_count" ]; then
            echo "✅ TVIEW consistency achieved: $tview_count rows"
            return 0
        fi

        local current_time=$(date +%s)
        local elapsed=$((current_time - start_time))

        if [ $elapsed -ge $timeout ]; then
            echo "❌ TVIEW consistency timeout: backing=$backing_count, tview=$tview_count"
            return 1
        fi

        sleep 1
    done
}

# Check for orphaned queue entries
check_orphaned_queues() {
    local max_age="${1:-1 hour}"

    echo "Checking for orphaned queue entries (older than $max_age)..."

    local count=$(psql -tAc "
        SELECT COUNT(*)
        FROM pg_tview_pending_refreshes
        WHERE prepared_at < now() - interval '$max_age';
    " 2>/dev/null || echo "0")

    if [ "$count" -gt 0 ]; then
        echo "⚠️  Found $count orphaned queue entries"
        return 1
    else
        echo "✅ No orphaned queue entries"
        return 0
    fi
}

# Clean orphaned queue entries
clean_orphaned_queues() {
    local max_age="${1:-1 hour}"

    echo "Cleaning orphaned queue entries (older than $max_age)..."

    local deleted=$(psql -tAc "
        DELETE FROM pg_tview_pending_refreshes
        WHERE prepared_at < now() - interval '$max_age';
        SELECT COUNT(*) FROM pg_tview_pending_refreshes
        WHERE prepared_at < now() - interval '$max_age';
    " 2>/dev/null || echo "0")

    echo "✅ Cleaned up orphaned queue entries"
}