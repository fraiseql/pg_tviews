# Two writes of different users, each a change no TVIEW joins or filters on
# (#263). Each user commented on the other's post, so each write's refresh reads
# the other's user. A statement trigger between pg_tviews' delta triggers (which
# lock the written values) and its flush waits on a gate, one per session, so
# both writes have locked their own value before either refreshes; the gates
# open one after the other. Neither changed a link, so neither refresh locks the
# values it reads: both commit, and the TVIEWs are fresh.

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
  SELECT tviews.pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id,
           jsonb_build_object('title', p.title,
             'author', jsonb_build_object('name', u.name, 'bio', u.bio),
             'comments', COALESCE(jsonb_agg(jsonb_build_object('content', c.content,
                 'author', jsonb_build_object('name', cu.name, 'bio', cu.bio))
               ORDER BY c.pk_comment) FILTER (WHERE c.pk_comment IS NOT NULL), '[]')) AS data
    FROM tb_post p JOIN tb_user u ON u.pk_user = p.fk_author
    LEFT JOIN tb_comment c ON c.fk_post = p.pk_post
    LEFT JOIN tb_user cu ON cu.pk_user = c.fk_author
    GROUP BY p.pk_post, p.id, p.title, u.name, u.bio $$);
  -- Fires after pg_tviews' delta triggers ("trg_tview_delta_…") and before its
  -- flush ("trg_tview_flush_…"): statement triggers fire in name order.
  CREATE FUNCTION gate() RETURNS trigger LANGUAGE plpgsql AS $$
    BEGIN
      PERFORM pg_advisory_xact_lock_shared(263, current_setting('test.gate')::int);
      RETURN NULL;
    END $$;
  CREATE TRIGGER trg_tview_e_gate AFTER UPDATE ON tb_user
    FOR EACH STATEMENT EXECUTE FUNCTION gate();
}

teardown
{
  DROP TABLE tv_post;
  DROP EXTENSION pg_tviews CASCADE;
  DROP TABLE tb_comment, tb_post, tb_user;
  DROP FUNCTION gate();
}

session gate
step g_close   { SELECT pg_advisory_lock(263, 1), pg_advisory_lock(263, 2); }
step g_open1   { SELECT pg_advisory_unlock(263, 1); }
step g_open2   { SELECT pg_advisory_unlock(263, 2); }

session one
setup          { SET deadlock_timeout = '100ms'; SET test.gate = '1'; }
step o_bio     { UPDATE tb_user SET bio = 'ada!' WHERE pk_user = 1; }

session two
setup          { SET deadlock_timeout = '100ms'; SET test.gate = '2'; }
step t_bio     { UPDATE tb_user SET bio = 'bob!' WHERE pk_user = 2; }
step t_check
{
  SELECT t.pk_post, t.data = v.data AS fresh
  FROM tv_post t FULL JOIN tviews.public__tv_post v USING (pk_post) ORDER BY 1;
}

permutation g_close o_bio t_bio g_open1 g_open2 t_check
