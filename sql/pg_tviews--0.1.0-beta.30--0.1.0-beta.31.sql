-- pg_tviews 0.1.0-beta.30 → 0.1.0-beta.31
--
-- Pending upgrade script: a pull request that changes the extension SQL adds its
-- statements here (docs/development/extension-versioning.md). Released scripts are
-- never edited.

CREATE OR REPLACE FUNCTION @extschema@.pg_tviews_catalog_revision()
RETURNS integer
LANGUAGE sql IMMUTABLE PARALLEL SAFE
AS 'SELECT 9';

-- #247: the transactions committed without publishing their changes, readable by
-- every session.
CREATE  FUNCTION @extschema@."pg_tviews_feed_gaps"() RETURNS bigint /* core::result::Result<i64, pgrx_pg_sys::submodules::panic::ErrorReport> */
STRICT
LANGUAGE c /* Rust */
AS 'MODULE_PATHNAME', 'pg_tviews_feed_gaps_wrapper';

-- #251: match a reference's table by name through pg_class, not to_regclass, which
-- needs USAGE on its schema: every write loads every TVIEW's references.
CREATE OR REPLACE VIEW @extschema@.pg_tview_references AS
SELECT x.*,
       pg_catalog.row_number() OVER (PARTITION BY x.entity, x.column_name
                                     ORDER BY x.preferred DESC NULLS LAST, x.parent) = 1
       AS chosen
FROM (SELECT m.entity,
             r.value->>'column' AS column_name,
             p.entity AS parent,
             p.identity->'columns'->0->>'name' AS parent_column,
             p.table_oid::pg_catalog.oid AS parent_table,
             (r.value->>'unique')::boolean AS is_unique,
             (r.value->>'nullable')::boolean AS is_nullable,
             (SELECT pg_catalog.format('%I.%I', inn.nspname, ic.relname)
              FROM pg_catalog.pg_index i
              JOIN pg_catalog.pg_class ic ON ic.oid = i.indexrelid
              JOIN pg_catalog.pg_namespace inn ON inn.oid = ic.relnamespace
              JOIN pg_catalog.pg_attribute a ON a.attrelid = i.indrelid AND a.attnum = i.indkey[0]
              WHERE i.indrelid = m.table_oid::pg_catalog.oid AND a.attname = r.value->>'column'
                AND i.indisvalid AND i.indpred IS NULL
                AND (ic.relname = ANY (m.managed_index_names)
                     OR EXISTS (SELECT 1 FROM pg_catalog.pg_constraint k
                                WHERE k.conindid = i.indexrelid))
              ORDER BY ic.relname = ANY (m.managed_index_names) DESC, ic.relname
              LIMIT 1) AS index_name,
             ARRAY(SELECT c->>'name'
                   FROM pg_catalog.jsonb_array_elements(m.identity->'columns') c) AS order_by,
             p.identity->'columns'->0->>'name' = r.value->>'key' AS preferred
      FROM @extschema@.pg_tview_meta m
      CROSS JOIN LATERAL pg_catalog.jsonb_array_elements(
          COALESCE(m.plan->'references', '[]'::pg_catalog.jsonb)) r
      JOIN @extschema@.pg_tview_meta p
        ON NOT COALESCE((p.plan->>'set_operation')::boolean, false)
       AND EXISTS (SELECT 1 FROM pg_catalog.jsonb_array_elements(p.plan->'paths') pp
                   WHERE COALESCE((pp->>'root')::boolean, false)
                     AND pp->>'initial_col' = r.value->>'key'
                     AND EXISTS (SELECT 1 FROM pg_catalog.pg_class sc
                                 JOIN pg_catalog.pg_namespace sn ON sn.oid = sc.relnamespace
                                 WHERE sc.oid = (pp->>'source_oid')::pg_catalog.oid
                                   AND pg_catalog.format('%I.%I', sn.nspname, sc.relname)
                                       = r.value->>'table'))) x;
