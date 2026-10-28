# READ COMMITTED: a updates a fact row while b deletes it. b waits for a, then
# deletes the updated row: its old row carries the updated amount, so the bucket
# loses exactly what it holds and goes away. No stale bucket.

setup
{
  TRUNCATE tf_sales_day, tf_sales_month, tf_sales_year, tf_sales_category_month;
  INSERT INTO tf_sales_day (pk, tenant_id, entity_id, category, day, amount, qty) VALUES (1, md5('1')::uuid, 1, 'a', '2026-07-01', 10, 1);
}

session a
step a_begin  { BEGIN; }
step a_update { UPDATE tf_sales_day SET amount = 25, day = '2026-09-01' WHERE pk = 1; }
step a_commit { COMMIT; }

session b
step b_delete { DELETE FROM tf_sales_day WHERE pk = 1; }
step b_check
{
  SELECT count(*) AS problems FROM facts_m0.verify('sales');
  SELECT count(*) AS month_rows FROM tf_sales_month;
}

permutation a_begin a_update b_delete a_commit b_check
