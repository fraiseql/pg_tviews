-- entity: post
-- A scalar subquery in the projection: its table maps through the correlation.
CREATE TABLE tb_user (pk_user int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text, org text);
CREATE TABLE tb_post (pk_post int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), fk_user int REFERENCES tb_user, title text, score int);
CREATE VIEW v_case AS
  SELECT p.pk_post, p.id, jsonb_build_object('title', p.title,
         'author', (SELECT u.name FROM tb_user u WHERE u.pk_user = p.fk_user)) AS data
  FROM tb_post p;
