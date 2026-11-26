# Value locks take equal link values as one, whatever their text (ADR 0207: the
# lock form is the hash the column type's equality uses). Session one moves a
# category to code 1.0, a link: its refresh reads the items of code 1.0 and holds
# their value lock until it commits. Session two moves an item to code 1.00,
# equal under `=`: it must wait for one, then see the category at 1.0 and refresh
# it. Had the locks been keyed by text, two would not wait, would refresh nothing
# (the category is still at 9 for it), and one would commit a document without
# the item. The second permutation is the control: an item moved to 3, a value
# one does not hold, does not wait.

setup
{
  CREATE EXTENSION IF NOT EXISTS jsonb_delta;
  CREATE EXTENSION pg_tviews;
  CREATE TABLE tb_cat (pk_cat bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                       code numeric NOT NULL, name text);
  CREATE TABLE tb_item (pk_item bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                        code numeric NOT NULL, label text);
  INSERT INTO tb_cat (pk_cat, code, name) VALUES (1, 9, 'c1');
  INSERT INTO tb_item (pk_item, code, label) VALUES (1, 1.0, 'i1'), (2, 7, 'i2');
  SELECT tviews.pg_tviews_create('tv_cat', $$
    SELECT c.pk_cat, c.id,
           jsonb_build_object('name', c.name,
             'items', (SELECT jsonb_agg(i.label ORDER BY i.pk_item) FROM tb_item i WHERE i.code = c.code)) AS data
    FROM tb_cat c $$);
}

teardown
{
  DROP TABLE tv_cat;
  DROP EXTENSION pg_tviews CASCADE;
  DROP TABLE tb_cat, tb_item;
}

session one
setup          { SET deadlock_timeout = '100ms'; }
step o_begin   { BEGIN; }
step o_move    { UPDATE tb_cat SET code = 1.0 WHERE pk_cat = 1; }
step o_commit  { COMMIT; }

session two
setup          { SET deadlock_timeout = '100ms'; }
step t_move    { UPDATE tb_item SET code = 1.00 WHERE pk_item = 2; }
step t_other   { UPDATE tb_item SET code = 3 WHERE pk_item = 2; }
step t_check
{
  SELECT t.pk_cat, t.data->'items' AS items, t.data = v.data AS fresh
  FROM tv_cat t FULL JOIN tviews.public__tv_cat v USING (pk_cat) ORDER BY 1;
}

permutation o_begin o_move t_move o_commit t_check
permutation o_begin o_move t_other o_commit t_check
