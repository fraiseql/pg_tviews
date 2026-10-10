-- A column called fk_* keeps the type its definition gives it.
--
-- Names carry no meaning for pg_tviews beyond pk_<entity>: an fk_* output that
-- holds a UUID or text is stored as such, and replacing the definition retypes
-- it like any other column. It was forced to BIGINT, and the TVIEW could not be
-- created.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/refresh/regress_fk_column_types.sql
-- expect-output: fk_column_types: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

CREATE TABLE tb_a (pk_a bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                   fk_owner uuid NOT NULL DEFAULT gen_random_uuid(), fk_code text, v text);
INSERT INTO tb_a (pk_a, fk_code, v) VALUES (1, 'c1', 'x');
SELECT pg_tviews_create('tv_a', $$
    SELECT pk_a, id, fk_owner, fk_code, jsonb_build_object('v', v) AS data FROM tb_a
$$);
DO $$ BEGIN
    IF format_type((SELECT atttypid FROM pg_attribute WHERE attrelid = 'tv_a'::regclass
                    AND attname = 'fk_owner'), NULL) <> 'uuid'
       OR format_type((SELECT atttypid FROM pg_attribute WHERE attrelid = 'tv_a'::regclass
                       AND attname = 'fk_code'), NULL) <> 'text' THEN
        RAISE EXCEPTION 'FAIL: fk_* columns were not stored with their definition''s types';
    END IF;
END $$;
UPDATE tb_a SET fk_code = 'c2';
SELECT assert_fresh('tv_a', 'pk_a', 'an UPDATE of a text fk_ column');

-- A replacement that changes an fk_* column's type retypes it.
SELECT pg_tviews_create_or_replace('public.tv_a', $$
    SELECT pk_a, id, fk_owner, length(fk_code) AS fk_code, jsonb_build_object('v', v) AS data FROM tb_a
$$);
DO $$ BEGIN
    IF format_type((SELECT atttypid FROM pg_attribute WHERE attrelid = 'tv_a'::regclass
                    AND attname = 'fk_code'), NULL) <> 'integer' THEN
        RAISE EXCEPTION 'FAIL: the replaced fk_code was not retyped';
    END IF;
END $$;
SELECT assert_fresh('tv_a', 'pk_a', 'a replacement retyping fk_code');

\echo 'fk_column_types: PASS'
