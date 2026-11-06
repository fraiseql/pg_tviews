-- Replacing a TVIEW that another role's TVIEW embeds, with a definition that
-- reads a table the other role holds no privilege on: the dependent is
-- re-derived as its own owner, the table is accepted for the role that called
-- the replace, and the dependent is recomputed before the statement ends.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/privileges/regress_replace_dependents_caller.sql
-- expect-output: replace_dependents_caller: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
DROP TABLE IF EXISTS tb_item, tb_extra, tb_note CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/who.sql
SELECT regress_roles('regress_rdc') \g /dev/null

CREATE FUNCTION must(ok boolean, msg text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF NOT ok THEN RAISE EXCEPTION 'FAIL: %', msg; END IF; END $$;

SET ROLE regress_rdc_owner_a;
CREATE TABLE tb_item (pk_item int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      title text);
CREATE TABLE tb_extra (pk_extra int PRIMARY KEY, fk_item int, label text);
INSERT INTO tb_item (pk_item, title) VALUES (1, 'a');
INSERT INTO tb_extra VALUES (1, 1, 'x');
SELECT tviews.pg_tviews_create('tv_item', $$
    SELECT pk_item, id, jsonb_build_object('title', title) AS data FROM tb_item $$) \g /dev/null
GRANT SELECT, TRIGGER ON tv_item TO regress_rdc_owner_b;

SET ROLE regress_rdc_owner_b;
CREATE TABLE tb_note (pk_note int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_item int, body text);
INSERT INTO tb_note (pk_note, fk_item, body) VALUES (1, 1, 'n');
SELECT tviews.pg_tviews_create('tv_note', $$
    SELECT n.pk_note, n.id, jsonb_build_object('body', n.body, 'item', i.data) AS data
    FROM tb_note n LEFT JOIN tv_item i ON i.pk_item = n.fk_item $$) \g /dev/null

SET ROLE regress_rdc_owner_a;
SELECT must(NOT has_table_privilege('regress_rdc_owner_b', 'tb_extra', 'TRIGGER'),
            'the dependent''s owner may trigger tb_extra');
SELECT tviews.pg_tviews_create_or_replace('tv_item', $$
    SELECT i.pk_item, i.id, jsonb_build_object('title', i.title, 'label', e.label) AS data
    FROM tb_item i LEFT JOIN tb_extra e ON e.fk_item = i.pk_item $$) \g /dev/null
RESET ROLE;
SELECT must((SELECT data->'item'->>'label' FROM tv_note WHERE pk_note = 1) = 'x',
            'tv_note was not recomputed after tv_item was replaced');

DROP EXTENSION pg_tviews CASCADE;
DROP TABLE tb_item, tb_extra, tb_note CASCADE;
DROP FUNCTION must(boolean, text);
SELECT regress_drop_roles('regress_rdc') \g /dev/null
\echo 'replace_dependents_caller: PASS'
