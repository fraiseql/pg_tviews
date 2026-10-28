\set t random(1, 10)
\set d random(0, 1000)
INSERT INTO tf_sales_day (tenant_id, entity_id, category, day, amount, qty)
SELECT md5(:t::text)::uuid, g % 2000, (ARRAY['a', 'b', 'c'])[1 + g % 3],
       date '2025-01-01' + (:d + g % 90), 1.50, 1
FROM generate_series(1, 1000) g;
