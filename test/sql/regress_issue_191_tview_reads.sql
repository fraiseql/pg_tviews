-- Regression test for issue #191: a TVIEW reading another TVIEW's table, directly
-- or through a view (an aggregating one included), is maintained like one reading
-- a base table: a refresh of the inner TVIEW maps to the outer TVIEW's rows
-- through the same lineage. A read no condition links to the outer key goes
-- through the uncascaded policy. An embed through fk_<entity> = pk_<entity> is
-- still propagated, with no trigger on the inner TVIEW's table.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_191_tview_reads.sql
--
-- expect-output: issue #191 tview reads: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir lib/assert_fresh.sql

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION '#191 FAIL: %', what; END IF; END $$;
CREATE FUNCTION error_of(stmt text) RETURNS text LANGUAGE plpgsql AS $$
BEGIN EXECUTE stmt; RETURN 'created';
EXCEPTION WHEN OTHERS THEN RETURN SQLERRM; END $$;
CREATE FUNCTION kinds(e text) RETURNS text LANGUAGE sql AS $$
    SELECT cascade_kinds::text || ' uncascaded=' || uncascaded_tables::text
    FROM tviews.registry WHERE entity = e $$;
CREATE FUNCTION triggers_on(t regclass) RETURNS bigint LANGUAGE sql AS $$
    SELECT count(*) FROM pg_trigger WHERE tgrelid = t AND NOT tgisinternal $$;

CREATE TABLE tb_order (pk_order bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text, deleted_at timestamptz);
CREATE TABLE tb_line (pk_line bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_order bigint REFERENCES tb_order, amount numeric, deleted_at timestamptz);
INSERT INTO tb_order (pk_order, name) VALUES (1, 'o1'), (2, 'o2');
INSERT INTO tb_line (pk_line, fk_order, amount) VALUES (10, 1, 5), (11, 1, 7), (20, 2, 1);

-- The issue: a TVIEW of lines, a view aggregating it per order, a TVIEW over that.
CREATE VIEW v_line AS SELECT l.pk_line, l.id, o.id AS order_id, l.amount
  FROM tb_line l JOIN tb_order o ON o.pk_order = l.fk_order WHERE l.deleted_at IS NULL;
SELECT tviews.pg_tviews_create('tv_line', 'SELECT pk_line, id, order_id, amount FROM v_line');
CREATE VIEW v_order_lines AS SELECT order_id, jsonb_agg(id ORDER BY id) AS line_ids, sum(amount) AS total
  FROM tv_line GROUP BY order_id;
SELECT tviews.pg_tviews_create('tv_order', $$
  SELECT o.pk_order, o.id, o.name, ol.line_ids, ol.total
  FROM tb_order o LEFT JOIN v_order_lines ol ON ol.order_id = o.id WHERE o.deleted_at IS NULL $$);
SELECT must((SELECT cascade_kinds ->> 'tv_line' = 'mapped' AND uncascaded_tables = '{}'
             FROM tviews.registry WHERE entity = 'order'), 'registry: ' || kinds('order'));

CREATE FUNCTION exercise(tag text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    UPDATE tb_line SET deleted_at = now() WHERE pk_line = 10;          -- tombstone one line
    PERFORM assert_fresh('tv_order', 'pk_order', tag || ': a line tombstoned');
    INSERT INTO tb_line (pk_line, fk_order, amount) VALUES (12, 1, 3);
    PERFORM assert_fresh('tv_order', 'pk_order', tag || ': a line inserted');
    UPDATE tb_line SET fk_order = 2 WHERE pk_line = 11;                -- moves between orders
    PERFORM assert_fresh('tv_order', 'pk_order', tag || ': a line moved');
    UPDATE tb_line SET amount = amount * 2;
    PERFORM assert_fresh('tv_order', 'pk_order', tag || ': every line updated');
    DELETE FROM tb_line WHERE pk_line = 12;
    PERFORM assert_fresh('tv_order', 'pk_order', tag || ': a line deleted');
    -- Back to the start.
    UPDATE tb_line SET deleted_at = NULL, fk_order = 1, amount = 5 WHERE pk_line = 10;
    UPDATE tb_line SET fk_order = 1, amount = 7 WHERE pk_line = 11;
    UPDATE tb_line SET amount = 1 WHERE pk_line = 20;
    PERFORM assert_fresh('tv_order', 'pk_order', tag || ': reset');
END $$;
SELECT exercise('through an aggregating view');
-- Both TVIEWs are written in one statement that reaches both.
UPDATE tb_order SET name = name || '!';
SELECT assert_fresh('tv_line', 'pk_line', 'tb_order written (tv_line)');
SELECT assert_fresh('tv_order', 'pk_order', 'tb_order written (tv_order)');
SELECT must((SELECT status FROM tviews.pg_tviews_health_check() WHERE component = 'triggers') = 'OK',
            'health: ' || (SELECT message FROM tviews.pg_tviews_health_check() WHERE component = 'triggers'));
SELECT tviews.pg_tviews_drop('tv_order');
SELECT must(triggers_on('tv_line') = 0, 'triggers left on tv_line after the reader was dropped');

-- A correlated subquery on the inner TVIEW's table, not on its key.
SELECT tviews.pg_tviews_create('tv_order', $$
  SELECT o.pk_order, o.id, o.name,
         (SELECT sum(l.amount) FROM tv_line l WHERE l.order_id = o.id) AS total
  FROM tb_order o $$);
SELECT must((SELECT cascade_kinds ->> 'tv_line' = 'mapped' FROM tviews.registry WHERE entity = 'order'),
            'correlated: ' || kinds('order'));
SELECT exercise('a correlated subquery');
SELECT tviews.pg_tviews_drop('tv_order');

-- A read nothing links to the key goes through the policy: refused by default ...
\set unlinked 'SELECT o.pk_order, o.id, o.name, (SELECT sum(amount) FROM tv_line) AS total FROM tb_order o'
SELECT must(error_of(format('SELECT tviews.pg_tviews_create(%L, %L)', 'tv_order', :'unlinked'))
            LIKE '%public.tv_line%', 'an unlinked read of tv_line was not refused');
-- ... and refreshed in full under full_refresh.
SELECT tviews.pg_tviews_create_or_replace('tv_order', :'unlinked',
                                          options => '{"uncascaded_policy": "full_refresh"}');
SELECT must((SELECT uncascaded_tables = '{tv_line}' FROM tviews.registry WHERE entity = 'order'),
            'full_refresh: ' || kinds('order'));
SELECT exercise('unlinked, full_refresh');
SELECT tviews.pg_tviews_drop('tv_order');

-- An embed through fk_<entity> = pk_<entity> stays propagated: no trigger on tv_line.
CREATE TABLE tb_note (pk_note bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_line bigint REFERENCES tb_line, body text);
INSERT INTO tb_note (pk_note, fk_line, body) VALUES (1, 10, 'n');
SELECT tviews.pg_tviews_create('tv_note', $$
  SELECT n.pk_note, n.id, n.fk_line, n.body, l.amount AS line_amount
  FROM tb_note n JOIN tv_line l ON l.pk_line = n.fk_line $$);
SELECT must((SELECT cascade_kinds ->> 'tv_line' = 'propagated' FROM tviews.registry WHERE entity = 'note'),
            'embed: ' || kinds('note'));
SELECT must(triggers_on('tv_line') = 0, 'an embed put a trigger on tv_line');
UPDATE tb_line SET amount = 42 WHERE pk_line = 10;
SELECT assert_fresh('tv_note', 'pk_note', 'the embedded line changed');

SELECT 'issue #191 tview reads: PASS' AS result;
