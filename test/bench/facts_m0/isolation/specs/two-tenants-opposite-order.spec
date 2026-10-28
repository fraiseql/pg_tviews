# Two transactions loading tenants 1 then 2 (a) and 2 then 1 (b), one statement
# per tenant. The advisory locks are taken per statement, so this order deadlocks:
# PostgreSQL detects it and aborts one transaction, and the grains stay exact.
# Loads are one tenant per transaction in practice (ADR 0260, Concurrency).

setup
{
  TRUNCATE tf_sales_day, tf_sales_month, tf_sales_year, tf_sales_category_month;
}

session a
step a_begin  { BEGIN; }
step a_t1     { INSERT INTO tf_sales_day (tenant_id, entity_id, category, day, amount, qty) VALUES (md5('1')::uuid, 1, 'a', '2026-07-03', 10, 1); }
step a_t2     { INSERT INTO tf_sales_day (tenant_id, entity_id, category, day, amount, qty) VALUES (md5('2')::uuid, 1, 'a', '2026-07-03', 10, 1); }
step a_commit { COMMIT; }

session b
step b_begin  { BEGIN; }
step b_t2     { INSERT INTO tf_sales_day (tenant_id, entity_id, category, day, amount, qty) VALUES (md5('2')::uuid, 1, 'a', '2026-07-04', 20, 2); }
step b_t1     { INSERT INTO tf_sales_day (tenant_id, entity_id, category, day, amount, qty) VALUES (md5('1')::uuid, 1, 'a', '2026-07-04', 20, 2); }
step b_commit { COMMIT; }
step b_check
{
  SELECT count(*) AS problems FROM facts_m0.verify('sales');
  SELECT count(*) AS day_rows FROM tf_sales_day;
}

permutation a_begin a_t1 b_begin b_t2 a_t2 b_t1 a_commit b_commit b_check
