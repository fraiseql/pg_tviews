# Two transactions writing rows of two TVIEWs that read the same tables
# (tv_author aggregates the titles of tb_book; tv_book copies the author's name).
# A flush visits TVIEWs in one order, the dependency order with ties by name
# (ADR 0207), and takes each one's key locks there, fan-out patches included: a
# rename of the author writes tv_author's row, then patches the book's; a title
# written into the book refreshes tv_author's row, then tv_book's. A trigger holds
# the rename after its first TVIEW write. In one order the title waits for the
# rename and both commit; applying the fan-out first, as before, took the book
# then the author against the title's author then book: 40P01.

setup
{
  CREATE EXTENSION IF NOT EXISTS jsonb_delta;
  CREATE EXTENSION pg_tviews;
  CREATE TABLE tb_author (pk_author bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                          name text);
  CREATE TABLE tb_book (pk_book bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                        fk_author bigint NOT NULL, title text);
  INSERT INTO tb_author (pk_author, name) VALUES (1, 'ann');
  INSERT INTO tb_book (pk_book, fk_author, title) VALUES (1, 1, 'b1');
  SELECT tviews.pg_tviews_create('tv_author', $$
    SELECT a.pk_author, a.id,
           jsonb_build_object('name', a.name, 'titles', jsonb_agg(b.title ORDER BY b.pk_book)) AS data
    FROM tb_author a LEFT JOIN tb_book b ON b.fk_author = a.pk_author
    GROUP BY a.pk_author, a.id, a.name $$);
  SELECT tviews.pg_tviews_create('tv_book', $$
    SELECT b.pk_book, b.id, b.fk_author,
           jsonb_build_object('title', b.title, 'author', jsonb_build_object('name', a.name)) AS data
    FROM tb_book b JOIN tb_author a ON a.pk_author = b.fk_author $$);
  -- Holds a session with test.gate set right after its first write of a TVIEW row.
  CREATE FUNCTION gate() RETURNS trigger LANGUAGE plpgsql AS $$
    BEGIN
      IF current_setting('test.gate', true) = 'on'
         AND current_setting('test.held', true) IS DISTINCT FROM 'on' THEN
        PERFORM set_config('test.held', 'on', true);
        PERFORM pg_advisory_xact_lock_shared(268, 1);
      END IF;
      RETURN NULL;
    END $$;
  CREATE TRIGGER zz_gate AFTER INSERT OR UPDATE ON tv_author FOR EACH ROW EXECUTE FUNCTION gate();
  CREATE TRIGGER zz_gate AFTER INSERT OR UPDATE ON tv_book FOR EACH ROW EXECUTE FUNCTION gate();
}

teardown
{
  DROP TABLE tv_book;
  DROP TABLE tv_author;
  DROP EXTENSION pg_tviews CASCADE;
  DROP TABLE tb_book, tb_author;
  DROP FUNCTION gate();
}

session gate
step g_close { SELECT pg_advisory_lock(268, 1); }
step g_open  { SELECT pg_advisory_unlock(268, 1); }

session rename
setup        { SET deadlock_timeout = '100ms'; SET test.gate = 'on'; }
step n_rename { UPDATE tb_author SET name = 'amy' WHERE pk_author = 1; }

session title
setup        { SET deadlock_timeout = '100ms'; }
step t_title { UPDATE tb_book SET title = 'b1!' WHERE pk_book = 1; }
step t_check
{
  SELECT 'author' AS tv, t.data = v.data AS fresh
  FROM tv_author t FULL JOIN tviews.public__tv_author v USING (pk_author)
  UNION ALL
  SELECT 'book', t.data = v.data FROM tv_book t FULL JOIN tviews.public__tv_book v USING (pk_book)
  ORDER BY 1;
}

permutation g_close n_rename t_title g_open t_check
