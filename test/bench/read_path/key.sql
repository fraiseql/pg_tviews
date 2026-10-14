-- rc_composed_key: rc_composed with the post's key also inside each comment's
-- document, as fraiseql's composed read correlates today ((data->>'fk_post')::int),
-- compacted and indexed on that expression. Run on a copy of rc_composed.
SET session_replication_role = replica;
UPDATE bench.tv_comment SET data = data || jsonb_build_object('fk_post', fk_post);
VACUUM FULL bench.tv_comment;
CREATE INDEX ON bench.tv_comment (((data->>'fk_post')::int));
ANALYZE bench.tv_comment;
