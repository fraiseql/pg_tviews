\set p random(1, 20000)
\set u random(1, 2000)
INSERT INTO bench.tb_comment (content, fk_post, fk_author) VALUES ('new comment', :p, :u);
