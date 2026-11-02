-- The functions a generated case (gen.py) calls: create a TVIEW, refused or not,
-- check every TVIEW against its view, read one back inside a mutation. Loaded by
-- run.sh into a fresh database before the case.

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
CREATE EXTENSION IF NOT EXISTS jsonb_delta;
CREATE EXTENSION IF NOT EXISTS pg_tviews;
\ir ../../lib/assert_fresh.sql

CREATE TABLE gen_shape (tv regclass PRIMARY KEY, key text NOT NULL);

-- Create one generated TVIEW. It is created and fresh, or refused with one of the
-- SQLSTATEs below (reported as REFUSED, not failed); any other error fails the run
-- with the definition. A definition embedding a TVIEW that was refused is skipped.
CREATE FUNCTION gen_create(tv text, key text, def text, options jsonb, needs text[])
RETURNS void LANGUAGE plpgsql AS $f$
DECLARE
    -- The refusals docs/reference/api.md ("Errors") and docs/reference/shapes.md
    -- ("Refused") document for a definition. 42601 (syntax_error) is not one: the
    -- generator always writes one SELECT with pk_<entity>, so it would be a bug of
    -- the generator, and fails the run.
    refusals CONSTANT text[] := ARRAY[
        '0A000',  -- feature_not_supported: a definition pg_tviews cannot maintain
                  -- (composite or expression DISTINCT ON key, inheritance, nesting)
        '22023',  -- invalid_parameter_value: refused by the `error` uncascaded policy,
                  -- an undeclared function read or time dependency, bad group_keys
        '42703',  -- undefined_column: a required column (pk_<entity>, ...) is missing
        '42804',  -- datatype_mismatch: pk_<entity> is not an integer
        '42P17',  -- invalid_object_definition: TVIEWs reading each other in a cycle
        '54001',  -- statement_too_complex: deeper than max_propagation_depth /
                  -- max_dependency_depth
        '21000'   -- cardinality_violation: two rows for one key of a UNION TVIEW
    ];
    state text;
    msg text;
BEGIN
    IF EXISTS (SELECT FROM unnest(needs) n WHERE to_regclass(n) IS NULL) THEN
        RAISE WARNING 'SKIPPED % (reads a TVIEW that was refused)', tv;
        RETURN;
    END IF;
    BEGIN
        PERFORM pg_tviews_create(tv, def, options);
    EXCEPTION WHEN OTHERS THEN
        GET STACKED DIAGNOSTICS state = RETURNED_SQLSTATE, msg = MESSAGE_TEXT;
        IF state = ANY (refusals) THEN
            RAISE WARNING 'REFUSED % (%): %', tv, state, msg;
            RETURN;
        END IF;
        RAISE EXCEPTION 'UNEXPECTED % creating %: %', state, tv, msg
            USING DETAIL = format('options %s, definition:%s', options, def);
    END;
    INSERT INTO gen_shape VALUES (tv::regclass, key);
    PERFORM assert_fresh(tv::regclass, key, 'creation');
END $f$;

-- Raise unless every created TVIEW equals its view.
CREATE FUNCTION gen_check(label text) RETURNS void LANGUAGE plpgsql AS $$
DECLARE
    s record;
    diff text;
    failures text := '';
BEGIN
    FOR s IN SELECT tv, key FROM gen_shape ORDER BY tv::text LOOP
        diff := fresh_diff(s.tv, s.key);
        IF diff IS NOT NULL THEN
            failures := failures || E'\n  ' || diff;
        END IF;
    END LOOP;
    IF failures <> '' THEN
        RAISE EXCEPTION 'DIVERGED after %:%', label, failures;
    END IF;
END $$;

-- What a mutation function does after its writes: read one row of a TVIEW back
-- (the `pick`-th shape, a key of its table) and raise unless it equals
-- the view's row. Rendered under the settings refreshes use, as fresh_diff is.
CREATE FUNCTION gen_read_back(pick int, label text) RETURNS void LANGUAGE plpgsql
SET TimeZone = 'UTC' SET DateStyle = 'ISO, YMD' SET IntervalStyle = 'postgres'
SET extra_float_digits = 1
AS $$
DECLARE
    s record;
    v regclass;
    k text;
    from_tv jsonb;
    from_view jsonb;
BEGIN
    SELECT tv, key INTO s FROM gen_shape ORDER BY tv::text
     OFFSET pick % greatest((SELECT count(*) FROM gen_shape), 1) LIMIT 1;
    IF NOT FOUND THEN
        RETURN;
    END IF;
    SELECT m.view_oid::oid::regclass INTO v
      FROM tviews.pg_tview_meta m WHERE m.table_oid::oid = s.tv::oid;
    -- The TVIEW's table is read first, as a mutation returning its row does.
    EXECUTE format('SELECT %1$I::text FROM %2$s ORDER BY %1$I OFFSET %3$s LIMIT 1',
                   s.key, s.tv, pick % 7) INTO k;
    IF k IS NULL THEN
        RETURN;
    END IF;
    EXECUTE format('SELECT data FROM %s WHERE %I::text = $1', s.tv, s.key) INTO from_tv USING k;
    EXECUTE format('SELECT data FROM %s WHERE %I::text = $1', v, s.key) INTO from_view USING k;
    IF from_tv IS DISTINCT FROM from_view THEN
        RAISE EXCEPTION 'STALE READ inside %: % % = % holds %, its view %',
            label, s.tv, s.key, k, from_tv, from_view;
    END IF;
END $$;
