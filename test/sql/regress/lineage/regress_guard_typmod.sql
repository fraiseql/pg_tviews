-- A guard's constants keep their type with its modifier: a `char(2)` code
-- compared with 'FR', a list of such codes, a `bit(3)` mask. Rendered without
-- the modifier, 'FR' would read as `character` (one character, 'F') and B'101'
-- as `bit` (one bit), no written row would meet the guard, and writes would
-- leave the TVIEW stale.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/lineage/regress_guard_typmod.sql
-- expect-output: guard typmod: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'guard typmod: %', what; END IF; END $$;
CREATE FUNCTION in_sync() RETURNS boolean LANGUAGE plpgsql AS $$
BEGIN
    RETURN NOT EXISTS (SELECT 1 FROM public.tv_shop t FULL JOIN tviews.public__tv_shop v
                       USING (pk_shop) WHERE t.data IS DISTINCT FROM v.data);
END $$;

CREATE TABLE tb_shop (pk_shop int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
CREATE TABLE tb_country (pk_country int PRIMARY KEY, code char(2), name text);
CREATE TABLE tb_flag (pk_flag int PRIMARY KEY, mask bit(3));
INSERT INTO tb_shop (pk_shop, name) VALUES (1, 's1'), (2, 's2');
SELECT tviews.pg_tviews_create('tv_shop', $$
    SELECT s.pk_shop, s.id, jsonb_build_object(
        'france', (SELECT c.name FROM tb_country c WHERE c.code = 'FR'),
        'iberia', (SELECT count(*) FROM tb_country c WHERE c.code IN ('ES', 'PT')),
        'flags', (SELECT count(*) FROM tb_flag f WHERE f.mask = B'101')) AS data
    FROM tb_shop s $$, '{"uncascaded_policy": "full_refresh"}');

-- `character` and `bit` without a length are char(1) and bit(1); `bpchar` and
-- `"bit"` are unconstrained.
SELECT must(t->>'guard' !~ '::character([),\]]|\[)' AND t->>'guard' !~ '::bit[),]',
            format('a guard constant lost its modifier: %s', t->>'guard'))
FROM tviews.pg_tview_meta m, jsonb_array_elements(m.plan->'tables') t
WHERE m.entity = 'shop' AND t ? 'guard';

INSERT INTO tb_country VALUES (1, 'FR', 'France');
SELECT must(in_sync(), 'after a row meeting the equality');
INSERT INTO tb_country VALUES (2, 'ES', 'Spain'), (3, 'PT', 'Portugal');
SELECT must(in_sync(), 'after rows meeting the list');
INSERT INTO tb_flag VALUES (1, B'101');
SELECT must(in_sync(), 'after a row meeting the mask');
UPDATE tb_country SET name = 'République française' WHERE code = 'FR';
SELECT must(in_sync(), 'after an update of a row meeting the equality');

SELECT 'guard typmod: PASS' AS result;

DROP EXTENSION pg_tviews CASCADE;
DROP TABLE tb_flag, tb_country, tb_shop;
DROP FUNCTION must(boolean, text), in_sync();
