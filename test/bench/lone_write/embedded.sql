-- The embed workload alone (test/concurrency/embed.sql: 200 items, 4 places, 8
-- groups, each item embedding the counts of current items in its place and its
-- group): the embedded side of composed.sql.
\set items 200
\set places 4
\set groups 8
\ir ../../concurrency/embed.sql
