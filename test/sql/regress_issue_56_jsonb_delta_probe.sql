-- Contract probe for issue #56 (Phase 4 Cycle 0): jsonb_smart_patch_nested must
-- MERGE the source at the target path, preserving sibling keys — NOT replace the
-- object at the path. Parent patch derivation applies the child's changed fields
-- at the embedding path; replace-at-path would clobber the embedded object's other
-- keys (e.g. author.name lost when patching author.bio). If this probe fails,
-- STOP and re-plan — the whole cascade fast path depends on merge semantics.
--
-- NULL values are set-to-null (not stripped), matching jsonb_build_object output,
-- so NULL new-values stay eligible.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_56_jsonb_delta_probe.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;

DO $$ BEGIN
  -- 1-segment nested merge keeps sibling `name`, updates `bio`, keeps top-level `title`.
  IF jsonb_smart_patch_nested(
        '{"author":{"name":"A","bio":"old"},"title":"T"}'::jsonb,
        '{"bio":"new"}'::jsonb, ARRAY['author'])
     <> '{"author":{"name":"A","bio":"new"},"title":"T"}'::jsonb THEN
    RAISE EXCEPTION '#56 PROBE FAIL: nested patch is not merge-at-path (1 segment)';
  END IF;

  -- 2-segment nested path merges at the deep path.
  IF jsonb_smart_patch_nested(
        '{"post":{"author":{"name":"A","bio":"old"}}}'::jsonb,
        '{"bio":"new"}'::jsonb, ARRAY['post','author'])
     <> '{"post":{"author":{"name":"A","bio":"new"}}}'::jsonb THEN
    RAISE EXCEPTION '#56 PROBE FAIL: nested patch is not merge-at-path (2 segments)';
  END IF;

  -- Top-level scalar merge keeps siblings.
  IF jsonb_smart_patch_scalar('{"name":"A","bio":"old"}'::jsonb, '{"bio":"new"}'::jsonb)
     <> '{"name":"A","bio":"new"}'::jsonb THEN
    RAISE EXCEPTION '#56 PROBE FAIL: scalar patch is not a shallow merge';
  END IF;

  -- NULL is set-to-null, not deleted (byte-identical to jsonb_build_object('bio', NULL)).
  IF jsonb_smart_patch_scalar('{"name":"A","bio":"old"}'::jsonb, '{"bio":null}'::jsonb)
     <> '{"name":"A","bio":null}'::jsonb THEN
    RAISE EXCEPTION '#56 PROBE FAIL: NULL value not preserved as json null';
  END IF;
END $$;

SELECT 'issue #56 jsonb_delta probe: PASS' AS result;
