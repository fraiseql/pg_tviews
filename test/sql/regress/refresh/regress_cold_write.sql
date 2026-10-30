-- Issue #266: what a backend's first write reads of the catalog.
--
-- A new backend, and every backend after TVIEW DDL anywhere, reads the catalog again
-- on its next write. That write reads the catalog rows of every TVIEW it reaches at
-- once, and each read runs from a plan the backend keeps: the plans outlive an
-- invalidation, which PostgreSQL's plan cache revalidates on its own.
--
-- pg_tviews_queue_stats() counts the backend's catalog lookups and kept plans; each
-- measured write runs in a new backend (\c) or reads them before and after.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/refresh/regress_cold_write.sql
-- expect-output: cold write: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE TABLE tb_author (pk_author bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                        name text, bio text, city text, email text);
CREATE TABLE tb_post (pk_post bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_author bigint NOT NULL REFERENCES tb_author, title text, body text);
CREATE TABLE tb_comment (pk_comment bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                         fk_post bigint NOT NULL REFERENCES tb_post,
                         fk_author bigint NOT NULL REFERENCES tb_author, body text);
INSERT INTO tb_author (pk_author, name, bio, city, email)
    SELECT g, 'a' || g, 'b' || g, 'c' || g, 'e' || g FROM generate_series(1, 5) g;
INSERT INTO tb_post (pk_post, fk_author, title, body)
    SELECT g, 1 + g % 5, 'p' || g, 'b' || g FROM generate_series(1, 20) g;
INSERT INTO tb_comment (pk_comment, fk_post, fk_author, body)
    SELECT g, 1 + g % 20, 1 + g % 5, 'c' || g FROM generate_series(1, 40) g;

CREATE FUNCTION lookups() RETURNS int LANGUAGE sql AS
$$ SELECT (tviews.pg_tviews_queue_stats()->>'catalog_lookups')::int $$;
CREATE FUNCTION plans() RETURNS int LANGUAGE sql AS
$$ SELECT (tviews.pg_tviews_queue_stats()->>'plans_prepared')::int $$;
CREATE FUNCTION must(ok boolean, msg text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'cold write FAIL: %', msg; END IF; END $$;

-- tv_comment maps a write to tb_author through its mapping query.
SELECT tviews.pg_tviews_create('tv_comment', $$
    SELECT c.pk_comment, c.id, c.fk_post,
           jsonb_build_object('body', c.body, 'author', jsonb_build_object('name', ca.name)) AS data
    FROM tb_comment c JOIN tb_author ca ON ca.pk_author = c.fk_author $$);

-- ── the names in a template cost no lookup ───────────────────────────────────
\c
BEGIN;
UPDATE tb_author SET name = name || '!' WHERE pk_author = 1;
SELECT lookups() AS narrow \gset
COMMIT;

SELECT tviews.pg_tviews_create_or_replace('tv_comment', $$
    SELECT c.pk_comment, c.id, c.fk_post,
           jsonb_build_object('body', c.body,
                              'author', jsonb_build_object('name', ca.name, 'bio', ca.bio, 'city', ca.city,
                                                          'email', ca.email, 'id', ca.id),
                              'post', jsonb_build_object('title', p.title, 'body', p.body,
                                                         'author_name', pa.name, 'author_city', pa.city)) AS data
    FROM tb_comment c
    JOIN tb_author ca ON ca.pk_author = c.fk_author
    JOIN tb_post p ON p.pk_post = c.fk_post
    JOIN tb_author pa ON pa.pk_author = p.fk_author $$);
\c
BEGIN;
UPDATE tb_author SET name = name || '!' WHERE pk_author = 1;
SELECT lookups() AS wide \gset
SELECT must(lookups() = :narrow,
            format('a cold write through a template of more names made %s catalog lookups, %s with fewer',
                   lookups(), :narrow));
COMMIT;

-- ── each further TVIEW a write reaches costs at most its template's read ────
SELECT tviews.pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_author,
           jsonb_build_object('title', p.title, 'author', jsonb_build_object('name', a.name)) AS data
    FROM tb_post p JOIN tb_author a ON a.pk_author = p.fk_author $$);
CREATE FUNCTION in_sync() RETURNS boolean LANGUAGE sql AS $$
    SELECT NOT EXISTS (SELECT 1 FROM tv_post t FULL JOIN tviews.public__tv_post v USING (pk_post)
                       WHERE t.data IS DISTINCT FROM v.data)
       AND NOT EXISTS (SELECT 1 FROM tv_comment t FULL JOIN tviews.public__tv_comment v USING (pk_comment)
                       WHERE t.data IS DISTINCT FROM v.data) $$;
SELECT tviews.pg_tviews_create('tv_author', $$
    SELECT a.pk_author, a.id,
           jsonb_build_object('name', a.name,
                              'n_posts', (SELECT count(*) FROM tb_post p WHERE p.fk_author = a.pk_author)) AS data
    FROM tb_author a $$);
\c
BEGIN;
UPDATE tb_author SET name = name || '!' WHERE pk_author = 2;
SELECT must(lookups() <= :wide + 2,
            format('a cold write reaching 3 TVIEWs made %s catalog lookups, one reaching 1 made %s',
                   lookups(), :wide));
COMMIT;
SELECT must(in_sync(), 'a TVIEW out of sync');

-- ── after an invalidation, no kept plan is prepared again ───────────────────
-- But the statements over a trigger's transition tables, which PostgreSQL does not
-- revalidate: tb_author's, two for each TVIEW mapping it through a query
-- (tv_comment, tv_post).
\c
UPDATE tb_comment SET body = body || '!' WHERE pk_comment = 1;
UPDATE tb_author SET name = name || '!' WHERE pk_author = 1;
SELECT plans() AS cold \gset
UPDATE tb_comment SET body = body || '!' WHERE pk_comment = 1;
UPDATE tb_author SET name = name || '!' WHERE pk_author = 1;
SELECT must(plans() = :cold, format('a warm write prepared %s plans', plans() - :cold));
SELECT tviews.pg_tviews_invalidate_caches('tviews.pg_tview_meta'::regclass);
UPDATE tb_comment SET body = body || '!' WHERE pk_comment = 2;
UPDATE tb_author SET name = name || '!' WHERE pk_author = 2;
SELECT must(plans() - :cold <= 4,
            format('the writes after an invalidation prepared %s plans again, of %s',
                   plans() - :cold, :cold));
SELECT must(in_sync(), 'a TVIEW out of sync after an invalidation');

-- ── the change feed names collections once a session turns it on ─────────────
\setenv PGTV_DB :DBNAME
\set notes `psql -X -q -d "$PGTV_DB" -c "LISTEN tviews_changes" -c "UPDATE tb_comment SET body = body || '!' WHERE pk_comment = 3" -c "SET pg_tviews.publish_changes = rows" -c "UPDATE tb_comment SET body = body || '?' WHERE pk_comment = 3" -c "SELECT 1" 2>&1 | grep 'Asynchronous notification' | sed -E 's/.*with payload "(.*)" received from.*/\1/' | paste -sd '|'`
SELECT must(:'notes' LIKE '%"c":{"Comment.fk_post":[%',
            'the feed turned on after a write named no collection: ' || :'notes');

\echo 'cold write: PASS'
