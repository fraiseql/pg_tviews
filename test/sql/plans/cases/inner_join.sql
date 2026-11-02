-- entity: post
-- An inner join: the joined table maps through its equality (mapped, one hop).
CREATE TABLE tb_user (pk_user int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text, org text);
CREATE TABLE tb_post (pk_post int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), fk_user int REFERENCES tb_user, title text, score int);
CREATE VIEW v_case AS
  SELECT p.pk_post, p.id, p.fk_user, jsonb_build_object('title', p.title, 'author', u.name) AS data
  FROM tb_post p JOIN tb_user u ON u.pk_user = p.fk_user;
