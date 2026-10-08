-- The plan, not a table's name, says which table holds a TVIEW's rows (ADR 0203).
--
-- tv_purchase is keyed on tb_order.pk_order: no table is called tb_purchase. An
-- UPDATE of a column its data only copies is patched in place, like any TVIEW's
-- (issue #56), and the row stays exactly what the view computes. The row trigger
-- found the patch's root by the name tb_<entity>, so this TVIEW was always
-- recomputed.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_plan_root_named_otherwise.sql
-- expect-output: root named otherwise: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE TABLE tb_customer (pk_customer bigint PRIMARY KEY, name text);
CREATE TABLE tb_order (
    pk_order bigint PRIMARY KEY,
    id uuid NOT NULL DEFAULT gen_random_uuid(),
    fk_customer bigint REFERENCES tb_customer,
    ref text
);
INSERT INTO tb_customer VALUES (1, 'ada');
INSERT INTO tb_order (pk_order, fk_customer, ref) VALUES (1, 1, 'R-1'), (2, 1, 'R-2');

SELECT pg_tviews_create('tv_purchase', $$
    SELECT o.pk_order AS pk_purchase, o.id,
           jsonb_build_object('ref', o.ref, 'c', c.name) AS data
    FROM tb_order o JOIN tb_customer c ON c.pk_customer = o.fk_customer
$$);

CREATE FUNCTION applied() RETURNS bigint LANGUAGE sql AS
    $f$ SELECT (pg_tviews_queue_stats()->>'direct_patches_applied')::bigint $f$;
CREATE TEMP TABLE before AS SELECT applied() AS n;

UPDATE tb_order SET ref = 'R-1b' WHERE pk_order = 1;

DO $$
BEGIN
    IF applied() <= (SELECT n FROM before) THEN
        RAISE EXCEPTION 'FAIL: the UPDATE of tb_order.ref was recomputed, not patched';
    END IF;
    IF (SELECT data FROM tv_purchase WHERE pk_purchase = 1)
       IS DISTINCT FROM '{"ref": "R-1b", "c": "ada"}'::jsonb THEN
        RAISE EXCEPTION 'FAIL: tv_purchase row 1 is %',
            (SELECT data FROM tv_purchase WHERE pk_purchase = 1);
    END IF;
    IF EXISTS (SELECT pk_purchase, data FROM tv_purchase
               EXCEPT SELECT pk_purchase, data FROM tviews.public__tv_purchase) THEN
        RAISE EXCEPTION 'FAIL: tv_purchase differs from its view';
    END IF;
END $$;

-- A write to the joined table is still a recompute, and fresh.
UPDATE tb_customer SET name = 'grace' WHERE pk_customer = 1;
DO $$
BEGIN
    IF EXISTS (SELECT pk_purchase, data FROM tv_purchase
               EXCEPT SELECT pk_purchase, data FROM tviews.public__tv_purchase) THEN
        RAISE EXCEPTION 'FAIL: tv_purchase is stale after a customer rename';
    END IF;
END $$;

\echo 'root named otherwise: PASS'
