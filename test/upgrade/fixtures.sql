-- Fixture TVIEWs for the upgrade checks (test/upgrade/upgrade_check.sh). Created with
-- the PREVIOUS release, so only use what every supported release offers: unqualified
-- pg_tviews_create / pg_tviews_create_aggregate on a search_path that reaches them.

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE TABLE tb_user (
    pk_user int PRIMARY KEY,
    id      uuid NOT NULL DEFAULT gen_random_uuid(),
    name    text NOT NULL,
    bio     text
);
CREATE TABLE tb_post (
    pk_post int PRIMARY KEY,
    id      uuid NOT NULL DEFAULT gen_random_uuid(),
    fk_user int NOT NULL REFERENCES tb_user,
    title   text NOT NULL
);
CREATE TABLE tb_comment (
    pk_comment int PRIMARY KEY,
    id         uuid NOT NULL DEFAULT gen_random_uuid(),
    fk_post    int NOT NULL REFERENCES tb_post,
    body       text NOT NULL
);
CREATE TABLE tb_order (
    pk_order int PRIMARY KEY,
    id       uuid NOT NULL DEFAULT gen_random_uuid(),
    fk_user  int NOT NULL REFERENCES tb_user,
    total    numeric NOT NULL
);
CREATE SCHEMA app;
CREATE TABLE app.tb_note (
    pk_note int PRIMARY KEY,
    id      uuid NOT NULL DEFAULT gen_random_uuid(),
    body    text NOT NULL
);

INSERT INTO tb_user (pk_user, name, bio) VALUES (1, 'alice', 'a'), (2, 'bob', 'b');
INSERT INTO tb_post (pk_post, fk_user, title) VALUES (1, 1, 'p1'), (2, 1, 'p2'), (3, 2, 'p3');
INSERT INTO tb_comment (pk_comment, fk_post, body) VALUES (1, 1, 'c1'), (2, 1, 'c2');
INSERT INTO tb_order (pk_order, fk_user, total) VALUES (1, 1, 10), (2, 2, 5);
INSERT INTO app.tb_note (pk_note, body) VALUES (1, 'n1');

-- Plain.
SELECT pg_tviews_create('tv_user', $$
    SELECT pk_user, id, jsonb_build_object('name', name, 'bio', bio) AS data FROM tb_user $$);
-- Cascading (embeds another TVIEW) and array (aggregates children).
SELECT pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user,
           jsonb_build_object(
               'title', p.title,
               'author', u.data,
               'comments', COALESCE(jsonb_agg(jsonb_build_object('body', c.body)
                   ORDER BY c.pk_comment) FILTER (WHERE c.pk_comment IS NOT NULL),
                   '[]'::jsonb)) AS data
    FROM tb_post p
    JOIN v_user u ON u.pk_user = p.fk_user
    LEFT JOIN tb_comment c ON c.fk_post = p.pk_post
    GROUP BY p.pk_post, p.id, p.fk_user, p.title, u.data $$);
-- Aggregate.
SELECT pg_tviews_create_aggregate('tv_user_orders', $$
    SELECT o.fk_user AS pk_user_orders, u.id,
           jsonb_build_object('orders', count(*), 'total', sum(o.total)) AS data
    FROM tb_order o JOIN tb_user u ON u.pk_user = o.fk_user
    GROUP BY o.fk_user, u.id
$$, '{"tb_order": "fk_user", "tb_user": "pk_user"}');
-- Off the search_path.
SET search_path TO app, public;
SELECT pg_tviews_create('tv_note', $$
    SELECT pk_note, id, jsonb_build_object('body', body) AS data FROM app.tb_note $$);
RESET search_path;
