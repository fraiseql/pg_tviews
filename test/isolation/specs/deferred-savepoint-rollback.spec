# A subtransaction rolled back inside a deferred mutation takes its queued work,
# its value locks and its row locks with it (#286): the handler's rolled-back
# write to parent 2 blocks nobody, and only parent 1's change is refreshed, when
# the outer statement ends. A concurrent writer of parent 2 goes through at once.

setup
{
  CREATE EXTENSION IF NOT EXISTS jsonb_delta;
  CREATE EXTENSION pg_tviews;
  CREATE TABLE tb_parent (pk_parent int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                          name text);
  CREATE TABLE tb_child (pk_child int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                         fk_parent int NOT NULL REFERENCES tb_parent, note text);
  INSERT INTO tb_parent (pk_parent, name) VALUES (1, 'p1'), (2, 'p2');
  INSERT INTO tb_child (pk_child, fk_parent, note) VALUES (1, 1, 'n1'), (2, 2, 'n2');
  SELECT tviews.pg_tviews_create('tv_parent', $$
    SELECT pk_parent, id, jsonb_build_object('name', name) AS data FROM tb_parent $$);
  SELECT tviews.pg_tviews_create('tv_child', $$
    SELECT c.pk_child, c.id, c.fk_parent, jsonb_build_object('note', c.note, 'parent', p.data) AS data
    FROM tb_child c JOIN tv_parent p ON p.pk_parent = c.fk_parent $$);
  CREATE FUNCTION keep_one() RETURNS void LANGUAGE plpgsql AS $$
  BEGIN
    UPDATE tb_parent SET name = 'kept' WHERE pk_parent = 1;
    BEGIN
      UPDATE tb_parent SET name = 'undone' WHERE pk_parent = 2;
      RAISE EXCEPTION 'undo';
    EXCEPTION WHEN raise_exception THEN NULL;
    END;
  END $$;
}

teardown
{
  DROP TABLE tv_child, tv_parent;
  DROP EXTENSION pg_tviews CASCADE;
  DROP TABLE tb_child, tb_parent;
  DROP FUNCTION keep_one();
}

session a
step a_begin    { BEGIN; }
step a_mutate   { SELECT keep_one(); }
step a_commit   { COMMIT; }

session b
setup           { SET lock_timeout = '5s'; }
step b_parent2  { UPDATE tb_parent SET name = 'b' WHERE pk_parent = 2; }
step b_check
{
  SELECT t.pk_child, t.data->'parent'->>'name' AS parent, t.data = v.data AS fresh
  FROM tv_child t FULL JOIN tviews.public__tv_child v USING (pk_child) ORDER BY 1;
}

permutation a_begin a_mutate b_parent2 a_commit b_check
