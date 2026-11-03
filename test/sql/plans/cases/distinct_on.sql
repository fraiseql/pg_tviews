-- entity: user
-- DISTINCT ON: the rows are keyed by the DISTINCT ON column (ADR 0169).
CREATE TABLE tb_user (pk_user int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text, org text);
CREATE TABLE tb_post (pk_post int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), fk_user int REFERENCES tb_user, title text, score int);
CREATE VIEW v_case AS
  SELECT DISTINCT ON (p.fk_user) p.fk_user AS pk_user, p.id, jsonb_build_object('best', p.title) AS data
  FROM tb_post p ORDER BY p.fk_user, p.score DESC, p.pk_post;
