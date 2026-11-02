-- pg_tviews 0.1.0-beta.34 → 0.1.0-beta.35
--
-- Pending upgrade script: a pull request that changes the extension SQL adds its
-- statements here (docs/development/extension-versioning.md). Released scripts are
-- never edited.

CREATE OR REPLACE FUNCTION @extschema@.pg_tviews_catalog_revision()
RETURNS integer
LANGUAGE sql IMMUTABLE PARALLEL SAFE
AS 'SELECT 11';

-- A restore rebinds the plan's column numbers by name (a table dumped with
-- dropped columns comes back without their holes).
CREATE OR REPLACE FUNCTION @extschema@.pg_tviews_meta_rebind()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, pg_temp
AS $$
DECLARE
    olds TEXT[];
    news TEXT[];
    old_cols TEXT[];
    new_cols TEXT[];
    missing TEXT;
    e JSONB;
    q TEXT;
    r TEXT;
    i INT;
    tables JSONB := '[]';
    paths JSONB := '[]';
BEGIN
    -- Each table's relid in the source database, and here (found by name).
    SELECT pg_catalog.array_agg(x.e->>'relid' ORDER BY x.n),
           pg_catalog.array_agg(
               pg_catalog.to_regclass(x.e->>'table')::pg_catalog.oid::pg_catalog.text
               ORDER BY x.n),
           pg_catalog.string_agg(x.e->>'table', ', ' ORDER BY x.n)
               FILTER (WHERE pg_catalog.to_regclass(x.e->>'table') IS NULL)
      INTO olds, news, missing
      FROM pg_catalog.jsonb_array_elements(NEW.plan->'tables') WITH ORDINALITY AS x(e, n);
    IF missing IS NOT NULL THEN
        RAISE EXCEPTION 'the plan of TVIEW tv_% names tables that do not exist: %',
            NEW.entity, missing
            USING ERRCODE = 'undefined_table',
                  HINT = 'Restore the tables the TVIEW reads before its catalog row.';
    END IF;
    -- Each numbered column as '<relid>:<attnum>', in the source database (old_cols)
    -- and here (new_cols): a table's `attnums` beside its `columns`, a path's
    -- `initial_attnum` beside its `initial_col`.
    WITH named AS (
        SELECT x.e->>'relid' AS rel, a.v AS attnum, c.v AS name
          FROM pg_catalog.jsonb_array_elements(NEW.plan->'tables') AS x(e),
               pg_catalog.jsonb_array_elements_text(x.e->'attnums') WITH ORDINALITY AS a(v, k)
               JOIN pg_catalog.jsonb_array_elements_text(x.e->'columns')
                    WITH ORDINALITY AS c(v, k) USING (k)
        UNION
        SELECT x.e->>'source_oid', x.e->>'initial_attnum', x.e->>'initial_col'
          FROM pg_catalog.jsonb_array_elements(NEW.plan->'paths') AS x(e)
         WHERE x.e->>'initial_attnum' IS NOT NULL AND x.e->>'source_oid' = ANY (olds)
    ), resolved AS (
        SELECT n.rel, n.attnum, n.name, t.rel_here,
               (SELECT a.attnum FROM pg_catalog.pg_attribute a
                 WHERE a.attrelid = t.rel_here::pg_catalog.oid AND a.attname = n.name
                   AND a.attnum > 0 AND NOT a.attisdropped) AS attnum_here
          FROM named n,
               LATERAL (SELECT news[pg_catalog.array_position(olds, n.rel)] AS rel_here) t
    )
    SELECT pg_catalog.array_agg(v.rel || ':' || v.attnum),
           pg_catalog.array_agg(v.rel_here || ':' || v.attnum_here),
           pg_catalog.string_agg(pg_catalog.format('%s.%I',
               v.rel_here::pg_catalog.oid::pg_catalog.regclass, v.name), ', ')
               FILTER (WHERE v.attnum_here IS NULL)
      INTO old_cols, new_cols, missing
      FROM resolved v;
    IF missing IS NOT NULL THEN
        RAISE EXCEPTION 'the plan of TVIEW tv_% names columns that do not exist: %',
            NEW.entity, missing
            USING ERRCODE = 'undefined_column',
                  HINT = 'Restore the tables the TVIEW reads, with the columns it reads, '
                         'before its catalog row.';
    END IF;
    FOR e IN SELECT value FROM pg_catalog.jsonb_array_elements(NEW.plan->'tables') LOOP
        r := e->>'relid';
        -- The table's own numbered columns: `links` and the read sets' `attnum` (0:
        -- the table itself) must be among them.
        IF EXISTS (SELECT 1 FROM pg_catalog.jsonb_array_elements_text(
                                     COALESCE(e->'links', '[]')) AS l(v)
                    WHERE pg_catalog.array_position(old_cols, r || ':' || l.v) IS NULL)
           OR EXISTS (SELECT 1 FROM pg_catalog.jsonb_array_elements(
                                     COALESCE(e->'reads', '[]')) AS s(v)
                       WHERE s.v->>'attnum' <> '0'
                         AND pg_catalog.array_position(old_cols, r || ':' || (s.v->>'attnum')) IS NULL)
        THEN
            RAISE EXCEPTION 'the plan of TVIEW tv_% numbers a column of % it does not name',
                NEW.entity, e->>'table'
                USING ERRCODE = 'data_corrupted',
                      HINT = 'SELECT tviews.pg_tviews_reregister(''' || NEW.entity
                             || ''') re-derives it.';
        END IF;
        IF e ? 'attnums' THEN
            e := pg_catalog.jsonb_set(e, '{attnums}', (SELECT COALESCE(pg_catalog.jsonb_agg(pg_catalog.split_part(
                            new_cols[pg_catalog.array_position(old_cols, r || ':' || a.v)], ':', 2
                        )::pg_catalog.int2 ORDER BY a.k), '[]')
                          FROM pg_catalog.jsonb_array_elements_text(e->'attnums')
                               WITH ORDINALITY AS a(v, k)));
        END IF;
        IF e ? 'reads' THEN
            e := pg_catalog.jsonb_set(e, '{reads}', (SELECT COALESCE(pg_catalog.jsonb_agg(s.v || pg_catalog.jsonb_build_object(
                          'attnum', CASE s.v->>'attnum' WHEN '0' THEN 0 ELSE pg_catalog.split_part(
                              new_cols[pg_catalog.array_position(old_cols, r || ':' || (s.v->>'attnum'))],
                              ':', 2)::pg_catalog.int2 END) ORDER BY s.k), '[]')
                        FROM pg_catalog.jsonb_array_elements(e->'reads') WITH ORDINALITY AS s(v, k)));
        END IF;
        IF pg_catalog.jsonb_typeof(e->'links') = 'array' THEN
            e := pg_catalog.jsonb_set(e, '{links}',
                     (SELECT COALESCE(pg_catalog.jsonb_agg(pg_catalog.split_part(
                                 new_cols[pg_catalog.array_position(old_cols, r || ':' || l.v)], ':', 2
                             )::pg_catalog.int2 ORDER BY l.k), '[]')
                        FROM pg_catalog.jsonb_array_elements_text(e->'links') WITH ORDINALITY AS l(v, k)));
        END IF;
        -- The mapping query and the read-set queries name relations and columns
        -- by number: {r:<relid>}, {c:<relid>:<attnum>}. Marked first, so a new
        -- number equal to another's old one is not rebound twice.
        q := e::pg_catalog.text;
        FOR i IN 1 .. COALESCE(pg_catalog.array_length(old_cols, 1), 0) LOOP
            q := pg_catalog.replace(q, '{c:' || old_cols[i] || '}', '{c:#' || new_cols[i] || '}');
        END LOOP;
        IF q ~ '\{c:[0-9]' THEN
            RAISE EXCEPTION 'the plan of TVIEW tv_% numbers a column it does not name in the '
                            'mapping of %', NEW.entity, e->>'table'
                USING ERRCODE = 'data_corrupted',
                      HINT = 'SELECT tviews.pg_tviews_reregister(''' || NEW.entity
                             || ''') re-derives it.';
        END IF;
        FOR i IN 1 .. pg_catalog.array_length(olds, 1) LOOP
            q := pg_catalog.replace(q, '{r:' || olds[i] || '}', '{r:#' || news[i] || '}');
        END LOOP;
        e := pg_catalog.replace(pg_catalog.replace(q, '{r:#', '{r:'), '{c:#', '{c:')::pg_catalog.jsonb;
        i := pg_catalog.array_position(olds, r);
        e := pg_catalog.jsonb_set(e, '{relid}', pg_catalog.to_jsonb(news[i]::pg_catalog.int8));
        tables := tables || pg_catalog.jsonb_build_array(e);
    END LOOP;
    FOR e IN SELECT value FROM pg_catalog.jsonb_array_elements(NEW.plan->'paths') LOOP
        i := pg_catalog.array_position(olds, e->>'source_oid');
        IF i IS NULL THEN
            RAISE EXCEPTION 'the plan of TVIEW tv_% has a path from %, which it maps no '
                            'write of', NEW.entity, e->>'source_table'
                USING ERRCODE = 'data_corrupted',
                      HINT = 'SELECT tviews.pg_tviews_reregister(''' || NEW.entity
                             || ''') re-derives it.';
        END IF;
        IF e->>'initial_attnum' IS NOT NULL THEN
            e := pg_catalog.jsonb_set(e, '{initial_attnum}', pg_catalog.to_jsonb(
                     pg_catalog.split_part(new_cols[pg_catalog.array_position(old_cols,
                         (e->>'source_oid') || ':' || (e->>'initial_attnum'))], ':', 2)::pg_catalog.int2));
        END IF;
        e := pg_catalog.jsonb_set(e, '{source_oid}',
                                  pg_catalog.to_jsonb(news[i]::pg_catalog.int8));
        paths := paths || pg_catalog.jsonb_build_array(e);
    END LOOP;
    NEW.plan := pg_catalog.jsonb_set(pg_catalog.jsonb_set(NEW.plan,
                    '{tables}', tables), '{paths}', paths);
    RETURN NEW;
END;
$$;

-- PL/pgSQL handlers keep the SQLSTATE of an error they cannot handle: a failed
-- cleanup after a dependent drop fails the DROP, the definition check catches
-- class 42 only, reregister_all(strict) raises the first failure's SQLSTATE.
CREATE OR REPLACE FUNCTION @extschema@.pg_tviews_handle_drop_event()
RETURNS event_trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, @extschema@, pg_temp
AS $$
DECLARE
    entity_name TEXT;
BEGIN
    FOR entity_name IN
        SELECT DISTINCT m.entity
        FROM pg_catalog.pg_event_trigger_dropped_objects() AS d
        JOIN @extschema@.pg_tview_meta AS m
          ON d.objid IN (m.view_oid, m.table_oid)
        WHERE (NOT d.original OR TG_TAG = 'DROP OWNED')
          AND d.classid = 'pg_catalog.pg_class'::pg_catalog.regclass
          AND d.objsubid = 0
    LOOP
        PERFORM @extschema@.pg_tviews_handle_dropped(entity_name);
    END LOOP;
END;
$$;

CREATE OR REPLACE FUNCTION @extschema@.pg_tviews_defines_view(view_oid OID, candidate TEXT)
RETURNS BOOLEAN
LANGUAGE plpgsql
AS $$
DECLARE
    same BOOLEAN;
BEGIN
    EXECUTE 'CREATE TEMP VIEW pg_tviews_rename_check AS ' OPERATOR(pg_catalog.||) candidate;
    same := pg_catalog.pg_get_viewdef('pg_temp.pg_tviews_rename_check'::regclass)
            OPERATOR(pg_catalog.=) pg_catalog.pg_get_viewdef(view_oid);
    DROP VIEW pg_temp.pg_tviews_rename_check;
    RETURN same;
EXCEPTION WHEN syntax_error_or_access_rule_violation THEN
    RETURN false;
END;
$$;

CREATE OR REPLACE FUNCTION @extschema@.pg_tviews_reregister_all(strict BOOLEAN DEFAULT false)
RETURNS TABLE (entity TEXT, status TEXT)
LANGUAGE plpgsql
SET search_path = pg_catalog, pg_temp
AS $$
#variable_conflict use_column
DECLARE
    next_entity TEXT;
    failures INTEGER := 0;
    first_state TEXT;
    first_message TEXT;
BEGIN
    FOR next_entity IN
        WITH RECURSIVE edges(entity, dependency) AS (
            SELECT DISTINCT r.entity, m.entity
            FROM @extschema@.pg_tview_reads r
            JOIN @extschema@.pg_tview_meta m
              ON r.relid IN (m.view_oid::oid, m.table_oid::oid)
            WHERE m.entity <> r.entity
        ),
        depth(entity, level) AS (
            SELECT m.entity, 0 FROM @extschema@.pg_tview_meta m
          UNION
            SELECT e.entity, d.level + 1
            FROM depth d JOIN edges e ON e.dependency = d.entity
            WHERE d.level < 100
        )
        SELECT d.entity FROM depth d GROUP BY d.entity ORDER BY max(d.level), d.entity
    LOOP
        entity := next_entity;
        BEGIN
            PERFORM @extschema@.pg_tviews_reregister(next_entity);
            status := 'reregistered';
        -- catches every error: it becomes the entity's status; strict raises the first.
        EXCEPTION WHEN OTHERS THEN
            status := SQLERRM;
            failures := failures + 1;
            first_state := COALESCE(first_state, SQLSTATE);
            first_message := COALESCE(first_message, 'tv_' || next_entity || ': ' || SQLERRM);
        END;
        RETURN NEXT;
    END LOOP;
    -- The first failure's SQLSTATE: a lock timeout or a deadlock stays retryable.
    IF strict AND failures > 0 THEN
        RAISE EXCEPTION 'pg_tviews: % TVIEW(s) could not be re-registered; first: %',
            failures, first_message
            USING ERRCODE = first_state,
                  HINT = 'SELECT * FROM tviews.pg_tviews_reregister_all() lists them';
    END IF;
END;
$$;
