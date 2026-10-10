-- The stress workload's extra layer over setup.sql: users belong to orgs, and
-- tv_feed reads a post's org two hops away (post -> user -> org).
SET client_min_messages = warning;
CREATE TABLE tb_org (pk_org bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
INSERT INTO tb_org (pk_org, name) SELECT g, 'o' || g FROM generate_series(1, 50) g;
ALTER TABLE tb_user ADD COLUMN fk_org bigint REFERENCES tb_org;
UPDATE tb_user SET fk_org = 1 + pk_user % 50;
CREATE INDEX ON tb_user (fk_org);
SELECT tviews.pg_tviews_create('tv_feed', $$
    SELECT p.pk_post AS pk_feed, p.id, jsonb_build_object('org', upper(o.name)) AS data
    FROM tb_post p JOIN tb_user u ON u.pk_user = p.fk_user JOIN tb_org o ON o.pk_org = u.fk_org $$);
