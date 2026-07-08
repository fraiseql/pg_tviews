-- Regression test for issue #51 (part 3/3):
--   "Aliased DISTINCT ON dedup key breaks incremental refresh."
--
-- When the DISTINCT ON key is projected under a DIFFERENT name than its
-- source column (e.g. `DISTINCT ON (c.id_contract) c.id_contract AS pk_contract`),
-- the backing view / tview expose the OUTPUT column (`pk_contract`) while the
-- stored dedup key is the raw SOURCE column (`id_contract`). Refresh then runs
--   SELECT COUNT(*) FROM v_contract WHERE id_contract::text = $1
-- which fails with `column "id_contract" does not exist`, and the tview never
-- reflects base-table changes. Additionally the materialized table is created
-- with NO primary key, so the UPSERT's ON CONFLICT target is invalid.
--
-- Correct behaviour: an aliased DISTINCT ON tview must reflect INSERT / UPDATE /
-- DELETE on its base table, keeping exactly one winning row per dedup key.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_51_distinct_on_alias.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

DROP TABLE IF EXISTS tv_contract CASCADE;
DROP VIEW  IF EXISTS v_contract CASCADE;
DROP TABLE IF EXISTS tb_contract CASCADE;

-- Contract-versioning table: each row is a contract version; `id_contract`
-- groups versions of the same logical contract (the dedup key), `pk_contract`
-- is the per-row identity. The read model keeps the latest version per contract,
-- keyed by the logical contract id projected as `pk_contract`.
CREATE TABLE tb_contract (
    pk_contract  INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id           UUID DEFAULT gen_random_uuid() NOT NULL,
    id_contract  INTEGER NOT NULL,
    version_no   INTEGER NOT NULL,
    status       TEXT
);
INSERT INTO tb_contract (id_contract, version_no, status) VALUES
  (100, 1, 'draft'), (100, 2, 'active'),
  (200, 1, 'draft');

SELECT pg_tviews_create('tv_contract', $TV$
  SELECT DISTINCT ON (c.id_contract)
         c.id_contract AS pk_contract,
         c.id,
         jsonb_build_object('status', c.status, 'version', c.version_no) AS data
  FROM tb_contract c
  ORDER BY c.id_contract, c.version_no DESC
$TV$);

-- Baseline: latest version per contract materialized (contract 100 -> active v2).
DO $$ BEGIN
  IF (SELECT data->>'status' FROM tv_contract WHERE pk_contract = 100) <> 'active' THEN
    RAISE EXCEPTION '#51 setup FAIL: tv_contract 100 did not materialize status=active (got %)',
      (SELECT data->>'status' FROM tv_contract WHERE pk_contract = 100);
  END IF;
  IF (SELECT count(*) FROM tv_contract) <> 2 THEN
    RAISE EXCEPTION '#51 setup FAIL: expected 2 tview rows, got %',
      (SELECT count(*) FROM tv_contract);
  END IF;
END $$;

-- (1) UPDATE the winning version's own column -> tview must reflect it.
UPDATE tb_contract SET status = 'signed' WHERE id_contract = 100 AND version_no = 2;
DO $$ BEGIN
  IF (SELECT data->>'status' FROM tv_contract WHERE pk_contract = 100) <> 'signed' THEN
    RAISE EXCEPTION '#51 FAIL: aliased DISTINCT ON refresh lost UPDATE — status is % (expected signed)',
      (SELECT data->>'status' FROM tv_contract WHERE pk_contract = 100);
  END IF;
END $$;

-- (2) INSERT a newer version -> new winner must surface, still one row per key.
INSERT INTO tb_contract (id_contract, version_no, status) VALUES (100, 3, 'renewed');
DO $$ BEGIN
  IF (SELECT data->>'status' FROM tv_contract WHERE pk_contract = 100) <> 'renewed' THEN
    RAISE EXCEPTION '#51 FAIL: newer version not reflected — status is % (expected renewed)',
      (SELECT data->>'status' FROM tv_contract WHERE pk_contract = 100);
  END IF;
  IF (SELECT count(*) FROM tv_contract WHERE pk_contract = 100) <> 1 THEN
    RAISE EXCEPTION '#51 FAIL: dedup broken — % rows for contract 100 (expected 1)',
      (SELECT count(*) FROM tv_contract WHERE pk_contract = 100);
  END IF;
END $$;

-- (3) DELETE all versions of a contract -> its tview row must disappear.
DELETE FROM tb_contract WHERE id_contract = 200;
DO $$ BEGIN
  IF EXISTS (SELECT 1 FROM tv_contract WHERE pk_contract = 200) THEN
    RAISE EXCEPTION '#51 FAIL: DELETE not propagated — tv_contract 200 still present';
  END IF;
END $$;

-- (4) DISTINCT ON keyed on `id` (UUID), UNALIASED — output name == source name,
-- and the tview PK is the UUID `id` column (a different create-table PK branch than
-- the aliased pk_<entity> case above). Confirms the source/output split stays
-- backward compatible when the two coincide.
DROP TABLE IF EXISTS tv_event CASCADE;
DROP VIEW  IF EXISTS v_event CASCADE;
DROP TABLE IF EXISTS tb_event CASCADE;
CREATE TABLE tb_event (
    pk_event INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id       UUID NOT NULL,
    seq      INTEGER NOT NULL,
    status   TEXT
);
INSERT INTO tb_event (id, seq, status) VALUES
  ('11111111-1111-1111-1111-111111111111', 1, 'new'),
  ('11111111-1111-1111-1111-111111111111', 2, 'ack'),
  ('22222222-2222-2222-2222-222222222222', 1, 'new');

SELECT pg_tviews_create('tv_event', $TV$
  SELECT DISTINCT ON (e.id) e.pk_event, e.id,
         jsonb_build_object('status', e.status, 'seq', e.seq) AS data
  FROM tb_event e
  ORDER BY e.id, e.seq DESC
$TV$);

DO $$ BEGIN
  IF (SELECT data->>'status' FROM tv_event
      WHERE id = '11111111-1111-1111-1111-111111111111') <> 'ack' THEN
    RAISE EXCEPTION '#51 setup FAIL: id-keyed DISTINCT ON did not materialize latest (ack)';
  END IF;
END $$;

UPDATE tb_event SET status = 'closed'
  WHERE id = '11111111-1111-1111-1111-111111111111' AND seq = 2;
DO $$ BEGIN
  IF (SELECT data->>'status' FROM tv_event
      WHERE id = '11111111-1111-1111-1111-111111111111') <> 'closed' THEN
    RAISE EXCEPTION '#51 FAIL: id-keyed DISTINCT ON refresh lost UPDATE — status is %',
      (SELECT data->>'status' FROM tv_event
       WHERE id = '11111111-1111-1111-1111-111111111111');
  END IF;
END $$;

SELECT '#51 PASS: aliased + id-keyed DISTINCT ON tviews refresh on INSERT/UPDATE/DELETE' AS result;
