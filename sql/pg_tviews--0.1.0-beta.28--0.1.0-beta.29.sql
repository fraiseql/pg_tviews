-- pg_tviews 0.1.0-beta.28 → 0.1.0-beta.29
--
-- Pending upgrade script: a pull request that changes the extension SQL adds its
-- statements here (docs/development/extension-versioning.md). Released scripts are
-- never edited.

-- #236: the validity of UNLOGGED TVIEWs is keyed by entity and dumped as extension
-- configuration, so a logical restore that carries an UNLOGGED TVIEW's rows also
-- carries their validity. Existing rows are converted.
ALTER TABLE @extschema@.pg_tview_valid RENAME TO pg_tview_valid_by_oid;
ALTER INDEX @extschema@.pg_tview_valid_pkey RENAME TO pg_tview_valid_by_oid_pkey;
CREATE UNLOGGED TABLE @extschema@.pg_tview_valid (
    entity TEXT NOT NULL PRIMARY KEY
);
COMMENT ON TABLE @extschema@.pg_tview_valid IS
    'Internal: UNLOGGED TVIEWs whose rows can be trusted; may change in any release';
GRANT SELECT ON @extschema@.pg_tview_valid TO PUBLIC;
INSERT INTO @extschema@.pg_tview_valid
SELECT m.entity
FROM @extschema@.pg_tview_valid_by_oid v
JOIN @extschema@.pg_tview_meta m ON m.table_oid::pg_catalog.oid = v.table_oid;
DROP TABLE @extschema@.pg_tview_valid_by_oid;
SELECT pg_catalog.pg_extension_config_dump('@extschema@.pg_tview_valid', '');

CREATE OR REPLACE FUNCTION @extschema@.pg_tviews_catalog_revision()
RETURNS integer
LANGUAGE sql IMMUTABLE PARALLEL SAFE
AS 'SELECT 7';

-- #236: pg_tviews_rebuild_all reports the rows each rebuild inserted
-- (`rows_inserted`, was `rows`, the table's size). A result column's name is part
-- of the function's type: dropped and created again, with the EXECUTE grants an
-- operator gave kept.
DO $$
DECLARE
    grantees text[];
    grantee text;
BEGIN
    SELECT pg_catalog.array_agg(DISTINCT pg_catalog.quote_ident(r.rolname)) INTO grantees
    FROM pg_catalog.pg_proc p,
         LATERAL pg_catalog.aclexplode(p.proacl) a
         JOIN pg_catalog.pg_roles r ON r.oid = a.grantee
    WHERE p.oid = '@extschema@.pg_tviews_rebuild_all(boolean)'::pg_catalog.regprocedure
      AND a.privilege_type = 'EXECUTE' AND a.grantee <> p.proowner;
    DROP FUNCTION @extschema@.pg_tviews_rebuild_all(boolean);
    CREATE FUNCTION @extschema@."pg_tviews_rebuild_all"(
        "only_empty" bool DEFAULT true /* bool */
    ) RETURNS TABLE (
        "entity" TEXT,  /* alloc::string::String */
        "rows_inserted" bigint  /* i64 */
    )
    STRICT
    LANGUAGE c /* Rust */
    AS 'MODULE_PATHNAME', 'pg_tviews_rebuild_all_wrapper';
    REVOKE EXECUTE ON FUNCTION @extschema@.pg_tviews_rebuild_all(BOOLEAN) FROM PUBLIC;
    FOREACH grantee IN ARRAY coalesce(grantees, '{}') LOOP
        EXECUTE pg_catalog.format('GRANT EXECUTE ON FUNCTION @extschema@.pg_tviews_rebuild_all(BOOLEAN) TO %s', grantee);
    END LOOP;
END $$;
