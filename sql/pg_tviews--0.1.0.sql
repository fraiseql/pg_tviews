-- pg_tviews extension SQL installation script
-- Generated for pgrx extension: pg_tviews version 0.1.0

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

-- Register functions from shared library

-- Runtime dependency check function
-- Returns true if jsonb_ivm extension is installed
CREATE OR REPLACE FUNCTION pg_tviews_check_jsonb_ivm()
RETURNS boolean
AS 'MODULE_PATHNAME', 'pg_tviews_check_jsonb_ivm'
LANGUAGE C STRICT;

COMMENT ON FUNCTION pg_tviews_check_jsonb_ivm() IS
'Check if jsonb_ivm extension is installed (enables performance optimizations)';
CREATE FUNCTION pg_tviews_version()
RETURNS text
LANGUAGE c IMMUTABLE STRICT PARALLEL SAFE
AS 'MODULE_PATHNAME', 'pg_tviews_version_wrapper';

CREATE FUNCTION pg_tviews_analyze_select(sql text)
RETURNS jsonb
LANGUAGE c STRICT
AS 'MODULE_PATHNAME', 'pg_tviews_analyze_select_wrapper';

CREATE FUNCTION pg_tviews_infer_types(table_name text, columns text[])
RETURNS jsonb
LANGUAGE c STRICT
AS 'MODULE_PATHNAME', 'pg_tviews_infer_types_wrapper';

CREATE FUNCTION pg_tviews_create(tview_name text, select_sql text)
RETURNS text
LANGUAGE c STRICT
AS 'MODULE_PATHNAME', 'pg_tviews_create_wrapper';

CREATE FUNCTION pg_tviews_drop(tview_name text, if_exists boolean DEFAULT false)
RETURNS text
LANGUAGE c STRICT
AS 'MODULE_PATHNAME', 'pg_tviews_drop_wrapper';

CREATE FUNCTION pg_tviews_cascade(base_table_oid oid, pk_value bigint)
RETURNS void
LANGUAGE c STRICT
AS 'MODULE_PATHNAME', 'pg_tviews_cascade_wrapper';

-- Register trigger function
CREATE FUNCTION pg_tview_trigger_handler()
RETURNS trigger
LANGUAGE c
AS 'MODULE_PATHNAME', 'pg_tview_trigger_handler_wrapper';
