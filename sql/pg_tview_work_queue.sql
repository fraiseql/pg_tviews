-- Phase 8E: Work Queue Table for Parallel Refresh
-- Creates pg_tview_work_queue table for distributing refresh tasks across workers

-- Table for parallel refresh work distribution
CREATE TABLE pg_tview_work_queue (
    batch_id BIGSERIAL PRIMARY KEY,
    refresh_keys JSONB NOT NULL,  -- Array of RefreshKey: [{"entity": "user", "pk": 1}, ...]
    status TEXT NOT NULL DEFAULT 'pending',  -- pending, processing, completed, failed
    worker_id INT,
    started_at TIMESTAMPTZ,
    completed_at TIMESTAMPTZ,
    error_message TEXT,

    -- Constraints
    CONSTRAINT work_queue_status_check CHECK (status IN ('pending', 'processing', 'completed', 'failed'))
);

CREATE INDEX ON pg_tview_work_queue(status) WHERE status = 'pending';
CREATE INDEX ON pg_tview_work_queue(status, started_at) WHERE status = 'processing';

-- Grant permissions for the extension
GRANT SELECT, INSERT, UPDATE, DELETE ON pg_tview_work_queue TO PUBLIC;