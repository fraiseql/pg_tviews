-- The one reader of a stored plan's document in the extension's SQL (ADR 0203):
-- every view and function reads plans through these, never through raw jsonb
-- paths (scripts/check-plan-readers.sh). Plain SQL, so the read contract calls no
-- function of the library (#133). A plan that does not decode reads as empty
-- here; the health check's `plans` component reports it.
-- The tables a plan maps, each with how a write to it maps to the TVIEW's keys
-- and the columns of it the TVIEW reads (none: unknown).
CREATE FUNCTION @extschema@.pg_tviews_plan_tables(plan jsonb)
RETURNS TABLE (relid oid, kind text, columns text[])
LANGUAGE sql IMMUTABLE PARALLEL SAFE
SET search_path = pg_catalog, pg_temp
AS $$
    SELECT (e->>'relid')::pg_catalog.oid, e->>'kind',
           ARRAY(SELECT pg_catalog.jsonb_array_elements_text(
                     CASE pg_catalog.jsonb_typeof(e->'columns') WHEN 'array' THEN e->'columns'
                                                                ELSE '[]'::pg_catalog.jsonb END))
      FROM pg_catalog.jsonb_array_elements(CASE pg_catalog.jsonb_typeof(plan->'tables')
                                           WHEN 'array' THEN plan->'tables'
                                           ELSE '[]'::pg_catalog.jsonb END) e
     WHERE pg_catalog.jsonb_typeof(e) = 'object'
$$;

-- The output columns a plan's foreign keys prove to hold another table's key.
CREATE FUNCTION @extschema@.pg_tviews_plan_references(plan jsonb)
RETURNS TABLE (column_name text, table_name text, key text, is_unique boolean,
               is_nullable boolean)
LANGUAGE sql IMMUTABLE PARALLEL SAFE
SET search_path = pg_catalog, pg_temp
AS $$
    SELECT r->>'column', r->>'table', r->>'key', (r->>'unique')::boolean,
           (r->>'nullable')::boolean
      FROM pg_catalog.jsonb_array_elements(CASE pg_catalog.jsonb_typeof(plan->'references')
                                           WHEN 'array' THEN plan->'references'
                                           ELSE '[]'::pg_catalog.jsonb END) r
     WHERE pg_catalog.jsonb_typeof(r) = 'object'
$$;

-- The tables holding a plan's identity, with the column it is read off: none for
-- a TVIEW whose rows come from a set operation.
CREATE FUNCTION @extschema@.pg_tviews_plan_roots(plan jsonb)
RETURNS TABLE (relid oid, key text)
LANGUAGE sql IMMUTABLE PARALLEL SAFE
SET search_path = pg_catalog, pg_temp
AS $$
    SELECT (p->>'source_oid')::pg_catalog.oid, p->>'initial_col'
      FROM pg_catalog.jsonb_array_elements(CASE pg_catalog.jsonb_typeof(plan->'paths')
                                           WHEN 'array' THEN plan->'paths'
                                           ELSE '[]'::pg_catalog.jsonb END) p
     WHERE pg_catalog.jsonb_typeof(p) = 'object'
       AND COALESCE((p->>'root')::boolean, false)
       AND NOT COALESCE((plan->>'set_operation')::boolean, false)
$$;

-- The columns of a TVIEW's table its rows are looked up by: those holding an
-- embedded TVIEW's key, those a fan-out patch writes through, and its references.
CREATE FUNCTION @extschema@.pg_tviews_plan_lookups(plan jsonb)
RETURNS SETOF text
LANGUAGE sql IMMUTABLE PARALLEL SAFE
SET search_path = pg_catalog, pg_temp
AS $$
    SELECT pg_catalog.jsonb_array_elements_text(e->'lookups')
      FROM pg_catalog.jsonb_array_elements(CASE pg_catalog.jsonb_typeof(plan->'embeds')
                                           WHEN 'array' THEN plan->'embeds'
                                           ELSE '[]'::pg_catalog.jsonb END) e
     WHERE pg_catalog.jsonb_typeof(e->'lookups') = 'array'
    UNION
    SELECT t->'fanout'->>'lookup_col'
      FROM pg_catalog.jsonb_array_elements(CASE pg_catalog.jsonb_typeof(plan->'tables')
                                           WHEN 'array' THEN plan->'tables'
                                           ELSE '[]'::pg_catalog.jsonb END) t
     WHERE pg_catalog.jsonb_typeof(t->'fanout') = 'object'
    UNION
    SELECT column_name FROM @extschema@.pg_tviews_plan_references(plan)
$$;
