-- Regression test for issue #150:
--   "Operations runbooks query relations that do not exist"
--
-- The runbooks under docs/operations and docs/TROUBLESHOOTING.md queried relations
-- and columns pg_tviews never had (pg_tviews_metadata, pg_tviews_queue,
-- last_refreshed, last_refresh_duration_ms, ...): an operator following them in an
-- incident got "relation does not exist". #138 checks the pg_tviews_* names; this
-- test runs every script under docs/operations/**/scripts/ against a database with
-- TVIEWs, and fails on the phantom columns, which #138 cannot see.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_150_runbook_scripts.sql
--
-- expect-output: issue #150 runbook scripts: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

-- A small set of TVIEWs, as an operator would have them.
CREATE TABLE tb_user (pk_user bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
CREATE TABLE tb_post (pk_post bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_user bigint NOT NULL REFERENCES tb_user, title text);
INSERT INTO tb_user VALUES (1, DEFAULT, 'ann'), (2, DEFAULT, 'bob');
INSERT INTO tb_post VALUES (1, DEFAULT, 1, 'p1'), (2, DEFAULT, 2, 'p2');
SELECT pg_tviews_create('tv_user', $$ SELECT pk_user, id, jsonb_build_object('name', name) AS data FROM tb_user $$);
SELECT pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user, jsonb_build_object('title', p.title, 'author', u.data) AS data
    FROM tb_post p JOIN tv_user u ON u.pk_user = p.fk_user $$);
UPDATE tb_post SET title = 'p1+' WHERE pk_post = 1;

-- Every SQL script runs to the end; every shell script exits 0.
\setenv PGTV_DB :DBNAME
\! cd "$(git rev-parse --show-toplevel)" && for f in $(git ls-files 'docs/operations/*/scripts/*.sql' 'docs/operations/**/scripts/*.sql' | sort -u); do psql -X -q -v ON_ERROR_STOP=1 -d "$PGTV_DB" -f "$f" > /dev/null || { echo "#150 FAIL: $f"; exit 1; }; done && for f in $(git ls-files 'docs/operations/*/scripts/*.sh' 'docs/operations/**/scripts/*.sh' | sort -u); do PGDATABASE="$PGTV_DB" bash "$f" > /dev/null || { echo "#150 FAIL: $f"; exit 1; }; done
\if :SHELL_ERROR
  DO $$ BEGIN RAISE EXCEPTION '#150 FAIL: a runbook script failed (see above)'; END $$;
\endif

-- Columns pg_tviews never had, anywhere in the published docs.
\set phantom `cd "$(git rev-parse --show-toplevel)" && git ls-files README.md INTEGRATION_GUIDE.md 'docs/*.md' 'docs/*.sql' 'docs/*.sh' | grep -v -e '^docs/archive/' -e '^docs/adr/' | xargs grep -lE '\b(last_refreshed|last_refresh_duration_ms|refresh_log)\b' | paste -sd, -`
SELECT :'phantom' = '' AS no_phantom \gset
\if :no_phantom
\else
  \echo '#150 FAIL: docs use columns pg_tviews never had (last_refreshed, ...):' :'phantom'
  DO $$ BEGIN RAISE EXCEPTION '#150 FAIL: phantom columns in the docs (see above)'; END $$;
\endif

SELECT 'issue #150 runbook scripts: PASS' AS result;
