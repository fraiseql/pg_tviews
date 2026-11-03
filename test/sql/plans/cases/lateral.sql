-- entity: user
-- LATERAL: the correlated subquery's table maps through its correlation.
CREATE TABLE tb_user (pk_user int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text, org text);
CREATE TABLE tb_post (pk_post int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), fk_user int REFERENCES tb_user, title text, score int);
CREATE VIEW v_case AS
  SELECT u.pk_user, u.id, jsonb_build_object('name', u.name, 'posts', l.posts) AS data
  FROM tb_user u, LATERAL (SELECT jsonb_agg(p.title ORDER BY p.pk_post) AS posts
                           FROM tb_post p WHERE p.fk_user = u.pk_user) l;
