# Two REPEATABLE READ writes of different users, each a change no TVIEW joins or
# filters on (#263). Each user wrote one post and commented on the other's, so
# each refresh patches both posts, one statement per shape: its own post, then the
# post it commented on, the other user's first. A row trigger on tv_post holds
# each refresh right after it wrote its first post, so each has locked the post the
# other writes last. Rows locked in key order before the patches, the second write
# waits on the first and fails with 40001, as a plain UPDATE of a row updated since
# its snapshot does; locked statement by statement, the two waited for each other
# and one failed with 40P01.

setup
{
  CREATE EXTENSION IF NOT EXISTS jsonb_delta;
  CREATE EXTENSION pg_tviews;
  CREATE TABLE tb_user (pk_user bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                        name text, bio text);
  CREATE TABLE tb_post (pk_post bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                        fk_author bigint NOT NULL REFERENCES tb_user, title text);
  CREATE TABLE tb_comment (pk_comment bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                           fk_post bigint NOT NULL REFERENCES tb_post,
                           fk_author bigint NOT NULL REFERENCES tb_user, content text);
  INSERT INTO tb_user (pk_user, name, bio) VALUES (1, 'ada', ''), (2, 'bob', '');
  INSERT INTO tb_post (pk_post, fk_author, title) VALUES (1, 1, 'pa'), (2, 2, 'pb');
  INSERT INTO tb_comment (pk_comment, fk_post, fk_author, content)
    VALUES (1, 2, 1, 'ada on pb'), (2, 1, 2, 'bob on pa');
  SELECT tviews.pg_tviews_create('tv_user', $$
    SELECT pk_user, id, jsonb_build_object('name', name, 'bio', bio) AS data FROM tb_user $$);
  SELECT tviews.pg_tviews_create('tv_comment', $$
    SELECT c.pk_comment, c.id, c.fk_post, c.fk_author,
           jsonb_build_object('content', c.content, 'author', u.data) AS data
    FROM tb_comment c JOIN tv_user u ON u.pk_user = c.fk_author $$);
  SELECT tviews.pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_author,
           jsonb_build_object('title', p.title, 'author', u.data,
             'comments', COALESCE((SELECT jsonb_agg(c.data ORDER BY c.pk_comment)
                                     FROM tv_comment c WHERE c.fk_post = p.pk_post), '[]')) AS data
    FROM tb_post p JOIN tv_user u ON u.pk_user = p.fk_author $$);
  -- Holds a refresh right after it wrote its first tv_post row, keeping that
  -- row locked; the gate of each session opens once.
  CREATE FUNCTION gate() RETURNS trigger LANGUAGE plpgsql AS $$
    BEGIN
      PERFORM pg_advisory_xact_lock_shared(264, current_setting('test.gate')::int);
      RETURN NULL;
    END $$;
  CREATE TRIGGER gate AFTER UPDATE ON tv_post FOR EACH ROW EXECUTE FUNCTION gate();
}

teardown
{
  DROP TABLE tv_post, tv_comment, tv_user;
  DROP EXTENSION pg_tviews CASCADE;
  DROP TABLE tb_comment, tb_post, tb_user;
  DROP FUNCTION gate();
}

session gate
step g_close   { SELECT pg_advisory_lock(264, 1), pg_advisory_lock(264, 2); }
step g_open1   { SELECT pg_advisory_unlock(264, 1); }
step g_open2   { SELECT pg_advisory_unlock(264, 2); }

session one
setup          { SET deadlock_timeout = '100ms'; SET test.gate = '1';
                 SET default_transaction_isolation = 'repeatable read'; }
step o_bio     { UPDATE tb_user SET bio = 'ada!' WHERE pk_user = 1; }

session two
setup          { SET deadlock_timeout = '100ms'; SET test.gate = '2';
                 SET default_transaction_isolation = 'repeatable read'; }
step t_bio     { UPDATE tb_user SET bio = 'bob!' WHERE pk_user = 2; }
step t_check
{
  SELECT t.pk_post, t.data = v.data AS fresh
  FROM tv_post t FULL JOIN tviews.public__tv_post v USING (pk_post) ORDER BY 1;
}

permutation g_close o_bio t_bio g_open1 g_open2 t_check
