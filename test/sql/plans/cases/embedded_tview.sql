-- entity: post
-- An embedded TVIEW: tv_user is read through its key (propagated, an embed).
CREATE TABLE tb_user (pk_user int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text, org text);
CREATE TABLE tb_post (pk_post int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), fk_user int REFERENCES tb_user, title text, score int);
SELECT tviews.pg_tviews_create('tv_user', $$SELECT pk_user, id, jsonb_build_object('name', name) AS data FROM tb_user$$);
CREATE VIEW v_case AS
  SELECT p.pk_post, p.id, p.fk_user, jsonb_build_object('title', p.title, 'author', u.data) AS data
  FROM tb_post p JOIN tv_user u ON u.pk_user = p.fk_user;
