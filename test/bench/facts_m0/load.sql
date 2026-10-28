-- ADR 0260 M0: a randomized, reproducible load of the day grain. Each call runs
-- `statements` writing statements (one transaction), drawn from: batch inserts,
-- updates moving rows across months, years and partitions, updates of a
-- dimension or of a measure (NULLs included), deletes, INSERT … ON CONFLICT,
-- MERGE, writes naming a partition, and a block of writes rolled back to a
-- savepoint. setseed(seed) makes a run repeatable.
CREATE FUNCTION facts_m0.random_day() RETURNS date LANGUAGE sql VOLATILE AS $$
  SELECT date '2025-01-01' + floor(random() * 1095)::int $$;

CREATE FUNCTION facts_m0.random_load(seed double precision, statements int, tenants int DEFAULT 3)
RETURNS jsonb LANGUAGE plpgsql AS $$
DECLARE
  i int;
  op int;
  k int;
  counts jsonb := '{}';
  kind text;
BEGIN
  PERFORM setseed(seed);
  FOR i IN 1..statements LOOP
    op := floor(random() * 10)::int;
    k := 1 + floor(random() * 20)::int;
    kind := CASE op WHEN 0 THEN 'insert' WHEN 1 THEN 'insert' WHEN 2 THEN 'move_day'
                    WHEN 3 THEN 'set_category' WHEN 4 THEN 'set_amount' WHEN 5 THEN 'delete'
                    WHEN 6 THEN 'on_conflict' WHEN 7 THEN 'merge' WHEN 8 THEN 'partition_write'
                    ELSE 'savepoint_rollback' END;
    CASE kind
    WHEN 'insert' THEN
      INSERT INTO tf_sales_day (tenant_id, entity_id, category, day, amount, qty)
      SELECT md5((1 + floor(random() * tenants))::text)::uuid, 1 + floor(random() * 10)::bigint,
             (ARRAY['a', 'b', 'c', NULL])[1 + floor(random() * 4)::int], facts_m0.random_day(),
             CASE WHEN random() < 0.1 THEN NULL ELSE round((random() * 1000)::numeric, 2) END,
             floor(random() * 10)::bigint
      FROM generate_series(1, k);
    WHEN 'move_day' THEN
      UPDATE tf_sales_day SET day = LEAST(GREATEST(day + floor(random() * 400 - 200)::int,
                                                   date '2025-01-01'), date '2027-12-31')
      WHERE pk IN (SELECT pk FROM tf_sales_day ORDER BY random() LIMIT k);
    WHEN 'set_category' THEN
      UPDATE tf_sales_day SET category = (ARRAY['a', 'b', 'c', NULL])[1 + floor(random() * 4)::int]
      WHERE pk IN (SELECT pk FROM tf_sales_day ORDER BY random() LIMIT k);
    WHEN 'set_amount' THEN
      UPDATE tf_sales_day SET amount = CASE WHEN random() < 0.2 THEN NULL
                                            ELSE round((random() * 1000)::numeric, 2) END,
                              qty = qty + 1
      WHERE pk IN (SELECT pk FROM tf_sales_day ORDER BY random() LIMIT k);
    WHEN 'delete' THEN
      DELETE FROM tf_sales_day WHERE pk IN (SELECT pk FROM tf_sales_day ORDER BY random() LIMIT k);
    WHEN 'on_conflict' THEN
      INSERT INTO tf_sales_day AS t (pk, tenant_id, entity_id, category, day, amount, qty)
      SELECT pk, tenant_id, entity_id, category, day, amount, qty FROM (
        (SELECT pk, tenant_id, entity_id, category, day, round((random() * 100)::numeric, 2) AS amount, qty
         FROM tf_sales_day ORDER BY random() LIMIT k)
        UNION ALL
        SELECT nextval('tf_sales_day_pk'), md5((1 + floor(random() * tenants))::text)::uuid, 1, 'a',
               facts_m0.random_day(), 1, 1
        FROM generate_series(1, k)) s
      ON CONFLICT (pk, day) DO UPDATE SET amount = t.amount + excluded.amount, qty = t.qty + 1;
    WHEN 'merge' THEN
      MERGE INTO tf_sales_day t
      USING ((SELECT pk, day, random() < 0.3 AS gone FROM tf_sales_day ORDER BY random() LIMIT k)
             UNION ALL
             SELECT -1 * nextval('tf_sales_day_pk'), facts_m0.random_day(), false) s
      ON t.pk = s.pk AND t.day = s.day
      WHEN MATCHED AND s.gone THEN DELETE
      WHEN MATCHED THEN UPDATE SET amount = COALESCE(t.amount, 0) + 1, entity_id = t.entity_id + 1
      WHEN NOT MATCHED THEN INSERT (pk, tenant_id, entity_id, category, day, amount, qty)
        VALUES (-s.pk, md5((1 + floor(random() * tenants))::text)::uuid, 2, 'b', s.day, 5, 5);
    WHEN 'partition_write' THEN
      INSERT INTO tf_sales_day_2026 (tenant_id, entity_id, category, day, amount, qty)
      SELECT md5((1 + floor(random() * tenants))::text)::uuid, 3, 'c',
             date '2026-01-01' + floor(random() * 365)::int, 7, 7
      FROM generate_series(1, k);
      UPDATE tf_sales_day_2025 SET amount = COALESCE(amount, 0) + 2
      WHERE pk IN (SELECT pk FROM tf_sales_day_2025 ORDER BY random() LIMIT k);
    ELSE
      BEGIN
        INSERT INTO tf_sales_day (tenant_id, entity_id, category, day, amount, qty)
        SELECT md5('1')::uuid, 9, 'a', facts_m0.random_day(), 999, 9 FROM generate_series(1, k);
        DELETE FROM tf_sales_day WHERE pk IN (SELECT pk FROM tf_sales_day ORDER BY random() LIMIT k);
        RAISE EXCEPTION 'roll back to the block''s savepoint';
      EXCEPTION WHEN raise_exception THEN NULL;
      END;
    END CASE;
    counts := jsonb_set(counts, ARRAY[kind], to_jsonb(COALESCE((counts->>kind)::int, 0) + 1));
  END LOOP;
  RETURN counts;
END $$;
