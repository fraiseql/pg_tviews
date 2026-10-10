-- Schema of the concurrency harness (run.sh): :users users, no posts. tv_post
-- embeds tv_user; tv_note joins tb_user directly, so a rename reaches both
-- through propagation and through a mapping query.
SET client_min_messages = warning;
CREATE EXTENSION IF NOT EXISTS jsonb_delta;
CREATE EXTENSION pg_tviews;
CREATE TABLE tb_user (pk_user bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      name text);
CREATE TABLE tb_post (pk_post bigserial PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_user bigint NOT NULL REFERENCES tb_user, title text);
CREATE INDEX ON tb_post (fk_user);
INSERT INTO tb_user (pk_user, name) SELECT g, 'u' || g FROM generate_series(1, :users) g;
SELECT tviews.pg_tviews_create('tv_user', $$
    SELECT pk_user, id, jsonb_build_object('name', name) AS data FROM tb_user $$);
SELECT tviews.pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user, jsonb_build_object('title', p.title, 'author', u.data) AS data
    FROM tb_post p JOIN tv_user u ON u.pk_user = p.fk_user $$);
SELECT tviews.pg_tviews_create('tv_note', $$
    SELECT p.pk_post AS pk_note, p.id, jsonb_build_object('author', upper(u.name)) AS data
    FROM tb_post p JOIN tb_user u ON u.pk_user = p.fk_user $$);
