\set u random(1, 2000)
UPDATE bench.tb_user SET username = 'user' || :u || '_' || (random() * 1e9)::bigint WHERE pk_user = :u;
