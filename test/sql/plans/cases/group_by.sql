-- entity: org
-- GROUP BY: an aggregate's rows are its groups; no table is the root.
CREATE TABLE tb_user (pk_user int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text, org text);
CREATE TABLE tb_post (pk_post int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), fk_user int REFERENCES tb_user, title text, score int);
CREATE TABLE tb_org (pk_org int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
ALTER TABLE tb_user ADD COLUMN fk_org int REFERENCES tb_org;
CREATE VIEW v_case AS
  SELECT o.pk_org, o.id, jsonb_build_object('name', o.name, 'members', count(u.pk_user)) AS data
  FROM tb_org o LEFT JOIN tb_user u ON u.fk_org = o.pk_org GROUP BY o.pk_org, o.id, o.name;
