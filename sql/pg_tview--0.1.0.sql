-- Extension metadata tables (created by SQL, not in _PG_init)
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
