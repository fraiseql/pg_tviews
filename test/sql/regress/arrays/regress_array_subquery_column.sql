-- A TVIEW column computed by ARRAY(SELECT ...) is materialized with the
-- subquery's element type (uuid[]), its elements in the subquery's order, next
-- to a jsonb_agg of the same rows inside `data`.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/arrays/regress_array_subquery_column.sql
-- expect-output: array_subquery_column: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'array subquery column FAIL: %', what; END IF; END $$;

CREATE TABLE tb_machine (
    pk_machine int PRIMARY KEY,
    id uuid NOT NULL DEFAULT gen_random_uuid(),
    serial_number text
);
CREATE TABLE tb_machine_item (
    pk_machine_item int PRIMARY KEY,
    id uuid NOT NULL DEFAULT gen_random_uuid(),
    fk_machine int REFERENCES tb_machine (pk_machine),
    name text
);
INSERT INTO tb_machine (pk_machine, serial_number) VALUES (1, 'M-001');
INSERT INTO tb_machine_item (pk_machine_item, fk_machine, name) VALUES (1, 1, 'Item A'), (2, 1, 'Item B');

SELECT pg_tviews_create('tv_machine', $$
    SELECT m.pk_machine, m.id, m.serial_number,
           ARRAY(SELECT mi.id FROM tb_machine_item mi
                 WHERE mi.fk_machine = m.pk_machine ORDER BY mi.pk_machine_item) AS machine_item_ids,
           jsonb_build_object(
               'id', m.id,
               'serial_number', m.serial_number,
               'items', (SELECT jsonb_agg(jsonb_build_object('id', mi.id, 'name', mi.name)
                                          ORDER BY mi.pk_machine_item)
                         FROM tb_machine_item mi WHERE mi.fk_machine = m.pk_machine)) AS data
    FROM tb_machine m $$);
SELECT assert_fresh('tv_machine', 'pk_machine', 'pg_tviews_create');

SELECT must((SELECT format_type(atttypid, atttypmod) FROM pg_attribute
             WHERE attrelid = 'tv_machine'::regclass AND attname = 'machine_item_ids'
               AND NOT attisdropped) = 'uuid[]',
            'tv_machine.machine_item_ids is not uuid[]');
SELECT must((SELECT machine_item_ids FROM tv_machine WHERE pk_machine = 1)
            = (SELECT array_agg(id ORDER BY pk_machine_item) FROM tb_machine_item WHERE fk_machine = 1),
            'machine_item_ids is not the two item ids in order');
SELECT must((SELECT jsonb_path_query_array(data, '$.items[*].name') FROM tv_machine WHERE pk_machine = 1)
            = '["Item A", "Item B"]'::jsonb,
            'data.items is not [Item A, Item B]');

DROP EXTENSION pg_tviews CASCADE;
\echo 'array_subquery_column: PASS'
