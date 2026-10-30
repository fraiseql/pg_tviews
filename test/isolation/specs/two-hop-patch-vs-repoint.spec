# A rename of a customer, written into the lines two joins away as a fan-out
# patch (line -> order -> customer), against an order re-pointed to another
# customer meanwhile. The rename's lookup locks the orders it goes through: it
# waits for an open re-point and then drops the order it moved, or the re-point
# waits for the rename and recomputes the lines after it. Either way the lines
# show their order's customer.

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
  INSERT INTO tb_order (pk_order, fk_customer) VALUES (10, 1);
  INSERT INTO tb_line (pk_line, fk_order, sku) VALUES (100, 10, 's');
  SELECT tviews.pg_tviews_create('tv_line', $$
    SELECT l.pk_line, l.id, l.fk_order,
           jsonb_build_object('sku', l.sku, 'customer', c.name) AS data
    FROM tb_line l JOIN tb_order o ON o.pk_order = l.fk_order
    JOIN tb_customer c ON c.pk_customer = o.fk_customer $$);
}

teardown
{
  DROP TABLE tv_line;
  DROP EXTENSION pg_tviews CASCADE;
  DROP TABLE tb_line, tb_order, tb_customer;
}

session repoint
step r_begin   { BEGIN; }
step r_move    { UPDATE tb_order SET fk_customer = 2 WHERE pk_order = 10; }
step r_commit  { COMMIT; }

session rename
step n_begin   { BEGIN; }
step n_rename  { UPDATE tb_customer SET name = 'amy' WHERE pk_customer = 1; }
step n_commit  { COMMIT; }
step n_check
{
  SELECT t.pk_line, t.data->>'customer' AS customer, t.data = v.data AS fresh
  FROM tv_line t FULL JOIN tviews.public__tv_line v USING (pk_line) ORDER BY 1;
}

permutation r_begin r_move n_begin n_rename r_commit n_commit n_check
permutation n_begin n_rename r_begin r_move n_commit r_commit n_check
