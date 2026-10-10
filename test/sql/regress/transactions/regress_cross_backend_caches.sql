-- A backend's caches follow DDL run in another backend.
--
-- Every pg_tviews cache is per backend. This session warms them with writes; a
-- dblink connection, another backend, then changes the TVIEWs. The next write in
-- this session must see the change: the replaced definition's column map, a TVIEW
-- created since that embeds one this session already refreshed, a TVIEW table
-- moved to another schema, and the whole extension dropped and created again.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/transactions/regress_cross_backend_caches.sql
-- expect-output: cross-backend caches: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
CREATE EXTENSION IF NOT EXISTS dblink;
\ir ../../lib/assert_fresh.sql

CREATE TABLE tb_item (pk_item int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
CREATE TABLE tb_order (pk_order int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                       fk_item int NOT NULL REFERENCES tb_item, qty int);
INSERT INTO tb_item (pk_item, name) SELECT g, 'n' || g FROM generate_series(1, 3) g;
INSERT INTO tb_order (pk_order, fk_item, qty) SELECT g, 1 + g % 3, g FROM generate_series(1, 6) g;
SELECT pg_tviews_create('tv_item', $$SELECT pk_item, id, jsonb_build_object('name', name) AS data FROM tb_item$$);
CREATE SCHEMA moved;

SELECT format('host=localhost dbname=%s user=%s port=%s', current_database(), current_user,
              current_setting('port')) AS conninfo \gset
SELECT dblink_connect('other', :'conninfo');
SELECT dblink_exec('other', $$SET search_path = "$user", public, tviews$$);

-- Warm this backend's caches: entity info, column map, graph, relation names.
UPDATE tb_item SET name = name || '+';
SELECT assert_fresh('tv_item', 'pk_item', 'warm');

-- (1) The other backend renames the key the column feeds.
SELECT * FROM dblink('other', $$SELECT pg_tviews_create_or_replace('tv_item',
    'SELECT pk_item, id, jsonb_build_object(''label'', name) AS data FROM tb_item')$$) AS t(result text);
UPDATE tb_item SET name = name || '1';
SELECT assert_fresh('tv_item', 'pk_item', 'after a replace in another backend');

-- (2) The other backend creates a TVIEW embedding tv_item.
SELECT * FROM dblink('other', $$SELECT pg_tviews_create('tv_order',
    'SELECT o.pk_order, o.id, o.fk_item, jsonb_build_object(''qty'', o.qty, ''item'', i.data) AS data
     FROM tb_order o JOIN tv_item i ON i.pk_item = o.fk_item')$$) AS t(result text);
UPDATE tb_item SET name = name || '2';
SELECT assert_fresh('tv_item', 'pk_item', 'tv_item after a new parent in another backend');
SELECT assert_fresh('tv_order', 'pk_order', 'tv_order created in another backend');

-- (3) The other backend moves tv_item to another schema.
SELECT dblink_exec('other', 'ALTER TABLE tv_item SET SCHEMA moved');
UPDATE tb_item SET name = name || '3';
SELECT assert_fresh('moved.tv_item', 'pk_item', 'after a move in another backend');
SELECT assert_fresh('tv_order', 'pk_order', 'tv_order after a move in another backend');

-- (4) The other backend drops the extension and creates it again, with tv_item
--     defined differently.
SELECT dblink_exec('other', 'DROP EXTENSION pg_tviews CASCADE');
SELECT dblink_exec('other', 'CREATE EXTENSION pg_tviews');
SELECT * FROM dblink('other', $$SELECT tviews.pg_tviews_create('tv_item',
    'SELECT pk_item, id, jsonb_build_object(''title'', upper(name)) AS data FROM tb_item')$$) AS t(result text);
UPDATE tb_item SET name = name || '4';
SELECT assert_fresh('tv_item', 'pk_item', 'after the extension was created again in another backend');

SELECT dblink_disconnect('other');
SELECT 'cross-backend caches: PASS' AS result;
