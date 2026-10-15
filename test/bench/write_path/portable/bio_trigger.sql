-- Portable-edition research (ADR 0232): the `bio` write of the write_path bench kept
-- fresh by a PL/pgSQL statement trigger instead of the extension, as a SQL-only
-- edition would generate it, written by hand for this schema. tv_user and tv_post
-- are plain tables filled from the same definitions as schema.sql's TVIEWs.
--   psql -f portable/bio_trigger.sql   (on a database without pg_tviews)
CREATE SCHEMA bench;
SET search_path = bench, public;
CREATE TABLE tb_user (
    pk_user int GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id uuid NOT NULL DEFAULT gen_random_uuid() UNIQUE,
    identifier text NOT NULL UNIQUE, email text NOT NULL UNIQUE, username text NOT NULL UNIQUE,
    full_name text NOT NULL, bio text,
    created_at timestamptz NOT NULL DEFAULT now(), updated_at timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE tb_post (
    pk_post int GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id uuid NOT NULL DEFAULT gen_random_uuid() UNIQUE,
    identifier text NOT NULL UNIQUE, title text NOT NULL, content text NOT NULL,
    fk_author int NOT NULL REFERENCES tb_user ON DELETE CASCADE,
    published boolean NOT NULL DEFAULT false,
    created_at timestamptz NOT NULL DEFAULT now(), updated_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX ON tb_post (fk_author);
INSERT INTO tb_user (identifier, email, username, full_name, bio)
SELECT 'u' || g, 'u' || g || '@x.io', 'user' || g, 'User ' || g, repeat('bio text ', 20)
FROM generate_series(1, 2000) g;
INSERT INTO tb_post (identifier, title, content, fk_author, published)
SELECT 'p' || g, 'Title ' || g, repeat('content words ', 60), 1 + (g % 2000), true
FROM generate_series(1, 20000) g;

CREATE TABLE tv_user (pk_user int PRIMARY KEY, id uuid NOT NULL, identifier text, data jsonb NOT NULL,
                      updated_at timestamptz NOT NULL DEFAULT now()) WITH (fillfactor = 85);
ALTER TABLE tv_user ALTER COLUMN data SET STORAGE MAIN, ALTER COLUMN data SET COMPRESSION lz4;
INSERT INTO tv_user (pk_user, id, identifier, data)
SELECT u.pk_user, u.id, u.identifier,
       jsonb_build_object('id', u.id::text, 'identifier', u.identifier, 'email', u.email,
           'username', u.username, 'full_name', u.full_name, 'bio', u.bio,
           'created_at', u.created_at, 'updated_at', u.updated_at)
FROM tb_user u;
CREATE TABLE tv_post (pk_post int PRIMARY KEY, id uuid NOT NULL, identifier text, fk_author int,
                      data jsonb NOT NULL, updated_at timestamptz NOT NULL DEFAULT now())
    WITH (fillfactor = 85);
ALTER TABLE tv_post ALTER COLUMN data SET STORAGE MAIN, ALTER COLUMN data SET COMPRESSION lz4;
INSERT INTO tv_post (pk_post, id, identifier, fk_author, data)
SELECT p.pk_post, p.id, p.identifier, p.fk_author,
       jsonb_build_object('id', p.id::text, 'identifier', p.identifier, 'title', p.title,
           'content', p.content, 'published', p.published,
           'created_at', p.created_at, 'updated_at', p.updated_at,
           'author', jsonb_build_object('id', u.id::text, 'username', u.username,
               'full_name', u.full_name, 'bio', u.bio))
FROM tb_post p JOIN tb_user u ON u.pk_user = p.fk_author;
CREATE INDEX ON tv_post (fk_author);

-- What a generator would emit for tb_user.bio: patch both copies of the column,
-- for the rows whose bio changed, from the statement's transition tables.
CREATE FUNCTION tb_user_refresh() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    UPDATE bench.tv_user t
    SET data = jsonb_set(t.data, '{bio}', to_jsonb(n.bio)), updated_at = now()
    FROM new_rows n JOIN old_rows o ON o.pk_user = n.pk_user
    WHERE t.pk_user = n.pk_user AND n.bio IS DISTINCT FROM o.bio;
    UPDATE bench.tv_post t
    SET data = jsonb_set(t.data, '{author,bio}', to_jsonb(n.bio)), updated_at = now()
    FROM new_rows n JOIN old_rows o ON o.pk_user = n.pk_user
    WHERE t.fk_author = n.pk_user AND n.bio IS DISTINCT FROM o.bio;
    RETURN NULL;
END $$;
CREATE TRIGGER tb_user_refresh AFTER UPDATE ON tb_user
    REFERENCING NEW TABLE AS new_rows OLD TABLE AS old_rows
    FOR EACH STATEMENT EXECUTE FUNCTION tb_user_refresh();
VACUUM ANALYZE;
