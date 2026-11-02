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
