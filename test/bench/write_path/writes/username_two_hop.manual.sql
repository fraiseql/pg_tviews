-- The patch #228 would generate for username_two_hop, written by hand: the post
-- author's name rewritten in each comment on the user's posts, found through the
-- intermediate table, triggers off.
\set u random(1, 2000)
BEGIN;
SET LOCAL session_replication_role = replica;
UPDATE bench.tb_user SET username = 'user' || :u || '_' || (random() * 1e9)::bigint WHERE pk_user = :u;
UPDATE bench.tv_comment t
SET data = jsonb_set(t.data, '{post,author,username}', to_jsonb(u.username)), updated_at = now()
FROM bench.tb_user u
WHERE u.pk_user = :u
  AND t.fk_post IN (SELECT p.pk_post FROM bench.tb_post p WHERE p.fk_author = :u);
COMMIT;
