-- The functions a generated case (gen.py) calls: create a TVIEW, refused or not,
-- check every TVIEW against its view, read one back inside a mutation. Loaded by
-- run.sh into a fresh database before the case.

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
CREATE EXTENSION IF NOT EXISTS jsonb_delta;
CREATE EXTENSION IF NOT EXISTS pg_tviews;
\ir ../../lib/assert_fresh.sql
\ir ../visibility.sql

-- The roles (run.sh creates them for the run, passing their names as gen_owner
-- and gen_writer): every TVIEW is created by gen_owner, who is no superuser and
-- owns no base table (it is granted SELECT and TRIGGER on each); every write is
-- made by gen_writer; the oracle reads as the owner (fresh_diff).
GRANT CREATE ON SCHEMA public TO :"gen_owner", :"gen_writer";
CREATE TABLE gen_shape (tv regclass PRIMARY KEY, key text NOT NULL,
                        keys_only boolean NOT NULL DEFAULT false);
GRANT SELECT, INSERT ON gen_shape TO :"gen_owner";
SELECT set_config('gen.owner', :'gen_owner', false) IS NOT NULL AS owner_noted;

-- The readers of the visibility oracle (visibility.sql), created by run.sh: A and
-- B read tenants 1 and 2, C tenant 1 without one column (the case's grants), N
-- has no policy. gen.wrong names the deliberately wrong pattern of self-test 3:
-- none, nopolicy (no row security on tv_*), narrower (a child table reader A
-- reads less of than its parent) or revoked (tv_* granted to a reader who may not
-- select a column its definition copies).
CREATE TABLE gen_reader (role name PRIMARY KEY, tenant int NOT NULL);
INSERT INTO gen_reader VALUES (:'gen_reader_a', 1), (:'gen_reader_b', 2),
                              (:'gen_reader_c', 1), (:'gen_reader_n', 1);
SELECT set_config('gen.wrong', :'gen_wrong', false) IS NOT NULL AS wrong_noted,
       set_config('gen.reader_a', :'gen_reader_a', false) IS NOT NULL AS reader_a_noted,
       set_config('gen.reader_n', :'gen_reader_n', false) IS NOT NULL AS reader_n_noted;

-- Self-test 3 (narrower): reader A reads fewer rows of child table `tb` than of
-- its parents, so a parent embeds rows A may not read.
CREATE FUNCTION gen_narrow(tb text, pk text) RETURNS void LANGUAGE plpgsql AS $f$
BEGIN
    IF current_setting('gen.wrong') = 'narrower' THEN
        EXECUTE format('CREATE POLICY gen_narrower ON %I AS RESTRICTIVE FOR SELECT TO %I '
                       'USING (%I %% 3 <> 0)', tb, current_setting('gen.reader_a'), pk);
    END IF;
END $f$;

-- Create one generated TVIEW. It is created and fresh, or refused with one of the
-- SQLSTATEs below (reported as REFUSED, not failed); any other error fails the run
-- with the definition. A definition embedding a TVIEW that was refused is skipped.
-- With `expect`, the definition is outside the dialect: it must be refused with that
-- SQLSTATE, and being created fails the run. Each outcome is reported as a REACHED
-- line naming the dialect.toml `productions` it exercises (coverage.py reads them).
CREATE FUNCTION gen_create(tv text, key text, def text, options jsonb, needs text[],
                           keys_only boolean, productions text[], expect text)
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
        PERFORM set_config('role', current_setting('gen.owner'), true);
        PERFORM pg_tviews_create(tv, def, options);
        PERFORM set_config('role', 'none', true);
    EXCEPTION WHEN OTHERS THEN
        GET STACKED DIAGNOSTICS state = RETURNED_SQLSTATE, msg = MESSAGE_TEXT;
        IF expect IS NOT NULL AND state <> expect THEN
            RAISE EXCEPTION 'UNEXPECTED % creating % (want its refusal, %): %', state, tv,
                expect, msg USING DETAIL = format('definition:%s', def);
        END IF;
        IF state = ANY (refusals) THEN
            RAISE WARNING 'REFUSED % (%): %', tv, state, msg;
            RAISE WARNING 'REACHED refused:% %', state, array_to_string(productions, ',');
            RETURN;
        END IF;
        RAISE EXCEPTION 'UNEXPECTED % creating %: %', state, tv, msg
            USING DETAIL = format('options %s, definition:%s', options, def);
    END;
    IF expect IS NOT NULL THEN
        RAISE EXCEPTION 'ACCEPTED % (want its refusal, %)', tv, expect
            USING DETAIL = format('definition:%s', def);
    END IF;
    RAISE WARNING 'REACHED created %', array_to_string(productions, ',');
    INSERT INTO gen_shape VALUES (tv::regclass, key, keys_only);
    PERFORM assert_fresh(tv::regclass, key, 'creation');
    PERFORM gen_expose(tv::regclass);
END $f$;

-- Raise unless every created TVIEW equals its view.
CREATE FUNCTION gen_check(label text) RETURNS void LANGUAGE plpgsql SECURITY DEFINER AS $$
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
-- the view's row. Rendered under the frame's settings, as fresh_diff is.
CREATE FUNCTION gen_read_back(pick int, label text) RETURNS void LANGUAGE plpgsql SECURITY DEFINER
-- BEGIN frame settings
-- generated by test/sql/lib/gen_frame.py from docs/reference/owner-frame.md
SET TimeZone = 'UTC'
SET DateStyle = 'ISO, YMD'
SET IntervalStyle = 'postgres'
SET extra_float_digits = '1'
SET bytea_output = 'hex'
SET lc_monetary = 'C'
SET lc_numeric = 'C'
SET lc_time = 'C'
SET timezone_abbreviations = 'Default'
SET array_nulls = 'on'
SET quote_all_identifiers = 'off'
SET transform_null_equals = 'off'
SET standard_conforming_strings = 'on'
SET xmlbinary = 'base64'
SET xmloption = 'content'
SET gin_fuzzy_search_limit = '0'
SET plan_cache_mode = 'auto'
SET jit = 'off'
SET row_security = 'on'
-- END frame
AS $$
DECLARE
    s record;
    v regclass;
    k text;
    from_tv jsonb;
    from_view jsonb;
    custom record;
    caller_custom jsonb := '{}';
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
    -- The view read under the frame's custom settings, as fresh_diff reads it.
    FOR custom IN SELECT * FROM public.frame_custom_settings(s.tv) LOOP
        caller_custom := caller_custom || jsonb_build_object(custom.name, current_setting(custom.name, true));
        PERFORM set_config(custom.name, custom.value, true);
    END LOOP;
    EXECUTE format('SELECT data FROM %s WHERE %I::text = $1', v, s.key) INTO from_view USING k;
    FOR custom IN SELECT e.key AS name, e.value FROM jsonb_each_text(caller_custom) e LOOP
        PERFORM set_config(custom.name, coalesce(custom.value, ''), true);
    END LOOP;
    -- As text: a number keeps its scale there, as in a binary comparison.
    IF from_tv::text IS DISTINCT FROM from_view::text THEN
        RAISE EXCEPTION 'STALE READ inside %: % % = % holds %, its view %',
            label, s.tv, s.key, k, from_tv, from_view;
    END IF;
END $$;

-- The oracle runs as the TVIEWs' owner, whoever calls it (a mutation runs as the
-- writer).
ALTER FUNCTION gen_check(text) OWNER TO :"gen_owner";
ALTER FUNCTION gen_read_back(int, text) OWNER TO :"gen_owner";

-- Self-test 2 of run.sh: the oracle reads outside the owner frame, in the
-- checking session's configuration (run.sh changes its TimeZone): the run must fail.
\if :gen_selftest_frame
ALTER FUNCTION fresh_diff(regclass, text) RESET ALL;
\endif

-- The rows of view `v`, as the owner reads them in the frame: each row's text
-- (a number keeps its scale there, an array its order), sorted: the rows of a
-- TVIEW are keyed, their order is not part of it.
CREATE FUNCTION gen_view_image(v regclass) RETURNS text LANGUAGE plpgsql SECURITY DEFINER
SET TimeZone = 'UTC' SET DateStyle = 'ISO, YMD' SET extra_float_digits = '1'
AS $$
DECLARE image text;
BEGIN
    EXECUTE format('SELECT string_agg(r::text, E''\n'' ORDER BY r::text) FROM %s r', v) INTO image;
    RETURN image;
END $$;
ALTER FUNCTION gen_view_image(regclass) OWNER TO :"gen_owner";

-- The independent check of the read sets: for each TVIEW, each table its plan
-- reads column by column (`read_set`), and each column of it outside that set,
-- an UPDATE of a few rows to another value, and one to the same value (which
-- moves the rows and changes their system columns), each rolled back, must leave
-- the backing view's rows as they were. No pg_tviews trigger fires
-- (session_replication_role = replica): the claim tested is the read set's alone.
-- Raises PERTURBED with the TVIEW, table and column otherwise.
CREATE FUNCTION gen_perturb() RETURNS void LANGUAGE plpgsql AS $$
DECLARE
    s record;
    t record;
    c record;
    v regclass;
    before text;
    after text;
    change text;
    tested int := 0;
BEGIN
    PERFORM set_config('session_replication_role', 'replica', true);
    FOR s IN SELECT g.tv, m.view_oid::oid::regclass AS view, m.plan
               FROM gen_shape g JOIN tviews.pg_tview_meta m ON m.table_oid::oid = g.tv::oid
              ORDER BY g.tv::text LOOP
        before := gen_view_image(s.view);
        FOR t IN
            SELECT e.relid, bool_and(e.read_set IS NOT NULL) AS known,
                   array_agg(DISTINCT a.n) FILTER (WHERE a.n IS NOT NULL) AS read_set
              FROM (SELECT (x->>'relid')::oid AS relid, x->'read_set' AS read_set
                      FROM jsonb_array_elements(s.plan->'tables') x
                    UNION ALL
                    SELECT (x->>'source_oid')::oid, x->'read_set'
                      FROM jsonb_array_elements(coalesce(s.plan->'paths', '[]')) x) e
              LEFT JOIN LATERAL jsonb_array_elements_text(coalesce(e.read_set, '[]')) a(n) ON true
             WHERE NOT EXISTS (SELECT FROM tviews.pg_tview_meta o WHERE o.table_oid::oid = e.relid)
             GROUP BY e.relid LOOP
            CONTINUE WHEN NOT t.known;
            FOR c IN SELECT a.attname, a.attnum FROM pg_attribute a
                      WHERE a.attrelid = t.relid AND a.attnum > 0 AND NOT a.attisdropped
                        AND a.attgenerated = '' AND a.attidentity <> 'a'
                        AND (a.attnum::text <> ALL (coalesce(t.read_set, '{}')))
                      ORDER BY a.attnum LOOP
                FOREACH change IN ARRAY ARRAY[
                    format('(SELECT o.%1$I FROM %2$s o WHERE o.%1$I IS DISTINCT FROM r.%1$I ORDER BY o.ctid LIMIT 1)',
                           c.attname, t.relid::regclass),
                    format('r.%I', c.attname)] LOOP
                    after := NULL;
                    BEGIN
                        EXECUTE format('UPDATE %1$s r SET %2$I = %3$s WHERE r.ctid = ANY (ARRAY(SELECT ctid FROM %1$s ORDER BY ctid LIMIT 3))',
                                       t.relid::regclass, c.attname, change);
                        after := coalesce(gen_view_image(s.view), '');
                        RAISE EXCEPTION 'gen_perturb: roll back';
                    EXCEPTION WHEN OTHERS THEN
                        -- An UPDATE the table refuses (a constraint) tests nothing.
                        IF SQLERRM <> 'gen_perturb: roll back' THEN
                            after := NULL;
                        END IF;
                    END;
                    IF after IS NOT NULL THEN
                        tested := tested + 1;
                        IF after IS DISTINCT FROM coalesce(before, '') THEN
                            RAISE EXCEPTION 'PERTURBED %: an UPDATE of %.% (outside its read set %) changed its view: SET % = %',
                                s.tv, t.relid::regclass, c.attname, t.read_set, c.attname, change;
                        END IF;
                    END IF;
                END LOOP;
            END LOOP;
        END LOOP;
    END LOOP;
    PERFORM set_config('session_replication_role', 'origin', true);
    RAISE NOTICE 'perturbed % column(s) outside the read sets', tested;
END $$;
