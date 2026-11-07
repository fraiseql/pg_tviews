-- Fixture TVIEWs for the rules the update applies to an existing catalog
-- (test/upgrade/upgrade_check.sh, after fixtures.sql). Created with the PREVIOUS
-- release; checked by verify_security.sql once the update ran.
--
-- - tv_doc, owned by upgrade_owner: the mapping query of tb_doc_note calls
--   sec.folded('upgrade'), which records who plans it. The update re-derives every
--   TVIEW as its owner: only upgrade_owner may be recorded.
-- - A column grant on tv_doc's backing view: the update leaves the view its owner's.
-- - tv_tag calls sec.label(), whose search_path no longer ends in pg_temp: the update
--   warns, and the health check reports it. A release that refuses such a function at
--   create time still holds one changed afterwards, so the path is set after the create.
-- - tv_shop's guards compare a char(2) code and test IS NULL over an outer join:
--   the update re-derives them by the current rules.

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
SET search_path TO "$user", public, tviews;

DO $$ BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_catalog.pg_roles WHERE rolname = 'upgrade_owner') THEN
        CREATE ROLE upgrade_owner;
    END IF;
    IF NOT EXISTS (SELECT 1 FROM pg_catalog.pg_roles WHERE rolname = 'upgrade_column_reader') THEN
        CREATE ROLE upgrade_column_reader;
    END IF;
END $$;

CREATE SCHEMA sec AUTHORIZATION upgrade_owner;
CREATE TABLE sec.who_log (step text, cu name);
GRANT INSERT, SELECT ON sec.who_log TO PUBLIC;
CREATE FUNCTION sec.who(step text) RETURNS int LANGUAGE plpgsql VOLATILE AS $$
BEGIN
    INSERT INTO sec.who_log VALUES (step, current_user);
    RETURN 1;
END $$;
-- IMMUTABLE with a constant argument: evaluated by whoever plans the query.
CREATE FUNCTION sec.folded(step text) RETURNS int LANGUAGE plpgsql IMMUTABLE AS $$
BEGIN
    RETURN sec.who(step);
END $$;
CREATE FUNCTION sec.label(t text) RETURNS text LANGUAGE sql IMMUTABLE
    SET search_path = sec, pg_temp AS $$ SELECT upper(t) $$;
ALTER FUNCTION sec.label(text) OWNER TO upgrade_owner;

SET ROLE upgrade_owner;
CREATE TABLE sec.tb_doc (pk_doc int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                         title text);
CREATE TABLE sec.tb_doc_note (pk_doc_note int PRIMARY KEY, fk_doc int, body text);
CREATE TABLE sec.tb_shop (pk_shop int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                          name text);
CREATE TABLE sec.tb_country (pk_country int PRIMARY KEY, code char(2), name text);
CREATE TABLE sec.tb_notice (pk_notice int PRIMARY KEY, kind text);
INSERT INTO sec.tb_doc (pk_doc, title) VALUES (1, 'd1');
INSERT INTO sec.tb_doc_note VALUES (1, 1, 'n1');
INSERT INTO sec.tb_shop (pk_shop, name) VALUES (1, 's1');
INSERT INTO sec.tb_country VALUES (1, 'FR', 'France');

SELECT pg_tviews_create('sec.tv_doc', $q$
    SELECT d.pk_doc, d.id, jsonb_build_object('title', d.title,
        'notes', (SELECT jsonb_agg(n.body) FROM sec.tb_doc_note n
                  WHERE n.fk_doc = d.pk_doc OPERATOR(pg_catalog.+) sec.folded('upgrade') - 1)) AS data
    FROM sec.tb_doc d $q$);
SELECT pg_tviews_create('sec.tv_tag', $q$
    SELECT d.pk_doc AS pk_tag, d.id, jsonb_build_object('label', sec.label(d.title)) AS data
    FROM sec.tb_doc d $q$);
SELECT pg_tviews_create('sec.tv_shop', $q$
    SELECT s.pk_shop, s.id, jsonb_build_object(
        'france', (SELECT c.name FROM sec.tb_country c WHERE c.code = 'FR'),
        'open', n.pk_notice IS NULL) AS data
    FROM sec.tb_shop s LEFT JOIN sec.tb_notice n ON n.kind = 'closed'
    WHERE n.pk_notice IS NULL $q$, '{"uncascaded_policy": "full_refresh"}');
ALTER FUNCTION sec.label(text) SET search_path = sec;
RESET ROLE;

SELECT format('GRANT SELECT (data) ON %s TO upgrade_column_reader', view)
FROM tviews.registry WHERE name = 'tv_doc' \gexec

TRUNCATE sec.who_log;
