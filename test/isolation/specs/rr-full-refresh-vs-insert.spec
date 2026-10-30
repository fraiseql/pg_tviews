# A REPEATABLE READ rename of a table the TVIEW can't map key by key (the
# full_refresh policy), after a post was inserted and committed since its
# snapshot. The insert's refresh wrote the post with the old name; the rename's
# full refresh compares the TVIEW with its view under the transaction's
# snapshot, where the post doesn't exist, and would leave it stale. As every
# refresh under REPEATABLE READ, the full refresh is checked against the latest
# snapshot: the rename fails with 40001. Under READ COMMITTED the full refresh
# sees the post, and both rows show the new name.

setup
{
  CREATE EXTENSION IF NOT EXISTS jsonb_delta;
  CREATE EXTENSION pg_tviews;
  CREATE TABLE tb_site (name text);
  CREATE TABLE tb_post (pk_post bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                        title text);
  INSERT INTO tb_site VALUES ('blog');
  INSERT INTO tb_post (pk_post, title) VALUES (1, 'p1');
  DO $$ BEGIN
    PERFORM tviews.pg_tviews_create('tv_post', $q$
        SELECT p.pk_post, p.id, jsonb_build_object('title', p.title, 'site', s.name) AS data
        FROM tb_post p CROSS JOIN tb_site s $q$,
        jsonb_build_object('uncascaded_policy', 'full_refresh'));
  END $$;
}

teardown
{
  DROP TABLE tv_post;
  DROP EXTENSION pg_tviews CASCADE;
  DROP TABLE tb_post, tb_site;
}

session inserter
step i_insert { INSERT INTO tb_post (pk_post, title) VALUES (2, 'p2'); }

session renamer
step r_begin  { BEGIN ISOLATION LEVEL REPEATABLE READ; SELECT count(*) AS posts FROM tb_post; }
step r_begin_rc { BEGIN; }
step r_rename { UPDATE tb_site SET name = 'news'; }
step r_commit { COMMIT; }
step r_check
{
  SELECT t.pk_post, t.data->>'site' AS site, t.data = v.data AS fresh
  FROM tv_post t FULL JOIN tviews.public__tv_post v USING (pk_post) ORDER BY 1;
}

permutation r_begin i_insert r_rename r_commit r_check
permutation r_begin_rc i_insert r_rename r_commit r_check
