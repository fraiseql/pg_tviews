\set p random(1, 2000)
UPDATE bench.tb_product SET price = price + 0.01 WHERE pk_product = :p;
