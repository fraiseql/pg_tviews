#!/bin/bash
# Recovery verification library for pg_tviews testing

# Verify TVIEW integrity after failure
verify_tview_integrity() {
    local entity_name="$1"

    echo "Verifying TVIEW integrity for $entity_name..."

    # Check if TVIEW exists
    local tview_exists=$(psql -tAc "
        SELECT COUNT(*) FROM information_schema.tables
        WHERE table_name = 'tv_$entity_name';
    ")

    if [ "$tview_exists" -eq 0 ]; then
        echo "❌ TVIEW tv_$entity_name does not exist"
        return 1
    fi

    # Check if backing table exists
    local backing_exists=$(psql -tAc "
        SELECT COUNT(*) FROM information_schema.tables
        WHERE table_name = 'tb_$entity_name';
    ")

    if [ "$backing_exists" -eq 0 ]; then
        echo "❌ Backing table tb_$entity_name does not exist"
        return 1
    fi

    # Check metadata exists
    local metadata_exists=$(psql -tAc "
        SELECT COUNT(*) FROM pg_tviews_metadata
        WHERE entity_name = '$entity_name';
    ")

    if [ "$metadata_exists" -eq 0 ]; then
        echo "❌ Metadata missing for $entity_name"
        return 1
    fi

    echo "✅ TVIEW integrity verified"
    return 0
}

# Verify TVIEW refresh capability
verify_tview_refresh() {
    local entity_name="$1"

    echo "Verifying TVIEW refresh capability for $entity_name..."

    # Insert test data
    local test_value="refresh-test-$(date +%s)"
    psql -c "INSERT INTO tb_$entity_name (data) VALUES ('$test_value');" 2>/dev/null

    if [ $? -ne 0 ]; then
        echo "❌ Failed to insert test data"
        return 1
    fi

    # Check if TVIEW was updated
    local count=$(psql -tAc "SELECT COUNT(*) FROM tv_$entity_name WHERE data = '$test_value';")

    if [ "$count" -eq 1 ]; then
        echo "✅ TVIEW refresh working"
        return 0
    else
        echo "❌ TVIEW not refreshed after insert"
        return 1
    fi
}

# Verify queue is clean
verify_queue_clean() {
    echo "Verifying queue is clean..."

    local queue_size=$(psql -tAc "SELECT jsonb_array_length(pg_tviews_debug_queue());" 2>/dev/null || echo "ERROR")

    if [ "$queue_size" = "ERROR" ]; then
        echo "❌ Failed to check queue"
        return 1
    fi

    if [ "$queue_size" -eq 0 ]; then
        echo "✅ Queue is clean"
        return 0
    else
        echo "⚠️  Queue has $queue_size pending entries"
        return 1
    fi
}

# Verify extension functionality
verify_extension_functionality() {
    echo "Verifying extension functionality..."

    # Test version function
    local version=$(psql -tAc "SELECT pg_tviews_version();" 2>/dev/null || echo "ERROR")

    if [ "$version" = "ERROR" ] || [ -z "$version" ]; then
        echo "❌ Version function not working"
        return 1
    fi

    # Test health check
    local health=$(psql -tAc "SELECT COUNT(*) FROM pg_tviews_health_check();" 2>/dev/null || echo "ERROR")

    if [ "$health" = "ERROR" ] || [ "$health" -eq 0 ]; then
        echo "❌ Health check not working"
        return 1
    fi

    echo "✅ Extension functionality verified"
    return 0
}

# Comprehensive recovery verification
verify_full_recovery() {
    local entity_name="$1"

    echo "Running comprehensive recovery verification..."

    verify_extension_functionality || return 1
    verify_tview_integrity "$entity_name" || return 1
    verify_tview_refresh "$entity_name" || return 1
    verify_queue_clean || return 1

    echo "✅ Full recovery verification passed"
    return 0
}

# Generate failure report
generate_failure_report() {
    local test_name="$1"
    local result="$2"
    local details="$3"

    echo "=== FAILURE REPORT ==="
    echo "Test: $test_name"
    echo "Result: $result"
    echo "Details: $details"
    echo "Timestamp: $(date)"
    echo "PostgreSQL Version: $(psql --version)"
    echo "pg_tviews Version: $(psql -tAc 'SELECT pg_tviews_version();' 2>/dev/null || echo 'UNKNOWN')"
    echo "===================="
}