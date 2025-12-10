-- Phase 8A: Persistent Queue Table for 2PC Support
-- Creates pg_tview_pending_refreshes table to persist refresh queues for prepared transactions

-- Table to persist queues for prepared transactions
CREATE TABLE pg_tview_pending_refreshes (
    gid TEXT PRIMARY KEY,  -- Global transaction ID
    refresh_queue JSONB NOT NULL,  -- Serialized queue: [{"entity": "user", "pk": 1}, ...]
    prepared_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    prepared_by TEXT,  -- Session user
    database_name TEXT NOT NULL DEFAULT current_database(),

    -- Metadata for monitoring
    queue_size INT NOT NULL,  -- Number of pending refreshes
    iteration_estimate INT,  -- Estimated propagation iterations

    -- Cleanup
    expires_at TIMESTAMPTZ,  -- Auto-cleanup after N hours

    CONSTRAINT pending_refreshes_gid_check CHECK (gid <> '')
);

CREATE INDEX ON pg_tview_pending_refreshes(prepared_at);
CREATE INDEX ON pg_tview_pending_refreshes(expires_at) WHERE expires_at IS NOT NULL;

-- Auto-cleanup function (called by cron or pg_cron)
CREATE OR REPLACE FUNCTION pg_tviews_cleanup_expired_queues()
RETURNS INT AS $$
DECLARE
    deleted_count INT;
BEGIN
    DELETE FROM pg_tview_pending_refreshes
    WHERE expires_at < now();

    GET DIAGNOSTICS deleted_count = ROW_COUNT;
    RETURN deleted_count;
END;
$$ LANGUAGE plpgsql;

-- Grant permissions for the extension
GRANT SELECT, INSERT, UPDATE, DELETE ON pg_tview_pending_refreshes TO PUBLIC;