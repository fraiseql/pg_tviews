# A resync of a user, stale after a write no trigger saw, while another
# transaction renames that user (#280). The resync recomputes from the rows as they
# are when its refresh runs, under the value locks a refresh takes; the rename
# refreshes as any write does. Once both have committed, every row equals its view.

setup
{
  CREATE EXTENSION IF NOT EXISTS jsonb_delta;
  CREATE EXTENSION pg_tviews;
  CREATE TABLE tb_user (pk_user int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                        name text NOT NULL);
  CREATE TABLE tb_post (pk_post int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                        fk_user int NOT NULL REFERENCES tb_user, title text NOT NULL);
  CREATE INDEX ON tb_post (fk_user);
  INSERT INTO tb_user (pk_user, name) VALUES (1, 'ann'), (2, 'bob');
  INSERT INTO tb_post (pk_post, fk_user, title) VALUES (1, 1, 'p1'), (2, 1, 'p2'), (3, 2, 'p3');
  DO $$ BEGIN
    PERFORM tviews.pg_tviews_create('tv_post', $q$
        SELECT p.pk_post, p.id, p.fk_user,
               jsonb_build_object('title', p.title, 'author', u.name) AS data
        FROM tb_post p JOIN tb_user u ON u.pk_user = p.fk_user $q$);
  END $$;
  SET session_replication_role = replica;
  UPDATE tb_user SET name = 'ann (stale)' WHERE pk_user = 1;
  RESET session_replication_role;
}

teardown
{
  DROP TABLE tv_post;
  DROP EXTENSION pg_tviews CASCADE;
  DROP TABLE tb_post, tb_user;
}

session resyncer
step s_begin  { BEGIN; }
step s_resync { SELECT * FROM tviews.pg_tviews_resync('tb_user', ARRAY[1]); }
step s_commit { COMMIT; }
step s_check
{
  SELECT t.pk_post, t.data->>'author' AS author, t.data = v.data AS fresh
  FROM tv_post t FULL JOIN tviews.public__tv_post v USING (pk_post) ORDER BY 1;
}

session renamer
step r_begin  { BEGIN; }
step r_rename { UPDATE tb_user SET name = 'annie' WHERE pk_user = 1; }
step r_commit { COMMIT; }

permutation s_begin s_resync r_begin r_rename s_commit r_commit s_check
permutation r_begin r_rename s_begin s_resync r_commit s_commit s_check
permutation s_begin r_begin r_rename s_resync r_commit s_commit s_check
