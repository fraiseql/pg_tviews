-- The catalog: the registry of TVIEWs (pg_tview_meta) and its revision.
-- @extschema@ is substituted by PostgreSQL with the extension's install schema.
    -- view_oid / table_oid are regclass, not oid: pg_dump writes them as qualified
    -- names, so a restored row names the restored relations.
    CREATE TABLE @extschema@.pg_tview_meta (
        entity TEXT NOT NULL PRIMARY KEY,
        view_oid REGCLASS NOT NULL,
        table_oid REGCLASS NOT NULL,
        definition TEXT NOT NULL,
        -- What registration derived from the backing view's query tree (ADR 0203),
        -- one versioned document: how a write to each base table maps to keys
        -- (tables), the tables whose rows carry a key (paths), the TVIEWs it
        -- embeds (embeds) and the direct-patch map (direct).
        plan JSONB NOT NULL,
        created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
        -- GraphQL type reported by pg_tviews_flush_and_report; NULL
        -- means PascalCase(entity).
        graphql_typename TEXT,
        -- Aggregate TVIEWs: source table name -> group key column.
        group_keys JSONB,
        -- A release changed what registration derives since this TVIEW was last
        -- registered: it keeps refreshing with its old metadata until
        -- pg_tviews_reregister() re-derives it. Upgrade scripts set it.
        needs_reregister BOOLEAN NOT NULL DEFAULT false,
        -- Base tables the backing view reads whose writes no cascade maps to this
        -- TVIEW's keys. regclass, like view_oid: a dump names
        -- them, so a restored row names the restored tables.
        uncascaded_oids REGCLASS[] NOT NULL DEFAULT '{}',
        -- The TVIEW's uncascaded_policy option: what a write to one of
        -- uncascaded_oids does.
        uncascaded_policy TEXT NOT NULL DEFAULT 'error'
            CHECK (uncascaded_policy IN ('warn', 'error', 'full_refresh')),
        -- The output column that names this TVIEW's rows (ADR 0169), read from the
        -- backing view's query tree: an object with its kind (pk, distinct_on) and
        -- its columns (name, type). Every registration writes it.
        identity JSONB,
        -- Tables declared with a policy of their own, in the
        -- uncascaded_tables option: uncascaded_table_policies[i] applies to writes
        -- to uncascaded_table_oids[i] instead of uncascaded_policy.
        uncascaded_table_oids REGCLASS[] NOT NULL DEFAULT '{}',
        uncascaded_table_policies TEXT[] NOT NULL DEFAULT '{}',
        -- The functions the definition calls that may read tables,
        -- declared in the function_reads option with the tables each reads: one
        -- (function, table) pair per table, NULL for a function reading none.
        -- Functions as text, schema.name(argument types): a regprocedure column
        -- would block pg_upgrade.
        function_read_functions TEXT[] NOT NULL DEFAULT '{}',
        function_read_tables REGCLASS[] NOT NULL DEFAULT '{}',
        -- How a TVIEW that reads the current time is brought up to date:
        -- 'external', pg_tviews_refresh_time_dependent() called at the
        -- boundary; NULL for a TVIEW that reads no time or declared nothing.
        time_refresh TEXT CHECK (time_refresh IN ('external')),
        -- The definition reads the current time (CURRENT_DATE, now()…), so its
        -- rows change with no write.
        time_dependent BOOLEAN NOT NULL DEFAULT false,
        -- The indexes pg_tviews created on the table and still owns, by name
        -- (resolved among the table's indexes: a dump restores indexes after
        -- this table's rows, so they cannot be regclass).
        managed_index_names TEXT[] NOT NULL DEFAULT '{}',
        -- The indexes option as declared (#226): an array of indexes, each an
        -- array of keys. pg_tviews creates them under managed_index_names.
        declared_indexes JSONB NOT NULL DEFAULT '[]'
    );

    COMMENT ON TABLE @extschema@.pg_tview_meta IS
        'Internal TVIEW registrations; may change in any release. Tools read @extschema@.registry';

    -- Indexes for catalog lookup performance (entity PK already has a unique index)
    CREATE INDEX idx_pg_tview_meta_table_oid
        ON @extschema@.pg_tview_meta(table_oid);

    -- Extension-owned tables are skipped by pg_dump unless marked: without this a
    -- dump/restore brings back tv_*, v_* and the triggers but no registered TVIEW.
    SELECT pg_catalog.pg_extension_config_dump('@extschema@.pg_tview_meta', '');

    -- The row trigger reads the catalog as the writing role. It holds
    -- view definitions, which pg_views already shows to everyone. Only the
    -- extension owner writes it.
    GRANT SELECT ON @extschema@.pg_tview_meta TO PUBLIC;

    -- The UNLOGGED TVIEWs whose rows can be trusted, by entity. UNLOGGED itself: a
    -- crash restart or a promotion empties it together with them, and a TVIEW
    -- missing here is filled from its view by the next write. Dumped as extension
    -- configuration: pg_dump writes its rows exactly when it writes UNLOGGED tables'
    -- rows (not with --no-unlogged-table-data), so a restore brings back both or
    -- neither. No foreign key: the two configuration tables restore in any order.
    CREATE UNLOGGED TABLE @extschema@.pg_tview_valid (
        entity TEXT NOT NULL PRIMARY KEY
    );
    COMMENT ON TABLE @extschema@.pg_tview_valid IS
        'Internal: UNLOGGED TVIEWs whose rows can be trusted; may change in any release';
    GRANT SELECT ON @extschema@.pg_tview_valid TO PUBLIC;
    SELECT pg_catalog.pg_extension_config_dump('@extschema@.pg_tview_valid', '');

    -- Revision of this catalog. The library refuses to work against a
    -- catalog of another revision; an upgrade script that changes the extension SQL
    -- redefines this function, and the library's revision::CATALOG_REVISION with it.
    CREATE FUNCTION @extschema@.pg_tviews_catalog_revision()
    RETURNS integer
    LANGUAGE sql IMMUTABLE PARALLEL SAFE
    AS 'SELECT 12';
