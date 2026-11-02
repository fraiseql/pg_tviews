-- entity: item
-- A UNION: one root per branch, keys disjoint by construction.
CREATE TABLE tb_user (pk_user int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text, org text);
CREATE TABLE tb_post (pk_post int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), fk_user int REFERENCES tb_user, title text, score int);
CREATE VIEW v_case AS
  SELECT pk_user * 2 AS pk_item, id, jsonb_build_object('kind', 'user', 'label', name) AS data FROM tb_user
  UNION ALL
  SELECT pk_post * 2 + 1, id, jsonb_build_object('kind', 'post', 'label', title) FROM tb_post;
