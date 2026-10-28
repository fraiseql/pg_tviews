# Two loads of one tenant into the same month and the same new buckets. The
# second waits for the first, then adds to the rows it created: no lost update.

setup
{
  TRUNCATE tf_sales_day, tf_sales_month, tf_sales_year, tf_sales_category_month;
}

session a
step a_begin  { BEGIN; }
step a_load   { INSERT INTO tf_sales_day (tenant_id, entity_id, category, day, amount, qty) SELECT md5('1')::uuid, g % 3, 'a', date '2026-07-01' + g % 28, 1, 1 FROM generate_series(1, 100) g; }
step a_commit { COMMIT; }

session b
step b_begin  { BEGIN; }
step b_load   { INSERT INTO tf_sales_day (tenant_id, entity_id, category, day, amount, qty) SELECT md5('1')::uuid, g % 3, 'a', date '2026-07-01' + g % 28, 2, 1 FROM generate_series(1, 100) g; }
step b_commit { COMMIT; }
step b_check
{
  SELECT count(*) AS problems FROM facts_m0.verify('sales');
  SELECT sum(_n) AS rows, sum(amount) AS amount FROM tf_sales_category_month;
}

permutation a_begin a_load b_begin b_load a_commit b_commit b_check
