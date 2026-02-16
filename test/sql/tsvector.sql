-- tsvector column support and per-query language override tests
CREATE EXTENSION IF NOT EXISTS pg_textsearch;

-- Test 1: CREATE INDEX on tsvector column (no text_config needed)
CREATE TABLE multilang (
    id serial PRIMARY KEY,
    content_tsv tsvector
);

INSERT INTO multilang (content_tsv) VALUES
    (to_tsvector('english', 'the quick brown fox jumps over the lazy dog')),
    (to_tsvector('english', 'a brown fox is quick and clever')),
    (to_tsvector('french', 'le renard brun rapide saute par-dessus le chien'));

CREATE INDEX idx_multilang ON multilang USING bm25 (content_tsv)
    WITH (k1=1.2, b=0.75);

-- Test 2: Query with explicit language parameter
SELECT id, content_tsv <@> to_bm25query('fox', 'idx_multilang', 'english') AS score
FROM multilang
ORDER BY score
LIMIT 10;

-- Test 3: Query with different language
SELECT id, content_tsv <@> to_bm25query('renard', 'idx_multilang', 'french') AS score
FROM multilang
ORDER BY score
LIMIT 10;

-- Test 3b: Verify French lemmatization actually differs from English
-- "chevaux" stems to "cheval" in French but stays "chevaux" in English
INSERT INTO multilang (content_tsv) VALUES
    (to_tsvector('french', 'les chevaux galopent dans la prairie'));

-- French query: chevaux→cheval matches the indexed stem
SELECT id, content_tsv <@> to_bm25query('chevaux', 'idx_multilang', 'french') AS score
FROM multilang
ORDER BY score
LIMIT 10;

-- English query: chevaux stays as-is, no match in index
SELECT id, content_tsv <@> to_bm25query('chevaux', 'idx_multilang', 'english') AS score
FROM multilang
ORDER BY score
LIMIT 10;

-- Test 4: Index scan with ORDER BY (using language override)
SET enable_seqscan = off;
EXPLAIN (COSTS OFF) SELECT id FROM multilang
ORDER BY content_tsv <@> to_bm25query('fox', 'idx_multilang', 'english')
LIMIT 5;

SELECT id FROM multilang
ORDER BY content_tsv <@> to_bm25query('fox', 'idx_multilang', 'english')
LIMIT 5;

RESET enable_seqscan;

-- Test 5: INSERT into tsvector-indexed table
INSERT INTO multilang (content_tsv) VALUES
    (to_tsvector('english', 'the fox ran across the field'));

SELECT id, content_tsv <@> to_bm25query('fox', 'idx_multilang', 'english') AS score
FROM multilang
ORDER BY score
LIMIT 10;

-- Test 6: tsvector index with text_config also specified (should work)
CREATE TABLE tsv_with_config (
    id serial PRIMARY KEY,
    content_tsv tsvector
);
CREATE INDEX idx_tsv_config ON tsv_with_config USING bm25 (content_tsv)
    WITH (text_config='english', k1=1.2, b=0.75);

INSERT INTO tsv_with_config (content_tsv) VALUES
    (to_tsvector('english', 'hello world'));

SELECT id, content_tsv <@> to_bm25query('hello', 'idx_tsv_config', 'english') AS score
FROM tsv_with_config
ORDER BY score
LIMIT 5;

-- Test 7: Backwards compat - text column index still works
CREATE TABLE docs (id serial PRIMARY KEY, body text);
INSERT INTO docs VALUES (1, 'hello world'), (2, 'goodbye world');
CREATE INDEX idx_docs ON docs USING bm25 (body) WITH (text_config='english');

SELECT id, body <@> to_bm25query('hello', 'idx_docs') AS score
FROM docs
ORDER BY score
LIMIT 5;

-- Test 8: to_bm25query with language on text column index
SELECT id, body <@> to_bm25query('hello', 'idx_docs', 'english') AS score
FROM docs
ORDER BY score
LIMIT 5;

-- Test 9: text column without text_config should error
CREATE TABLE no_config (id serial PRIMARY KEY, body text);
CREATE INDEX idx_no_config ON no_config USING bm25 (body);

-- Cleanup
DROP TABLE multilang CASCADE;
DROP TABLE tsv_with_config CASCADE;
DROP TABLE docs CASCADE;
DROP TABLE IF EXISTS no_config CASCADE;
