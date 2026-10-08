-- Regression test for issue #197: a trigger that writes its own table in nested
-- statements (a tree cascade, one nested UPDATE per level) must not flush the
-- refresh queue once per nested statement. The keys stay queued until the
-- outermost writing statement's flush, so each TVIEW row is recomputed once,
-- from the final state. Outside triggers, a statement still sees the TVIEWs
-- refreshed by the statements before it (a function that writes, then reads).
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/transactions/regress_trigger_writes_own_table.sql
--
-- expect-output: issue #197 nested flush: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION '#197 FAIL: %', what; END IF; END $$;
CREATE FUNCTION stat(name text) RETURNS numeric LANGUAGE sql AS $$
    SELECT (tviews.pg_tviews_queue_stats() ->> name)::numeric $$;

-- The issue's tree, with the path as an array of ancestor keys (no ltree).
CREATE TABLE tb_node (
    pk_node    bigint PRIMARY KEY,
    id         uuid NOT NULL DEFAULT gen_random_uuid() UNIQUE,
    fk_parent  bigint REFERENCES tb_node,
    name       text NOT NULL,
    full_label text NOT NULL,
    path       bigint[] NOT NULL
);
CREATE INDEX ON tb_node USING gin (path);
CREATE INDEX ON tb_node (fk_parent);
-- depth 6, fan-out 3: 364 nodes; node 2 heads a subtree of 121
INSERT INTO tb_node (pk_node, fk_parent, name, full_label, path) VALUES (1, NULL, 'n1', 'n1', '{1}');
DO $$
DECLARE lvl int; next_pk bigint := 2; r record; k int;
BEGIN
  FOR lvl IN 2..6 LOOP
    FOR r IN SELECT * FROM tb_node WHERE cardinality(path) = lvl - 1 ORDER BY pk_node LOOP
      FOR k IN 1..3 LOOP
        INSERT INTO tb_node VALUES (next_pk, gen_random_uuid(), r.pk_node, 'n' || next_pk,
                                    r.full_label || '/n' || next_pk, r.path || next_pk);
        next_pk := next_pk + 1;
      END LOOP;
    END LOOP;
  END LOOP;
END $$;
SELECT must((SELECT count(*) FROM tb_node) = 364, 'tree size');

-- Trigger-maintained denormalization: one nested statement per level.
CREATE FUNCTION a_cascade_full_label() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  UPDATE tb_node c SET full_label = NEW.full_label || '/' || c.name WHERE c.fk_parent = NEW.pk_node;
  RETURN NULL;
END $$;
CREATE TRIGGER a_cascade_full_label AFTER UPDATE OF full_label ON tb_node
  FOR EACH ROW WHEN (NEW.full_label IS DISTINCT FROM OLD.full_label)
  EXECUTE FUNCTION a_cascade_full_label();

CREATE VIEW v_node AS
SELECT n.pk_node, n.id,
       jsonb_build_object('full_label', n.full_label,
                          'ancestors', (SELECT array_agg(a.name ORDER BY cardinality(a.path))
                                          FROM tb_node a WHERE a.path <@ n.path)) AS data
  FROM tb_node n;

SET pg_tviews.uncascaded_policy = 'error';
SELECT tviews.pg_tviews_create('tv_node', 'SELECT pk_node, id, data FROM v_node');

-- 1. Control: outside triggers, each statement still refreshes before the next
-- reads (a function that writes, reads, writes again).
CREATE FUNCTION rename_twice() RETURNS text LANGUAGE plpgsql AS $$
DECLARE seen text;
BEGIN
  UPDATE tb_node SET name = 'first' WHERE pk_node = 364;
  SELECT data -> 'ancestors' ->> -1 INTO seen FROM tv_node WHERE pk_node = 364;
  UPDATE tb_node SET name = 'second' WHERE pk_node = 364;
  RETURN seen;
END $$;
BEGIN;
SELECT must(rename_twice() = 'first', 'a read after a write in the same function saw a stale row');
SELECT must((SELECT data -> 'ancestors' ->> -1 FROM tv_node WHERE pk_node = 364) = 'second', 'second write');
ROLLBACK;

-- 2. The issue: a root rename cascades level by level; one flush, each row once.
BEGIN;
DO $$
DECLARE it numeric := stat('total_iterations'); rc numeric := stat('view_recomputes');
BEGIN
  UPDATE tb_node SET name = 'R', full_label = 'R' WHERE pk_node = 1;
  PERFORM must(stat('total_iterations') - it = 1,
               format('root rename flushed %s times (one per nested statement)', stat('total_iterations') - it));
  PERFORM must(stat('view_recomputes') - rc = 364,
               format('root rename recomputed %s rows for 364 changed', stat('view_recomputes') - rc));
END $$;
SELECT assert_fresh('tv_node', 'pk_node', 'a root rename');
ROLLBACK;

-- 3. A middle node: its subtree of 121 rows.
BEGIN;
DO $$
DECLARE it numeric := stat('total_iterations'); rc numeric := stat('view_recomputes');
BEGIN
  UPDATE tb_node SET name = 'M', full_label = 'n1/M' WHERE pk_node = 2;
  PERFORM must(stat('total_iterations') - it = 1,
               format('middle rename flushed %s times', stat('total_iterations') - it));
  PERFORM must(stat('view_recomputes') - rc = 121,
               format('middle rename recomputed %s rows for 121 changed', stat('view_recomputes') - rc));
END $$;
SELECT assert_fresh('tv_node', 'pk_node', 'a middle rename');
COMMIT;
SELECT assert_fresh('tv_node', 'pk_node', 'a middle rename (committed)');

-- 4. A nested write from a user statement trigger that fires after the flush
-- trigger (its name sorts after trg_tview_flush_*): the outermost statement
-- refreshes it when it finishes.
CREATE FUNCTION zz_touch_root() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  IF pg_trigger_depth() = 1 THEN
    UPDATE tb_node SET name = 'late' WHERE pk_node = 364;
  END IF;
  RETURN NULL;
END $$;
CREATE TRIGGER zz_touch_root AFTER UPDATE ON tb_node
  FOR EACH STATEMENT EXECUTE FUNCTION zz_touch_root();
BEGIN;
UPDATE tb_node SET name = 'early' WHERE pk_node = 363;
SELECT must((SELECT data -> 'ancestors' ->> -1 FROM tv_node WHERE pk_node = 364) = 'late',
            'a write from a statement trigger firing after the flush stayed queued');
SELECT assert_fresh('tv_node', 'pk_node', 'a late statement-trigger write');
ROLLBACK;
DROP TRIGGER zz_touch_root ON tb_node;

-- 5. An error in a nested write caught by an EXCEPTION block: later statements
-- still flush on their own (no frame left behind).
CREATE FUNCTION b_guarded() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  BEGIN
    UPDATE tb_node SET name = NULL WHERE pk_node = NEW.pk_node;  -- NOT NULL violation
  EXCEPTION WHEN not_null_violation THEN NULL;
  END;
  RETURN NULL;
END $$;
CREATE TRIGGER b_guarded AFTER UPDATE OF name ON tb_node
  FOR EACH ROW WHEN (NEW.name = 'guarded') EXECUTE FUNCTION b_guarded();
BEGIN;
UPDATE tb_node SET name = 'guarded' WHERE pk_node = 300;
SELECT must((SELECT data -> 'ancestors' ->> -1 FROM tv_node WHERE pk_node = 300) = 'guarded',
            'statement with a caught nested error');
UPDATE tb_node SET name = 'after' WHERE pk_node = 301;
SELECT must((SELECT data -> 'ancestors' ->> -1 FROM tv_node WHERE pk_node = 301) = 'after',
            'a statement after a caught nested error was not refreshed');
SELECT assert_fresh('tv_node', 'pk_node', 'after a caught nested error');
ROLLBACK;
DROP TRIGGER b_guarded ON tb_node;

-- 6. A writable CTE writing the tracked table twice, and a DO block reading
-- between two writes.
BEGIN;
WITH a AS (UPDATE tb_node SET name = 'cte1' WHERE pk_node = 10 RETURNING 1)
UPDATE tb_node SET name = 'cte2' WHERE pk_node = 11;
SELECT assert_fresh('tv_node', 'pk_node', 'a writable CTE');
DO $$
BEGIN
  UPDATE tb_node SET name = 'do1' WHERE pk_node = 12;
  PERFORM must((SELECT data -> 'ancestors' ->> -1 FROM tv_node WHERE pk_node = 12) = 'do1',
               'a DO block read after its own write saw a stale row');
END $$;
ROLLBACK;

SELECT 'issue #197 nested flush: PASS' AS result;
