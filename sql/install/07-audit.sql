-- Audit logging table for DDL operations
CREATE TABLE @extschema@.pg_tview_audit_log (
    log_id BIGSERIAL PRIMARY KEY,
    operation TEXT NOT NULL,  -- CREATE, DROP, REFRESH
    entity TEXT NOT NULL,
    performed_by TEXT NOT NULL DEFAULT current_user,
    performed_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    transaction_id BIGINT DEFAULT pg_current_xact_id()::text::bigint,
    rows_affected BIGINT,
    details JSONB,
    client_addr INET DEFAULT inet_client_addr(),
    client_port INTEGER DEFAULT inet_client_port()
);

CREATE INDEX idx_audit_log_entity_time ON @extschema@.pg_tview_audit_log(entity, performed_at);

COMMENT ON TABLE @extschema@.pg_tview_audit_log IS 'Audit log for TVIEW operations';

-- Writes buffered audit entries. Only the extension owner may call it:
-- the library calls it as that owner, for whichever role triggered the entries, and
-- performed_by is the session user, whatever role the caller has set.
CREATE FUNCTION @extschema@.pg_tviews_audit_write(entries JSONB)
RETURNS void
LANGUAGE sql
SET search_path = pg_catalog, @extschema@, pg_temp
AS $$
    INSERT INTO @extschema@.pg_tview_audit_log
        (operation, entity, performed_by, rows_affected, details)
    SELECT e->>'op', e->>'entity', SESSION_USER, (e->>'rows')::bigint,
           CASE WHEN e->'details' = 'null'::jsonb THEN NULL ELSE e->'details' END
    FROM jsonb_array_elements(entries) AS e;
$$;
REVOKE EXECUTE ON FUNCTION @extschema@.pg_tviews_audit_write(JSONB) FROM PUBLIC;
