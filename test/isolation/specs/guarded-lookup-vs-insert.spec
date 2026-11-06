# A post inserted while a category is written. The TVIEW reads the category named
# 'default' in a subquery linked to no post, under the full_refresh policy: a write
# to that row refreshes every post, a write to another category none (#284). Either
# way, once both have committed, every row equals its view.

setup
{
  CREATE EXTENSION IF NOT EXISTS jsonb_delta;
  CREATE EXTENSION pg_tviews;
  CREATE TABLE tb_category (pk_category bigint PRIMARY KEY, name text NOT NULL, label text);
  CREATE TABLE tb_post (pk_post bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                        title text);
  INSERT INTO tb_category VALUES (1, 'default', 'General'), (2, 'other', 'Misc');
  INSERT INTO tb_post (pk_post, title) VALUES (1, 'p1');
  DO $$ BEGIN
    PERFORM tviews.pg_tviews_create('tv_post', $q$
        SELECT p.pk_post, p.id, jsonb_build_object('title', p.title,
            'category', (SELECT c.label FROM tb_category c WHERE c.name = 'default')) AS data
        FROM tb_post p $q$,
        jsonb_build_object('uncascaded_policy', 'full_refresh'));
  END $$;
}

teardown
{
  DROP TABLE tv_post;
  DROP EXTENSION pg_tviews CASCADE;
  DROP TABLE tb_post, tb_category;
}

session inserter
step i_begin  { BEGIN; }
step i_insert { INSERT INTO tb_post (pk_post, title) VALUES (2, 'p2'); }
step i_commit { COMMIT; }

session writer
step w_begin   { BEGIN; }
step w_default { UPDATE tb_category SET label = 'Everything' WHERE name = 'default'; }
step w_other   { UPDATE tb_category SET label = 'Other' WHERE name = 'other'; }
step w_rename  { UPDATE tb_category
                 SET name = CASE pk_category WHEN 1 THEN 'old' ELSE 'default' END; }
step w_commit  { COMMIT; }
step w_check
{
  SELECT t.pk_post, t.data->>'category' AS category, t.data = v.data AS fresh
  FROM tv_post t FULL JOIN tviews.public__tv_post v USING (pk_post) ORDER BY 1;
}

permutation i_begin i_insert w_begin w_default i_commit w_commit w_check
permutation w_begin w_default i_begin i_insert w_commit i_commit w_check
permutation i_begin i_insert w_begin w_other i_commit w_commit w_check
permutation w_begin w_other i_begin i_insert w_commit i_commit w_check
permutation w_begin w_rename i_begin i_insert w_commit i_commit w_check
