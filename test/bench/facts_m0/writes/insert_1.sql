\set t random(1, 10)
\set e random(1, 2000)
\set d random(0, 1094)
INSERT INTO tf_sales_day (tenant_id, entity_id, category, day, amount, qty)
VALUES (md5(:t::text)::uuid, :e, 'a', date '2025-01-01' + :d, 1.50, 1);
