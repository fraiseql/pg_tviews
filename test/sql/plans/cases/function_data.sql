-- entity: post
-- data computed by an immutable function of the row's own columns.
CREATE TABLE tb_user (pk_user int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text, org text);
CREATE TABLE tb_post (pk_post int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), fk_user int REFERENCES tb_user, title text, score int);
CREATE FUNCTION post_label(title text, score int) RETURNS text
  LANGUAGE sql IMMUTABLE RETURN title || ' (' || score || ')';
CREATE VIEW v_case AS
  SELECT p.pk_post, p.id, jsonb_build_object('label', post_label(p.title, p.score)) AS data
  FROM tb_post p;
