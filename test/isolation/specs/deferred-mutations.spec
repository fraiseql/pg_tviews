# A mutation refreshes the TVIEWs once, when its outer statement ends (#286).
# Two mutations of one parent, each three single-field UPDATEs in a function called
# as one SELECT: the second waits on the first's row lock and runs after its commit;
# a write to a child of that parent in between waits on the value locks the first
# took at capture until its commit, then refreshes on top of it. Both TVIEWs equal
# their backing views at the end, holding the last writer's values.

setup
{
  CREATE EXTENSION IF NOT EXISTS jsonb_delta;
  CREATE EXTENSION pg_tviews;
  CREATE TABLE tb_parent (pk_parent int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                          name text, city text, zip text);
  CREATE TABLE tb_child (pk_child int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                         fk_parent int NOT NULL REFERENCES tb_parent, note text);
  INSERT INTO tb_parent (pk_parent, name, city, zip) VALUES (1, 'p', 'c', 'z');
  INSERT INTO tb_child (pk_child, fk_parent, note) VALUES (1, 1, 'n1'), (2, 1, 'n2');
  SELECT tviews.pg_tviews_create('tv_parent', $$
    SELECT pk_parent, id, jsonb_build_object('name', name, 'city', city, 'zip', zip) AS data
    FROM tb_parent $$);
  SELECT tviews.pg_tviews_create('tv_child', $$
    SELECT c.pk_child, c.id, c.fk_parent, jsonb_build_object('note', c.note, 'parent', p.data) AS data
    FROM tb_child c JOIN tv_parent p ON p.pk_parent = c.fk_parent $$);
  CREATE FUNCTION rename3(v text) RETURNS text LANGUAGE plpgsql AS $$
  BEGIN
    UPDATE tb_parent SET name = v WHERE pk_parent = 1;
    UPDATE tb_parent SET city = v WHERE pk_parent = 1;
    UPDATE tb_parent SET zip = v WHERE pk_parent = 1;
    RETURN (SELECT data->>'name' FROM tv_parent WHERE pk_parent = 1);
  END $$;
}

teardown
{
  DROP TABLE tv_child, tv_parent;
  DROP EXTENSION pg_tviews CASCADE;
  DROP TABLE tb_child, tb_parent;
  DROP FUNCTION rename3(text);
}

session a
step a_begin    { BEGIN; }
step a_mutate   { SELECT rename3('a'); }
step a_commit   { COMMIT; }

session b
setup           { SET lock_timeout = '10s'; }
step b_mutate   { SELECT rename3('b'); }
step b_child    { UPDATE tb_child SET note = 'b' WHERE pk_child = 2; }
step b_check
{
  SELECT 'parent' AS tv, t.pk_parent AS pk, t.data->>'name' AS name, t.data = v.data AS fresh
  FROM tv_parent t FULL JOIN tviews.public__tv_parent v USING (pk_parent)
  UNION ALL
  SELECT 'child', t.pk_child, t.data->'parent'->>'name', t.data = v.data
  FROM tv_child t FULL JOIN tviews.public__tv_child v USING (pk_child)
  ORDER BY 1, 2;
}

permutation a_begin a_mutate b_mutate a_commit b_check
permutation a_begin a_mutate b_child a_commit b_check
