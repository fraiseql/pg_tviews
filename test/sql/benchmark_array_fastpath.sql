-- benchmark_array_fastpath.sql
--
-- GO/NO-GO benchmark for the pg_tviews <-> jsonb_delta array fast-path
-- (joint-design thread: jsonb_delta issue #25).
--
-- Question: is it worth reworking the pg_tviews refresh queue to track
-- per-element array deltas and call jsonb_delta's surgical binary functions
-- (jsonb_array_update_where / jsonb_array_delete_where, pointer-passthrough)
-- INSTEAD of the current full-array recompute-and-replace?
--
-- The thread established two costs that are irreducibly O(array size) and that
-- no jsonb_delta function can remove:
--   1. producing `desired` (our jsonb_agg over N child rows)
--   2. the physical whole-tuple + TOAST rewrite (WAL/storage track doc size)
-- So the surgical function's CPU win (claimed 1.6-3.9x) may be swamped once the
-- O(doc) write is added. This benchmark measures both levels to decide.
--
-- Two comparison methods, per array size N and per op (update / delete):
--   * full_recompute : rebuild the whole `comments` array from the child rows
--                      via jsonb_agg (what apply_full_replacement re-runs today)
--   * surgical       : patch the one changed element on the existing doc via
--                      jsonb_array_update_where / jsonb_array_delete_where
--
-- Two levels:
--   * function : pure expression evaluation, no table write (isolates CPU)
--   * e2e      : full UPDATE statement, incl. tuple/TOAST/WAL rewrite
--
-- Requires: jsonb_delta 0.3.0 (pointer-passthrough is a 0.3.0 property).
--
-- Run: psql -d bench_array_fastpath -f test/sql/benchmark_array_fastpath.sql

\set ON_ERROR_STOP on
SET jit = off;  -- amortised per-op timing; jit compile spikes would hit both
                -- methods equally and only add variance to short loops.

CREATE EXTENSION IF NOT EXISTS jsonb_delta VERSION '0.3.0';

DROP SCHEMA IF EXISTS bench CASCADE;
CREATE SCHEMA bench;
SET search_path = bench, public;

-- Child rows (base table) and the materialised tview row.
CREATE TABLE tb_comment (pk_comment bigint, fk_post bigint, body text);
CREATE TABLE tv_post (pk_post bigint PRIMARY KEY, data jsonb);

-- Deterministic ~128-char comment body. Uses md5 hex (near-incompressible) so
-- TOAST cannot shrink the payload: this keeps on-disk doc size realistic and
-- makes the physical write cost a CONSERVATIVE stress test for the GO decision
-- (compressible natural-language text would only make the write cheaper and
-- thus flatter the surgical path).
CREATE FUNCTION mkbody(i bigint) RETURNS text LANGUAGE sql IMMUTABLE AS
$$ SELECT md5(i::text) || md5((i + 1)::text) || md5((i + 2)::text)
          || md5((i + 3)::text) $$;

-- Rebuild the dataset for a given N: N children under one post, and the
-- tv_post row whose data mirrors pg_tviews' #50 array shape.
CREATE FUNCTION setup(n int) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
  DELETE FROM tb_comment;
  DELETE FROM tv_post;
  INSERT INTO tb_comment
    SELECT g, 1, mkbody(g) FROM generate_series(1, n) g;
  INSERT INTO tv_post
    SELECT 1, jsonb_build_object(
      'id', 1, 'title', 'post title',
      'comments', (SELECT jsonb_agg(
                     jsonb_build_object('id', pk_comment, 'body', body)
                     ORDER BY pk_comment)
                   FROM tb_comment WHERE fk_post = 1));
END $$;

-- Core benchmark. Returns one row per (op, level, method).
-- p_kf = function-level iterations, p_ke = e2e iterations.
CREATE FUNCTION run(p_n int, p_kf int, p_ke int)
RETURNS TABLE(n int, op text, lvl text, method text, per_op_us numeric, doc_bytes int)
LANGUAGE plpgsql AS $$
DECLARE
  v_doc  jsonb;
  v_out  jsonb;
  mid    bigint;
  t0     timestamptz;
  t1     timestamptz;
  acc    interval;
  i      int;
BEGIN
  PERFORM setup(p_n);
  SELECT data INTO v_doc FROM tv_post WHERE pk_post = 1;
  mid := GREATEST(1, (p_n / 2)::bigint);  -- middle element => average scan depth
  n := p_n;
  doc_bytes := pg_column_size(v_doc);

  ---------------------------------------------------------------------------
  -- FUNCTION LEVEL (pure CPU, no write)
  ---------------------------------------------------------------------------

  -- full recompute: rebuild the whole doc from children. Op-independent
  -- (delete rebuilds N-1, ~same O(N) cost); measured once, reported for both.
  t0 := clock_timestamp();
  FOR i IN 1..p_kf LOOP
    v_out := jsonb_build_object(
      'id', 1, 'title', 'post title',
      'comments', (SELECT jsonb_agg(
                     jsonb_build_object('id', pk_comment, 'body', body)
                     ORDER BY pk_comment)
                   FROM tb_comment WHERE fk_post = 1));
  END LOOP;
  t1 := clock_timestamp();
  lvl := 'function'; method := 'full_recompute';
  per_op_us := EXTRACT(epoch FROM (t1 - t0)) * 1e6 / p_kf;
  op := 'update'; RETURN NEXT;
  op := 'delete'; RETURN NEXT;

  -- surgical update
  t0 := clock_timestamp();
  FOR i IN 1..p_kf LOOP
    v_out := jsonb_array_update_where(
               v_doc, 'comments', 'id', to_jsonb(mid),
               jsonb_build_object('body', mkbody(mid) || '-upd'));
  END LOOP;
  t1 := clock_timestamp();
  method := 'surgical';
  per_op_us := EXTRACT(epoch FROM (t1 - t0)) * 1e6 / p_kf;
  op := 'update'; RETURN NEXT;

  -- surgical delete
  t0 := clock_timestamp();
  FOR i IN 1..p_kf LOOP
    v_out := jsonb_array_delete_where(v_doc, 'comments', 'id', to_jsonb(mid));
  END LOOP;
  t1 := clock_timestamp();
  per_op_us := EXTRACT(epoch FROM (t1 - t0)) * 1e6 / p_kf;
  op := 'delete'; RETURN NEXT;

  ---------------------------------------------------------------------------
  -- END TO END (full UPDATE, incl. tuple/TOAST/WAL rewrite)
  ---------------------------------------------------------------------------
  lvl := 'e2e';

  -- UPDATE op keeps doc size stable across iterations (body length ~constant),
  -- so we can loop without resetting.

  -- full recompute update
  UPDATE tv_post SET data = v_doc WHERE pk_post = 1;
  t0 := clock_timestamp();
  FOR i IN 1..p_ke LOOP
    UPDATE tv_post SET data = jsonb_build_object(
      'id', 1, 'title', 'post title',
      'comments', (SELECT jsonb_agg(
                     jsonb_build_object('id', pk_comment,
                       'body', CASE WHEN pk_comment = mid
                                    THEN body || (i % 10)::text ELSE body END)
                     ORDER BY pk_comment)
                   FROM tb_comment WHERE fk_post = 1))
    WHERE pk_post = 1;
  END LOOP;
  t1 := clock_timestamp();
  method := 'full_recompute';
  per_op_us := EXTRACT(epoch FROM (t1 - t0)) * 1e6 / p_ke;
  op := 'update'; RETURN NEXT;

  -- surgical update
  UPDATE tv_post SET data = v_doc WHERE pk_post = 1;
  t0 := clock_timestamp();
  FOR i IN 1..p_ke LOOP
    UPDATE tv_post SET data = jsonb_array_update_where(
      data, 'comments', 'id', to_jsonb(mid),
      jsonb_build_object('body', mkbody(mid) || (i % 10)::text))
    WHERE pk_post = 1;
  END LOOP;
  t1 := clock_timestamp();
  method := 'surgical';
  per_op_us := EXTRACT(epoch FROM (t1 - t0)) * 1e6 / p_ke;
  op := 'update'; RETURN NEXT;

  -- DELETE op shrinks the array, so each timed delete needs a fresh full doc.
  -- Reset (untimed) then time one delete; both methods get identical treatment.

  -- full recompute delete
  acc := '0';
  FOR i IN 1..p_ke LOOP
    UPDATE tv_post SET data = v_doc WHERE pk_post = 1;      -- reset (untimed)
    t0 := clock_timestamp();
    UPDATE tv_post SET data = jsonb_build_object(
      'id', 1, 'title', 'post title',
      'comments', (SELECT jsonb_agg(
                     jsonb_build_object('id', pk_comment, 'body', body)
                     ORDER BY pk_comment)
                   FROM tb_comment WHERE fk_post = 1 AND pk_comment <> mid))
    WHERE pk_post = 1;
    t1 := clock_timestamp();
    acc := acc + (t1 - t0);
  END LOOP;
  method := 'full_recompute';
  per_op_us := EXTRACT(epoch FROM acc) * 1e6 / p_ke;
  op := 'delete'; RETURN NEXT;

  -- surgical delete
  acc := '0';
  FOR i IN 1..p_ke LOOP
    UPDATE tv_post SET data = v_doc WHERE pk_post = 1;      -- reset (untimed)
    t0 := clock_timestamp();
    UPDATE tv_post SET data =
      jsonb_array_delete_where(data, 'comments', 'id', to_jsonb(mid))
    WHERE pk_post = 1;
    t1 := clock_timestamp();
    acc := acc + (t1 - t0);
  END LOOP;
  method := 'surgical';
  per_op_us := EXTRACT(epoch FROM acc) * 1e6 / p_ke;
  op := 'delete'; RETURN NEXT;
END $$;

-- Drive all array sizes; iteration counts scaled so each measurement runs
-- long enough for stable timing without exploding total wall time.
CREATE TEMP TABLE results AS
  SELECT * FROM run(10,   20000, 2000)
  UNION ALL SELECT * FROM run(100,   5000, 1000)
  UNION ALL SELECT * FROM run(1000,  1000,  300)
  UNION ALL SELECT * FROM run(5000,   300,  100);

\echo
\echo '================ RAW PER-OP TIMINGS (microseconds) ================'
SELECT n,
       doc_bytes,
       op,
       lvl,
       round(max(per_op_us) FILTER (WHERE method='full_recompute'), 2) AS full_us,
       round(max(per_op_us) FILTER (WHERE method='surgical'),       2) AS surgical_us,
       round(max(per_op_us) FILTER (WHERE method='full_recompute')
           / nullif(max(per_op_us) FILTER (WHERE method='surgical'),0), 2) AS speedup_x
FROM results
GROUP BY n, doc_bytes, op, lvl
ORDER BY op, lvl, n;

\echo
\echo '================ SPEEDUP: function vs e2e (update) ================'
\echo '(surgical faster than full_recompute when speedup_x > 1)'
SELECT n, doc_bytes,
       round(max(per_op_us) FILTER (WHERE lvl='function' AND method='full_recompute')
           / nullif(max(per_op_us) FILTER (WHERE lvl='function' AND method='surgical'),0),2) AS fn_speedup_x,
       round(max(per_op_us) FILTER (WHERE lvl='e2e' AND method='full_recompute')
           / nullif(max(per_op_us) FILTER (WHERE lvl='e2e' AND method='surgical'),0),2) AS e2e_speedup_x
FROM results
WHERE op = 'update'
GROUP BY n, doc_bytes
ORDER BY n;
