\set p random(1, 20000)
UPDATE bench.tb_post SET title = 'Title ' || random() WHERE pk_post = :p;
