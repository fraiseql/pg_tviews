# Two loads of one tenant, each writing July then August (a) and August then
# July (b) in two statements. Without the per-tenant advisory lock each waits on
# the bucket rows the other inserted first: a deadlock. With it, b waits at its
# first statement until a commits, and the grains are exact.

setup
{
  TRUNCATE tf_sales_day, tf_sales_month, tf_sales_year, tf_sales_category_month;
}

session a
step a_begin { BEGIN; }
step a_july  { INSERT INTO tf_sales_day (tenant_id, entity_id, category, day, amount, qty) VALUES (md5('1')::uuid, 1, 'a', '2026-07-03', 10, 1); }
step a_aug   { INSERT INTO tf_sales_day (tenant_id, entity_id, category, day, amount, qty) VALUES (md5('1')::uuid, 1, 'a', '2026-08-03', 10, 1); }
step a_commit { COMMIT; }

session b
step b_begin { BEGIN; }
step b_aug   { INSERT INTO tf_sales_day (tenant_id, entity_id, category, day, amount, qty) VALUES (md5('1')::uuid, 1, 'a', '2026-08-04', 20, 2); }
step b_july  { INSERT INTO tf_sales_day (tenant_id, entity_id, category, day, amount, qty) VALUES (md5('1')::uuid, 1, 'a', '2026-07-04', 20, 2); }
step b_commit { COMMIT; }
step b_check
{
  SELECT count(*) AS problems FROM facts_m0.verify('sales');
  SELECT bucket, amount, qty, _n FROM tf_sales_month ORDER BY bucket;
}

permutation a_begin a_july b_begin b_aug a_aug a_commit b_july b_commit b_check
