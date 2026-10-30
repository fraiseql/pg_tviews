# A rename of a customer by a writer whose policy hides every order, written
# into the lines two joins away as a fan-out patch (line -> order -> customer),
# while the table's owner re-points one of those orders (#268). The lookup
# through the orders runs as the TVIEW's owner, so it reaches the hidden orders
# and waits for the re-point; nothing it read may reach the writer. The hidden
# orders and lines carry marker values, and the writer's error, its detail,
# hint and context are checked for them: a lock timeout, a 40001 under
# REPEATABLE READ, and, waiting, a write that ends with every line fresh.

setup
{
  CREATE EXTENSION IF NOT EXISTS jsonb_delta;
  CREATE EXTENSION pg_tviews;
  CREATE ROLE regress_hidden_writer;
  CREATE TABLE tb_customer (pk_customer bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                            name text);
  CREATE TABLE tb_order (pk_order bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                         fk_customer bigint NOT NULL, note text NOT NULL);
  CREATE TABLE tb_line (pk_line bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                        fk_order bigint NOT NULL, sku text);
  INSERT INTO tb_customer (pk_customer, name) VALUES (1, 'ann'), (2, 'bob');
  INSERT INTO tb_order (pk_order, fk_customer, note) VALUES (7770010, 1, 'hidden-note');
  INSERT INTO tb_line (pk_line, fk_order, sku) VALUES (100, 7770010, 'hidden-sku');
  SELECT tviews.pg_tviews_create('tv_line', $$
    SELECT l.pk_line, l.id, l.fk_order,
           jsonb_build_object('sku', l.sku, 'customer', c.name) AS data
    FROM tb_line l JOIN tb_order o ON o.pk_order = l.fk_order
    JOIN tb_customer c ON c.pk_customer = o.fk_customer $$);
  GRANT SELECT, UPDATE ON tb_customer TO regress_hidden_writer;
  GRANT SELECT ON tb_order TO regress_hidden_writer;
  ALTER TABLE tb_order ENABLE ROW LEVEL SECURITY;
  CREATE POLICY none_visible ON tb_order USING (false);
  -- The rename, its error caught whole: SQLSTATE, message, and whether any
  -- field of it names a hidden value.
  CREATE FUNCTION rename_customer(new_name text)
    RETURNS TABLE (state text, message text, names_hidden boolean)
    LANGUAGE plpgsql AS $$
    DECLARE m text; d text; h text; c text;
    BEGIN
      UPDATE tb_customer SET name = new_name WHERE pk_customer = 1;
      RETURN QUERY SELECT '00000'::text, NULL::text, false;
    EXCEPTION WHEN OTHERS THEN
      GET STACKED DIAGNOSTICS state = RETURNED_SQLSTATE, m = MESSAGE_TEXT,
          d = PG_EXCEPTION_DETAIL, h = PG_EXCEPTION_HINT, c = PG_EXCEPTION_CONTEXT;
      RETURN QUERY SELECT state, m, concat_ws(' ', m, d, h, c) ~ '7770010|hidden';
    END $$;
}

teardown
{
  DROP TABLE tv_line;
  DROP EXTENSION pg_tviews CASCADE;
  DROP TABLE tb_line, tb_order, tb_customer;
  DROP FUNCTION rename_customer(text);
  DROP ROLE regress_hidden_writer;
}

session owner
step o_begin    { BEGIN; }
step o_move     { UPDATE tb_order SET fk_customer = 2 WHERE pk_order = 7770010; }
step o_sleep    { SELECT pg_sleep(1); }
step o_commit   { COMMIT; }
step o_rollback { ROLLBACK; }
step o_check
{
  SELECT t.pk_line, t.data->>'customer' AS customer, t.data = v.data AS fresh
  FROM tv_line t FULL JOIN tviews.public__tv_line v USING (pk_line) ORDER BY 1;
}

session writer
setup           { SET ROLE regress_hidden_writer; }
step w_timeout  { SET lock_timeout = '200ms'; }
step w_rr       { BEGIN ISOLATION LEVEL REPEATABLE READ; SELECT count(*) AS orders_seen FROM tb_order; }
step w_rename   { SELECT * FROM rename_customer('amy'); }
step w_commit   { COMMIT; }

# The writer gives up waiting: its lock timeout names no hidden value.
permutation o_begin o_move w_timeout w_rename o_sleep o_rollback o_check
# REPEATABLE READ fails at once with 40001, naming no hidden value.
permutation o_begin o_move w_rr w_rename w_commit o_rollback o_check
# The writer waits for the re-point, then patches no line it moved away.
permutation o_begin o_move w_rename o_commit o_check
