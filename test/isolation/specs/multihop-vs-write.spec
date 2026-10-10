# A post inserted while the organisation of its author is renamed, two hops
# away: once both have committed, the new post holds the organisation's
# committed name, whether through embedded TVIEWs (tv_post → tv_user → tv_org)
# or a two-hop join of base tables (tv_feed).

setup
{
  CREATE EXTENSION IF NOT EXISTS jsonb_delta;
  CREATE EXTENSION pg_tviews;
  CREATE TABLE tb_org (pk_org bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                       name text);
  CREATE TABLE tb_user (pk_user bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                        fk_org bigint NOT NULL REFERENCES tb_org, name text);
  CREATE TABLE tb_post (pk_post bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                        fk_user bigint NOT NULL REFERENCES tb_user, title text);
  INSERT INTO tb_org (pk_org, name) VALUES (1, 'acme');
  INSERT INTO tb_user (pk_user, fk_org, name) VALUES (1, 1, 'ada');
  INSERT INTO tb_post (pk_post, fk_user, title) VALUES (1, 1, 'p1');
  SELECT tviews.pg_tviews_create('tv_org', $$
      SELECT pk_org, id, jsonb_build_object('name', name) AS data FROM tb_org $$);
  SELECT tviews.pg_tviews_create('tv_user', $$
      SELECT u.pk_user, u.id, u.fk_org, jsonb_build_object('name', u.name, 'org', o.data) AS data
      FROM tb_user u JOIN tv_org o ON o.pk_org = u.fk_org $$);
  SELECT tviews.pg_tviews_create('tv_post', $$
      SELECT p.pk_post, p.id, p.fk_user, jsonb_build_object('title', p.title, 'author', u.data) AS data
      FROM tb_post p JOIN tv_user u ON u.pk_user = p.fk_user $$);
  SELECT tviews.pg_tviews_create('tv_feed', $$
      SELECT p.pk_post AS pk_feed, p.id, jsonb_build_object('org', upper(o.name)) AS data
      FROM tb_post p JOIN tb_user u ON u.pk_user = p.fk_user JOIN tb_org o ON o.pk_org = u.fk_org $$);
}

teardown
{
  DROP TABLE tv_feed, tv_post, tv_user, tv_org;
  DROP EXTENSION pg_tviews CASCADE;
  DROP TABLE tb_post, tb_user, tb_org;
}

session inserter
step i_begin  { BEGIN; }
step i_insert { INSERT INTO tb_post (pk_post, fk_user, title) VALUES (2, 1, 'p2'); }
step i_commit { COMMIT; }

session renamer
step r_begin  { BEGIN; }
step r_rename { UPDATE tb_org SET name = 'umbrella' WHERE pk_org = 1; }
step r_commit { COMMIT; }
step r_check
{
  SELECT 'post' AS tv, t.pk_post AS pk, t.data->'author'->'org'->>'name' AS org, t.data = v.data AS fresh
    FROM tv_post t JOIN tviews.public__tv_post v USING (pk_post)
  UNION ALL
  SELECT 'feed', t.pk_feed, t.data->>'org', t.data = v.data
    FROM tv_feed t JOIN tviews.public__tv_feed v USING (pk_feed)
  ORDER BY 1, 2;
}

permutation i_begin i_insert r_begin r_rename i_commit r_commit r_check
permutation r_begin r_rename i_begin i_insert r_commit i_commit r_check
