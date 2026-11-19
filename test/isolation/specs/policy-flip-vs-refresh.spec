# A writer moves a row to another tenant (the column a policy on tv_* filters
# on) while a full refresh runs, on a table under FORCE ROW LEVEL SECURITY with
# an owner policy that filters nothing. Whichever commits first, tv_contract
# ends holding the committed tenant, and a reader reads each tenant's rows.

setup
{
  CREATE EXTENSION IF NOT EXISTS jsonb_delta;
  CREATE EXTENSION pg_tviews;
  CREATE ROLE regress_pf_owner;
  CREATE ROLE regress_pf_reader;
  GRANT CREATE ON SCHEMA public TO regress_pf_owner;
  SET ROLE regress_pf_owner;
  CREATE TABLE tb_contract (pk_contract bigint PRIMARY KEY,
                            id uuid NOT NULL DEFAULT gen_random_uuid(),
                            tenant_id int NOT NULL, name text);
  INSERT INTO tb_contract (pk_contract, tenant_id, name) VALUES (1, 10, 'c1'), (2, 10, 'c2');
  SELECT tviews.pg_tviews_create('tv_contract', $$
    SELECT c.pk_contract, c.id, c.tenant_id, jsonb_build_object('name', c.name) AS data
    FROM tb_contract c $$);
  ALTER TABLE tv_contract ENABLE ROW LEVEL SECURITY, FORCE ROW LEVEL SECURITY;
  CREATE POLICY tviews_owner ON tv_contract FOR ALL TO regress_pf_owner
    USING (true) WITH CHECK (true);
  CREATE POLICY tenant ON tv_contract FOR SELECT TO regress_pf_reader
    USING (tenant_id = current_setting('app.tenant_id')::int);
  GRANT SELECT ON tv_contract TO regress_pf_reader;
  RESET ROLE;
}

teardown
{
  DROP TABLE tv_contract;
  DROP EXTENSION pg_tviews CASCADE;
  DROP TABLE tb_contract;
  REVOKE CREATE ON SCHEMA public FROM regress_pf_owner;
  DROP ROLE regress_pf_owner;
  DROP ROLE regress_pf_reader;
}

session mover
setup         { SET ROLE regress_pf_owner; }
step m_begin  { BEGIN; }
step m_move   { UPDATE tb_contract SET tenant_id = 20 WHERE pk_contract = 1; }
step m_commit { COMMIT; }

session refresher
setup         { SET ROLE regress_pf_owner; }
step r_begin   { BEGIN; }
step r_refresh { SELECT tviews.pg_tviews_refresh('tv_contract') IS NOT NULL AS refreshed; }
step r_commit  { COMMIT; }

session checker
step c_check
{
  SELECT t.pk_contract, t.tenant_id, t.tenant_id = v.tenant_id AND t.data = v.data AS fresh
  FROM tv_contract t JOIN tviews.public__tv_contract v USING (pk_contract) ORDER BY 1;
}
step c_reader
{
  BEGIN;
  SET LOCAL ROLE regress_pf_reader;
  SET LOCAL app.tenant_id = '20';
  SELECT pk_contract FROM tv_contract ORDER BY 1;
  COMMIT;
}

permutation m_begin m_move r_begin r_refresh m_commit r_commit c_check c_reader
permutation r_begin r_refresh m_begin m_move r_commit m_commit c_check c_reader
