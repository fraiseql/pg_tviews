-- entity: user
-- A CTE read once: its table maps through the CTE's columns.
CREATE TABLE tb_user (pk_user int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text, org text);
CREATE TABLE tb_post (pk_post int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), fk_user int REFERENCES tb_user, title text, score int);
CREATE VIEW v_case AS
  WITH counts AS (SELECT fk_user, count(*) AS n FROM tb_post GROUP BY fk_user)
  SELECT u.pk_user, u.id, jsonb_build_object('name', u.name, 'posts', coalesce(c.n, 0)) AS data
  FROM tb_user u LEFT JOIN counts c ON c.fk_user = u.pk_user;
