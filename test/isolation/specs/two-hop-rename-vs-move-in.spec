# A rename of a customer, written into the lines two joins away as a fan-out
# patch (line -> order -> customer), while an order of another customer moves to
# it (#268). The move has locked the order it moved and waits on a gate between
# pg_tviews' delta triggers and its flush; the rename then locks its customer,
# looks up that customer's orders and locks the ones it reached, which the move
# holds none of. Its lookup never locks before it reads: a lock on the orders
# taken before the lookup would wait for the move, which waits for the rename's
# customer to recompute its line, a deadlock. So would a lock on all the orders
# past the escalation threshold: the rename then maps and recomputes the lines
# instead of patching them. Both commit, and the lines show their order's
# customer.

setup
{
  CREATE EXTENSION IF NOT EXISTS jsonb_delta;
  CREATE EXTENSION pg_tviews;
  CREATE TABLE tb_customer (pk_customer bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                            name text);
  CREATE TABLE tb_order (pk_order bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                         fk_customer bigint NOT NULL);
  CREATE TABLE tb_line (pk_line bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                        fk_order bigint NOT NULL, sku text);
  INSERT INTO tb_customer (pk_customer, name) VALUES (1, 'ann'), (2, 'bob');
  INSERT INTO tb_order (pk_order, fk_customer) VALUES (10, 1), (20, 2), (21, 2);
  INSERT INTO tb_line (pk_line, fk_order, sku) VALUES (100, 10, 's'), (200, 20, 't'), (210, 21, 'u');
  SELECT tviews.pg_tviews_create('tv_line', $$
    SELECT l.pk_line, l.id, l.fk_order,
           jsonb_build_object('sku', l.sku, 'customer', c.name) AS data
    FROM tb_line l JOIN tb_order o ON o.pk_order = l.fk_order
    JOIN tb_customer c ON c.pk_customer = o.fk_customer $$);
  -- Fires after pg_tviews' delta triggers ("trg_tview_delta_…") and before its
  -- flush ("trg_tview_flush_…"): statement triggers fire in name order.
  CREATE FUNCTION gate() RETURNS trigger LANGUAGE plpgsql AS $$
    BEGIN
      PERFORM pg_advisory_xact_lock_shared(268, 1);
      RETURN NULL;
    END $$;
  CREATE TRIGGER trg_tview_e_gate AFTER UPDATE ON tb_order
    FOR EACH STATEMENT EXECUTE FUNCTION gate();
}

teardown
{
  DROP TABLE tv_line;
  DROP EXTENSION pg_tviews CASCADE;
  DROP TABLE tb_line, tb_order, tb_customer;
  DROP FUNCTION gate();
}

session gate
step g_close   { SELECT pg_advisory_lock(268, 1); }
step g_open    { SELECT pg_advisory_unlock(268, 1); }

session move
setup          { SET deadlock_timeout = '100ms'; }
step m_move    { UPDATE tb_order SET fk_customer = 2 WHERE pk_order = 10; }

session rename
setup          { SET deadlock_timeout = '100ms'; }
step n_escalate { SET pg_tviews.lock_escalation_threshold = 1; }
step n_begin   { BEGIN; }
step n_rename  { UPDATE tb_customer SET name = 'bea' WHERE pk_customer = 2; }
step n_commit  { COMMIT; }
step n_check
{
  SELECT t.pk_line, t.data->>'customer' AS customer, t.data = v.data AS fresh
  FROM tv_line t FULL JOIN tviews.public__tv_line v USING (pk_line) ORDER BY 1;
}

permutation g_close m_move n_begin n_rename g_open n_commit n_check
# The rename reaches two orders, past a threshold of one.
permutation g_close m_move n_escalate n_begin n_rename g_open n_commit n_check
