-- What pg_tviews relies on jsonb_delta's patch functions to do (decision D5 of the
-- 2026-11 architecture wave: jsonb_delta is a required dependency). The own-row
-- patch calls jsonb_smart_patch_scalar (a top-level merge), the chained patch of
-- an embedded document jsonb_smart_patch_nested (a merge at a path). A jsonb_delta
-- release that changes any of these fails here.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/direct_patch/regress_jsonb_delta_semantics.sql
-- expect-output: jsonb_delta_semantics: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;

CREATE FUNCTION must(ok boolean, msg text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF NOT ok THEN RAISE EXCEPTION 'jsonb_delta FAIL: %', msg; END IF; END $$;
-- The SQLSTATE `expr` raises, or 'ok'.
CREATE FUNCTION state(expr text) RETURNS text LANGUAGE plpgsql AS $$
BEGIN
    EXECUTE 'SELECT ' || expr;
    RETURN 'ok';
EXCEPTION WHEN OTHERS THEN
    RETURN SQLSTATE;
END $$;

-- Top-level merge: keys replaced or added, the rest kept; a JSON null is a value.
SELECT must(jsonb_smart_patch_scalar('{"a":1,"b":{"c":2}}', '{"a":5}') = '{"a":5,"b":{"c":2}}',
            'scalar: replace a key');
SELECT must(jsonb_smart_patch_scalar('{"a":1}', '{"z":{"q":1}}') = '{"a":1,"z":{"q":1}}',
            'scalar: add a key');
SELECT must(jsonb_smart_patch_scalar('{"a":1}', '{"a":null}') = '{"a":null}',
            'scalar: a JSON null replaces');
SELECT must(jsonb_smart_patch_scalar('{"a\"q":1,"é":2}', '{"é":"ü"}') = '{"a\"q":1,"é":"ü"}',
            'scalar: quoted and non-ASCII keys');

-- Merge at a path: the object there merged one level deep, created when absent,
-- missing intermediate objects created.
SELECT must(jsonb_smart_patch_nested('{"a":1,"b":{"c":2,"d":3}}', '{"c":9}', '{b}')
            = '{"a":1,"b":{"c":9,"d":3}}', 'nested: merge');
SELECT must(jsonb_smart_patch_nested('{"a":1}', '{"c":9}', '{b}') = '{"a":1,"b":{"c":9}}',
            'nested: absent path');
SELECT must(jsonb_smart_patch_nested('{"a":1,"b":{}}', '{"c":9}', '{b,x}')
            = '{"a":1,"b":{"x":{"c":9}}}', 'nested: missing intermediate object');
SELECT must(jsonb_smart_patch_nested('{"b":{"c":{"x":1,"y":2}}}', '{"c":{"x":7}}', '{b}')
            = '{"b":{"c":{"x":7}}}', 'nested: a nested object is replaced, not merged');

-- What is not an object where an object is merged fails (a refresh then fails
-- loudly rather than writing a wrong document).
SELECT must(state($$jsonb_smart_patch_nested('{"b":null}', '{"c":9}', '{b}')$$) <> 'ok',
            'nested: JSON null at the path');
SELECT must(state($$jsonb_smart_patch_nested('{"b":5}', '{"c":9}', '{b}')$$) <> 'ok',
            'nested: scalar at the path');
SELECT must(state($$jsonb_smart_patch_nested('{"b":[1,2]}', '{"c":9}', '{b}')$$) <> 'ok',
            'nested: array at the path');
SELECT must(state($$jsonb_smart_patch_nested('[1,2]', '{"c":9}', '{b}')$$) <> 'ok',
            'nested: non-object root');
SELECT must(state($$jsonb_smart_patch_scalar('[1,2]', '{"c":9}')$$) <> 'ok',
            'scalar: non-object root');
SELECT must(state($$jsonb_smart_patch_scalar('{"a":1}', '5')$$) <> 'ok',
            'scalar: non-object patch');

-- STRICT: a NULL argument gives NULL.
SELECT must(jsonb_smart_patch_scalar(NULL, '{"a":1}') IS NULL
            AND jsonb_smart_patch_scalar('{}', NULL) IS NULL
            AND jsonb_smart_patch_nested('{}', '{"a":1}', NULL) IS NULL, 'STRICT');

-- A TOASTed document is patched whole.
CREATE TABLE big AS
SELECT jsonb_build_object('fill', (SELECT string_agg(md5(i::text), '') FROM generate_series(1, 800) i),
                          'b', jsonb_build_object('c', 1)) AS d;
SELECT must((SELECT jsonb_smart_patch_nested(d, '{"c":2}', '{b}') = d || '{"b":{"c":2}}'
             FROM big), 'TOASTed document');

\echo 'jsonb_delta_semantics: PASS'
