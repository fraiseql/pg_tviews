-- The patch #228 would generate for username_array, written by hand: the
-- comments' authors rewritten in place in each post embedding them, found
-- through the base join, triggers off.
\set u random(1, 2000)
BEGIN;
SET LOCAL session_replication_role = replica;
UPDATE bench.tb_user SET username = 'user' || :u || '_' || (random() * 1e9)::bigint WHERE pk_user = :u;
UPDATE bench.tv_post t
SET data = jsonb_set(t.data, '{comments}', (
        SELECT jsonb_agg(CASE WHEN e #>> '{author,id}' = u.id::text
                              THEN jsonb_set(e, '{author,username}', to_jsonb(u.username)) ELSE e END
                         ORDER BY o)
        FROM jsonb_array_elements(t.data -> 'comments') WITH ORDINALITY a(e, o))),
    updated_at = now()
FROM bench.tb_user u
WHERE u.pk_user = :u
  AND t.pk_post IN (SELECT c.fk_post FROM bench.tb_comment c WHERE c.fk_author = :u);
COMMIT;
