-- Regression test (#225): how a TVIEW's `data` column is stored.
--
-- Options data_compression (pglz | lz4), data_storage (extended | main |
-- external) and toast_tuple_target, in PostgreSQL's own words. A TVIEW stores
-- data compressed (lz4 when the server has it) and inline (MAIN) unless told
-- otherwise, every row has the declared storage (a change rewrites the rows),
-- and the registry publishes it.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/ddl/regress_data_storage.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

-- lz4 is a build option of PostgreSQL: the default compression is lz4 where the
-- server has it, pglz otherwise.
SELECT CASE WHEN 'lz4' = ANY (enumvals) THEN 'lz4' ELSE 'pglz' END AS dflt,
       CASE WHEN 'lz4' = ANY (enumvals) THEN 'l' ELSE 'p' END AS dc
FROM pg_settings WHERE name = 'default_toast_compression' \gset

CREATE TABLE tb_doc (pk_doc int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), body text);
-- Bodies large enough to be compressed (> 2 kB), compressible.
INSERT INTO tb_doc (pk_doc, body) SELECT g, repeat('lorem ipsum ', 400) FROM generate_series(1, 20) g;

-- `data`'s compression, storage, the table's toast_tuple_target, and the
-- compression of the stored values.
CREATE FUNCTION storage_of(tv regclass) RETURNS text LANGUAGE plpgsql AS $f$
DECLARE c "char"; s "char"; t text; v text;
BEGIN
    SELECT attcompression, attstorage INTO c, s FROM pg_attribute
    WHERE attrelid = tv AND attname = 'data';
    SELECT coalesce((SELECT option_value FROM pg_options_to_table(reloptions)
                     WHERE option_name = 'toast_tuple_target'), '-')
    INTO t FROM pg_class WHERE oid = tv;
    EXECUTE format('SELECT string_agg(DISTINCT coalesce(pg_column_compression(data), ''none''), '','')
                    FROM %s', tv) INTO v;
    RETURN format('%s/%s/%s/%s', nullif(c, ''), s, t, v);
END $f$;
CREATE FUNCTION expect(got text, want text, what text) RETURNS void LANGUAGE plpgsql AS $f$
BEGIN
    IF got IS DISTINCT FROM want THEN
        RAISE EXCEPTION '#225 FAIL: %: got %, want %', what, got, want;
    END IF;
END $f$;
CREATE FUNCTION expect(got boolean, want boolean, what text) RETURNS void LANGUAGE sql AS $f$
    SELECT expect(got::text, want::text, what) $f$;
CREATE FUNCTION error_of(stmt text) RETURNS text LANGUAGE plpgsql AS $f$
DECLARE msg text; state text;
BEGIN
    EXECUTE stmt;
    RETURN NULL;
EXCEPTION WHEN OTHERS THEN
    GET STACKED DIAGNOSTICS msg = MESSAGE_TEXT, state = RETURNED_SQLSTATE;
    RETURN state || ' ' || msg;
END $f$;

-- 1. Defaults: lz4 (or pglz), MAIN, PostgreSQL's TOAST tuple target.
SELECT tviews.pg_tviews_create('tv_doc', $$
    SELECT pk_doc, id, jsonb_build_object('body', body) AS data FROM tb_doc $$);
SELECT expect(storage_of('tv_doc'), :'dc' || '/m/-/' || :'dflt', 'defaults');
SELECT expect((SELECT options->>'data_compression' || '/' || (options->>'data_storage') || '/'
                      || coalesce(options->>'toast_tuple_target', 'null')
               FROM tviews.registry WHERE entity = 'doc'), :'dflt' || '/main/null', 'registry defaults');
-- Rows written by refreshes have it too.
UPDATE tb_doc SET body = repeat('dolor sit ', 500) WHERE pk_doc = 1;
SELECT expect(storage_of('tv_doc'), :'dc' || '/m/-/' || :'dflt', 'after a refresh');

-- 2. Declared otherwise: a change rewrites the rows already there.
SELECT expect(tviews.pg_tviews_create_or_replace('tv_doc', $$
    SELECT pk_doc, id, jsonb_build_object('body', body) AS data FROM tb_doc $$,
    '{"data_compression": "pglz", "data_storage": "extended", "toast_tuple_target": 4000}'),
    'altered', 'storage change returns altered');
SELECT expect(storage_of('tv_doc'), 'p/x/4000/pglz', 'pglz + extended');
SELECT expect((SELECT options->>'data_compression' || '/' || (options->>'data_storage') || '/'
                      || (options->>'toast_tuple_target')
               FROM tviews.registry WHERE entity = 'doc'), 'pglz/extended/4000', 'registry declared');
SELECT expect(tviews.pg_tviews_create_or_replace('tv_doc', $$
    SELECT pk_doc, id, jsonb_build_object('body', body) AS data FROM tb_doc $$,
    '{"data_compression": "pglz", "data_storage": "extended", "toast_tuple_target": 4000}'),
    'unchanged', 'same storage is unchanged');
-- Back to the defaults (an option not passed is at its default).
SELECT expect(tviews.pg_tviews_create_or_replace('tv_doc', $$
    SELECT pk_doc, id, jsonb_build_object('body', body) AS data FROM tb_doc $$),
    'altered', 'back to defaults');
SELECT expect(storage_of('tv_doc'), :'dc' || '/m/-/' || :'dflt', 'defaults again');
-- EXTERNAL: no compression at all.
SELECT tviews.pg_tviews_create_or_replace('tv_doc', $$
    SELECT pk_doc, id, jsonb_build_object('body', body) AS data FROM tb_doc $$,
    '{"data_storage": "external"}');
SELECT expect(storage_of('tv_doc'), :'dc' || '/e/-/none', 'external');
-- The rows are still the view's.
SELECT expect((SELECT count(*) FROM tv_doc t JOIN tviews.public__tv_doc v USING (pk_doc)
               WHERE t.data = v.data)::text, '20', 'rows kept');

-- 3. Wrong values fail, naming the option.
SELECT expect(error_of($$SELECT tviews.pg_tviews_create('tv_bad', 'SELECT pk_doc AS pk_bad, id, jsonb_build_object() AS data FROM tb_doc', '{"data_compression": "zstd"}')$$) LIKE '22023 %data_compression%pglz or lz4%', true,
              'unknown compression');
SELECT expect(error_of($$SELECT tviews.pg_tviews_create('tv_bad', 'SELECT pk_doc AS pk_bad, id, jsonb_build_object() AS data FROM tb_doc', '{"data_storage": "plain"}')$$) LIKE '%data_storage%', true, 'plain storage refused');
SELECT expect(error_of($$SELECT tviews.pg_tviews_create('tv_bad', 'SELECT pk_doc AS pk_bad, id, jsonb_build_object() AS data FROM tb_doc', '{"toast_tuple_target": 64}')$$) LIKE '%toast_tuple_target%128%8160%', true, 'toast_tuple_target range');

-- A declared lz4 on a server built without it fails, naming the remedy.
SELECT :'dflt' = 'pglz' AS no_lz4 \gset
\if :no_lz4
SELECT expect(error_of($$SELECT tviews.pg_tviews_create('tv_bad', 'SELECT pk_doc AS pk_bad, id, jsonb_build_object() AS data FROM tb_doc', '{"data_compression": "lz4"}')$$) LIKE '22023 %data_compression%lz4 is not available%pglz%', true,
              'declared lz4 without lz4');
\endif

-- 4. CREATE TABLE tv_* AS carries toast_tuple_target with fillfactor.
CREATE TABLE tv_doc2 WITH (fillfactor = 90, toast_tuple_target = 3000) AS
    SELECT pk_doc AS pk_doc2, id, jsonb_build_object('body', body) AS data FROM tb_doc;
SELECT expect(storage_of('tv_doc2'), :'dc' || '/m/3000/' || :'dflt', 'CTAS');

SELECT 'issue #225 data storage: PASS' AS result;
-- expect-output: issue #225 data storage: PASS
