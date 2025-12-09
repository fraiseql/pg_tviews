-- pg_tviews extension SQL script
-- This file is loaded when CREATE EXTENSION pg_tviews is executed

-- Create metadata tables
CREATE TABLE IF NOT EXISTS public.pg_tview_meta (
    entity TEXT NOT NULL PRIMARY KEY,
    view_oid OID NOT NULL,
    table_oid OID NOT NULL,
    definition TEXT NOT NULL,
    dependencies OID[] NOT NULL DEFAULT '{}',
    fk_columns TEXT[] NOT NULL DEFAULT '{}',
    uuid_fk_columns TEXT[] NOT NULL DEFAULT '{}',
    dependency_types TEXT[] NOT NULL DEFAULT '{}',
    dependency_paths TEXT[][] NOT NULL DEFAULT '{}',
    array_match_keys TEXT[] NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS public.pg_tview_helpers (
    helper_name TEXT NOT NULL PRIMARY KEY,
    is_helper BOOLEAN NOT NULL DEFAULT TRUE,
    used_by TEXT[] NOT NULL DEFAULT '{}',
    depends_on TEXT[] NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

COMMENT ON TABLE public.pg_tview_meta IS 'Metadata for TVIEW materialized tables';
COMMENT ON TABLE public.pg_tview_helpers IS 'Tracks helper views used by TVIEWs';

-- Register SQL functions from .so library
-- These are exported by the pg_tviews extension
CREATE FUNCTION pg_tviews_create(
    tview_name text,
    select_sql text
) RETURNS text
LANGUAGE c
AS 'MODULE_PATHNAME', 'pg_tviews_create_wrapper';

CREATE FUNCTION pg_tviews_drop(
    tview_name text,
    if_exists boolean DEFAULT false
) RETURNS text
LANGUAGE c
AS 'MODULE_PATHNAME', 'pg_tviews_drop_wrapper';

CREATE FUNCTION pg_tviews_cascade(
    base_table_oid oid,
    pk_value bigint
) RETURNS void
LANGUAGE c
AS 'MODULE_PATHNAME', 'pg_tviews_cascade_wrapper';
