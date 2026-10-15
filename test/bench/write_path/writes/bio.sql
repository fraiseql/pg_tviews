\set u random(1, 2000)
UPDATE bench.tb_user SET bio = 'bio ' || random() WHERE pk_user = :u;
