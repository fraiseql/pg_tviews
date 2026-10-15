# A post inserted while a table the TVIEW can't map key by key (read with no
# condition linking it to the key, under the full_refresh policy) changes: the
# full refresh can't see the uncommitted post, so the post must wait or be
# waited for. Once both have committed, every row holds the committed name.

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
step i_begin  { BEGIN; }
step i_insert { INSERT INTO tb_post (pk_post, title) VALUES (2, 'p2'); }
step i_commit { COMMIT; }

session renamer
step r_begin  { BEGIN; }
step r_rename { UPDATE tb_site SET name = 'news'; }
step r_commit { COMMIT; }
step r_check
{
  SELECT t.pk_post, t.data->>'site' AS site, t.data = v.data AS fresh
  FROM tv_post t JOIN tviews.public__tv_post v USING (pk_post) ORDER BY 1;
}

permutation i_begin i_insert r_begin r_rename i_commit r_commit r_check
permutation r_begin r_rename i_begin i_insert r_commit i_commit r_check
