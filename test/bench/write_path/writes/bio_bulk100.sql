\set u random(1, 1900)
UPDATE bench.tb_user SET bio = 'bio ' || random() WHERE pk_user BETWEEN :u AND :u + 99;
