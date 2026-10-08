-- Every relative link in README.md and docs/ (history excluded: docs/archive,
-- docs/adr) resolves to a file or directory of the repository. Links in fenced
-- code blocks, external links (http, https, mailto) and same-page anchors are not
-- checked. It runs from inside the repository; it needs no extension.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/docs/regress_docs_relative_links.sql
--
-- expect-output: docs links PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

\set tracked `cd "$(git rev-parse --show-toplevel)" && git ls-files`
\set links `cd "$(git rev-parse --show-toplevel)" && git ls-files README.md 'docs/*.md' | grep -vE '^docs/(archive|adr)/' | xargs awk 'BEGIN { fence = sprintf("%c%c%c", 96, 96, 96) } FNR == 1 { fenced = 0 } { t = $0; sub(/^[ \t]+/, "", t) } index(t, fence) == 1 || index(t, "~~~") == 1 { fenced = !fenced; next } !fenced { line = $0; while (match(line, /\]\([^) ]+/)) { print FILENAME "\t" substr(line, RSTART + 2, RLENGTH - 2); line = substr(line, RSTART + RLENGTH) } }'`

CREATE TEMP TABLE known AS
    SELECT DISTINCT p AS path
    FROM unnest(string_to_array(:'tracked', E'\n')) AS f(file),
         LATERAL (SELECT array_to_string((string_to_array(f.file, '/'))[1:n], '/') AS p
                  FROM generate_series(1, cardinality(string_to_array(f.file, '/'))) AS n) d;

CREATE TEMP TABLE links AS
    SELECT split_part(l, E'\t', 1) AS src,
           split_part(l, E'\t', 2) AS target
    FROM unnest(string_to_array(:'links', E'\n')) AS l
    WHERE l <> '';

-- `rel` as seen from directory `dir`, both relative to the repository root.
CREATE FUNCTION pg_temp.resolve(dir text, rel text) RETURNS text LANGUAGE plpgsql AS $$
DECLARE
    parts text[] := CASE WHEN rel LIKE '/%' OR dir = '' THEN '{}'
                         ELSE string_to_array(dir, '/') END;
    part text;
BEGIN
    FOREACH part IN ARRAY string_to_array(rel, '/') LOOP
        IF part = '..' THEN
            parts := parts[1:cardinality(parts) - 1];
        ELSIF part <> '' AND part <> '.' THEN
            parts := parts || part;
        END IF;
    END LOOP;
    RETURN array_to_string(parts, '/');
END $$;

DO $$
DECLARE
    broken text;
    n bigint;
BEGIN
    IF (SELECT count(*) FROM links) < 300 THEN
        RAISE EXCEPTION 'docs links FAIL: found only % links; is this run from the repository?',
            (SELECT count(*) FROM links);
    END IF;
    SELECT count(*), string_agg(src || ' -> ' || target, ', ' ORDER BY src, target)
    INTO n, broken
    FROM (SELECT src, target,
                 pg_temp.resolve(
                     regexp_replace(src, '/?[^/]*$', ''),
                     regexp_replace(target, '#.*$', '')) AS resolved
          FROM links
          WHERE target !~ '^(https?:|mailto:|#)') l
    WHERE resolved NOT IN (SELECT path FROM known) AND resolved <> '';
    IF n > 0 THEN
        RAISE EXCEPTION 'docs links FAIL: % broken: %', n, broken;
    END IF;
END $$;

SELECT 'docs links PASS' AS result;
