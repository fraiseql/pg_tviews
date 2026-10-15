# One transaction deletes every tag of posts 1 and 2: their groups leave the
# aggregate TVIEW through the batch refresh, which deletes the keys its upsert
# no longer produced. The other adds a tag to post 1, bringing its group back.
# Whichever commits first, the TVIEW ends equal to its view: post 1's group
# with the new tag, no row for post 2.

setup
{
  CREATE EXTENSION IF NOT EXISTS jsonb_delta;
  CREATE EXTENSION pg_tviews;
  CREATE TABLE tb_tag (pk_tag bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                       fk_post bigint NOT NULL, name text NOT NULL);
  INSERT INTO tb_tag (pk_tag, fk_post, name) VALUES (1, 1, 'a'), (2, 1, 'b'), (3, 2, 'c'), (4, 3, 'd');
  SELECT tviews.pg_tviews_create('tv_post_tags', $$
      SELECT g.fk_post AS pk_post_tags, md5(g.fk_post::text)::uuid AS id,
             jsonb_build_object('tags', jsonb_agg(g.name ORDER BY g.name)) AS data
      FROM tb_tag g GROUP BY g.fk_post $$,
      jsonb_build_object('group_keys', jsonb_build_object('tb_tag', 'fk_post')));
}

teardown
{
  DROP TABLE tv_post_tags;
  DROP EXTENSION pg_tviews CASCADE;
  DROP TABLE tb_tag;
}

session d
step d_begin  { BEGIN; }
step d_delete { DELETE FROM tb_tag WHERE fk_post IN (1, 2); }
step d_commit { COMMIT; }

session i
step i_begin  { BEGIN; }
step i_insert { INSERT INTO tb_tag (pk_tag, fk_post, name) VALUES (5, 1, 'e'); }
step i_commit { COMMIT; }
step i_check
{
  SELECT pk_post_tags, t.data, v.data AS view_data, t.data IS NOT DISTINCT FROM v.data AS fresh
  FROM tv_post_tags t FULL JOIN tviews.public__tv_post_tags v USING (pk_post_tags)
  ORDER BY 1;
}

# The delete commits first; the insert's refresh waited for it.
permutation d_begin d_delete i_begin i_insert d_commit i_commit i_check
# The insert commits first; the delete's refresh waited for it and keeps post 1.
permutation i_begin i_insert d_begin d_delete i_commit d_commit i_check
